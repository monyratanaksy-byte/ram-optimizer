//! Fake but lively data, so the UI can be built and previewed on any OS.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use crate::{AppUsage, MemorySnapshot, MemorySource, StartupEntry, GIB, MIB};

const DRIFT_INTERVAL: Duration = Duration::from_millis(900);
const PAGE_FILE: u64 = 8 * GIB;
/// Chrome "leaks" this fraction per tick so the leak detector has something to find.
const CHROME_LEAK: f64 = 0.0005;
/// Trimmed apps page memory back in as they run; this much of the gap per tick.
const REGROW: f64 = 0.01;

struct MockApp {
    usage: AppUsage,
    /// Underlying size; `usage.bytes` jitters around it so the numbers look live
    /// without random-walking away.
    base: f64,
    /// What the app settles back to after a trim.
    natural: f64,
}

pub struct MockSource {
    total: u64,
    /// Kernel, drivers and everything not listed as an app.
    system: u64,
    compressed: u64,
    apps: Vec<MockApp>,
    low_priority: HashSet<String>,
    foreground: Option<String>,
    startup: Vec<StartupEntry>,
    rng: u64,
    last_drift: Instant,
}

impl Default for MockSource {
    fn default() -> Self {
        Self::new()
    }
}

impl MockSource {
    pub fn new() -> Self {
        let app = |name: &str, mib: u64, idle_minutes: Option<u32>| MockApp {
            usage: AppUsage {
                name: name.to_string(),
                bytes: mib * MIB,
                idle_minutes,
                foreground: false,
            },
            base: (mib * MIB) as f64,
            natural: (mib * MIB) as f64,
        };
        Self {
            total: 32 * GIB,
            system: 9 * GIB,
            compressed: 620 * MIB,
            apps: vec![
                app("Chrome", 2_150, None),
                app("Steam", 940, Some(17)),
                app("Discord", 860, Some(42)),
                app("Visual Studio Code", 780, None),
                app("Teams", 690, Some(128)),
                app("Spotify", 410, Some(65)),
                app("Explorer", 160, None),
            ],
            low_priority: HashSet::new(),
            foreground: None,
            startup: [
                ("Discord", true, Some("Discord")),
                ("Steam", true, Some("Steam")),
                ("Spotify", false, Some("Spotify")),
                ("Microsoft OneDrive", true, None),
                ("Teams", true, Some("Teams")),
            ]
            .into_iter()
            .map(|(name, enabled, app)| StartupEntry {
                id: name.to_string(),
                name: name.to_string(),
                enabled,
                app: app.map(str::to_string),
            })
            .collect(),
            rng: 0x2545_F491_4F6C_DD1D,
            last_drift: Instant::now(),
        }
    }

    /// Pretends the user switched to `app` (tests only; the demo has none in front).
    pub fn set_foreground(&mut self, app: Option<&str>) {
        self.foreground = app.map(str::to_string);
    }

    pub fn is_low_priority(&self, app: &str) -> bool {
        self.low_priority.contains(app)
    }

    fn index(&self, app: &str) -> Result<usize, String> {
        self.apps
            .iter()
            .position(|a| a.usage.name == app)
            .ok_or_else(|| format!("{app} isn't running"))
    }

    /// xorshift64, mapped to [0, 1).
    fn next_unit(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }

    fn drift(&mut self) {
        if self.last_drift.elapsed() < DRIFT_INTERVAL {
            return;
        }
        self.last_drift = Instant::now();
        for i in 0..self.apps.len() {
            let jitter = (self.next_unit() - 0.5) * 0.01;
            let app = &mut self.apps[i];
            if app.usage.name == "Chrome" {
                app.natural *= 1.0 + CHROME_LEAK;
            }
            app.base += (app.natural - app.base) * REGROW;
            app.usage.bytes = (app.base * (1.0 + jitter)) as u64;
        }
    }
}

impl MemorySource for MockSource {
    fn snapshot(&mut self) -> MemorySnapshot {
        self.drift();
        let apps: u64 = self.apps.iter().map(|a| a.usage.bytes).sum();
        let used = (self.system + self.compressed + apps).min(self.total);
        MemorySnapshot {
            total: self.total,
            used,
            compressed: self.compressed,
            // Windows keeps a little over half the remaining RAM as standby cache.
            cached: (self.total - used) * 11 / 20,
            committed: used + used / 3 + 2 * GIB,
            commit_limit: self.total + PAGE_FILE,
        }
    }

    fn apps(&mut self) -> Vec<AppUsage> {
        let mut apps: Vec<AppUsage> = self
            .apps
            .iter()
            .map(|a| AppUsage {
                foreground: self.foreground.as_deref() == Some(a.usage.name.as_str()),
                ..a.usage.clone()
            })
            .collect();
        apps.sort_by_key(|a| std::cmp::Reverse(a.bytes));
        apps
    }

    fn trim(&mut self, name: &str) -> Result<u64, String> {
        let app = self
            .apps
            .iter_mut()
            .find(|a| a.usage.name == name)
            .ok_or_else(|| format!("{name} isn't running"))?;
        let freed = app.usage.bytes * 45 / 100;
        app.base *= 0.55;
        app.usage.bytes -= freed;
        Ok(freed)
    }

    fn set_background_priority(&mut self, app: &str, low: bool) -> Result<(), String> {
        if !self.apps.iter().any(|a| a.usage.name == app) {
            return Err(format!("{app} isn't running"));
        }
        if low {
            self.low_priority.insert(app.to_string());
        } else {
            self.low_priority.remove(app);
        }
        Ok(())
    }

    fn clear_standby(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn close(&mut self, app: &str) -> Result<bool, String> {
        let index = self.index(app)?;
        match app {
            // Like the real apps, these hide in the tray instead of quitting...
            "Discord" | "Spotify" => Ok(true),
            // ...and this one has no window open at all.
            "Steam" => Ok(false),
            _ => {
                self.apps.remove(index);
                Ok(true)
            }
        }
    }

    fn force_quit(&mut self, app: &str) -> Result<(), String> {
        let index = self.index(app)?;
        self.apps.remove(index);
        Ok(())
    }

    fn startup_entries(&mut self) -> Vec<StartupEntry> {
        self.startup.clone()
    }

    fn set_startup_enabled(&mut self, id: &str, enabled: bool) -> Result<(), String> {
        let entry = self
            .startup
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| format!("{id} isn't a startup app"))?;
        entry.enabled = enabled;
        Ok(())
    }

    fn restore_all(&mut self) {
        self.low_priority.clear();
    }

    fn enforce_limits(&mut self, caps: &BTreeMap<String, u64>) -> Vec<String> {
        for app in &mut self.apps {
            let Some(&cap) = caps.get(&app.usage.name) else { continue };
            // Like the real limit, the app settles just under its cap.
            let ceiling = cap as f64 * 0.97;
            if app.base > ceiling {
                app.base = ceiling;
                app.usage.bytes = app.usage.bytes.min(ceiling as u64);
            }
        }
        Vec::new()
    }
}
