//! Shairport Sync metadata pipe (SRS §23).
//!
//! Items are XML-style records: type and code as 8 hex digits, optional base64 data.
//! `core` + `minm`/`asar`/`asal` are title/artist/album; `ssnc` + `PICT` is cover art.

use base64::Engine;
use std::sync::{Arc, Mutex};

pub const MAX_ARTWORK_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackMetadata {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
    pub source: Option<String>,
    pub has_artwork: bool,
    pub rtp_start: Option<u32>,
    pub rtp_current: Option<u32>,
    pub rtp_end: Option<u32>,
    pub duration_ms: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Artwork {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

pub type SharedArtwork = Arc<Mutex<Option<Artwork>>>;

pub const RTP_PROGRESS_RATE: f32 = 44100.0;

impl TrackMetadata {
    pub fn duration_s(&self) -> Option<f32> {
        if let Some(ms) = self.duration_ms {
            return Some(ms as f32 / 1000.0);
        }
        Some(rtp_secs(self.rtp_start?, self.rtp_end?))
    }

    pub fn position_s(&self) -> Option<f32> {
        Some(rtp_secs(self.rtp_start?, self.rtp_current?))
    }
}

fn rtp_secs(start: u32, later: u32) -> f32 {
    later.wrapping_sub(start) as f32 / RTP_PROGRESS_RATE
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaItem {
    pub type_code: String,
    pub code: String,
    pub data: Vec<u8>,
}

pub fn decode_fourcc(hex: &str) -> Option<String> {
    let hex = hex.trim();
    if hex.len() != 8 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0u8; 4];
    for i in 0..4 {
        bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(bytes.iter().map(|b| char::from(*b)).collect())
}

pub fn parse_item(xml: &str) -> Option<MetaItem> {
    let type_hex = tag_value(xml, "type")?;
    let code_hex = tag_value(xml, "code")?;
    let type_code = decode_fourcc(&type_hex)?;
    let code = decode_fourcc(&code_hex)?;
    let data = tag_value(xml, "data")
        .map(|b64| {
            let compact: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
            base64::engine::general_purpose::STANDARD
                .decode(compact)
                .unwrap_or_default()
        })
        .unwrap_or_default();
    Some(MetaItem {
        type_code,
        code,
        data,
    })
}

pub fn drain_items(buf: &mut String) -> Vec<MetaItem> {
    let mut out = Vec::new();
    loop {
        let Some(start) = buf.find("<item>") else {
            buf.clear();
            break;
        };
        if start > 0 {
            buf.drain(..start);
        }
        let Some(rel_end) = buf.find("</item>") else {
            break;
        };
        let end = rel_end + "</item>".len();
        let xml = buf[..end].to_string();
        buf.drain(..end);
        if let Some(item) = parse_item(&xml) {
            out.push(item);
        }
    }
    out
}

pub fn apply_item(
    meta: &mut TrackMetadata,
    artwork: &mut Option<Artwork>,
    item: &MetaItem,
) -> bool {
    match (item.type_code.as_str(), item.code.as_str()) {
        ("core", "minm") => {
            meta.title = utf8_opt(&item.data);
            true
        }
        ("core", "asar") => {
            meta.artist = utf8_opt(&item.data);
            true
        }
        ("core", "asal") => {
            meta.album = utf8_opt(&item.data);
            true
        }
        ("ssnc", "clip") => {
            meta.source = utf8_opt(&item.data);
            true
        }
        ("ssnc", "mdst") => {
            meta.title = None;
            meta.artist = None;
            meta.album = None;
            meta.rtp_start = None;
            meta.rtp_current = None;
            meta.rtp_end = None;
            meta.duration_ms = None;
            true
        }
        ("ssnc", "prgr") => {
            if let Some((start, current, end)) = parse_prgr(&item.data) {
                meta.rtp_start = Some(start);
                meta.rtp_current = Some(current);
                meta.rtp_end = Some(end);
                true
            } else {
                false
            }
        }
        ("core", "astm") => {
            if let Some(ms) = parse_astm(&item.data) {
                meta.duration_ms = Some(ms);
                true
            } else {
                false
            }
        }
        ("ssnc", "pend") | ("ssnc", "pfls") => {
            *meta = TrackMetadata {
                source: meta.source.clone(),
                ..TrackMetadata::default()
            };
            *artwork = None;
            true
        }
        ("ssnc", "PICT") if !item.data.is_empty() && item.data.len() <= MAX_ARTWORK_BYTES => {
            *artwork = Some(Artwork {
                content_type: sniff_image(&item.data).to_string(),
                bytes: item.data.clone(),
            });
            meta.has_artwork = true;
            true
        }
        _ => false,
    }
}

fn tag_value(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}");
    let start_tag = xml.find(&open)?;
    let content_start = xml[start_tag..].find('>')? + start_tag + 1;
    let close = format!("</{name}>");
    let end = xml[content_start..].find(&close)? + content_start;
    Some(xml[content_start..end].to_string())
}

fn utf8_opt(bytes: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(bytes).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn parse_prgr(data: &[u8]) -> Option<(u32, u32, u32)> {
    let s = String::from_utf8_lossy(data);
    let mut parts = s.trim().split('/');
    let start = parts.next()?.parse().ok()?;
    let current = parts.next()?.parse().ok()?;
    let end = parts.next()?.parse().ok()?;
    Some((start, current, end))
}

fn parse_astm(data: &[u8]) -> Option<u32> {
    (data.len() >= 4).then(|| u32::from_be_bytes([data[0], data[1], data[2], data[3]]))
}

pub fn sniff_image(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item_xml(ty: &str, code: &str, data: &str) -> String {
        let b64 = base64::engine::general_purpose::STANDARD.encode(data.as_bytes());
        format!(
            "<item><type>{ty}</type><code>{code}</code><length>{}</length>\n<data encoding=\"base64\">{b64}</data>\n</item>",
            data.len()
        )
    }

    #[test]
    fn decodes_fourcc() {
        assert_eq!(decode_fourcc("636f7265").as_deref(), Some("core"));
        assert_eq!(decode_fourcc("6d696e6d").as_deref(), Some("minm"));
        assert_eq!(decode_fourcc("61736172").as_deref(), Some("asar"));
        assert_eq!(decode_fourcc("50494354").as_deref(), Some("PICT"));
    }

    #[test]
    fn parses_title_artist_album() {
        let mut buf = format!(
            "{}{}{}",
            item_xml("636f7265", "6d696e6d", "Hey Jude"),
            item_xml("636f7265", "61736172", "The Beatles"),
            item_xml("636f7265", "6173616c", "The Beatles 1967–1970")
        );
        let items = drain_items(&mut buf);
        let mut meta = TrackMetadata::default();
        let mut art = None;
        for item in items {
            apply_item(&mut meta, &mut art, &item);
        }
        assert_eq!(meta.title.as_deref(), Some("Hey Jude"));
        assert_eq!(meta.artist.as_deref(), Some("The Beatles"));
        assert_eq!(meta.album.as_deref(), Some("The Beatles 1967–1970"));
    }

    #[test]
    fn pict_and_pend() {
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3];
        let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
        let mut buf = format!(
            "<item><type>73736e63</type><code>50494354</code><length>{}</length><data encoding=\"base64\">{b64}</data></item>",
            jpeg.len()
        );
        let items = drain_items(&mut buf);
        let mut meta = TrackMetadata::default();
        let mut art = None;
        apply_item(&mut meta, &mut art, &items[0]);
        assert!(meta.has_artwork);
        assert_eq!(art.as_ref().unwrap().content_type, "image/jpeg");

        let mut pend = String::from(
            "<item><type>73736e63</type><code>70656e64</code><length>0</length></item>",
        );
        for item in drain_items(&mut pend) {
            apply_item(&mut meta, &mut art, &item);
        }
        assert_eq!(meta, TrackMetadata::default());
        assert!(art.is_none());
    }

    #[test]
    fn split_across_chunks() {
        let xml = item_xml("636f7265", "6d696e6d", "Helter Skelter");
        let (a, b) = xml.split_at(20);
        let mut buf = a.to_string();
        assert!(drain_items(&mut buf).is_empty());
        buf.push_str(b);
        let items = drain_items(&mut buf);
        assert_eq!(items.len(), 1);
        assert_eq!(
            String::from_utf8(items[0].data.clone()).unwrap(),
            "Helter Skelter"
        );
    }

    #[test]
    fn decode_fourcc_rejects_invalid() {
        assert!(decode_fourcc("short").is_none());
        assert!(decode_fourcc("gggggggg").is_none());
    }

    #[test]
    fn sniff_image_detects_png_and_unknown() {
        assert_eq!(sniff_image(&[0x89, 0x50, 0x4E, 0x47, 0]), "image/png");
        assert_eq!(sniff_image(&[0x00, 0x01]), "application/octet-stream");
    }

    #[test]
    fn oversized_artwork_is_ignored() {
        let huge = vec![0xFF, 0xD8, 0xFF];
        let mut data = huge;
        data.resize(MAX_ARTWORK_BYTES + 1, 0);
        let item = MetaItem {
            type_code: "ssnc".into(),
            code: "PICT".into(),
            data,
        };
        let mut meta = TrackMetadata::default();
        let mut art = None;
        assert!(!apply_item(&mut meta, &mut art, &item));
        assert!(!meta.has_artwork);
    }

    #[test]
    fn parse_and_apply_remaining_codes() {
        assert!(parse_item("not xml").is_none());
        assert!(decode_fourcc("zzzzzzzz").is_none());

        let mut buf = String::from("garbage<item><type>xx</type></item>");
        assert!(drain_items(&mut buf).is_empty());

        let mut incomplete = String::from("<item><type>636f7265</type>");
        assert!(drain_items(&mut incomplete).is_empty());
        assert!(incomplete.starts_with("<item>"));

        let mut meta = TrackMetadata::default();
        let mut art = Some(Artwork {
            bytes: vec![1],
            content_type: "image/png".into(),
        });
        let clip = MetaItem {
            type_code: "ssnc".into(),
            code: "clip".into(),
            data: b"iPhone".to_vec(),
        };
        assert!(apply_item(&mut meta, &mut art, &clip));
        assert_eq!(meta.source.as_deref(), Some("iPhone"));

        let mdst = MetaItem {
            type_code: "ssnc".into(),
            code: "mdst".into(),
            data: vec![],
        };
        meta.title = Some("x".into());
        assert!(apply_item(&mut meta, &mut art, &mdst));
        assert!(meta.title.is_none());

        let pfls = MetaItem {
            type_code: "ssnc".into(),
            code: "pfls".into(),
            data: vec![],
        };
        assert!(apply_item(&mut meta, &mut art, &pfls));
        assert!(art.is_none());

        assert!(utf8_opt(b"   ").is_none());
        assert_eq!(utf8_opt(b" hi ").as_deref(), Some("hi"));
        assert!(tag_value("<x>", "data").is_none());
        assert_eq!(sniff_image(&[0xFF, 0xD8, 0xFF, 1]), "image/jpeg");

        assert!(parse_prgr(b"nope").is_none());
        assert!(parse_astm(&[1, 2]).is_none());
        let prgr = MetaItem {
            type_code: "ssnc".into(),
            code: "prgr".into(),
            data: b"4294967196/50/44100".to_vec(),
        };
        assert!(apply_item(&mut meta, &mut art, &prgr));
        assert!((meta.position_s().unwrap() - (50u32.wrapping_sub(4294967196) as f32 / 44100.0)).abs() < 1e-3);
        assert!(meta.duration_s().unwrap() > 0.0);
        let astm = MetaItem {
            type_code: "core".into(),
            code: "astm".into(),
            data: 180_000u32.to_be_bytes().to_vec(),
        };
        assert!(apply_item(&mut meta, &mut art, &astm));
        assert!((meta.duration_s().unwrap() - 180.0).abs() < 1e-6);
        let bad_prgr = MetaItem {
            type_code: "ssnc".into(),
            code: "prgr".into(),
            data: b"1/2".to_vec(),
        };
        assert!(!apply_item(&mut meta, &mut art, &bad_prgr));
        let bad_astm = MetaItem {
            type_code: "core".into(),
            code: "astm".into(),
            data: vec![1],
        };
        assert!(!apply_item(&mut meta, &mut art, &bad_astm));
        assert!(apply_item(&mut meta, &mut art, &mdst));
        assert!(meta.duration_s().is_none());
        assert!(meta.position_s().is_none());
    }
}
