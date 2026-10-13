//! Copies the state into the window's properties and lists.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use ram_core::auto::AutoClear;
use ram_core::checkup::Level;
use ram_core::closing::CloseState;
use ram_core::history::{Peak, MINUTES_PER_DAY};
use ram_core::{format_size, to_gb, Pressure, MIB};
use slint::{Model, VecModel};

use crate::state::{State, AUTO_COOLDOWN, RECENT_LEN};
use crate::{AppRow, AppWindow, FindingRow, GamingRow, StartupRow};

/// Sustained growth above this shows a "+X MB/min" chip.
const GROWTH_CHIP_PER_MIN: f64 = 20.0 * MIB as f64;
/// Sparkline drawing area; matches the Path size in widgets.slint.
const SPARK_W: f32 = 320.0;
const SPARK_H: f32 = 48.0;
/// The 24-hour graph averages every 10 minutes into one point.
const DAY_BUCKET: usize = 10;
/// How many apps the close-when-gaming list offers.
const GAMING_CHOICES: usize = 8;

pub fn render(ui: &AppWindow, state: &mut State) {
    render_summary(ui, state);
    render_apps(ui, state);
    render_history(ui, state);
    render_settings(ui, state);

    let (note, warn) = if state.game.is_active() {
        ("Paused while game mode is on.".to_string(), false)
    } else {
        auto_note(&state.auto, state.snap.used_ratio())
    };
    ui.set_auto_note(note.into());
    ui.set_auto_warn(warn);
    ui.set_game_mode(state.game.is_active());
    ui.set_status(state.current_status().into());
}

fn render_summary(ui: &AppWindow, state: &State) {
    let snap = &state.snap;
    let total = snap.total.max(1) as f32;
    let in_use = snap.used.saturating_sub(snap.compressed);

    ui.set_used_gb(format!("{:.1} GB", to_gb(snap.used)).into());
    ui.set_available_gb(format_size(snap.available()).into());
    ui.set_used_caption(
        format!("of {:.0} GB in use · {:.0}%", to_gb(snap.total), snap.used_ratio() * 100.0).into(),
    );
    let (pressure, high) = match snap.pressure() {
        Pressure::Healthy => ("Healthy", false),
        Pressure::Busy => ("Busy", false),
        Pressure::High => ("High", true),
    };
    ui.set_pressure_text(pressure.into());
    ui.set_pressure_high(high);

    ui.set_share_in_use(in_use as f32 / total);
    ui.set_share_compressed(snap.compressed as f32 / total);
    ui.set_share_cached(snap.cached as f32 / total);
    ui.set_legend_in_use(format_size(in_use).into());
    ui.set_legend_compressed(format_size(snap.compressed).into());
    ui.set_legend_cached(format_size(snap.cached).into());
    ui.set_legend_free(format_size(snap.free()).into());
}

fn render_apps(ui: &AppWindow, state: &State) {
    let now = Instant::now();
    let largest = state.apps.first().map_or(1, |a| a.bytes.max(1));
    let rows = state
        .apps
        .iter()
        .map(|app| {
            let growth = state
                .growth
                .rate_per_minute(&app.name)
                .filter(|rate| *rate >= GROWTH_CHIP_PER_MIN)
                .map(|rate| format!("+{:.0} MB/min", rate / MIB as f64))
                .unwrap_or_default();
            let closing = state.closing.state(&app.name, now);
            AppRow {
                name: app.name.as_str().into(),
                letter: initial(&app.name).into(),
                size: format_size(app.bytes).into(),
                share: app.bytes as f32 / largest as f32,
                idle: app.idle_minutes.map(|m| format!("idle {m}m")).unwrap_or_default().into(),
                growth: growth.into(),
                closing: closing == Some(CloseState::Closing),
                can_force: closing == Some(CloseState::CanForce),
            }
        })
        .collect();
    sync_rows(&state.app_rows, rows);
    ui.set_app_count(format!("{} running", state.apps.len()).into());
}

