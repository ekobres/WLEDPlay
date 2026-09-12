//! Optional AirPlay metadata (future).

#[derive(Debug, Clone, Default)]
pub struct TrackMetadata {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
}
