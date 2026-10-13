//! Per-minute memory history for the last 24 hours, with which app was the
//! biggest at each peak, plus per-app sizes for the last hour.

use std::collections::{HashMap, VecDeque};

use crate::AppUsage;

pub const MINUTES_PER_DAY: usize = 24 * 60;
const SAMPLES_PER_MINUTE: u32 = 60;
const APP_MINUTES: usize = 60;

#[derive(Clone, Debug, PartialEq)]
pub struct MinuteSample {
    /// Average used-RAM ratio over the minute.
    pub average: f32,
    /// Highest used-RAM ratio seen in the minute.
    pub peak: f32,
    /// Largest app when the peak was recorded.
    pub top_app: Option<(String, u64)>,
}

#[derive(Default)]
struct Accumulator {
    sum: f32,
    count: u32,
    peak: f32,
    top_app: Option<(String, u64)>,
}

#[derive(Default)]
pub struct LongHistory {
    /// Oldest first; the last entry is the most recent complete minute.
    minutes: VecDeque<MinuteSample>,
    current: Accumulator,
    /// Each app's size at the end of each of the last 60 minutes.
    app_minutes: HashMap<String, VecDeque<u64>>,
}

/// The highest point in a range.
#[derive(Clone, Debug, PartialEq)]
pub struct Peak {
    pub ratio: f32,
    pub minutes_ago: usize,
    pub top_app: Option<(String, u64)>,
}

impl LongHistory {
    /// Call once per second (the sample rate the minutes assume).
    pub fn record(&mut self, used_ratio: f32, apps: &[AppUsage]) {
        let current = &mut self.current;
        current.sum += used_ratio;
        current.count += 1;
        if used_ratio >= current.peak {
            current.peak = used_ratio;
            current.top_app = apps.first().map(|a| (a.name.clone(), a.bytes));
        }
        if current.count >= SAMPLES_PER_MINUTE {
            self.close_minute(apps);
        }
    }

    fn close_minute(&mut self, apps: &[AppUsage]) {
        let current = std::mem::take(&mut self.current);
        if self.minutes.len() == MINUTES_PER_DAY {
            self.minutes.pop_front();
        }
        self.minutes.push_back(MinuteSample {
            average: current.sum / current.count.max(1) as f32,
            peak: current.peak,
            top_app: current.top_app,
        });

        self.app_minutes.retain(|name, _| apps.iter().any(|a| &a.name == name));
        for app in apps {
            let series = self.app_minutes.entry(app.name.clone()).or_default();
            if series.len() == APP_MINUTES {
                series.pop_front();
            }
            series.push_back(app.bytes);
        }
    }

    /// Per-minute averages for the last `minutes` minutes, oldest first.
    pub fn averages(&self, minutes: usize) -> Vec<f32> {
        let skip = self.minutes.len().saturating_sub(minutes);
        self.minutes.iter().skip(skip).map(|m| m.average).collect()
    }

    pub fn peak(&self, minutes: usize) -> Option<Peak> {
        let skip = self.minutes.len().saturating_sub(minutes);
        let len = self.minutes.len();
        self.minutes
            .iter()
            .enumerate()
            .skip(skip)
            .max_by(|(_, a), (_, b)| a.peak.total_cmp(&b.peak))
            .map(|(index, sample)| Peak {
                ratio: sample.peak,
                minutes_ago: len - 1 - index,
                top_app: sample.top_app.clone(),
            })
    }

    /// How much an app grew over the last hour, if it has been tracked for
    /// most of it and has grown steadily (each quarter higher than the last).
    pub fn steady_growth(&self, app: &str) -> Option<u64> {
        let series = self.app_minutes.get(app)?;
        if series.len() < APP_MINUTES * 3 / 4 {
            return None;
        }
        let quarter = series.len() / 4;
        let averages: Vec<u64> = series
            .iter()
            .collect::<Vec<_>>()
            .chunks(quarter)
            .take(4)
            .map(|chunk| chunk.iter().copied().sum::<u64>() / chunk.len() as u64)
            .collect();
        if !averages.windows(2).all(|pair| pair[1] > pair[0]) {
            return None;
        }
        Some(series.back()?.saturating_sub(*series.front()?))
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

    fn record_minutes(history: &mut LongHistory, minutes: usize, ratio: f32, apps: &[AppUsage]) {
        for _ in 0..minutes * SAMPLES_PER_MINUTE as usize {
            history.record(ratio, apps);
        }
    }

    #[test]
    fn averages_each_minute() {
        let mut history = LongHistory::default();
        record_minutes(&mut history, 2, 0.5, &[]);
        assert_eq!(history.averages(60), vec![0.5, 0.5]);
    }

    #[test]
    fn finds_peak_and_its_biggest_app() {
        let mut history = LongHistory::default();
        record_minutes(&mut history, 3, 0.4, &[app("Chrome", 100)]);
        record_minutes(&mut history, 1, 0.9, &[app("Game", 9_000)]);
        record_minutes(&mut history, 2, 0.5, &[app("Chrome", 100)]);

        let peak = history.peak(60).unwrap();
        assert_eq!(peak.ratio, 0.9);
        assert_eq!(peak.minutes_ago, 2);
        assert_eq!(peak.top_app, Some(("Game".into(), 9_000)));
    }

    #[test]
    fn keeps_one_day() {
        let mut history = LongHistory::default();
        record_minutes(&mut history, MINUTES_PER_DAY + 5, 0.3, &[]);
        assert_eq!(history.averages(usize::MAX).len(), MINUTES_PER_DAY);
    }

    #[test]
    fn spots_steady_growth_but_not_noise() {
        let mut history = LongHistory::default();
        for minute in 0..60u64 {
            let wobble = if minute % 2 == 0 { 50 * MIB } else { 0 };
            let apps = [app("Leaky", 500 * MIB + minute * 20 * MIB), app("Noisy", 800 * MIB + wobble)];
            record_minutes(&mut history, 1, 0.5, &apps);
        }
        assert_eq!(history.steady_growth("Leaky"), Some(59 * 20 * MIB));
        assert_eq!(history.steady_growth("Noisy"), None);
    }
}