fn render_history(ui: &AppWindow, state: &State) {
    let (values, capacity, caption, minutes) = match ui.get_history_range() {
        1 => (state.long.averages(60), 60, "Last hour", 60),
        2 => {
            let day: Vec<f32> = state
                .long
                .averages(MINUTES_PER_DAY)
                .chunks(DAY_BUCKET)
                .map(|chunk| chunk.iter().sum::<f32>() / chunk.len() as f32)
                .collect();
            (day, MINUTES_PER_DAY / DAY_BUCKET, "Last 24 hours", MINUTES_PER_DAY)
        }
        _ => (state.recent.iter().copied().collect(), RECENT_LEN, "Last 60 seconds", 0),
    };

    let filling = values.len() < capacity && minutes > 0;
    ui.set_history_caption(if filling { format!("{caption} · filling in") } else { caption.to_string() }.into());

    match sparkline(&values, capacity) {
        Some((line, area)) => {
            ui.set_history_line(line.into());
            ui.set_history_area(area.into());
        }
        None => {
            ui.set_history_line("M 0 44 L 320 44".into());
            ui.set_history_area("M 0 44 L 320 44 L 320 48 L 0 48 Z".into());
        }
    }

    let peak = if minutes == 0 {
        values.iter().copied().reduce(f32::max).map(|ratio| Peak {
            ratio,
            minutes_ago: 0,
            top_app: None,
        })
    } else {
        state.long.peak(minutes)
    };
    ui.set_history_peak(peak.as_ref().map_or(String::new(), |p| format!("peak {:.0}%", p.ratio * 100.0)).into());

    let detail = match &peak {
        Some(peak) if minutes > 0 => {
            let biggest = peak
                .top_app
                .as_ref()
                .map(|(name, bytes)| format!("\nBiggest app then: {name} ({})", format_size(*bytes)))
                .unwrap_or_default();
            format!("Highest was {:.0}% {}{biggest}", peak.ratio * 100.0, minutes_ago(peak.minutes_ago))
        }
        None if minutes > 0 => "Keep the app running to build up history.\nIt keeps the last 24 hours.".into(),
        _ => String::new(),
    };
    ui.set_history_detail(detail.into());
}

fn render_settings(ui: &AppWindow, state: &mut State) {
    // Close-when-gaming choices: the picked apps first, then other big apps.
    let picked = &state.settings.close_when_gaming;
    let mut gaming: Vec<GamingRow> = picked
        .iter()
        .map(|name| GamingRow { name: name.as_str().into(), checked: true })
        .collect();
    for app in &state.apps {
        if gaming.len() >= GAMING_CHOICES.max(picked.len()) {
            break;
        }
        if !picked.contains(&app.name) {
            gaming.push(GamingRow { name: app.name.as_str().into(), checked: false });
        }
    }
    sync_rows(&state.gaming_rows, gaming);

    let startup = state
        .startup
        .iter()
        .map(|entry| {
            let running = entry
                .app
                .as_ref()
                .and_then(|app| state.apps.iter().find(|a| &a.name == app))
                .or_else(|| state.apps.iter().find(|a| a.name.eq_ignore_ascii_case(&entry.name)));
            let detail = match (entry.enabled, running) {
                (_, Some(app)) => format!("Using {} now", format_size(app.bytes)),
                (true, None) => "Not running right now".to_string(),
                (false, None) => "Off".to_string(),
            };
            StartupRow {
                id: entry.id.as_str().into(),
                name: entry.name.as_str().into(),
                letter: initial(&entry.name).into(),
                detail: detail.into(),
                enabled: entry.enabled,
            }
        })
        .collect();
    sync_rows(&state.startup_rows, startup);
    ui.set_startup_note(
        if state.startup.is_empty() {
            "No startup apps found."
        } else {
            "Turning one off doesn't uninstall it. It just won't launch when Windows starts."
        }
        .into(),
    );

    let findings: Vec<FindingRow> = state
        .findings
        .iter()
        .map(|f| FindingRow {
            level: match f.level {
                Level::Good => 0,
                Level::Info => 1,
                Level::Warning => 2,
            },
            title: f.title.as_str().into(),
            detail: f.detail.as_str().into(),
        })
        .collect();
    let warnings = state.findings.iter().filter(|f| f.level == Level::Warning).count();
    if !state.findings.is_empty() {
        ui.set_checkup_summary(
            match warnings {
                0 => "Everything looks good.".to_string(),
                1 => "1 thing worth fixing.".to_string(),
                n => format!("{n} things worth fixing."),
            }
            .into(),
        );
    }
    sync_rows(&state.finding_rows, findings);
    ui.set_checkup_running(state.checkup_running());
}

