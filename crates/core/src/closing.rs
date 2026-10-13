//! Tracks apps the user asked to close, so the UI can show progress, offer a
//! force quit when an app ignores the request, and report what was freed.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::AppUsage;

/// How long an app gets to close itself before force quit is offered.
pub const FORCE_AFTER: Duration = Duration::from_secs(4);
/// Stop tracking an app that is still running after this long.
const GIVE_UP_AFTER: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseState {
    /// Waiting for the app to exit.
    Closing,
    /// The app ignored the request (or has no window); offer force quit.
    CanForce,
}

struct Pending {
    since: Instant,
    /// Last known size, reported as freed once the app is gone.
    bytes: u64,
    forced: bool,
}

#[derive(Default)]
pub struct CloseTracker {
    pending: HashMap<String, Pending>,
}

impl CloseTracker {
    /// Records a close request. `asked_window` is false when the app had no
    /// window to close (typical for tray apps), so force quit is offered at once.
    pub fn requested(&mut self, app: &AppUsage, asked_window: bool, now: Instant) {
        let since = if asked_window {
            now
        } else {
            now.checked_sub(FORCE_AFTER).unwrap_or(now)
        };
        self.pending.insert(
            app.name.clone(),
            Pending {
                since,
                bytes: app.bytes,
                forced: false,
            },
        );
    }

    pub fn forced(&mut self, name: &str, now: Instant) {
        if let Some(pending) = self.pending.get_mut(name) {
            pending.forced = true;
            pending.since = now;
        }
    }

    pub fn state(&self, name: &str, now: Instant) -> Option<CloseState> {
        let pending = self.pending.get(name)?;
        if pending.forced || now.duration_since(pending.since) < FORCE_AFTER {
            Some(CloseState::Closing)
        } else {
            Some(CloseState::CanForce)
        }
    }

    /// Call after each refresh. Returns apps that have exited, with the
    /// memory they were using.
    pub fn update(&mut self, apps: &[AppUsage], now: Instant) -> Vec<(String, u64)> {
        let mut exited = Vec::new();
        self.pending.retain(|name, pending| {
            if !apps.iter().any(|a| &a.name == name) {
                exited.push((name.clone(), pending.bytes));
                return false;
            }
            now.duration_since(pending.since) < GIVE_UP_AFTER
        });
        exited
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MIB;

    fn app(name: &str) -> AppUsage {
        AppUsage {
            name: name.into(),
            bytes: 500 * MIB,
            idle_minutes: None,
            foreground: false,
        }
    }

    #[test]
    fn offers_force_quit_when_app_ignores_close() {
        let mut tracker = CloseTracker::default();
        let now = Instant::now();
        tracker.requested(&app("Discord"), true, now);

        assert_eq!(tracker.state("Discord", now), Some(CloseState::Closing));
        assert_eq!(tracker.state("Discord", now + FORCE_AFTER), Some(CloseState::CanForce));
    }

    #[test]
    fn offers_force_quit_at_once_without_a_window() {
        let mut tracker = CloseTracker::default();
        let now = Instant::now();
        tracker.requested(&app("Steam"), false, now);

        assert_eq!(tracker.state("Steam", now), Some(CloseState::CanForce));
    }

    #[test]
    fn reports_exited_apps_with_their_size() {
        let mut tracker = CloseTracker::default();
        let now = Instant::now();
        tracker.requested(&app("Chrome"), true, now);

        assert!(tracker.update(&[app("Chrome")], now).is_empty());
        assert_eq!(tracker.update(&[], now), vec![("Chrome".to_string(), 500 * MIB)]);
        assert_eq!(tracker.state("Chrome", now), None);
    }

    #[test]
    fn force_quit_shows_closing_again() {
        let mut tracker = CloseTracker::default();
        let now = Instant::now();
        tracker.requested(&app("Steam"), false, now);
        tracker.forced("Steam", now);

        assert_eq!(tracker.state("Steam", now + FORCE_AFTER * 2), Some(CloseState::Closing));
    }

    #[test]
    fn gives_up_on_apps_that_never_exit() {
        let mut tracker = CloseTracker::default();
        let now = Instant::now();
        tracker.requested(&app("Stubborn"), true, now);

        tracker.update(&[app("Stubborn")], now + GIVE_UP_AFTER);
        assert_eq!(tracker.state("Stubborn", now + GIVE_UP_AFTER), None);
    }
}
