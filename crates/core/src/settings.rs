//! User settings, saved as simple `key=value` lines.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub fn index(self) -> i32 {
        match self {
            Theme::System => 0,
            Theme::Light => 1,
            Theme::Dark => 2,
        }
    }

    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Theme::Light,
            2 => Theme::Dark,
            _ => Theme::System,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub auto_clear: bool,
    /// Percent of RAM in use that triggers auto-clear.
    pub auto_threshold: u8,
    /// Seconds between auto-clear runs while RAM stays over the threshold.
    pub auto_interval: u32,
    pub start_with_windows: bool,
    /// Ask Windows for admin rights at launch, for the few features that
    /// need them (clearing the cache, managing apps that run as admin).
    pub admin_mode: bool,
    pub alerts: bool,
    pub theme: Theme,
    /// Turn game mode on when a game starts, off when it exits.
    pub auto_game_mode: bool,
    /// Apps game mode closes when it turns on.
    pub close_when_gaming: Vec<String>,
    /// Per-app RAM caps in bytes.
    pub limits: BTreeMap<String, u64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_clear: true,
            auto_threshold: 80,
            auto_interval: 300,
            start_with_windows: true,
            admin_mode: false,
            alerts: true,
            theme: Theme::System,
            auto_game_mode: true,
            close_when_gaming: Vec::new(),
            limits: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// Where settings live: `%APPDATA%\RAM Optimizer` on Windows,
    /// `~/Library/Application Support/RAM Optimizer` on macOS.
    pub fn default_path() -> Option<PathBuf> {
        let base = if cfg!(windows) {
            PathBuf::from(std::env::var_os("APPDATA")?)
        } else if cfg!(target_os = "macos") {
            PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support")
        } else {
            PathBuf::from(std::env::var_os("HOME")?).join(".config")
        };
        Some(base.join("RAM Optimizer").join("settings.ini"))
    }

    /// Loads settings, falling back to defaults for anything missing or unreadable.
    pub fn load() -> Self {
        Self::default_path()
            .and_then(|path| fs::read_to_string(path).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::default_path().ok_or("No settings folder")?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        fs::write(path, self.to_text()).map_err(|e| e.to_string())
    }

    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            let flag = value == "true";
            match key {
                "auto_clear" => settings.auto_clear = flag,
                "auto_threshold" => {
                    if let Ok(percent) = value.parse::<u8>() {
                        settings.auto_threshold = percent.clamp(30, 95);
                    }
                }
                "auto_interval" => {
                    if let Ok(seconds) = value.parse::<u32>() {
                        settings.auto_interval = crate::auto::nearest_interval(seconds);
                    }
                }
                "start_with_windows" => settings.start_with_windows = flag,
                "admin_mode" => settings.admin_mode = flag,
                "alerts" => settings.alerts = flag,
                "theme" => settings.theme = Theme::from_index(value.parse().unwrap_or(0)),
                "auto_game_mode" => settings.auto_game_mode = flag,
                "close_when_gaming" => {
                    settings.close_when_gaming = value
                        .split('|')
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                // "Chrome:2048|Discord:512", in MB.
                "limits" => {
                    settings.limits = value
                        .split('|')
                        .filter_map(|entry| {
                            let (name, mb) = entry.rsplit_once(':')?;
                            let mb = mb.trim().parse::<u64>().ok().filter(|&mb| mb > 0)?;
                            Some((name.trim().to_string(), mb * crate::MIB))
                        })
                        .filter(|(name, _)| !name.is_empty())
                        .collect();
                }
                _ => {}
            }
        }
        settings
    }

    pub fn to_text(&self) -> String {
        let limits: Vec<String> = self
            .limits
            .iter()
            .map(|(name, bytes)| format!("{name}:{}", bytes / crate::MIB))
            .collect();
        format!(
            "auto_clear={}\nauto_threshold={}\nauto_interval={}\nstart_with_windows={}\nadmin_mode={}\nalerts={}\ntheme={}\nauto_game_mode={}\nclose_when_gaming={}\nlimits={}\n",
            self.auto_clear,
            self.auto_threshold,
            self.auto_interval,
            self.start_with_windows,
            self.admin_mode,
            self.alerts,
            self.theme.index(),
            self.auto_game_mode,
            self.close_when_gaming.join("|"),
            limits.join("|"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let settings = Settings {
            auto_clear: false,
            auto_threshold: 65,
            auto_interval: 15,
            start_with_windows: false,
            admin_mode: true,
            alerts: false,
            theme: Theme::Dark,
            auto_game_mode: false,
            close_when_gaming: vec!["Chrome".into(), "Spotify".into()],
            limits: BTreeMap::from([("Chrome".into(), 2048 * crate::MIB), ("Discord".into(), 512 * crate::MIB)]),
        };
        assert_eq!(Settings::parse(&settings.to_text()), settings);
    }

    #[test]
    fn falls_back_to_defaults() {
        let settings = Settings::parse("auto_threshold=nonsense\nunknown=1\nalerts=false");
        assert_eq!(settings.auto_threshold, 80);
        assert!(!settings.alerts);
        assert!(settings.auto_clear);
    }

    #[test]
    fn skips_broken_limits() {
        let settings = Settings::parse("limits=Chrome:2048|Bad|Zero:0|:5|Slack:abc");
        assert_eq!(settings.limits, BTreeMap::from([("Chrome".into(), 2048 * crate::MIB)]));
    }

    #[test]
    fn clamps_threshold() {
        assert_eq!(Settings::parse("auto_threshold=5").auto_threshold, 30);
    }
}
