//! Game mode: makes room for a game without fighting it.
//!
//! On start it lowers the memory priority of background apps (so Windows takes
//! memory from them first), trims the idle ones once and clears the standby
//! cache. While on, it only trims again if RAM gets genuinely tight, because
//! repeated trimming makes apps page back in from disk and causes stutter.
//! It never closes apps.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::{trim_idle, AppUsage, MemorySnapshot, MemorySource};

/// A game must stay in front this long before auto game mode kicks in.
const DETECT_AFTER: Duration = Duration::from_secs(3);

/// While on, idle background apps are trimmed again only above this usage.
const RELIEF_AT: f32 = 0.90;
const RELIEF_COOLDOWN: Duration = Duration::from_secs(180);

#[derive(Debug, Default, PartialEq)]
pub struct GameReport {
    /// Apps whose memory priority was lowered.
    pub lowered: usize,
    pub freed: u64,
    pub cache_cleared: bool,
}

#[derive(Default)]
pub struct GameMode {
    active: bool,
    lowered: HashSet<String>,
    last_relief: Option<Instant>,
}

impl GameMode {
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn start(&mut self, source: &mut dyn MemorySource, apps: &[AppUsage]) -> GameReport {
        self.active = true;
        self.last_relief = None;
        let mut report = GameReport::default();
        for app in apps.iter().filter(|a| !a.foreground) {
            if source.set_background_priority(&app.name, true).is_ok() {
                self.lowered.insert(app.name.clone());
                report.lowered += 1;
            }
        }
        report.freed = trim_idle(source, apps).0;
        report.cache_cleared = source.clear_standby().is_ok();
        report
    }

    /// Call once per refresh. Returns bytes freed when RAM was tight enough to
    /// trim again.
    pub fn tick(
        &mut self,
        source: &mut dyn MemorySource,
        apps: &[AppUsage],
        snap: &MemorySnapshot,
        now: Instant,
    ) -> Option<u64> {
        if !self.active {
            return None;
        }
        // Whatever the user switches to (most likely the game) gets its
        // normal priority back.
        for app in apps.iter().filter(|a| a.foreground) {
            if self.lowered.remove(&app.name) {
                let _ = source.set_background_priority(&app.name, false);
            }
        }
        let cooling_down = self
            .last_relief
            .is_some_and(|at| now.duration_since(at) < RELIEF_COOLDOWN);
        if snap.used_ratio() < RELIEF_AT || cooling_down {
            return None;
        }
        self.last_relief = Some(now);
        Some(trim_idle(source, apps).0)
    }

    /// Turns game mode off; returns how many apps got their priority back.
    pub fn stop(&mut self, source: &mut dyn MemorySource) -> usize {
        self.active = false;
        let count = self.lowered.len();
        for name in self.lowered.drain() {
            let _ = source.set_background_priority(&name, false);
        }
        count
    }
}

#[derive(Debug, PartialEq)]
pub enum Detection {
    Start(String),
    Stop(String),
}

/// Auto game mode: starts game mode once a game has been in front for a few
/// seconds, and stops it when that game exits. If the user turns game mode
/// off by hand, that game is left alone until it exits.
#[derive(Default)]
pub struct GameDetector {
    candidate: Option<(String, Instant)>,
    active_game: Option<String>,
    dismissed: Option<String>,
}

