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
    pub stream_type: Option<String>,
    pub stream_description: Option<String>,
    pub has_artwork: bool,
    pub rtp_start: Option<u32>,
    pub rtp_current: Option<u32>,
    pub rtp_end: Option<u32>,
    pub duration_ms: Option<u32>,
    /// Wall clock when `rtp_current` was sampled (`prgr` is not a 1 Hz stream).
    pub progress_unix_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Artwork {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

pub type SharedArtwork = Arc<Mutex<Option<Artwork>>>;

pub const RTP_PROGRESS_RATE: f32 = 44100.0;

pub fn stream_encoding(kind: Option<&str>, description: Option<&str>) -> String {
    match (nonempty(kind), nonempty(description)) {
        (Some(kind), Some(description)) => format!("{kind} · {description}"),
        (Some(kind), None) => kind.to_string(),
        (None, Some(description)) => description.to_string(),
        (None, None) => String::new(),
    }
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    match value {
        Some(text) if !text.is_empty() => Some(text),
        _ => None,
    }
}

impl TrackMetadata {
    pub fn duration_s(&self) -> Option<f32> {
        if let Some(ms) = self.duration_ms.filter(|&ms| ms > 0) {
            return Some(ms as f32 / 1000.0);
        }
        let d = rtp_secs(self.rtp_start?, self.rtp_end?);
        (d > 0.0).then_some(d)
    }

    pub fn position_s(&self) -> Option<f32> {
        Some(rtp_secs(self.rtp_start?, self.rtp_current?))
    }

    fn clear_progress(&mut self) {
        self.rtp_start = None;
        self.rtp_current = None;
        self.rtp_end = None;
        self.duration_ms = None;
        self.progress_unix_ms = None;
    }
}

pub fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Extrapolate a frozen `prgr` snapshot. Shairport only emits progress on play/stop.
pub fn interpolated_position(
    base_s: Option<f32>,
    progress_unix_ms: Option<u64>,
    duration_s: Option<f32>,
) -> Option<f32> {
    let base = base_s?;
    let extra = progress_unix_ms
        .map(|t| unix_ms_now().saturating_sub(t) as f32 / 1000.0)
        .unwrap_or(0.0);
    let pos = (base + extra).max(0.0);
    Some(match duration_s.filter(|d| *d > 0.0) {
        Some(d) => pos.min(d),
        None => pos,
    })
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
            let title = utf8_opt(&item.data);
            if matches!((&meta.title, &title), (Some(old), Some(new)) if old != new) {
                meta.clear_progress();
            }
            meta.title = title;
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
        ("ssnc", "styp") => {
            meta.stream_type = utf8_opt(&item.data);
            true
        }
        ("ssnc", "sdsc") => {
            meta.stream_description = utf8_opt(&item.data);
            true
        }
        ("ssnc", "mdst") | ("ssnc", "mden") => true,
        ("ssnc", "prgr") => {
            if let Some((start, current, end)) = parse_prgr(&item.data) {
                meta.rtp_start = Some(start);
                meta.rtp_current = Some(current);
                meta.rtp_end = Some(end);
                meta.progress_unix_ms = Some(unix_ms_now());
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

/// One Shairport `mdst` … `mden` bundle. Fields are published together at the end.
#[derive(Debug, Clone, Default)]
pub struct MetaBatch {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub artwork: Option<Artwork>,
    pub saw_title: bool,
    pub saw_artist: bool,
    pub saw_album: bool,
    pub saw_art: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemEffect {
    Ignored,
    Applied,
    Staged,
    Published,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accept {
    pub effect: ItemEffect,
    pub art_changed: bool,
}

impl Accept {
    fn of(effect: ItemEffect) -> Self {
        Self {
            effect,
            art_changed: false,
        }
    }
}

pub fn artwork_stamp(art: &Option<Artwork>) -> u64 {
    let Some(art) = art else {
        return 0;
    };
    let mut hash = (art.bytes.len() as u64).wrapping_add(1);
    for byte in &art.bytes {
        hash = hash.wrapping_mul(131).wrapping_add(u64::from(*byte));
    }
    hash
}

fn commit_batch(meta: &mut TrackMetadata, artwork: &mut Option<Artwork>, batch: MetaBatch) -> bool {
    let identity = batch.saw_title || batch.saw_artist || batch.saw_album;
    let mut identity_changed = false;
    if identity {
        let title = if batch.saw_title {
            batch.title.clone()
        } else {
            None
        };
        let artist = if batch.saw_artist {
            batch.artist.clone()
        } else {
            None
        };
        let album = if batch.saw_album {
            batch.album.clone()
        } else {
            None
        };
        identity_changed = meta.title != title || meta.artist != artist || meta.album != album;
        if meta.title != title {
            meta.clear_progress();
        }
        meta.title = title;
        meta.artist = artist;
        meta.album = album;
    }
    if batch.saw_art {
        let changed = artwork_stamp(artwork) != artwork_stamp(&batch.artwork);
        if changed {
            *artwork = batch.artwork;
        }
        meta.has_artwork = artwork.is_some();
        changed
    } else if identity_changed {
        let changed = artwork.is_some();
        *artwork = None;
        meta.has_artwork = false;
        changed
    } else {
        false
    }
}

/// Apply one item. Inside an open bundle, title, artist, album, and art wait for `mden`.
pub fn accept_item(
    meta: &mut TrackMetadata,
    artwork: &mut Option<Artwork>,
    batch: &mut Option<MetaBatch>,
    item: &MetaItem,
) -> Accept {
    match (item.type_code.as_str(), item.code.as_str()) {
        ("ssnc", "mdst") => {
            let mut published = Accept::of(ItemEffect::Applied);
            if let Some(pending) = batch.take() {
                published.art_changed = commit_batch(meta, artwork, pending);
                published.effect = ItemEffect::Published;
            }
            *batch = Some(MetaBatch::default());
            published
        }
        ("ssnc", "mden") => {
            if let Some(pending) = batch.take() {
                Accept {
                    effect: ItemEffect::Published,
                    art_changed: commit_batch(meta, artwork, pending),
                }
            } else {
                Accept::of(ItemEffect::Applied)
            }
        }
        ("core", "minm") if batch.is_some() => {
            let pending = batch.as_mut().unwrap();
            pending.title = utf8_opt(&item.data);
            pending.saw_title = true;
            Accept::of(ItemEffect::Staged)
        }
        ("core", "asar") if batch.is_some() => {
            let pending = batch.as_mut().unwrap();
            pending.artist = utf8_opt(&item.data);
            pending.saw_artist = true;
            Accept::of(ItemEffect::Staged)
        }
        ("core", "asal") if batch.is_some() => {
            let pending = batch.as_mut().unwrap();
            pending.album = utf8_opt(&item.data);
            pending.saw_album = true;
            Accept::of(ItemEffect::Staged)
        }
        ("ssnc", "PICT") if batch.is_some() => {
            if item.data.is_empty() || item.data.len() > MAX_ARTWORK_BYTES {
                return Accept::of(ItemEffect::Ignored);
            }
            let pending = batch.as_mut().unwrap();
            pending.artwork = Some(Artwork {
                content_type: sniff_image(&item.data).to_string(),
                bytes: item.data.clone(),
            });
            pending.saw_art = true;
            Accept::of(ItemEffect::Staged)
        }
        _ => {
            let before = artwork_stamp(artwork);
            if apply_item(meta, artwork, item) {
                Accept {
                    effect: ItemEffect::Applied,
                    art_changed: before != artwork_stamp(artwork),
                }
            } else {
                Accept::of(ItemEffect::Ignored)
            }
        }
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
    if let Ok(text) = std::str::from_utf8(data) {
        let text = text.trim();
        if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
            return text.parse().ok().filter(|&ms| ms > 0);
        }
    }
    let ms = match data.len() {
        4 => u32::from_be_bytes(data.try_into().ok()?),
        8 => u32::try_from(u64::from_be_bytes(data.try_into().ok()?)).ok()?,
        _ => return None,
    };
    (ms > 0).then_some(ms)
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

    fn raw_item(type_code: &str, code: &str, data: &[u8]) -> MetaItem {
        MetaItem {
            type_code: type_code.into(),
            code: code.into(),
            data: data.to_vec(),
        }
    }

    #[test]
    fn bundle_holds_the_new_title_until_the_artist_and_picture_arrive() {
        let mut meta = TrackMetadata {
            artist: Some("Old".into()),
            album: Some("Old Album".into()),
            title: Some("Old Song".into()),
            has_artwork: true,
            rtp_start: Some(1),
            ..TrackMetadata::default()
        };
        let mut art = Some(Artwork {
            bytes: vec![1, 2, 3],
            content_type: "image/jpeg".into(),
        });
        let old_stamp = artwork_stamp(&art);
        let mut batch = None;
        let opened = accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("ssnc", "mdst", &[]),
        );
        assert_eq!(opened.effect, ItemEffect::Applied);
        assert!(!opened.art_changed);
        accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("core", "minm", b"Oh Yeah"),
        );
        assert_eq!(meta.title.as_deref(), Some("Old Song"));
        assert_eq!(meta.artist.as_deref(), Some("Old"));
        let committed = accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("ssnc", "mdst", &[]),
        );
        assert_eq!(committed.effect, ItemEffect::Published);
        assert!(committed.art_changed);
        assert_eq!(meta.title.as_deref(), Some("Oh Yeah"));
        assert!(meta.artist.is_none());
        assert!(meta.album.is_none());
        assert!(art.is_none());
        assert!(meta.rtp_start.is_none());

        accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("core", "asar", b"Yello"),
        );
        accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("core", "asal", b"Stella"),
        );
        let huge = vec![9u8; MAX_ARTWORK_BYTES + 1];
        let ignored = accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("ssnc", "PICT", &huge),
        );
        assert_eq!(ignored.effect, ItemEffect::Ignored);
        let empty = accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("ssnc", "PICT", &[]),
        );
        assert_eq!(empty.effect, ItemEffect::Ignored);
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 4, 5];
        accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("ssnc", "PICT", &jpeg),
        );
        let published = accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("ssnc", "mden", &[]),
        );
        assert_eq!(published.effect, ItemEffect::Published);
        assert!(published.art_changed);
        assert_eq!(meta.artist.as_deref(), Some("Yello"));
        assert_eq!(meta.album.as_deref(), Some("Stella"));
        assert_eq!(
            art.as_ref().map(|item| item.bytes.as_slice()),
            Some(&jpeg[..])
        );
        assert_ne!(artwork_stamp(&art), old_stamp);
        assert_eq!(artwork_stamp(&None), 0);

        let bare = accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("ssnc", "mden", &[]),
        );
        assert_eq!(bare.effect, ItemEffect::Applied);
        assert!(!bare.art_changed);
        let unknown = accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("core", "zzzz", b"x"),
        );
        assert_eq!(unknown.effect, ItemEffect::Ignored);
    }

    #[test]
    fn empty_bundle_keeps_the_current_track_and_cover() {
        let mut meta = TrackMetadata {
            title: Some("Stay".into()),
            has_artwork: true,
            ..TrackMetadata::default()
        };
        let mut art = Some(Artwork {
            bytes: vec![9],
            content_type: "image/png".into(),
        });
        let mut batch = None;
        accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("ssnc", "mdst", &[]),
        );
        let done = accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("ssnc", "mden", &[]),
        );
        assert_eq!(done.effect, ItemEffect::Published);
        assert!(!done.art_changed);
        assert_eq!(meta.title.as_deref(), Some("Stay"));
        assert!(art.is_some());
        let outside = accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("ssnc", "PICT", &[0xFF, 0xD8, 0xFF, 1]),
        );
        assert_eq!(outside.effect, ItemEffect::Applied);
        assert!(outside.art_changed);
        assert!(meta.has_artwork);
        accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("ssnc", "mdst", &[]),
        );
        accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("ssnc", "PICT", &[0xFF, 0xD8, 0xFF, 1]),
        );
        let same = accept_item(
            &mut meta,
            &mut art,
            &mut batch,
            &raw_item("ssnc", "mden", &[]),
        );
        assert!(!same.art_changed);
        assert!(meta.has_artwork);
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
        let styp = MetaItem {
            type_code: "ssnc".into(),
            code: "styp".into(),
            data: b"ALAC".to_vec(),
        };
        let sdsc = MetaItem {
            type_code: "ssnc".into(),
            code: "sdsc".into(),
            data: b"44100/16/2".to_vec(),
        };
        assert!(apply_item(&mut meta, &mut art, &styp));
        assert!(apply_item(&mut meta, &mut art, &sdsc));
        assert_eq!(
            stream_encoding(
                meta.stream_type.as_deref(),
                meta.stream_description.as_deref()
            ),
            "ALAC · 44100/16/2"
        );
        assert_eq!(stream_encoding(Some("AAC"), None), "AAC");
        assert_eq!(stream_encoding(None, Some("AAC-LC")), "AAC-LC");
        assert_eq!(stream_encoding(Some(""), Some("")), "");
        assert_eq!(stream_encoding(None, None), "");

        let mdst = MetaItem {
            type_code: "ssnc".into(),
            code: "mdst".into(),
            data: vec![],
        };
        meta.title = Some("x".into());
        assert!(apply_item(&mut meta, &mut art, &mdst));
        assert_eq!(meta.title.as_deref(), Some("x"));

        let pfls = MetaItem {
            type_code: "ssnc".into(),
            code: "pfls".into(),
            data: vec![],
        };
        assert!(apply_item(&mut meta, &mut art, &pfls));
        assert!(art.is_none());
        assert!(meta.stream_type.is_none());
        assert!(meta.stream_description.is_none());
        assert_eq!(meta.source.as_deref(), Some("iPhone"));

        assert!(utf8_opt(b"   ").is_none());
        assert_eq!(utf8_opt(b" hi ").as_deref(), Some("hi"));
        assert!(tag_value("<x>", "data").is_none());
        assert_eq!(sniff_image(&[0xFF, 0xD8, 0xFF, 1]), "image/jpeg");

        assert!(parse_prgr(b"nope").is_none());
        assert!(parse_astm(&[1, 2]).is_none());
        assert!(parse_astm(&[0, 0, 0, 0]).is_none());
        assert!(parse_astm(b"abc").is_none());
        assert!(parse_astm(b"nope!").is_none());
        assert!(parse_astm(b"").is_none());
        assert!(parse_astm(b"  ").is_none());
        assert!(parse_astm(b"0").is_none());
        let prgr = MetaItem {
            type_code: "ssnc".into(),
            code: "prgr".into(),
            data: b"4294967196/50/44100".to_vec(),
        };
        assert!(apply_item(&mut meta, &mut art, &prgr));
        assert!(
            (meta.position_s().unwrap() - (50u32.wrapping_sub(4294967196) as f32 / 44100.0)).abs()
                < 1e-3
        );
        assert!(meta.duration_s().unwrap() > 0.0);
        let astm = MetaItem {
            type_code: "core".into(),
            code: "astm".into(),
            data: 180_000u32.to_be_bytes().to_vec(),
        };
        assert!(apply_item(&mut meta, &mut art, &astm));
        assert!((meta.duration_s().unwrap() - 180.0).abs() < 1e-6);
        assert!(apply_item(&mut meta, &mut art, &mdst));
        assert!((meta.duration_s().unwrap() - 180.0).abs() < 1e-6);
        assert!(meta.position_s().is_some());
        let same_title = MetaItem {
            type_code: "core".into(),
            code: "minm".into(),
            data: b"Same".to_vec(),
        };
        assert!(apply_item(&mut meta, &mut art, &same_title));
        assert!(meta.position_s().is_some());
        assert!(apply_item(&mut meta, &mut art, &same_title));
        assert!(meta.position_s().is_some());
        let new_title = MetaItem {
            type_code: "core".into(),
            code: "minm".into(),
            data: b"Other".to_vec(),
        };
        assert!(apply_item(&mut meta, &mut art, &new_title));
        assert!(meta.position_s().is_none());
        assert!(meta.duration_s().is_none());
        meta.duration_ms = Some(0);
        meta.rtp_start = Some(10);
        meta.rtp_end = Some(10);
        assert!(meta.duration_s().is_none());
        let astm8 = MetaItem {
            type_code: "core".into(),
            code: "astm".into(),
            data: 90_000u64.to_be_bytes().to_vec(),
        };
        assert!(apply_item(&mut meta, &mut art, &astm8));
        assert!((meta.duration_s().unwrap() - 90.0).abs() < 1e-6);
        let astm_ascii = MetaItem {
            type_code: "core".into(),
            code: "astm".into(),
            data: b"45000".to_vec(),
        };
        assert!(apply_item(&mut meta, &mut art, &astm_ascii));
        assert!((meta.duration_s().unwrap() - 45.0).abs() < 1e-6);
        let zero_astm = MetaItem {
            type_code: "core".into(),
            code: "astm".into(),
            data: 0u32.to_be_bytes().to_vec(),
        };
        assert!(!apply_item(&mut meta, &mut art, &zero_astm));
        assert!((meta.duration_s().unwrap() - 45.0).abs() < 1e-6);
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
        let huge_astm = MetaItem {
            type_code: "core".into(),
            code: "astm".into(),
            data: u64::MAX.to_be_bytes().to_vec(),
        };
        assert!(!apply_item(&mut meta, &mut art, &huge_astm));
        assert!(
            (interpolated_position(Some(1.0), Some(unix_ms_now()), Some(3.0)).unwrap() - 1.0).abs()
                < 0.05
        );
        assert!(
            (interpolated_position(
                Some(1.0),
                Some(unix_ms_now().saturating_sub(2000)),
                Some(10.0)
            )
            .unwrap()
                - 3.0)
                .abs()
                < 0.2
        );
        assert!(interpolated_position(None, None, None).is_none());
        assert_eq!(
            interpolated_position(Some(12.0), None, Some(10.0)).unwrap(),
            10.0
        );
        assert_eq!(interpolated_position(Some(1.25), None, None).unwrap(), 1.25);
    }
}
