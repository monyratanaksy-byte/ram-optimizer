// Hide the console window in Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod notify;
mod state;
mod tray;
mod update;
mod view;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use ram_core::auto::INTERVALS;
use ram_core::mock::MockSource;
use ram_core::settings::{Settings, Theme};
use ram_core::{MemorySource, GIB};
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, ModelRc, SharedString};

use state::State;
use tray::{Tray, TrayAction};

slint::include_modules!();

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const TRAY_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Command-line flags.
struct Args {
    /// Show demo data instead of the real system.
    demo: bool,
    /// Launched at sign-in: start hidden in the tray, don't ask for admin.
    startup: bool,
    /// Don't ask for admin (already asked once). Windows only.
    #[cfg_attr(not(windows), allow(dead_code))]
    no_elevate: bool,
    /// Started by an update: wait for the old copy (this process ID) to exit.
    #[cfg_attr(not(windows), allow(dead_code))]
    wait_pid: Option<u32>,
    /// Started by an update: say so.
    updated: bool,
    /// Ask the running copy to quit, then exit (used by the installer).
    #[cfg_attr(not(windows), allow(dead_code))]
    quit: bool,
    /// Quit the running copy and stop starting with Windows (uninstaller).
    #[cfg_attr(not(windows), allow(dead_code))]
    uninstall: bool,
}

impl Args {
    fn parse() -> Self {
        let has = |flag: &str| std::env::args().any(|arg| arg == flag);
        Self {
            demo: has("--demo"),
            startup: has("--startup"),
            no_elevate: has("--no-elevate"),
            wait_pid: std::env::args()
                .skip_while(|arg| arg != "--wait-pid")
                .nth(1)
                .and_then(|pid| pid.parse().ok()),
            updated: has("--updated"),
            quit: has("--quit"),
            uninstall: has("--uninstall"),
        }
    }
}

fn main() {
    std::panic::set_hook(Box::new(|info| log_error(&format!("crash: {info}"))));
    let args = Args::parse();

    #[cfg(windows)]
    if !args.demo {
        use ram_core::win::instance::SingleInstance;
        if args.quit || args.uninstall {
            // The running copy restores every app it changed on the way out.
            let quit = SingleInstance::quit_existing(Duration::from_secs(10));
            if args.uninstall {
                if let Err(err) = ram_core::win::autostart::set_enabled(false, false) {
                    log_error(&format!("removing start with Windows: {err}"));
                }
            }
            // The installer ends the old copy itself when this fails.
            std::process::exit(if quit { 0 } else { 1 });
        }
        if let Some(pid) = args.wait_pid {
            ram_core::win::wait_for_exit(pid, Duration::from_secs(15));
        }
        update::clean_up();
        // Already running: bring that copy's window up instead (but stay
        // quiet when this is just the sign-in launch).
        if SingleInstance::find_existing(!args.startup) {
            return;
        }
        // Admin rights only when the user turned admin mode on.
        let wants_admin = Settings::load().admin_mode
            && !args.startup
            && !args.no_elevate
            && !ram_core::win::WindowsSource::is_elevated();
        if wants_admin {
            let mut relaunch: Vec<String> = std::env::args().skip(1).collect();
            relaunch.push("--no-elevate".into());
            if ram_core::win::relaunch_as_admin(&relaunch) {
                return;
            }
            // Declined: carry on without admin rights.
        }
    }

    if let Err(err) = run(&args) {
        log_error(&format!("error: {err}"));
        std::process::exit(1);
    }
}

