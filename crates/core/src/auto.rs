//! Frees memory automatically when usage crosses a threshold.

use std::time::{Duration, Instant};

/// Gaps between runs the user can pick, in seconds.
pub const INTERVALS: [u32; 6] = [5, 10, 15, 30, 60, 300];
/// Below this, idle apps that wake up keep reloading from disk.
pub const SHORT_INTERVAL: Duration = Duration::from_secs(30);

/// The allowed interval closest to `seconds`.
pub fn nearest_interval(seconds: u32) -> u32 {
    INTERVALS
        .into_iter()
        .min_by_key(|&option| option.abs_diff(seconds))
        .expect("INTERVALS isn't empty")
}

pub struct AutoClear {
    pub enabled: bool,
    /// Used-RAM ratio (0..1) that triggers a run.
    pub threshold: f32,
    /// Minimum gap between runs, so a threshold below normal usage doesn't
    /// trim apps nonstop (which would make them slower, not faster).
    pub cooldown: Duration,
    /// When the last run happened and how many bytes it freed.
    last_run: Option<(Instant, u64)>,
}

impl AutoClear {
    pub fn new(threshold: f32, cooldown: Duration) -> Self {
        Self {
            enabled: true,
            threshold,
            cooldown,
            last_run: None,
        }
    }

    pub fn is_due(&self, used_ratio: f32, now: Instant) -> bool {
        self.enabled
            && used_ratio >= self.threshold
            && self
                .last_run
                .is_none_or(|(at, _)| now.duration_since(at) >= self.cooldown)
    }

    pub fn record_run(&mut self, now: Instant, freed: u64) {
        self.last_run = Some((now, freed));
    }

    pub fn last_run(&self) -> Option<(Instant, u64)> {
        self.last_run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COOLDOWN: Duration = Duration::from_secs(300);

    #[test]
    fn runs_only_above_threshold() {
        let auto = AutoClear::new(0.8, COOLDOWN);
        let now = Instant::now();
        assert!(!auto.is_due(0.79, now));
        assert!(auto.is_due(0.80, now));
    }

    #[test]
    fn respects_enabled_flag() {
        let mut auto = AutoClear::new(0.5, COOLDOWN);
        auto.enabled = false;
        assert!(!auto.is_due(0.9, Instant::now()));
    }

    #[test]
    fn snaps_to_an_allowed_interval() {
        assert_eq!(nearest_interval(0), 5);
        assert_eq!(nearest_interval(12), 10);
        assert_eq!(nearest_interval(200), 300);
        assert_eq!(nearest_interval(9999), 300);
    }

    #[test]
    fn waits_for_cooldown() {
        let mut auto = AutoClear::new(0.5, COOLDOWN);
        let start = Instant::now();
        auto.record_run(start, 1);
        assert!(!auto.is_due(0.9, start + Duration::from_secs(299)));
        assert!(auto.is_due(0.9, start + COOLDOWN));
    }
}
