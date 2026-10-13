//! Spots apps whose memory keeps climbing, a common sign of a leak.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::AppUsage;

/// Fewest samples before a rate is reported, so one spike isn't called a leak.
const MIN_SAMPLES: usize = 10;

pub struct GrowthTracker {
    window: Duration,
    samples: HashMap<String, VecDeque<(Instant, u64)>>,
}

impl GrowthTracker {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            samples: HashMap::new(),
        }
    }

    pub fn record(&mut self, apps: &[AppUsage]) {
        self.record_at(Instant::now(), apps);
    }

    fn record_at(&mut self, now: Instant, apps: &[AppUsage]) {
        self.samples.retain(|name, _| apps.iter().any(|a| &a.name == name));
        for app in apps {
            let series = self.samples.entry(app.name.clone()).or_default();
            series.push_back((now, app.bytes));
            while series
                .front()
                .is_some_and(|(at, _)| now.duration_since(*at) > self.window)
            {
                series.pop_front();
            }
        }
    }

    /// Growth in bytes per minute over the window (least-squares slope),
    /// or `None` until there is enough history.
    pub fn rate_per_minute(&self, name: &str) -> Option<f64> {
        let series = self.samples.get(name)?;
        if series.len() < MIN_SAMPLES {
            return None;
        }
        let (start, _) = series.front()?;
        let points: Vec<(f64, f64)> = series
            .iter()
            .map(|(at, bytes)| (at.duration_since(*start).as_secs_f64() / 60.0, *bytes as f64))
            .collect();
        let n = points.len() as f64;
        let mean_x = points.iter().map(|p| p.0).sum::<f64>() / n;
        let mean_y = points.iter().map(|p| p.1).sum::<f64>() / n;
        let var_x: f64 = points.iter().map(|p| (p.0 - mean_x).powi(2)).sum();
        if var_x == 0.0 {
            return None;
        }
        let cov: f64 = points.iter().map(|p| (p.0 - mean_x) * (p.1 - mean_y)).sum();
        Some(cov / var_x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MIB;

    fn app(name: &str, bytes: u64) -> AppUsage {
        AppUsage {
            name: name.into(),
            bytes,
            idle_minutes: None,
            foreground: false,
        }
    }

    #[test]
    fn detects_steady_growth() {
        let mut tracker = GrowthTracker::new(Duration::from_secs(60));
        let start = Instant::now();
        for sec in 0..20u64 {
            let at = start + Duration::from_secs(sec);
            tracker.record_at(at, &[app("leaky", 500 * MIB + sec * MIB), app("flat", 300 * MIB)]);
        }
        let leaky = tracker.rate_per_minute("leaky").unwrap();
        assert!((leaky - 60.0 * MIB as f64).abs() < MIB as f64);
        assert!(tracker.rate_per_minute("flat").unwrap().abs() < 1.0);
    }

    #[test]
    fn needs_enough_samples() {
        let mut tracker = GrowthTracker::new(Duration::from_secs(60));
        tracker.record(&[app("new", MIB)]);
        assert_eq!(tracker.rate_per_minute("new"), None);
    }

    #[test]
    fn forgets_closed_apps() {
        let mut tracker = GrowthTracker::new(Duration::from_secs(60));
        tracker.record(&[app("gone", MIB)]);
        tracker.record(&[]);
        assert!(tracker.samples.is_empty());
    }
}
