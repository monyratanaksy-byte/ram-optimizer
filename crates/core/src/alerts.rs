//! Decides when to notify: memory about to run out, or an app that keeps
//! growing (a likely leak). Each alert fires once, not every second.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::history::LongHistory;
use crate::{format_size, AppUsage, MemorySnapshot, GIB};

/// Alert when RAM or the commit limit is this full...
const LOW_MEMORY_AT: f32 = 0.90;
/// ...and only again after usage drops back below this.
const REARM_BELOW: f32 = 0.80;
/// An app growing this much in an hour gets flagged.
const GROWTH_ALERT: u64 = GIB;
const GROWTH_REPEAT: Duration = Duration::from_secs(2 * 60 * 60);

#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    pub title: String,
    pub body: String,
}

pub struct AlertRules {
    low_memory_armed: bool,
    growth_alerted: HashMap<String, Instant>,
}

impl Default for AlertRules {
    fn default() -> Self {
        Self {
            low_memory_armed: true,
            growth_alerted: HashMap::new(),
        }
    }
}

impl AlertRules {
    pub fn check(
        &mut self,
        snap: &MemorySnapshot,
        apps: &[AppUsage],
        history: &LongHistory,
        now: Instant,
    ) -> Vec<Alert> {
        let mut alerts = Vec::new();

        let fullest = snap.used_ratio().max(snap.commit_ratio());
        if fullest >= LOW_MEMORY_AT && self.low_memory_armed {
            self.low_memory_armed = false;
            let biggest = apps
                .first()
                .map(|a| format!(" {} is using the most ({}).", a.name, format_size(a.bytes)))
                .unwrap_or_default();
            alerts.push(Alert {
                title: format!("Memory is {:.0}% full", fullest * 100.0),
                body: format!("Apps may slow down or crash soon.{biggest}"),
            });
        } else if fullest < REARM_BELOW {
            self.low_memory_armed = true;
        }

        for app in apps {
            let Some(grown) = history.steady_growth(&app.name) else { continue };
            let recently = self
                .growth_alerted
                .get(&app.name)
                .is_some_and(|at| now.duration_since(*at) < GROWTH_REPEAT);
            if grown < GROWTH_ALERT || recently {
                continue;
            }
            self.growth_alerted.insert(app.name.clone(), now);
            alerts.push(Alert {
                title: format!("{} keeps growing", app.name),
                body: format!(
                    "It grew {} in the last hour and now uses {}. Restarting it would free that memory.",
                    format_size(grown),
                    format_size(app.bytes)
                ),
            });
        }

        alerts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(used_ratio: f32) -> MemorySnapshot {
        let total = 32 * GIB;
        MemorySnapshot {
            total,
            used: (total as f32 * used_ratio) as u64,
            compressed: 0,
            cached: 0,
            committed: 0,
            commit_limit: 64 * GIB,
        }
    }

    #[test]
    fn low_memory_fires_once_until_usage_drops() {
        let mut rules = AlertRules::default();
        let history = LongHistory::default();
        let now = Instant::now();

        assert_eq!(rules.check(&snapshot(0.92), &[], &history, now).len(), 1);
        assert!(rules.check(&snapshot(0.95), &[], &history, now).is_empty());
        assert!(rules.check(&snapshot(0.85), &[], &history, now).is_empty());
        rules.check(&snapshot(0.70), &[], &history, now);
        assert_eq!(rules.check(&snapshot(0.91), &[], &history, now).len(), 1);
    }
}