/// Explains what auto-clear will do next; `true` means the setting needs attention.
fn auto_note(auto: &AutoClear, used_ratio: f32) -> (String, bool) {
    let percent = used_ratio * 100.0;
    if !auto.enabled {
        return ("Off. Turn it on to free idle apps automatically.".into(), false);
    }
    if used_ratio >= auto.threshold {
        let minutes = AUTO_COOLDOWN.as_secs() / 60;
        return (
            format!("RAM is already at {percent:.0}%, so this runs every {minutes} min. Raise the limit if apps feel slow."),
            true,
        );
    }
    match auto.last_run() {
        Some((at, freed)) => (
            format!("Last run {} ago, freed {}.", ago(at.elapsed()), format_size(freed)),
            false,
        ),
        None => (format!("Waiting. RAM is at {percent:.0}%."), false),
    }
}

fn ago(elapsed: Duration) -> String {
    match elapsed.as_secs() {
        s if s < 60 => format!("{s}s"),
        s => format!("{} min", s / 60),
    }
}

fn minutes_ago(minutes: usize) -> String {
    match minutes {
        0 => "in the last minute".to_string(),
        m if m < 60 => format!("{m} min ago"),
        m => format!("{} h {} min ago", m / 60, m % 60),
    }
}

fn initial(name: &str) -> String {
    name.chars().take(1).flat_map(char::to_uppercase).collect()
}

/// Updates rows in place so list items aren't rebuilt (and lose hover or
/// press state) on every refresh.
fn sync_rows<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    if model.row_count() != rows.len() {
        model.set_vec(rows);
        return;
    }
    for (i, row) in rows.into_iter().enumerate() {
        if model.row_data(i).as_ref() != Some(&row) {
            model.set_row_data(i, row);
        }
    }
}

/// SVG path data for a history line and the shaded area under it. Values
/// fill from the right, so the newest is always at the right edge.
fn sparkline(values: &[f32], capacity: usize) -> Option<(String, String)> {
    if values.len() < 2 {
        return None;
    }
    let peak = values.iter().copied().reduce(f32::max)?;
    let low = values.iter().copied().reduce(f32::min)?;
    // Show at least a 10-point band so small wiggles don't look dramatic.
    let mid = (low + peak) / 2.0;
    let half = ((peak - low) / 2.0).max(0.05);
    let (lo, hi) = (mid - half, mid + half);

    let capacity = capacity.max(values.len());
    let step = SPARK_W / (capacity - 1) as f32;
    let offset = capacity - values.len();
    let mut line = String::new();
    for (i, value) in values.iter().enumerate() {
        let x = (offset + i) as f32 * step;
        let y = 4.0 + (1.0 - (value - lo) / (hi - lo)) * (SPARK_H - 8.0);
        let _ = write!(line, "{} {x:.1} {y:.1} ", if i == 0 { "M" } else { "L" });
    }
    let first_x = offset as f32 * step;
    let area = format!("{line}L {SPARK_W} {SPARK_H} L {first_x:.1} {SPARK_H} Z");
    Some((line, area))
}