fn run(args: &Args) -> Result<(), slint::PlatformError> {
    slint::BackendSelector::new()
        .with_winit_window_attributes_hook(without_os_shadow)
        .select()?;

    let (source, demo) = memory_source(args.demo);
    #[cfg(windows)]
    let instance = if demo { None } else { ram_core::win::instance::SingleInstance::claim() };
    if !demo {
        notify::init();
    }
    if args.updated {
        notify::show("RAM Optimizer updated", &format!("Now on version {}.", update::CURRENT_VERSION));
    }
    let (update_sender, updates) = std::sync::mpsc::channel();
    // Only real Windows builds update; a Mac build must never fetch the .exe.
    let updater = if cfg!(windows) && !demo {
        update::start(update_sender)
    } else {
        update::Updater::disabled()
    };

    let ui = AppWindow::new()?;
    let settings = Settings::load();
    apply_settings_to_ui(&ui, &settings);
    // e.g. "RAM Optimizer v0.4.0 · admin", so an update is visible at a glance.
    let mode = if demo {
        " · demo data"
    } else if cfg!(windows) && is_elevated() {
        " · admin"
    } else {
        ""
    };
    ui.set_title_text(format!("RAM Optimizer v{}{mode}", update::CURRENT_VERSION).into());
    let updates_note = if cfg!(windows) && !demo { " · updates automatically" } else { "" };
    ui.set_version_text(format!("Version {}{updates_note}", update::CURRENT_VERSION).into());
    if !demo {
        set_autostart(settings.start_with_windows, settings.admin_mode);
        #[cfg(windows)]
        ram_core::win::sync_installed_version(update::CURRENT_VERSION);
    }

    let state = Rc::new(RefCell::new(State::new(source, settings)));
    {
        let state = state.borrow();
        ui.set_apps(ModelRc::from(state.app_rows.clone()));
        ui.set_startup_rows(ModelRc::from(state.startup_rows.clone()));
        ui.set_gaming_rows(ModelRc::from(state.gaming_rows.clone()));
        ui.set_findings(ModelRc::from(state.finding_rows.clone()));
    }

    let tray: Rc<RefCell<Option<Tray>>> = Rc::new(RefCell::new(None));
    // Shown once, the first time the window hides to the tray.
    let tray_hint_shown = Rc::new(Cell::new(false));

    wire_window(&ui, &state, &tray, &tray_hint_shown);
    wire_actions(&ui, &state);
    wire_settings(&ui, &state, demo);

    {
        let mut state = state.borrow_mut();
        state.poll(true);
        view::render(&ui, &mut state);
    }

    // Once a second: measure, run the automatic features, redraw.
    let refresh = slint::Timer::default();
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        let tray = tray.clone();
        #[cfg(windows)]
        let updater = updater.clone();
        let mut ready_update = None;
        refresh.start(slint::TimerMode::Repeated, REFRESH_INTERVAL, move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            if let Ok(update) = updates.try_recv() {
                ready_update = Some(update);
            }
            // Install as soon as it's verified, but not mid-game: restarting
            // would undo game mode while you play.
            if !state.game.is_active() {
                if let Some(update) = ready_update.take() {
                    install_update(&ui, &mut state, &update);
                }
            }
            state.tick();
            for alert in std::mem::take(&mut state.pending_alerts) {
                notify::show(&alert.title, &alert.body);
                state.set_status(alert.title);
            }
            view::render(&ui, &mut state);
            if let Some(tray) = tray.borrow().as_ref() {
                let tooltip = format!("RAM Optimizer · {:.0}% used", state.snap.used_ratio() * 100.0);
                tray.update(&tooltip, state.game.is_active());
            }
            #[cfg(windows)]
            if instance.as_ref().is_some_and(|i| i.show_requested()) {
                show_window(&ui, &updater);
            }
            #[cfg(windows)]
            if instance.as_ref().is_some_and(|i| i.quit_requested()) {
                quit(&mut state);
            }
        });
    }

    // The tray icon has to be created once the event loop runs (macOS).
    {
        let tray = tray.clone();
        slint::Timer::single_shot(Duration::ZERO, move || match Tray::new() {
            Ok(created) => *tray.borrow_mut() = Some(created),
            Err(err) => log_error(&format!("tray icon unavailable: {err}")),
        });
    }
    let tray_poll = slint::Timer::default();
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        let tray = tray.clone();
        let updater = updater.clone();
        tray_poll.start(slint::TimerMode::Repeated, TRAY_POLL_INTERVAL, move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let actions = tray.borrow().as_ref().map(Tray::actions).unwrap_or_default();
            for action in actions {
                let mut state = state.borrow_mut();
                match action {
                    TrayAction::Show => show_window(&ui, &updater),
                    TrayAction::Optimize => {
                        state.optimize();
                        notify::show("Optimized", state.current_status());
                    }
                    TrayAction::ToggleGameMode => {
                        let on = !state.game.is_active();
                        state.set_game_mode(on);
                    }
                    TrayAction::Quit => quit(&mut state),
                }
                state.poll(false);
                view::render(&ui, &mut state);
            }
        });
    }

    if !args.startup {
        ui.show()?;
    }
    let result = slint::run_event_loop_until_quit();
    state.borrow_mut().shut_down();
    result
}

