//! Automatic updates from GitHub Releases.
//!
//! A background thread asks GitHub for the latest release every few hours.
//! If it's newer, it downloads `RAMOptimizer.exe` and its `.sig`, and keeps
//! the download only if the signature matches the public key built into the
//! app. Installing swaps the files and restarts the app.
//!
//! Only Windows builds check for updates; elsewhere this module is unused.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

use ram_core::update;

/// GitHub repository the releases come from ("owner/name").
pub const REPO: &str = "monyratanaksy-byte/ram-optimizer";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const PUBLIC_KEY: &str = include_str!("../update-key.pub");
const EXE_ASSET: &str = "RAMOptimizer.exe";
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_DOWNLOAD: u64 = 64 * 1024 * 1024;

/// A verified update, saved next to the running executable.
pub struct ReadyUpdate {
    pub version: String,
    pub path: PathBuf,
}

/// Checks shortly after startup, then every few hours, until an update is
/// downloaded and verified.
pub fn start_checking(sender: Sender<ReadyUpdate>) {
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_CHECK_AFTER);
        loop {
            match check_and_download() {
                Ok(Some(ready)) => {
                    let _ = sender.send(ready);
                    return;
                }
                Ok(None) => {}
                Err(err) => crate::log_error(&format!("update check: {err}")),
            }
            std::thread::sleep(CHECK_EVERY);
        }
    });
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
