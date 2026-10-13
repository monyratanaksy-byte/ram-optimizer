//! Automatic updates from GitHub Releases.
//!
//! A background thread asks GitHub for the latest release when the app
//! starts, whenever the window is opened, and every few hours. If it's
//! newer, it downloads `RAMOptimizer.exe` and its `.sig`, and keeps
//! the download only if the signature matches the public key built into the
//! app. Installing swaps the files and restarts the app.
//!
//! Only Windows builds check for updates; elsewhere this module is unused.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use ram_core::update;

/// GitHub repository the releases come from ("owner/name").
pub const REPO: &str = "monyratanaksy-byte/ram-optimizer";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const PUBLIC_KEY: &str = include_str!("../update-key.pub");
const EXE_ASSET: &str = "RAMOptimizer.exe";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// Offline (or GitHub unreachable): try again this soon.
const RETRY_OFFLINE: Duration = Duration::from_secs(60);
/// Opening the window often shouldn't hammer GitHub, which allows 60
/// unauthenticated requests an hour.
const MIN_GAP: Duration = Duration::from_secs(5 * 60);
const MAX_DOWNLOAD: u64 = 64 * 1024 * 1024;

/// A verified update, saved next to the running executable.
pub struct ReadyUpdate {
    pub version: String,
    pub path: PathBuf,
}

/// Handle for asking the update thread to check right away.
#[derive(Clone)]
pub struct Updater(Sender<()>);

impl Updater {
    /// An updater that never checks (demo mode, non-Windows).
    pub fn disabled() -> Self {
        Self(mpsc::channel().0)
    }

    /// Checks now, unless the last successful check was moments ago.
    pub fn check_now(&self) {
        let _ = self.0.send(());
    }
}

/// Checks right away, then on request, every few hours, and every minute
/// while offline, until an update is downloaded and verified.
pub fn start(ready: Sender<ReadyUpdate>) -> Updater {
    let (trigger, requests) = mpsc::channel();
    std::thread::spawn(move || schedule(&requests, &ready, &TIMING, check_and_download));
    Updater(trigger)
}

struct Timing {
    every: Duration,
    retry_offline: Duration,
    min_gap: Duration,
}

const TIMING: Timing = Timing {
    every: CHECK_EVERY,
    retry_offline: RETRY_OFFLINE,
    min_gap: MIN_GAP,
};

/// The update thread's loop: runs `check` now, then whenever asked (at most
/// once per `min_gap` after a successful check), every `every`, and every
/// `retry_offline` while checks fail. Returns once an update is ready or the
/// app is gone.
fn schedule(
    requests: &Receiver<()>,
    ready: &Sender<ReadyUpdate>,
    timing: &Timing,
    mut check: impl FnMut() -> Result<Option<ReadyUpdate>, String>,
) {
    let mut next_check = Instant::now();
    let mut last_success: Option<Instant> = None;
    let mut offline = false;
    loop {
        match requests.recv_timeout(next_check.saturating_duration_since(Instant::now())) {
            Ok(()) if last_success.is_some_and(|at| at.elapsed() < timing.min_gap) => continue,
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        match check() {
            Ok(Some(update)) => {
                let _ = ready.send(update);
                return;
            }
            Ok(None) => {
                offline = false;
                last_success = Some(Instant::now());
                next_check = Instant::now() + timing.every;
            }
            Err(err) => {
                // Log once per outage, not every minute.
                if !offline {
                    crate::log_error(&format!("update check failed, retrying every minute: {err}"));
                }
                offline = true;
                next_check = Instant::now() + timing.retry_offline;
            }
        }
    }
}

fn check_and_download() -> Result<Option<ReadyUpdate>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut response = get(&url)?;
    let release: serde_json::Value = response.body_mut().read_json().map_err(|e| e.to_string())?;
    let version = release["tag_name"]
        .as_str()
        .ok_or("release has no tag")?
        .trim_start_matches('v')
        .to_string();
    if !update::is_newer(&version, CURRENT_VERSION) {
        return Ok(None);
    }

    let asset_url = |name: &str| -> Result<String, String> {
        release["assets"]
            .as_array()
            .and_then(|assets| assets.iter().find(|a| a["name"] == name))
            .and_then(|asset| asset["browser_download_url"].as_str())
            .map(str::to_string)
            .ok_or_else(|| format!("release {version} has no {name}"))
    };
    let exe = download(&asset_url(EXE_ASSET)?)?;
    let signature = download(&asset_url(&format!("{EXE_ASSET}.sig"))?)?;
    let signature = String::from_utf8(signature).map_err(|_| "unreadable signature file")?;
    update::verify(PUBLIC_KEY, &version, &exe, &signature)?;

    let path = sibling("exe.new")?;
    std::fs::write(&path, &exe).map_err(|e| format!("can't save update: {e}"))?;
    Ok(Some(ReadyUpdate { version, path }))
}

