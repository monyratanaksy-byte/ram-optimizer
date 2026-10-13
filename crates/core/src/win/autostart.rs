//! Starting RAM Optimizer when you sign in.
//!
//! With admin rights it registers a scheduled task that runs elevated, so it
//! starts with every feature and no permission prompt at each sign-in.
//! Without them it uses the `Run` registry key and starts in limited mode.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

use super::registry::{RegKey, Root};
use super::WindowsSource;

/// Name of our task and `Run` value; also hidden from the startup apps list.
pub const OWN_ENTRY: &str = "RAM Optimizer";
/// Launched at sign-in: start hidden in the tray.
pub const STARTUP_ARG: &str = "--startup";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let run = RegKey::create(Root::CurrentUser, RUN_KEY)?;
    let marker = marker_path();
    if !enabled {
        run.delete(OWN_ENTRY);
        if let Some(marker) = &marker {
            let _ = std::fs::remove_file(marker);
        }
        if task_exists() {
            schtasks(&["/Delete", "/TN", OWN_ENTRY, "/F"]);
        }
        return Ok(());
    }
    if WindowsSource::is_elevated() {
        // Already registered for this exact .exe: nothing to do, and no
        // need to start schtasks.exe on every launch.
        let registered = marker.as_ref().and_then(|m| std::fs::read_to_string(m).ok());
        if registered.as_deref() == Some(exe.to_string_lossy().as_ref()) {
            return Ok(());
        }
        create_task(&exe)?;
        run.delete(OWN_ENTRY);
        if let Some(marker) = &marker {
            let _ = std::fs::write(marker, exe.to_string_lossy().as_bytes());
        }
        return Ok(());
    }
    if task_exists() {
        // Registered earlier with admin rights; keep that (it starts elevated).
        return Ok(());
    }
    run.set_string(
        OWN_ENTRY,
        &format!("\"{}\" {STARTUP_ARG} --no-elevate", exe.display()),
    )
}

/// Remembers which .exe the scheduled task was registered for.
fn marker_path() -> Option<std::path::PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(Path::new(&appdata).join("RAM Optimizer").join("autostart-task.txt"))
}

fn task_exists() -> bool {
    schtasks(&["/Query", "/TN", OWN_ENTRY])
}

fn create_task(exe: &Path) -> Result<(), String> {
    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(domain), Ok(name)) => format!("{domain}\\{name}"),
        (_, Ok(name)) => name,
        _ => return Err("Couldn't tell who is signed in".into()),
    };
    // Built as XML because the plain schtasks flags can't turn off the
    // defaults that would skip it on battery or stop it after 3 days.
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Starts RAM Optimizer when you sign in.</Description></RegistrationInfo>
  <Triggers>
    <LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId><Delay>PT10S</Delay></LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec><Command>{exe}</Command><Arguments>{STARTUP_ARG}</Arguments></Exec>
  </Actions>
</Task>"#,
        user = escape_xml(&user),
        exe = escape_xml(&exe.display().to_string()),
    );
    let path = std::env::temp_dir().join("ram-optimizer-task.xml");
    // Task Scheduler expects UTF-16 with a byte order mark.
    let bytes: Vec<u8> = std::iter::once(0xFEFF_u16)
        .chain(xml.encode_utf16())
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let created = schtasks(&["/Create", "/TN", OWN_ENTRY, "/XML", &path.display().to_string(), "/F"]);
    let _ = std::fs::remove_file(&path);
    if !created {
        return Err("Couldn't register with Task Scheduler".into());
    }
    Ok(())
}

/// Runs schtasks.exe without flashing a console window.
fn schtasks(args: &[&str]) -> bool {
    Command::new("schtasks.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .is_ok_and(|out| out.status.success())
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
