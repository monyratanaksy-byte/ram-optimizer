//! RAM check-up: turns facts about the memory setup into findings.
//!
//! The facts come as `key|value|...` lines (on Windows, read from the
//! firmware tables and registry), so the rules here can be tested anywhere.

use crate::{format_size, to_gb, MemorySnapshot, GIB};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Good,
    Info,
    Warning,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub level: Level,
    pub title: String,
    pub detail: String,
}

impl Finding {
    fn new(level: Level, title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            level,
            title: title.into(),
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Default, PartialEq)]
struct Stick {
    capacity: u64,
    /// Highest speed the stick reports, in MT/s.
    rated: u32,
    /// Speed it currently runs at, in MT/s.
    configured: u32,
    part: String,
}

#[derive(Debug, Default)]
struct Facts {
    sticks: Vec<Stick>,
    slots: Option<u32>,
    auto_page_file: Option<bool>,
    compression: Option<bool>,
    /// Laptops pick RAM speed themselves and rarely offer XMP or EXPO.
    laptop: bool,
}

/// SMBIOS chassis types for portable PCs (laptop, notebook, convertible...).
const PORTABLE_CHASSIS: &[u32] = &[8, 9, 10, 14, 30, 31, 32];

fn parse(lines: &str) -> Facts {
    let mut facts = Facts::default();
    for line in lines.lines() {
        let fields: Vec<&str> = line.trim().split('|').collect();
        match fields.as_slice() {
            ["stick", capacity, rated, configured, part, ..] => facts.sticks.push(Stick {
                capacity: capacity.parse().unwrap_or(0),
                rated: rated.parse().unwrap_or(0),
                configured: configured.parse().unwrap_or(0),
                part: part.trim().to_string(),
            }),
            ["slots", count] => facts.slots = count.parse().ok().filter(|&n| n > 0),
            ["autopagefile", value] => facts.auto_page_file = parse_bool(value),
            ["compression", value] => facts.compression = parse_bool(value),
            ["chassis", types] => {
                facts.laptop = types
                    .split(',')
                    .filter_map(|t| t.trim().parse::<u32>().ok())
                    .any(|t| PORTABLE_CHASSIS.contains(&t));
            }
            _ => {}
        }
    }
    facts
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Collects the raw facts. Takes a second or two on Windows, so call it off
/// the UI thread. Elsewhere it returns demo facts.
pub fn gather_facts() -> String {
    #[cfg(windows)]
    {
        crate::win::checkup_facts()
    }
    #[cfg(not(windows))]
    {
        std::thread::sleep(std::time::Duration::from_millis(800));
        [
            "stick|17179869184|5600|4800|CMK32GX5M2B5600C36",
            "stick|17179869184|5600|4800|CMK32GX5M2B5600C36",
            "slots|4",
            "autopagefile|True",
            "compression|True",
        ]
        .join("\n")
    }
}

/// Builds the findings from the raw facts and the current memory snapshot.
pub fn evaluate(lines: &str, snap: &MemorySnapshot) -> Vec<Finding> {
    let facts = parse(lines);
    let mut findings = Vec::new();

    // Amount of RAM.
    let total_gb = to_gb(snap.total);
    if snap.total < 8 * GIB {
        findings.push(Finding::new(
            Level::Warning,
            format!("{total_gb:.0} GB of RAM"),
            "That's tight for modern apps and games. 16 GB is a comfortable minimum today.",
        ));
    } else {
        findings.push(Finding::new(Level::Good, format!("{total_gb:.0} GB of RAM"), "Enough for most apps and games."));
    }

    // Sticks, channels and speed.
    let sticks = &facts.sticks;
    if let [stick] = sticks.as_slice() {
        let slots = facts.slots.map(|n| format!(" (1 of {n} slots used)")).unwrap_or_default();
        let model = if stick.part.is_empty() {
            String::new()
        } else {
            format!(" Yours is {} ({}).", stick.part, format_size(stick.capacity))
        };
        findings.push(Finding::new(
            Level::Warning,
            format!("One memory stick{slots}"),
            format!(
                "Your RAM runs in single-channel mode, which can be noticeably slower in games. A matching second stick (same size and speed) doubles memory bandwidth.{model}"
            ),
        ));
    } else if sticks.len() > 1 {
        let slots = facts.slots.map(|n| format!(" ({} of {n} slots used)", sticks.len())).unwrap_or_default();
        findings.push(Finding::new(Level::Good, format!("{} memory sticks{slots}", sticks.len()), "Running in multi-channel mode."));
        let first = &sticks[0];
        if sticks.iter().any(|s| s.capacity != first.capacity || s.part != first.part) {
            findings.push(Finding::new(
                Level::Info,
                "Mixed memory sticks",
                "Your sticks differ in size or model. That works, but matched pairs are faster and more stable.",
            ));
        }
    }

    let configured = sticks.iter().map(|s| s.configured).filter(|&v| v > 0).min();
    let rated = sticks.iter().map(|s| s.rated).filter(|&v| v > 0).min();
    match (configured, rated) {
        (Some(running), Some(rated)) if facts.laptop && (running as f32) < rated as f32 * 0.95 => {
            findings.push(Finding::new(
                Level::Info,
                format!("RAM runs at {running} MT/s, rated {rated}"),
                "Laptops set memory speed themselves and almost never offer XMP or EXPO. The difference is small, so there's nothing to do here.",
            ));
        }
        (Some(running), Some(rated)) if (running as f32) < rated as f32 * 0.95 => {
            findings.push(Finding::new(
                Level::Warning,
                format!("RAM runs at {running} MT/s, rated {rated}"),
                "It's slower than it can go. Turn on XMP (Intel) or EXPO (AMD) in your BIOS to run it at full speed.",
            ));
        }
        (Some(running), _) if facts.laptop => findings.push(Finding::new(
            Level::Good,
            format!("RAM runs at {running} MT/s"),
            "Set automatically by your laptop.",
        )),
        (Some(running), _) => findings.push(Finding::new(
            Level::Info,
            format!("RAM runs at {running} MT/s"),
            "If the box your RAM came in lists a higher speed, turn on XMP or EXPO in your BIOS.",
        )),
        _ => {}
    }

    // Page file.
    let page_file = snap.commit_limit.saturating_sub(snap.total);
    if snap.commit_limit > 0 && page_file < GIB / 2 {
        findings.push(Finding::new(
            Level::Warning,
            "Page file is off",
            "When RAM fills up, apps will crash instead of slowing down. Let Windows manage the page file size.",
        ));
    } else if facts.auto_page_file == Some(false) {
        findings.push(Finding::new(
            Level::Info,
            format!("Custom page file ({})", format_size(page_file)),
            "Letting Windows manage its size is usually the safest choice.",
        ));
    } else if snap.commit_limit > 0 {
        findings.push(Finding::new(Level::Good, "Page file managed by Windows", "Apps have room to spill over safely."));
    }

    if snap.commit_ratio() > 0.85 {
        findings.push(Finding::new(
            Level::Warning,
            format!("{:.0}% of the commit limit in use", snap.commit_ratio() * 100.0),
            "Programs have reserved almost all RAM plus page file. Close something or enlarge the page file.",
        ));
    }

    // Memory compression.
    match facts.compression {
        Some(true) => findings.push(Finding::new(Level::Good, "Memory compression is on", "Windows squeezes rarely used memory instead of writing it to disk.")),
        Some(false) => findings.push(Finding::new(
            Level::Warning,
            "Memory compression is off",
            "Turning it on lets Windows fit more in RAM. In an admin PowerShell, run: Enable-MMAgent -MemoryCompression",
        )),
        None => {}
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(total_gb: u64, page_file_gb: u64) -> MemorySnapshot {
        MemorySnapshot {
            total: total_gb * GIB,
            used: total_gb * GIB / 2,
            compressed: 0,
            cached: 0,
            committed: total_gb * GIB / 2,
            commit_limit: (total_gb + page_file_gb) * GIB,
        }
    }

    fn titles(findings: &[Finding], level: Level) -> Vec<String> {
        findings.iter().filter(|f| f.level == level).map(|f| f.title.clone()).collect()
    }

    #[test]
    fn healthy_setup() {
        let lines = "stick|17179869184|6000|6000|KF560C36-16\nstick|17179869184|6000|6000|KF560C36-16\nslots|4\nautopagefile|True\ncompression|True";
        let findings = evaluate(lines, &snapshot(32, 8));
        assert!(titles(&findings, Level::Warning).is_empty(), "{findings:?}");
        assert!(titles(&findings, Level::Good).contains(&"2 memory sticks (2 of 4 slots used)".to_string()));
    }

    #[test]
    fn flags_common_problems() {
        let lines = "stick|8589934592|3200|2133|ABC\nslots|2\ncompression|False";
        let warnings = titles(&evaluate(lines, &snapshot(8, 0)), Level::Warning);
        assert!(warnings.contains(&"One memory stick (1 of 2 slots used)".to_string()));
        assert!(warnings.contains(&"RAM runs at 2133 MT/s, rated 3200".to_string()));
        assert!(warnings.contains(&"Page file is off".to_string()));
        assert!(warnings.contains(&"Memory compression is off".to_string()));
    }

    #[test]
    fn laptops_get_no_bios_advice() {
        let lines = "stick|17179869184|5600|5200|M425R2GA3BB0-CWMOD\nslots|2\nchassis|10";
        let findings = evaluate(lines, &snapshot(16, 8));
        let speed = findings.iter().find(|f| f.title.starts_with("RAM runs at")).unwrap();
        assert_eq!(speed.level, Level::Info);
        assert!(!speed.detail.contains("BIOS"));
        let stick = findings.iter().find(|f| f.title.starts_with("One memory stick")).unwrap();
        assert!(stick.detail.contains("M425R2GA3BB0-CWMOD (16.0 GB)"));
    }

    #[test]
    fn ignores_garbage() {
        let findings = evaluate("nonsense\nstick|x|y\n", &snapshot(16, 4));
        assert!(!findings.is_empty());
    }
}
