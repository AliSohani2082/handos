pub const COAST_US: u64 = 150_000;
pub const MIN_PRESENCE: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Tracked,
    Coasting,
    Lost,
}

pub fn quality(accepted: bool, last_accepted_us: Option<u64>, now_us: u64) -> Quality {
    if accepted {
        Quality::Tracked
    } else if last_accepted_us.is_some_and(|t| now_us.saturating_sub(t) <= COAST_US) {
        Quality::Coasting
    } else {
        Quality::Lost
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coasting_expires() {
        assert_eq!(quality(true, Some(100), 100), Quality::Tracked);
        assert_eq!(quality(false, Some(100), 150_100), Quality::Coasting);
        assert_eq!(quality(false, Some(100), 150_101), Quality::Lost);
    }
}
