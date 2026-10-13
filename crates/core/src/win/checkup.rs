//! Collects the facts for the RAM check-up from WMI, via PowerShell.

use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Prints `key|value|...` lines; see `checkup::evaluate` for the format.
const SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
Get-CimInstance Win32_PhysicalMemory | ForEach-Object {
  "stick|$($_.Capacity)|$($_.Speed)|$($_.ConfiguredClockSpeed)|$(([string]$_.PartNumber).Trim())"
}
"slots|$((Get-CimInstance Win32_PhysicalMemoryArray | Measure-Object -Property MemoryDevices -Sum).Sum)"
"autopagefile|$((Get-CimInstance Win32_ComputerSystem).AutomaticManagedPagefile)"
"compression|$((Get-MMAgent).MemoryCompression)"
"chassis|$((Get-CimInstance Win32_SystemEnclosure).ChassisTypes -join ',')"
"#;

/// Takes a second or two, so call it off the UI thread.
pub fn facts() -> String {
    Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", SCRIPT])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default()
}