/// Window controls: close hides to the tray, minimize, drag.
fn wire_window(
    ui: &AppWindow,
    state: &Rc<RefCell<State>>,
    tray: &Rc<RefCell<Option<Tray>>>,
    tray_hint_shown: &Rc<Cell<bool>>,
) {
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        let tray = tray.clone();
        let hint_shown = tray_hint_shown.clone();
        ui.on_close_app(move || {
            if tray.borrow().is_none() {
                quit(&mut state.borrow_mut());
            }
            if let Some(ui) = ui_weak.upgrade() {
                let _ = ui.hide();
            }
            if !hint_shown.replace(true) {
                notify::show(
                    "RAM Optimizer is still running",
                    "It keeps watching memory from the tray. Right-click the tray icon to quit.",
                );
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_minimize(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.window().set_minimized(true);
            }
        });
    }
    {
        // The window has no title bar, so ask the OS to drag it natively.
        let ui_weak = ui.as_weak();
        ui.on_start_drag(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.window().with_winit_window(|window| {
                    let _ = window.drag_window();
                });
            }
        });
    }
}

/// Buttons that act on memory and apps.
fn wire_actions(ui: &AppWindow, state: &Rc<RefCell<State>>) {
    ui.on_optimize(bind(ui, state, |s| s.optimize()));
    ui.on_refresh(bind(ui, state, |_| {}));

    let with_name = |action: fn(&mut State, &str)| {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        move |name: SharedString| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            action(&mut state, &name);
            state.poll(false);
            view::render(&ui, &mut state);
        }
    };
    ui.on_quit_app(with_name(State::close));
    ui.on_force_quit_app(with_name(State::force_quit));
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_set_limit(move |name, tenths| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            let cap = (tenths > 0).then(|| tenths as u64 * GIB / 10);
            state.set_limit(&name, cap);
            state.poll(false);
            view::render(&ui, &mut state);
        });
    }

    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_game_mode_toggled(move |on| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            state.set_game_mode(on);
            state.poll(false);
            view::render(&ui, &mut state);
        });
    }
}

/// Settings panel: toggles, startup apps, close-when-gaming list, check-up.
fn wire_settings(ui: &AppWindow, state: &Rc<RefCell<State>>, demo: bool) {
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_settings_changed(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            let before = state.settings.start_with_windows;
            read_settings_from_ui(&ui, &mut state.settings);
            state.auto.enabled = state.settings.auto_clear;
            state.auto.threshold = state.settings.auto_threshold as f32 / 100.0;
            state.auto.cooldown = Duration::from_secs(state.settings.auto_interval.into());
            if !demo && state.settings.start_with_windows != before {
                set_autostart(state.settings.start_with_windows, state.settings.admin_mode);
            }
            state.save_settings();
            view::render(&ui, &mut state);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_admin_mode_toggled(move |on| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            set_admin_mode(&ui, &mut state, on, demo);
            view::render(&ui, &mut state);
        });
    }
    ui.on_settings_opened(bind(ui, state, State::refresh_startup));
    ui.on_run_checkup(bind(ui, state, State::start_checkup));
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_set_startup(move |id, on| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            state.set_startup(&id, on);
            view::render(&ui, &mut state);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = state.clone();
        ui.on_toggle_gaming_close(move |name, close| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut state = state.borrow_mut();
            state.set_close_when_gaming(&name, close);
            view::render(&ui, &mut state);
        });
    }
}

/// Admin mode takes effect by restarting with admin rights (Windows asks
/// first); turning it off takes effect at the next start.
fn set_admin_mode(ui: &AppWindow, state: &mut State, on: bool, demo: bool) {
    state.settings.admin_mode = on;
    state.save_settings();
    if demo {
        state.set_status(if on { "Admin mode on" } else { "Admin mode off" });
        return;
    }
    set_autostart(state.settings.start_with_windows, on);
    if !on {
        let note = if is_elevated() { "Admin mode turns off next time the app starts" } else { "Admin mode off" };
        state.set_status(note);
        return;
    }
    if is_elevated() {
        state.set_status("Admin mode on");
        return;
    }
    #[cfg(windows)]
    {
        // The new copy waits for this one to exit before taking over.
        let args = ["--wait-pid".to_string(), std::process::id().to_string(), "--no-elevate".to_string()];
        if ram_core::win::relaunch_as_admin(&args) {
            quit(state);
        }
    }
    // Declined (or not Windows): stay as is.
    state.settings.admin_mode = false;
    state.save_settings();
    set_autostart(state.settings.start_with_windows, false);
    ui.set_admin_mode(false);
    state.set_status("Admin mode needs Windows' permission");
}