fn get(url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    ureq::get(url)
        .header("User-Agent", concat!("RAM-Optimizer/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    get(url)?
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD)
        .read_to_vec()
        .map_err(|e| e.to_string())
}

/// `RAMOptimizer.exe` -> `RAMOptimizer.exe.new` / `.exe.old`, same folder.
fn sibling(extension: &str) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(exe.with_extension(extension))
}

/// Puts the update in place of the running executable and starts it. The
/// caller exits right after. Windows lets a running .exe be renamed but not
/// overwritten, so the current one steps aside as `.exe.old`.
pub fn install_and_restart(ready: &ReadyUpdate, hidden: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let old = sibling("exe.old")?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("can't move the old version aside: {e}"))?;
    if let Err(err) = std::fs::rename(&ready.path, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("can't put the update in place: {err}"));
    }

    let mut command = std::process::Command::new(&exe);
    // The new copy waits for this one to exit, keeps the same admin rights
    // (it inherits them) and says it was updated.
    command
        .args(["--wait-pid", &std::process::id().to_string()])
        .args(["--updated", "--no-elevate"]);
    if hidden {
        command.arg("--startup");
    }
    command.spawn().map_err(|e| format!("can't start the new version: {e}"))?;
    Ok(())
}

/// Removes what the last update left behind.
pub fn clean_up() {
    for extension in ["exe.old", "exe.new"] {
        if let Ok(path) = sibling(extension) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const HOUR: Duration = Duration::from_secs(3600);
    const SETTLE: Duration = Duration::from_millis(150);

    /// Runs the scheduler on a thread with `results` as successive check
    /// outcomes (`true` = online, no update; `false` = offline). Returns the
    /// trigger and the number of checks made so far.
    fn run(timing: Timing, results: Vec<bool>) -> (Sender<()>, Arc<AtomicUsize>) {
        let (trigger, requests) = mpsc::channel();
        let (ready, _updates) = mpsc::channel();
        let checks = Arc::new(AtomicUsize::new(0));
        let counter = checks.clone();
        std::thread::spawn(move || {
            let _keep = _updates;
            schedule(&requests, &ready, &timing, move || {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                if results.get(n).copied().unwrap_or(true) { Ok(None) } else { Err("offline".into()) }
            });
        });
        std::thread::sleep(SETTLE);
        (trigger, checks)
    }

    #[test]
    fn checks_at_start() {
        let (_trigger, checks) = run(Timing { every: HOUR, retry_offline: HOUR, min_gap: HOUR }, vec![true]);
        assert_eq!(checks.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn opening_the_window_checks_again_but_not_too_often() {
        let (trigger, checks) = run(Timing { every: HOUR, retry_offline: HOUR, min_gap: HOUR }, vec![true]);
        trigger.send(()).unwrap();
        std::thread::sleep(SETTLE);
        assert_eq!(checks.load(Ordering::SeqCst), 1, "within the minimum gap");

        let (trigger, checks) = run(Timing { every: HOUR, retry_offline: HOUR, min_gap: Duration::ZERO }, vec![true]);
        trigger.send(()).unwrap();
        std::thread::sleep(SETTLE);
        assert_eq!(checks.load(Ordering::SeqCst), 2, "after the minimum gap");
    }

    #[test]
    fn retries_while_offline_until_connected() {
        let timing = Timing { every: HOUR, retry_offline: Duration::from_millis(40), min_gap: HOUR };
        let (_trigger, checks) = run(timing, vec![false, false, true]);
        std::thread::sleep(SETTLE);
        assert_eq!(checks.load(Ordering::SeqCst), 3, "two failed tries, then success, then waits");
    }

    #[test]
    fn opening_the_window_while_offline_retries_at_once() {
        let (trigger, checks) = run(Timing { every: HOUR, retry_offline: HOUR, min_gap: HOUR }, vec![false, true]);
        trigger.send(()).unwrap();
        std::thread::sleep(SETTLE);
        assert_eq!(checks.load(Ordering::SeqCst), 2);
    }
}
