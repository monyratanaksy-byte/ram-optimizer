//! Everything the app knows and does, independent of drawing it.

use std::collections::{HashSet, VecDeque};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use ram_core::alerts::{Alert, AlertRules};
use ram_core::auto::AutoClear;
use ram_core::checkup::{self, Finding};
use ram_core::closing::{CloseState, CloseTracker};
use ram_core::game::{Detection, GameDetector, GameMode};
use ram_core::history::LongHistory;
use ram_core::settings::Settings;
use ram_core::trend::GrowthTracker;
use ram_core::{
    format_size, trim_background, trim_idle, AppUsage, MemorySnapshot, MemorySource, StartupEntry,
};
use slint::VecModel;

use crate::{AppRow, FindingRow, GamingRow, StartupRow};

/// Samples in the one-minute history (one per second).
pub const RECENT_LEN: usize = 60;
pub const AUTO_COOLDOWN: Duration = Duration::from_secs(5 * 60);
const STATUS_DURATION: Duration = Duration::from_secs(4);
const GROWTH_WINDOW: Duration = Duration::from_secs(60);

pub struct State {
    pub source: Box<dyn MemorySource>,
    pub settings: Settings,
    pub snap: MemorySnapshot,
    pub apps: Vec<AppUsage>,
    pub growth: GrowthTracker,
    /// Used-RAM ratio, one sample per second, oldest first.
    pub recent: VecDeque<f32>,
    pub long: LongHistory,
    pub auto: AutoClear,
    pub game: GameMode,
    detector: GameDetector,
    pub closing: CloseTracker,
    /// Close-when-gaming apps to force quit if they ignore the close request.
    force_if_ignored: HashSet<String>,
    alerts: AlertRules,
    /// New alerts, waiting to be shown as notifications.
    pub pending_alerts: Vec<Alert>,
    pub startup: Vec<StartupEntry>,
    pub findings: Vec<Finding>,
    checkup: Option<Receiver<String>>,
    pub status: Option<(String, Instant)>,

    // Models the UI's lists are bound to.
    pub app_rows: Rc<VecModel<AppRow>>,
    pub startup_rows: Rc<VecModel<StartupRow>>,
    pub gaming_rows: Rc<VecModel<GamingRow>>,
    pub finding_rows: Rc<VecModel<FindingRow>>,
}

impl State {
    pub fn new(mut source: Box<dyn MemorySource>, settings: Settings) -> Self {
        let mut auto = AutoClear::new(settings.auto_threshold as f32 / 100.0, AUTO_COOLDOWN);
        auto.enabled = settings.auto_clear;
        Self {
            snap: source.snapshot(),
            apps: source.apps(),
            source,
            settings,
            growth: GrowthTracker::new(GROWTH_WINDOW),
            recent: VecDeque::with_capacity(RECENT_LEN),
            long: LongHistory::default(),
            auto,
            game: GameMode::default(),
            detector: GameDetector::default(),
            closing: CloseTracker::default(),
            force_if_ignored: HashSet::new(),
            alerts: AlertRules::default(),
            pending_alerts: Vec::new(),
            startup: Vec::new(),
            findings: Vec::new(),
            checkup: None,
            status: None,
            app_rows: Rc::new(VecModel::default()),
            startup_rows: Rc::new(VecModel::default()),
            gaming_rows: Rc::new(VecModel::default()),
            finding_rows: Rc::new(VecModel::default()),
        }
    }

    /// Re-reads memory. Only the once-a-second tick records history, so
    /// clicks don't skew it.
    pub fn poll(&mut self, record: bool) {
        self.snap = self.source.snapshot();
        self.apps = self.source.apps();
        for (name, bytes) in self.closing.update(&self.apps, Instant::now()) {
            self.force_if_ignored.remove(&name);
            self.set_status(format!("Closed {name}: {} freed", format_size(bytes)));
        }
        if record {
            if self.recent.len() == RECENT_LEN {
                self.recent.pop_front();
            }
            self.recent.push_back(self.snap.used_ratio());
            self.growth.record(&self.apps);
            self.long.record(self.snap.used_ratio(), &self.apps);
        }
    }