impl GameDetector {
    pub fn update(
        &mut self,
        foreground_game: Option<&str>,
        mut is_running: impl FnMut(&str) -> bool,
        game_mode_on: bool,
        now: Instant,
    ) -> Option<Detection> {
        if self.dismissed.as_deref().is_some_and(|game| !is_running(game)) {
            self.dismissed = None;
        }

        if let Some(game) = self.active_game.clone() {
            if !game_mode_on {
                // Turned off by hand: don't start again for this game.
                self.dismissed = self.active_game.take();
                return None;
            }
            if !is_running(&game) {
                self.active_game = None;
                return Some(Detection::Stop(game));
            }
            return None;
        }

        if game_mode_on {
            // Turned on by hand; the user is in charge.
            self.candidate = None;
            return None;
        }

        let Some(game) = foreground_game.filter(|g| self.dismissed.as_deref() != Some(*g)) else {
            self.candidate = None;
            return None;
        };
        match &self.candidate {
            Some((candidate, since)) if candidate == game => {
                if now.duration_since(*since) < DETECT_AFTER {
                    return None;
                }
                self.candidate = None;
                self.active_game = Some(game.to_string());
                Some(Detection::Start(game.to_string()))
            }
            _ => {
                self.candidate = Some((game.to_string(), now));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockSource;
    use crate::GIB;

    fn snapshot(used_ratio: f32) -> MemorySnapshot {
        let total = 32 * GIB;
        MemorySnapshot {
            total,
            used: (total as f32 * used_ratio) as u64,
            compressed: 0,
            cached: 0,
            committed: 0,
            commit_limit: 0,
        }
    }

    #[test]
    fn start_lowers_background_apps_and_trims_idle_ones() {
        let mut source = MockSource::new();
        source.set_foreground(Some("Chrome"));
        let apps = source.apps();
        let mut game = GameMode::default();

        let report = game.start(&mut source, &apps);

        assert_eq!(report.lowered, apps.len() - 1);
        assert!(report.freed > 0);
        assert!(!source.is_low_priority("Chrome"));
        assert!(source.is_low_priority("Discord"));
    }

    #[test]
    fn switching_to_an_app_restores_it() {
        let mut source = MockSource::new();
        let mut game = GameMode::default();
        let apps = source.apps();
        game.start(&mut source, &apps);
        assert!(source.is_low_priority("Steam"));

        source.set_foreground(Some("Steam"));
        let apps = source.apps();
        game.tick(&mut source, &apps, &snapshot(0.5), Instant::now());

        assert!(!source.is_low_priority("Steam"));
        assert!(source.is_low_priority("Discord"));
    }

    #[test]
    fn trims_again_only_when_ram_is_tight_and_not_too_often() {
        let mut source = MockSource::new();
        let mut game = GameMode::default();
        let apps = source.apps();
        game.start(&mut source, &apps);
        let now = Instant::now();

        assert_eq!(game.tick(&mut source, &apps, &snapshot(0.6), now), None);
        assert!(game.tick(&mut source, &apps, &snapshot(0.95), now).is_some());
        assert_eq!(game.tick(&mut source, &apps, &snapshot(0.95), now + Duration::from_secs(60)), None);
        assert!(game
            .tick(&mut source, &apps, &snapshot(0.95), now + RELIEF_COOLDOWN)
            .is_some());
    }

    #[test]
    fn detector_starts_after_a_few_seconds_and_stops_on_exit() {
        let mut detector = GameDetector::default();
        let now = Instant::now();
        let running = |_: &str| true;

        assert_eq!(detector.update(Some("Elden Ring"), running, false, now), None);
        assert_eq!(
            detector.update(Some("Elden Ring"), running, false, now + DETECT_AFTER),
            Some(Detection::Start("Elden Ring".into()))
        );
        // Alt-tabbing out doesn't stop it.
        assert_eq!(detector.update(None, running, true, now + DETECT_AFTER * 2), None);
        assert_eq!(
            detector.update(None, |_| false, true, now + DETECT_AFTER * 3),
            Some(Detection::Stop("Elden Ring".into()))
        );
    }

    #[test]
    fn detector_respects_manual_off() {
        let mut detector = GameDetector::default();
        let now = Instant::now();
        let running = |_: &str| true;
        detector.update(Some("Game"), running, false, now);
        detector.update(Some("Game"), running, false, now + DETECT_AFTER);

        // User turns game mode off while the game still runs.
        assert_eq!(detector.update(Some("Game"), running, false, now + DETECT_AFTER * 2), None);
        assert_eq!(detector.update(Some("Game"), running, false, now + DETECT_AFTER * 10), None);
    }

    #[test]
    fn stop_restores_everything() {
        let mut source = MockSource::new();
        let mut game = GameMode::default();
        let apps = source.apps();
        game.start(&mut source, &apps);

        let restored = game.stop(&mut source);

        assert_eq!(restored, apps.len());
        assert!(apps.iter().all(|a| !source.is_low_priority(&a.name)));
        assert!(!game.is_active());
    }
}