fn apply_settings_to_ui(ui: &AppWindow, settings: &Settings) {
    ui.set_auto_enabled(settings.auto_clear);
    ui.set_auto_threshold(settings.auto_threshold.into());
    let interval = INTERVALS.iter().position(|&s| s == settings.auto_interval).unwrap_or(INTERVALS.len() - 1);
    ui.set_auto_interval(interval as i32);
    ui.set_start_with_windows(settings.start_with_windows);
    ui.set_admin_mode(settings.admin_mode);
    ui.set_alerts_enabled(settings.alerts);
    ui.set_auto_game_mode(settings.auto_game_mode);
    ui.global::<Palette>().set_theme(settings.theme.index());
}

fn read_settings_from_ui(ui: &AppWindow, settings: &mut Settings) {
    settings.auto_clear = ui.get_auto_enabled();
    settings.auto_threshold = ui.get_auto_threshold().clamp(30, 95) as u8;
    settings.auto_interval = usize::try_from(ui.get_auto_interval())
        .ok()
        .and_then(|i| INTERVALS.get(i).copied())
        .unwrap_or(settings.auto_interval);
    settings.start_with_windows = ui.get_start_with_windows();
    settings.alerts = ui.get_alerts_enabled();
    settings.auto_game_mode = ui.get_auto_game_mode();
    settings.theme = Theme::from_index(ui.global::<Palette>().get_theme());
}

/// Wraps a state change as a UI callback that refreshes the window afterwards.
fn bind(
    ui: &AppWindow,
    state: &Rc<RefCell<State>>,
    action: impl Fn(&mut State) + 'static,
) -> impl FnMut() + 'static {
    let ui = ui.as_weak();
    let state = state.clone();
    move || {
        let Some(ui) = ui.upgrade() else { return };
        let mut state = state.borrow_mut();
        action(&mut state);
        state.poll(false);
        view::render(&ui, &mut state);
    }
}

/// Swaps in a verified update and restarts into it. On failure the app just
/// keeps running the current version.
fn install_update(ui: &AppWindow, state: &mut State, update: &update::ReadyUpdate) {
    state.shut_down();
    match update::install_and_restart(update, !ui.window().is_visible()) {
        Ok(()) => std::process::exit(0),
        Err(err) => log_error(&format!("installing update {}: {err}", update.version)),
    }
}

/// Shows the window and checks for updates, since opening the app is when
/// you'd expect it to be up to date.
fn show_window(ui: &AppWindow, updater: &update::Updater) {
    updater.check_now();
    let _ = ui.show();
    ui.window().set_minimized(false);
    ui.window().with_winit_window(|window| window.focus_window());
}

/// Restores every priority the app changed, then exits.
fn quit(state: &mut State) -> ! {
    state.shut_down();
    std::process::exit(0);
}

fn is_elevated() -> bool {
    #[cfg(windows)]
    return ram_core::win::WindowsSource::is_elevated();
    #[cfg(not(windows))]
    false
}

/// Registers (or removes) starting at sign-in, off the UI thread since it
/// may call Task Scheduler.
fn set_autostart(enabled: bool, admin_mode: bool) {
    #[cfg(windows)]
    std::thread::spawn(move || {
        if let Err(err) = ram_core::win::autostart::set_enabled(enabled, admin_mode) {
            log_error(&format!("start with Windows: {err}"));
        }
    });
    #[cfg(not(windows))]
    let _ = (enabled, admin_mode);
}

/// The card draws its own shadow. The macOS window shadow would trace the
/// transparent margin as a hard outline, and isn't updated when the window
/// resizes. (Windows draws no shadow for frameless windows by default.)
fn without_os_shadow(
    attributes: slint::winit_030::winit::window::WindowAttributes,
) -> slint::winit_030::winit::window::WindowAttributes {
    #[cfg(target_os = "macos")]
    let attributes = {
        use slint::winit_030::winit::platform::macos::WindowAttributesExtMacOS;
        attributes.with_has_shadow(false)
    };
    attributes
}

/// The real Windows source, or demo data elsewhere (and with `--demo`).
fn memory_source(demo: bool) -> (Box<dyn MemorySource>, bool) {
    #[cfg(windows)]
    if !demo {
        return (Box::new(ram_core::win::WindowsSource::new()), false);
    }
    let _ = demo;
    (Box::new(MockSource::new()), true)
}

/// Release builds on Windows have no console, so crashes and startup errors
/// go to `ram-optimizer.log` next to the executable.
fn log_error(message: &str) {
    use std::io::Write;
    let path = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("ram-optimizer.log")))
        .unwrap_or_else(|| std::env::temp_dir().join("ram-optimizer.log"));
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{message}");
    }
    eprintln!("{message}");
}
