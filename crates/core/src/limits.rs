//! Per-app RAM limits ("keep Chrome under 2 GB").
//!
//! Windows can cap a process's working set: pages over the cap go to the
//! standby list or page file instead of failing, so the app keeps running,
//! just slower if the cap is too tight. An app is many processes, so the
//! [`Limiter`] splits the app's cap between them in proportion to their size,
//! tightens when the app goes over, hands unused room to the processes that
//! need it, and lets go once the app is well under its cap.
//!
//! This file only decides; the memory source applies the changes.

use std::collections::HashMap;

use crate::MIB;

/// No process is squeezed below this; it would just thrash.
pub const FLOOR: u64 = 32 * MIB;
/// Limits come off once the app falls this far under its cap.
const RELEASE_BELOW: f64 = 0.8;
/// Unused room worth handing to processes at their limit, as part of the cap.
const SLACK_WORTH_SHARING: u64 = 20; // 1/20 = 5%
/// Only tighten a limit by at least 1/50 (2%), so tiny swings don't churn.
const MIN_TIGHTEN: u64 = 50;

/// One process of a limited app.
#[derive(Clone, Copy, Debug)]
pub struct ProcessUsage {
    pub pid: u32,
    /// All RAM the process maps; the cap applies to this.
    pub working_set: u64,
    /// RAM only this process uses; what the app's size (and the cap) counts.
    pub private: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// Cap the process's working set at `max` bytes.
    Set { pid: u32, max: u64 },
    /// Remove the cap.
    Clear { pid: u32 },
}

/// Keeps one app under its cap. Remembers the limit given to each process.
#[derive(Debug, Default)]
pub struct Limiter {
    limits: HashMap<u32, u64>,
}

impl Limiter {
    /// Decides what to change, given the app's processes right now.
    pub fn plan(&mut self, cap: u64, processes: &[ProcessUsage]) -> Vec<Change> {
        self.limits.retain(|pid, _| processes.iter().any(|p| p.pid == *pid));
        let total: u64 = processes.iter().map(|p| p.private).sum();
        let mut changes = Vec::new();

        if total > cap {
            // Shrink every process by the same factor. Scaling the working
            // set scales its private part along with it.
            let factor = cap as f64 / total as f64;
            for p in processes {
                let target = ((p.working_set as f64 * factor) as u64).max(FLOOR);
                let tighter = match self.limits.get(&p.pid) {
                    Some(&max) => target < max - max / MIN_TIGHTEN,
                    None => true,
                };
                if tighter {
                    self.limits.insert(p.pid, target);
                    changes.push(Change::Set { pid: p.pid, max: target });
                }
            }
        } else if (total as f64) < cap as f64 * RELEASE_BELOW {
            changes.extend(self.release().into_iter().map(|pid| Change::Clear { pid }));
        } else if !self.limits.is_empty() {
            // Processes that shrank left room under the cap; give it to the
            // ones pressing against their limit (say, the tab now in use).
            let slack = cap - total;
            let pinned: Vec<u32> = processes
                .iter()
                .filter(|p| self.limits.get(&p.pid).is_some_and(|&max| p.working_set >= max / 10 * 9))
                .map(|p| p.pid)
                .collect();
            if slack > cap / SLACK_WORTH_SHARING && !pinned.is_empty() {
                let extra = slack / pinned.len() as u64;
                for pid in pinned {
                    let max = self.limits.get_mut(&pid).expect("pinned processes have limits");
                    *max += extra;
                    changes.push(Change::Set { pid, max: *max });
                }
            }
        }
        changes
    }

    /// Forgets every limit; returns the processes to clear.
    pub fn release(&mut self) -> Vec<u32> {
        self.limits.drain().map(|(pid, _)| pid).collect()
    }

    pub fn is_active(&self) -> bool {
        !self.limits.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GIB;

    fn process(pid: u32, private_mib: u64) -> ProcessUsage {
        // Shared DLLs add a little on top of the private part.
        ProcessUsage { pid, working_set: private_mib * MIB * 5 / 4, private: private_mib * MIB }
    }

    fn maxes(changes: &[Change]) -> Vec<(u32, u64)> {
        changes
            .iter()
            .filter_map(|c| match *c {
                Change::Set { pid, max } => Some((pid, max / MIB)),
                Change::Clear { .. } => None,
            })
            .collect()
    }

    #[test]
    fn leaves_apps_under_the_cap_alone() {
        let mut limiter = Limiter::default();
        assert!(limiter.plan(2 * GIB, &[process(1, 800), process(2, 400)]).is_empty());
        assert!(!limiter.is_active());
    }

    #[test]
    fn shrinks_every_process_in_proportion() {
        let mut limiter = Limiter::default();
        // 3 GB of private memory under a 2 GB cap: everything scales by 2/3.
        let changes = limiter.plan(2 * GIB, &[process(1, 2048), process(2, 1024)]);
        assert_eq!(maxes(&changes), vec![(1, 1706), (2, 853)]);
        assert!(limiter.is_active());
    }

    #[test]
    fn never_squeezes_below_the_floor() {
        let mut limiter = Limiter::default();
        let changes = limiter.plan(100 * MIB, &[process(1, 20), process(2, 2000)]);
        assert!(maxes(&changes).iter().all(|&(_, max)| max >= FLOOR / MIB));
        // Already at the floor: nothing more to do, even though it's still over.
        assert!(limiter.plan(100 * MIB, &[process(1, 20), process(2, 2000)]).is_empty());
    }

    #[test]
    fn holds_steady_once_at_the_cap() {
        let mut limiter = Limiter::default();
        limiter.plan(2 * GIB, &[process(1, 2048), process(2, 1024)]);
        // Clamped, the app sits just under its cap.
        assert!(limiter.plan(2 * GIB, &[process(1, 1360), process(2, 680)]).is_empty());
    }

    #[test]
    fn new_processes_get_limited_too() {
        let mut limiter = Limiter::default();
        limiter.plan(2 * GIB, &[process(1, 2048), process(2, 1024)]);
        let changes = limiter.plan(2 * GIB, &[process(1, 1365), process(2, 682), process(3, 500)]);
        assert!(maxes(&changes).iter().any(|&(pid, _)| pid == 3));
    }

    #[test]
    fn shares_room_with_processes_at_their_limit() {
        let mut limiter = Limiter::default();
        limiter.plan(2 * GIB, &[process(1, 2048), process(2, 1024)]);
        // Process 2 shrank on its own; process 1 is pressing on its limit.
        let changes = limiter.plan(2 * GIB, &[process(1, 1365), process(2, 300)]);
        let given = maxes(&changes);
        assert_eq!(given.len(), 1);
        assert_eq!(given[0].0, 1);
        assert!(given[0].1 > 1706);
    }

    #[test]
    fn lets_go_when_well_under() {
        let mut limiter = Limiter::default();
        limiter.plan(2 * GIB, &[process(1, 2048), process(2, 1024)]);
        let changes = limiter.plan(2 * GIB, &[process(1, 900)]);
        assert_eq!(changes, vec![Change::Clear { pid: 1 }]);
        assert!(!limiter.is_active());
    }
}
