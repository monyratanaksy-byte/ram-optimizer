//! Programs that launch at sign-in, read and toggled like Task Manager does.
//!
//! Entries live in the `Run` registry keys and the Startup folders. Turning
//! one off doesn't delete it: Windows keeps an on/off flag per entry under
//! `Explorer\StartupApproved`, which is what Task Manager writes too, so
//! changes show up there and are fully reversible.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use super::app_name;
use super::autostart::OWN_ENTRY;
use super::registry::{string_value, RegKey, Root};
use crate::StartupEntry;

const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

struct Location {
    tag: &'static str,
    root: Root,
    /// `Run` registry key, or `None` for a Startup folder.
    run_key: Option<&'static str>,
    approved: &'static str,
}

const LOCATIONS: &[Location] = &[
    Location {
        tag: "hkcu-run",
        root: Root::CurrentUser,
        run_key: Some(r"Software\Microsoft\Windows\CurrentVersion\Run"),
        approved: "Run",
    },
    Location {
        tag: "hklm-run",
        root: Root::LocalMachine,
        run_key: Some(r"Software\Microsoft\Windows\CurrentVersion\Run"),
        approved: "Run",
    },
    Location {
        tag: "hklm-run32",
        root: Root::LocalMachine,
        run_key: Some(r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run"),
        approved: "Run32",
    },
    Location {
        tag: "user-folder",
        root: Root::CurrentUser,
        run_key: None,
        approved: "StartupFolder",
    },
    Location {
        tag: "common-folder",
        root: Root::LocalMachine,
        run_key: None,
        approved: "StartupFolder",
    },
];

pub fn entries() -> Vec<StartupEntry> {
    let mut entries = Vec::new();
    for location in LOCATIONS {
        let approved = RegKey::open(location.root, &format!(r"{APPROVED}\{}", location.approved));
        let is_enabled = |value_name: &str| {
            // Missing flag means enabled; an odd first byte means disabled.
            approved
                .as_ref()
                .and_then(|key| key.get(value_name))
                .and_then(|flag| flag.first().copied())
                .is_none_or(|first| first % 2 == 0)
        };

        let items: Vec<(String, String, String)> = match location.run_key {
            Some(path) => RegKey::open(location.root, path)
                .map(|key| key.values())
                .unwrap_or_default()
                .into_iter()
                .map(|(name, data)| {
                    let command = string_value(&data);
                    let exe = executable(&command);
                    (name.clone(), name, exe)
                })
                .collect(),
            None => startup_folder(location.tag)
                .and_then(|dir| std::fs::read_dir(dir).ok())
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|file| !file.eq_ignore_ascii_case("desktop.ini"))
                .map(|file| {
                    let stem = Path::new(&file)
                        .file_stem()
                        .map_or(file.clone(), |s| s.to_string_lossy().into_owned());
                    (file, stem.clone(), stem)
                })
                .collect(),
        };

        for (value_name, display, exe) in items {
            if value_name == OWN_ENTRY {
                continue;
            }
            entries.push(StartupEntry {
                id: format!("{}|{value_name}", location.tag),
                enabled: is_enabled(&value_name),
                app: app_name(&exe),
                name: display,
            });
        }
    }
    entries.sort_by_key(|e| e.name.to_lowercase());
    entries
}

pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    let (tag, value_name) = id.split_once('|').ok_or("Unknown startup entry")?;
    let location = LOCATIONS
        .iter()
        .find(|l| l.tag == tag)
        .ok_or("Unknown startup entry")?;
    let key = RegKey::create(location.root, &format!(r"{APPROVED}\{}", location.approved))
        .map_err(|_| {
            if location.root == Root::LocalMachine {
                "This one is set for all users. Run as administrator to change it.".to_string()
            } else {
                "Windows didn't allow changing this startup app".to_string()
            }
        })?;
    // Same 12-byte flag Task Manager writes: 2 = on; 3 = off, plus when.
    let mut flag = [0u8; 12];
    flag[0] = if enabled { 2 } else { 3 };
    if !enabled {
        flag[4..].copy_from_slice(&filetime_now().to_le_bytes());
    }
    key.set_binary(value_name, &flag)
}

/// The executable's file name from a command line like
/// `"C:\Program Files\App\app.exe" --minimized`.
fn executable(command: &str) -> String {
    let command = command.trim();
    // Squirrel installers (Discord, Teams classic...) start the real app
    // through `Update.exe --processStart App.exe`.
    if let Some((_, rest)) = command.split_once("--processStart") {
        if let Some(target) = rest.split_whitespace().next() {
            return target.trim_matches('"').to_string();
        }
    }
    let path = if let Some(rest) = command.strip_prefix('"') {
        rest.split('"').next().unwrap_or(rest)
    } else {
        let lower = command.to_ascii_lowercase();
        match lower.find(".exe") {
            Some(end) => &command[..end + 4],
            None => command.split_whitespace().next().unwrap_or(command),
        }
    };
    Path::new(path)
        .file_name()
        .map_or(path.to_string(), |name| name.to_string_lossy().into_owned())
}

fn startup_folder(tag: &str) -> Option<std::path::PathBuf> {
    let (var, rest) = match tag {
        "user-folder" => ("APPDATA", r"Microsoft\Windows\Start Menu\Programs\Startup"),
        _ => ("PROGRAMDATA", r"Microsoft\Windows\Start Menu\Programs\StartUp"),
    };
    Some(Path::new(&std::env::var_os(var)?).join(rest))
}

/// Current time as a Windows FILETIME (100 ns ticks since 1601).
fn filetime_now() -> u64 {
    const EPOCH_DIFFERENCE: u64 = 11_644_473_600;
    let since_unix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    (since_unix.as_secs() + EPOCH_DIFFERENCE) * 10_000_000 + u64::from(since_unix.subsec_nanos() / 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_executable() {
        assert_eq!(executable(r#""C:\Users\me\AppData\Local\Discord\Update.exe" --processStart Discord.exe"#), "Discord.exe");
        assert_eq!(executable(r#""C:\Program Files\App\app.exe" --hidden"#), "app.exe");
        assert_eq!(executable(r"C:\Steam\steam.exe -silent"), "steam.exe");
        assert_eq!(executable("OneDrive.exe /background"), "OneDrive.exe");
    }
}
