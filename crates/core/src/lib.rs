//! Memory stats and actions, independent of any UI.
//!
//! The UI talks to a [`MemorySource`]: [`win::WindowsSource`] reads and acts on
//! the real system, [`mock::MockSource`] provides demo data on any OS.

pub mod alerts;
pub mod auto;
pub mod checkup;
pub mod closing;
pub mod game;
pub mod history;
pub mod mock;
pub mod settings;
pub mod trend;
pub mod update;
#[cfg(windows)]
pub mod win;

pub const MIB: u64 = 1024 * 1024;
pub const GIB: u64 = 1024 * MIB;

#[derive(Clone, Debug)]
pub struct MemorySnapshot {
    /// Installed physical RAM.
    pub total: u64,
    /// Physical RAM in use, including `compressed` (excludes the standby cache).
    pub used: u64,
    /// RAM holding compressed pages; part of `used`.
    pub compressed: u64,
    /// Standby cache: file data kept around in case it's needed again.
    /// Windows hands it to apps instantly, so it is effectively free.
    pub cached: u64,
    /// Memory programs have reserved (Windows "commit charge").
    pub committed: u64,
    /// RAM plus page file: the most that can be committed.
    pub commit_limit: u64,
}

impl MemorySnapshot {
    /// RAM holding nothing at all.
    pub fn free(&self) -> u64 {
        self.total.saturating_sub(self.used).saturating_sub(self.cached)
    }

    /// RAM apps can get right now: free plus cache.
    pub fn available(&self) -> u64 {
        self.total.saturating_sub(self.used)
    }

    pub fn used_ratio(&self) -> f32 {
        ratio(self.used, self.total)
    }

    pub fn commit_ratio(&self) -> f32 {
        ratio(self.committed, self.commit_limit)
    }

    pub fn pressure(&self) -> Pressure {
        match self.used_ratio() {
            r if r >= 0.85 => Pressure::High,
            r if r >= 0.70 => Pressure::Busy,
            _ => Pressure::Healthy,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pressure {
    Healthy,
    Busy,
    High,
}

/// One app, with all of its processes grouped together.
#[derive(Clone, Debug)]
pub struct AppUsage {
    pub name: String,
    pub bytes: u64,
    /// Minutes since the app was last in the foreground, once it has been in
    /// the background long enough to count as idle; `None` otherwise.
    pub idle_minutes: Option<u32>,
    /// The app the user is currently using.
    pub foreground: bool,
}

/// A program set to launch when Windows starts.
#[derive(Clone, Debug, PartialEq)]
pub struct StartupEntry {
    /// Opaque handle for [`MemorySource::set_startup_enabled`].
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// The app it launches, matching [`AppUsage::name`], to show its RAM use.
    pub app: Option<String>,
}

pub trait MemorySource {
    fn snapshot(&mut self) -> MemorySnapshot;
    /// Apps sorted largest first.
    fn apps(&mut self) -> Vec<AppUsage>;
    /// Pushes an app's memory out of RAM; returns the bytes freed. Windows
    /// pages it back in when the app needs it again.
    fn trim(&mut self, app: &str) -> Result<u64, String>;
    /// `low` tells Windows to page this app out first when RAM gets tight;
    /// `false` restores the priority it had before.
    fn set_background_priority(&mut self, app: &str, low: bool) -> Result<(), String>;
    /// Empties the standby cache. Needs administrator rights on Windows.
    fn clear_standby(&mut self) -> Result<(), String>;
    /// Asks the app to close its windows, as if the user clicked their close
    /// buttons, so it can still offer to save work. Returns `false` if the app
    /// has no window to close (typical for apps living in the tray).
    fn close(&mut self, app: &str) -> Result<bool, String>;
    /// Ends the app immediately, like Task Manager's "End task".
    fn force_quit(&mut self, app: &str) -> Result<(), String>;
    /// Undoes every priority change, so nothing stays lowered after exit.
    fn restore_all(&mut self) {}
    /// The app in front, if it looks like a game.
    fn foreground_game(&mut self) -> Option<String> {
        None
    }
    /// Whether any process of this app is running, including ones too small
    /// or too protected to be listed.
    fn is_running(&mut self, app: &str) -> bool {
        self.apps().iter().any(|a| a.name == app)
    }
    fn startup_entries(&mut self) -> Vec<StartupEntry> {
        Vec::new()
    }
    /// Turns a startup entry on or off the same way Task Manager does.
    fn set_startup_enabled(&mut self, _id: &str, _enabled: bool) -> Result<(), String> {
        Err("Startup apps can only be changed on Windows".into())
    }
}

/// Trims apps that have been in the background long enough to count as idle.
/// Returns bytes freed and how many apps were trimmed.
pub fn trim_idle(source: &mut dyn MemorySource, apps: &[AppUsage]) -> (u64, usize) {
    trim_where(source, apps, |a| a.idle_minutes.is_some() && !a.foreground)
}

/// Trims every app except the one in front. For explicit user requests.
pub fn trim_background(source: &mut dyn MemorySource, apps: &[AppUsage]) -> (u64, usize) {
    trim_where(source, apps, |a| !a.foreground)
}

fn trim_where(
    source: &mut dyn MemorySource,
    apps: &[AppUsage],
    pick: impl Fn(&AppUsage) -> bool,
) -> (u64, usize) {
    apps.iter()
        .filter(|a| pick(a))
        .filter_map(|a| source.trim(&a.name).ok())
        .fold((0, 0), |(freed, count), bytes| (freed + bytes, count + 1))
}

pub fn to_gb(bytes: u64) -> f64 {
    bytes as f64 / GIB as f64
}

pub fn format_size(bytes: u64) -> String {
    if bytes >= GIB {
        format!("{:.1} GB", to_gb(bytes))
    } else {
        format!("{:.0} MB", bytes as f64 / MIB as f64)
    }
}

fn ratio(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(used: u64, cached: u64) -> MemorySnapshot {
        MemorySnapshot {
            total: 32 * GIB,
            used,
            compressed: 0,
            cached,
            committed: 0,
            commit_limit: 0,
        }
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_size(860 * MIB), "860 MB");
        assert_eq!(format_size(2 * GIB + 100 * MIB), "2.1 GB");
    }

    #[test]
    fn ratios_handle_zero() {
        let snap = MemorySnapshot {
            total: 0,
            used: 0,
            compressed: 0,
            cached: 0,
            committed: 0,
            commit_limit: 0,
        };
        assert_eq!(snap.used_ratio(), 0.0);
        assert_eq!(snap.commit_ratio(), 0.0);
    }

    #[test]
    fn cache_counts_as_available_not_free() {
        let snap = snapshot(12 * GIB, 10 * GIB);
        assert_eq!(snap.free(), 10 * GIB);
        assert_eq!(snap.available(), 20 * GIB);
    }

    #[test]
    fn pressure_levels() {
        assert_eq!(snapshot(16 * GIB, 0).pressure(), Pressure::Healthy);
        assert_eq!(snapshot(24 * GIB, 0).pressure(), Pressure::Busy);
        assert_eq!(snapshot(28 * GIB, 0).pressure(), Pressure::High);
    }
}