    /// Everything that happens once a second.
    pub fn tick(&mut self) {
        self.poll(true);
        self.update_auto_game();
        self.tick_game_mode();
        self.force_ignored_closes();
        self.run_auto_clear();
        self.check_alerts();
        self.poll_checkup();
    }

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some((message.into(), Instant::now()));
    }

    /// The status line, cleared once it has been shown for a few seconds.
    pub fn current_status(&mut self) -> &str {
        if self.status.as_ref().is_some_and(|(_, at)| at.elapsed() >= STATUS_DURATION) {
            self.status = None;
        }
        self.status.as_ref().map_or("", |(message, _)| message.as_str())
    }

    pub fn optimize(&mut self) {
        let (freed, count) = trim_background(self.source.as_mut(), &self.apps);
        self.set_status(format!("Freed {} from {count} background apps", format_size(freed)));
    }

    /// Runs auto-clear if usage is over the threshold and the cooldown has
    /// passed. Paused during game mode, which manages memory itself.
    fn run_auto_clear(&mut self) {
        let ratio = self.snap.used_ratio();
        let now = Instant::now();
        if self.game.is_active() || !self.auto.is_due(ratio, now) {
            return;
        }
        let (freed, _) = trim_idle(self.source.as_mut(), &self.apps);
        self.auto.record_run(now, freed);
        self.set_status(format!("Auto-cleared {} at {:.0}%", format_size(freed), ratio * 100.0));
        self.poll(false);
    }

    fn check_alerts(&mut self) {
        if !self.settings.alerts {
            return;
        }
        let alerts = self.alerts.check(&self.snap, &self.apps, &self.long, Instant::now());
        self.pending_alerts.extend(alerts);
    }

    fn update_auto_game(&mut self) {
        if !self.settings.auto_game_mode {
            return;
        }
        let game = self.source.foreground_game();
        let Self { source, detector, game: mode, .. } = self;
        let detection = detector.update(
            game.as_deref(),
            |name| source.is_running(name),
            mode.is_active(),
            Instant::now(),
        );
        match detection {
            Some(Detection::Start(name)) => {
                let summary = self.start_game_mode();
                self.set_status(format!("{name} detected. {summary}"));
            }
            Some(Detection::Stop(name)) => {
                let restored = self.game.stop(self.source.as_mut());
                self.set_status(format!("{name} closed. Game mode off, {restored} apps back to normal"));
            }
            None => {}
        }
    }

    fn tick_game_mode(&mut self) {
        let freed = self
            .game
            .tick(self.source.as_mut(), &self.apps, &self.snap, Instant::now());
        if let Some(freed) = freed {
            self.set_status(format!("RAM was tight: game mode freed {}", format_size(freed)));
            self.poll(false);
        }
    }

    /// Turns game mode on or off by hand.
    pub fn set_game_mode(&mut self, on: bool) {
        if on {
            let summary = self.start_game_mode();
            self.set_status(summary);
        } else {
            let restored = self.game.stop(self.source.as_mut());
            self.set_status(format!("Game mode off: {restored} apps back to normal"));
        }
    }

    /// Starts game mode, including closing the apps picked for it. Returns a
    /// short summary for the status line.
    fn start_game_mode(&mut self) -> String {
        let report = self.game.start(self.source.as_mut(), &self.apps);
        let now = Instant::now();
        let mut closed = 0;
        for name in self.settings.close_when_gaming.clone() {
            let Some(app) = self.apps.iter().find(|a| a.name == name && !a.foreground).cloned() else {
                continue;
            };
            if let Ok(asked_window) = self.source.close(&name) {
                self.closing.requested(&app, asked_window, now);
                self.force_if_ignored.insert(name);
                closed += 1;
            }
        }
        let cache = if report.cache_cleared { " + cache" } else { "" };
        let closing = if closed > 0 { format!(", closing {closed}") } else { String::new() };
        format!(
            "Game mode: {} lowered, {}{cache} freed{closing}",
            report.lowered,
            format_size(report.freed)
        )
    }

    /// Apps the user picked for closing during game mode get force quit if
    /// they ignore the close request (most tray apps do).
    fn force_ignored_closes(&mut self) {
        let now = Instant::now();
        for name in self.force_if_ignored.clone() {
            match self.closing.state(&name, now) {
                Some(CloseState::CanForce) => {
                    if self.source.force_quit(&name).is_ok() {
                        self.closing.forced(&name, now);
                    }
                }
                Some(CloseState::Closing) => {}
                None => {
                    self.force_if_ignored.remove(&name);
                }
            }
        }
    }

    pub fn close(&mut self, name: &str) {
        let Some(app) = self.apps.iter().find(|a| a.name == name).cloned() else {
            return;
        };
        match self.source.close(name) {
            Ok(asked_window) => {
                self.closing.requested(&app, asked_window, Instant::now());
                if asked_window {
                    self.set_status(format!("Asked {name} to close"));
                } else {
                    self.set_status(format!("{name} has no window. Force quit to close it."));
                }
            }
            Err(err) => self.set_status(err),
        }
    }

    pub fn force_quit(&mut self, name: &str) {
        match self.source.force_quit(name) {
            Ok(()) => self.closing.forced(name, Instant::now()),
            Err(err) => self.set_status(err),
        }
    }

    pub fn refresh_startup(&mut self) {
        self.startup = self.source.startup_entries();
    }

    pub fn set_startup(&mut self, id: &str, enabled: bool) {
        let name = self
            .startup
            .iter()
            .find(|e| e.id == id)
            .map_or(id.to_string(), |e| e.name.clone());
        match self.source.set_startup_enabled(id, enabled) {
            Ok(()) if enabled => self.set_status(format!("{name} will start with Windows")),
            Ok(()) => self.set_status(format!("{name} won't start with Windows")),
            Err(err) => self.set_status(err),
        }
        self.refresh_startup();
    }

    pub fn set_close_when_gaming(&mut self, name: &str, close: bool) {
        let list = &mut self.settings.close_when_gaming;
        list.retain(|n| n != name);
        if close {
            list.push(name.to_string());
        }
        self.save_settings();
    }

    pub fn save_settings(&mut self) {
        if let Err(err) = self.settings.save() {
            self.set_status(format!("Couldn't save settings: {err}"));
        }
    }

    /// Runs the check-up in the background; results arrive via `tick`.
    pub fn start_checkup(&mut self) {
        if self.checkup.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(checkup::gather_facts());
        });
        self.checkup = Some(receiver);
    }

    pub fn checkup_running(&self) -> bool {
        self.checkup.is_some()
    }

    fn poll_checkup(&mut self) {
        let Some(receiver) = &self.checkup else { return };
        match receiver.try_recv() {
            Ok(facts) => {
                self.findings = checkup::evaluate(&facts, &self.snap);
                self.checkup = None;
            }
            Err(TryRecvError::Disconnected) => self.checkup = None,
            Err(TryRecvError::Empty) => {}
        }
    }

    /// Puts every app's priority back; call before exiting.
    pub fn shut_down(&mut self) {
        self.game.stop(self.source.as_mut());
        self.source.restore_all();
    }
}
