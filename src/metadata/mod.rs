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
}

#[derive(Debug, Clone)]
pub struct Artwork {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

pub type SharedArtwork = Arc<Mutex<Option<Artwork>>>;

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
            true
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
        assert_eq!(String::from_utf8(items[0].data.clone()).unwrap(), "Helter Skelter");
    }
}
