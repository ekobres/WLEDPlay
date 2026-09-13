//! Presentation timing compensation (future use).

#[derive(Debug, Clone)]
pub struct SyncOffset {
    pub offset_ms: i32,
}

impl SyncOffset {
    pub fn new(offset_ms: i32) -> Self {
        Self { offset_ms }
    }

    pub fn delay(&self) -> std::time::Duration {
        if self.offset_ms > 0 {
            std::time::Duration::from_millis(self.offset_ms as u64)
        } else {
            std::time::Duration::ZERO
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SyncOffset;

    #[test]
    fn positive_offset_delays() {
        let s = SyncOffset::new(250);
        assert_eq!(s.delay(), std::time::Duration::from_millis(250));
    }

    #[test]
    fn non_positive_offset_is_zero_delay() {
        assert_eq!(SyncOffset::new(0).delay(), std::time::Duration::ZERO);
        assert_eq!(SyncOffset::new(-500).delay(), std::time::Duration::ZERO);
    }
}
