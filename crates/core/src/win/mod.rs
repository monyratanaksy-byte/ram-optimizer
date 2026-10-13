//! Live memory data and actions on Windows.
//!
//! - Totals come from `GlobalMemoryStatusEx`; the free page list from the
//!   "Free & Zero Page List Bytes" performance counter (everything else that's
//!   available is standby cache).
//! - Apps are processes grouped by executable, measured by private working set
//!   (RAM only that app uses), like Task Manager.
//! - System processes and services are never listed or touched.

pub mod autostart;
mod checkup;
pub mod instance;
pub mod registry;
mod startup;

pub use checkup::facts as checkup_facts;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, BOOL, ERROR_NOT_ALL_ASSIGNED, HANDLE, HWND, INVALID_HANDLE_VALUE,
    LPARAM, LUID, RECT,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LookupPrivilegeValueW, TokenElevation,
    LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ELEVATION,
    TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Memory::{
    GetProcessWorkingSetSizeEx, SetProcessWorkingSetSizeEx, QUOTA_LIMITS_HARDWS_MAX_DISABLE,
    QUOTA_LIMITS_HARDWS_MAX_ENABLE, QUOTA_LIMITS_HARDWS_MIN_DISABLE,
};
use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue,
    PdhOpenQueryW, PDH_FMT_COUNTERVALUE, PDH_FMT_LARGE,
};
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessInformation, OpenProcess, OpenProcessToken,
    ProcessMemoryPriority, QueryFullProcessImageNameW, SetProcessInformation,
    SetProcessWorkingSetSize, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    TerminateProcess, MEMORY_PRIORITY_INFORMATION, MEMORY_PRIORITY_LOW, MEMORY_PRIORITY_NORMAL,
    PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    PROCESS_SET_QUOTA, PROCESS_TERMINATE,
};
use windows_sys::Win32::UI::Shell::{SetCurrentProcessExplicitAppUserModelID, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindow, GetWindowRect, GetWindowThreadProcessId,
    IsWindowVisible, PostMessageW, GW_OWNER, SW_SHOWNORMAL, WM_CLOSE,
};

use crate::limits::{Change, Limiter, ProcessUsage};
use crate::{AppUsage, MemorySnapshot, MemorySource, StartupEntry, MIB};

/// Background this long before an app counts as idle.
const IDLE_AFTER: Duration = Duration::from_secs(3 * 60);
/// Apps smaller than this aren't worth listing.
const MIN_APP_BYTES: u64 = 25 * MIB;

/// `NtSetSystemInformation` class and command for purging the standby list.
const SYSTEM_MEMORY_LIST_INFORMATION: i32 = 80;
const MEMORY_PURGE_STANDBY_LIST: i32 = 4;

#[link(name = "ntdll")]
extern "system" {
    fn NtSetSystemInformation(class: i32, info: *mut c_void, length: u32) -> i32;
}

/// Lowercase executable names (without `.exe`) that are part of Windows
/// itself. Services in session 0 are filtered separately.
const SYSTEM_PROCESSES: &[&str] = &[
    "applicationframehost",
    "conhost",
    "csrss",
    "ctfmon",
    "dllhost",
    "dwm",
    "fontdrvhost",
    "lockapp",
    "lsass",
    "memory compression",
    "registry",
    "runtimebroker",
    "searchapp",
    "searchhost",
    "securityhealthsystray",
    "services",
    "shellexperiencehost",
    "sihost",
    "smartscreen",
    "smss",
    "startmenuexperiencehost",
    "svchost",
    "system",
    "taskhostw",
    "textinputhost",
    "useroobebroker",
    "widgets",
    "widgetservice",
    "wininit",
    "winlogon",
];

/// Install folders of the big game stores (lowercase).
const GAME_FOLDERS: &[&str] = &[
    r"\steamapps\common\",
    r"\epic games\",
    r"\riot games\",
    r"\xboxgames\",
    r"\gog galaxy\games\",
    r"\gog games\",
    r"\ea games\",
    r"\ubisoft game launcher\games\",
];
/// Launchers that live in those folders but aren't games.
const LAUNCHER_FOLDERS: &[&str] = &[r"\epic games\launcher\", r"\riot client\"];
/// Apps that often run fullscreen but aren't games (lowercase executable names).
const NOT_GAMES: &[&str] = &[
    "applicationframehost",
    "brave",
    "chrome",
    "code",
    "discord",
    "explorer",
    "firefox",
    "lockapp",
    "mpc-be64",
    "mpc-hc64",
    "ms-teams",
    "msedge",
    "obs64",
    "opera",
    "opera_gx",
    "potplayermini64",
    "powerpnt",
    "searchhost",
    "shellexperiencehost",
    "spotify",
    "startmenuexperiencehost",
    "teams",
    "video.ui",
    "vivaldi",
    "vlc",
    "wmplayer",
    "zoom",
];

/// Apps that must never be closed: File Explorer also runs the taskbar and
/// desktop, so closing it would take those down with it.
const NEVER_CLOSE: &[&str] = &["File Explorer"];

/// Friendly names, and helper processes folded into their main app.
const ALIASES: &[(&str, &str)] = &[
    ("battle.net", "Battle.net"),
    ("brave", "Brave"),
    ("chrome", "Chrome"),
    ("code", "Visual Studio Code"),
    ("discord", "Discord"),
    ("epicgameslauncher", "Epic Games"),
    ("epicwebhelper", "Epic Games"),
    ("explorer", "File Explorer"),
    ("firefox", "Firefox"),
    ("ms-teams", "Teams"),
    ("msedge", "Microsoft Edge"),
    ("msedgewebview2", "Edge WebView"),
    ("obs64", "OBS Studio"),
    ("opera", "Opera"),
    ("opera_gx", "Opera GX"),
    ("slack", "Slack"),
    ("spotify", "Spotify"),
    ("steam", "Steam"),
    ("steamwebhelper", "Steam"),
    ("teams", "Teams"),
];

pub struct WindowsSource {
    started: Instant,
    own_pid: u32,
    /// App name to process IDs, from the latest scan.
    groups: HashMap<String, Vec<u32>>,
    /// Memory of each process in `groups`, from the latest scan.
    memory: HashMap<u32, ProcessMemory>,
    /// One per app with a RAM limit.
    limiters: HashMap<String, Limiter>,
    /// Processes we capped, with their original (min, max) working set.
    capped: HashMap<u32, (usize, usize)>,
    last_foreground: HashMap<String, Instant>,
    /// Processes whose memory priority we lowered, with the original value.
    lowered: HashMap<u32, u32>,
    /// Size of the Memory Compression store, from the latest scan.
    compressed: u64,
    /// Every app name seen in the latest scan, including processes too
    /// small or too protected to measure.
    running: HashSet<String>,
    free_counter: Option<FreeCounter>,
}

impl Default for WindowsSource {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsSource {
    pub fn new() -> Self {
        // Needed to give processes a working set limit above Windows' default.
        // Standard accounts hold it; it just may not be switched on.
        let _ = enable_privilege("SeIncreaseWorkingSetPrivilege");
        Self {
            started: Instant::now(),
            own_pid: std::process::id(),
            groups: HashMap::new(),
            memory: HashMap::new(),
            limiters: HashMap::new(),
            capped: HashMap::new(),
            last_foreground: HashMap::new(),
            lowered: HashMap::new(),
            compressed: 0,
            running: HashSet::new(),
            free_counter: FreeCounter::new(),
        }
    }

    pub fn is_elevated() -> bool {
        let Some(token) = open_own_token(TOKEN_QUERY) else {
            return false;
        };
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut length = 0u32;
        let ok = unsafe {
            GetTokenInformation(
                token.0,
                TokenElevation,
                &mut elevation as *mut _ as *mut c_void,
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut length,
            )
        };
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

impl Drop for WindowsSource {
    fn drop(&mut self) {
        self.restore_all();
    }
}

impl MemorySource for WindowsSource {
    fn snapshot(&mut self) -> MemorySnapshot {
        let mut status: MEMORYSTATUSEX = unsafe { zeroed() };
        status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        unsafe { GlobalMemoryStatusEx(&mut status) };

        let total = status.ullTotalPhys;
        let available = status.ullAvailPhys.min(total);
        let used = total - available;
        let free = self
            .free_counter
            .as_ref()
            .and_then(FreeCounter::read)
            .unwrap_or(available)
            .min(available);
        MemorySnapshot {
            total,
            used,
            compressed: self.compressed.min(used),
            cached: available - free,
            committed: status.ullTotalPageFile.saturating_sub(status.ullAvailPageFile),
            commit_limit: status.ullTotalPageFile,
        }
    }

    fn apps(&mut self) -> Vec<AppUsage> {
        let now = Instant::now();
        let foreground_pid = foreground_pid();
        let mut totals: HashMap<String, u64> = HashMap::new();
        let mut foreground_app = None;
        self.groups.clear();
        self.memory.clear();
        self.running.clear();

        for process in list_processes() {
            if process.exe.eq_ignore_ascii_case("Memory Compression") {
                self.compressed = process_memory(process.pid).map_or(0, |m| m.working_set);
                continue;
            }
            if process.pid <= 4 || process.pid == self.own_pid || session_of(process.pid) == Some(0) {
                continue;
            }
            let Some(name) = app_name(&process.exe) else { continue };
            self.running.insert(name.clone());
            let Some(memory) = process_memory(process.pid) else { continue };
            *totals.entry(name.clone()).or_default() += memory.private;
            if Some(process.pid) == foreground_pid {
                foreground_app = Some(name.clone());
            }
            self.groups.entry(name).or_default().push(process.pid);
            self.memory.insert(process.pid, memory);
        }

        if let Some(name) = &foreground_app {
            self.last_foreground.insert(name.clone(), now);
        }
        let mut apps: Vec<AppUsage> = totals
            .into_iter()
            .filter(|(_, bytes)| *bytes >= MIN_APP_BYTES)
            .map(|(name, bytes)| {
                let foreground = foreground_app.as_deref() == Some(name.as_str());
                let since = self.last_foreground.get(&name).copied().unwrap_or(self.started);
                let background_for = now.duration_since(since);
                let idle = !foreground && background_for >= IDLE_AFTER;
                AppUsage {
                    idle_minutes: idle.then_some((background_for.as_secs() / 60) as u32),
                    foreground,
                    name,
                    bytes,
                }
            })
            .collect();
        apps.sort_by_key(|a| std::cmp::Reverse(a.bytes));
        apps
    }

    fn trim(&mut self, app: &str) -> Result<u64, String> {
        let pids = self.pids(app)?;
        let measure = |pids: &[u32]| -> u64 {
            pids.iter().filter_map(|&pid| process_memory(pid)).map(|m| m.private).sum()
        };
        let before = measure(&pids);
        let trimmed = pids
            .iter()
            .filter(|&&pid| {
                let Some(process) = Handle::open_process(PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION, pid) else {
                    return false;
                };
                // (-1, -1) asks Windows to remove as many pages as possible.
                unsafe { SetProcessWorkingSetSize(process.0, usize::MAX, usize::MAX) != 0 }
            })
            .count();
        if trimmed == 0 {
            return Err(format!("Windows didn't allow trimming {app}"));
        }
        Ok(before.saturating_sub(measure(&pids)))
    }

    fn set_background_priority(&mut self, app: &str, low: bool) -> Result<(), String> {
        let pids = self.pids(app)?;
        if !low {
            for pid in pids {
                if let Some(original) = self.lowered.remove(&pid) {
                    set_memory_priority(pid, original);
                }
            }
            return Ok(());
        }
        let mut changed = 0;
        for pid in pids {
            if self.lowered.contains_key(&pid) {
                changed += 1;
                continue;
            }
            let original = memory_priority(pid).unwrap_or(MEMORY_PRIORITY_NORMAL);
            if set_memory_priority(pid, MEMORY_PRIORITY_LOW) {
                self.lowered.insert(pid, original);
                changed += 1;
            }
        }
        if changed == 0 {
            return Err(format!("Windows didn't allow changing {app}"));
        }
        Ok(())
    }

    fn clear_standby(&mut self) -> Result<(), String> {
        if !Self::is_elevated() {
            return Err("Run as administrator to clear the cache".into());
        }
        enable_privilege("SeProfileSingleProcessPrivilege")?;
        let mut command = MEMORY_PURGE_STANDBY_LIST;
        let status = unsafe {
            NtSetSystemInformation(
                SYSTEM_MEMORY_LIST_INFORMATION,
                &mut command as *mut _ as *mut c_void,
                size_of::<i32>() as u32,
            )
        };
        if status < 0 {
            return Err(format!("Windows refused to clear the cache (0x{status:08X})"));
        }
        Ok(())
    }

    fn restore_all(&mut self) {
        for (pid, original) in self.lowered.drain() {
            set_memory_priority(pid, original);
        }
        self.limiters.clear();
        for pid in self.capped.keys().copied().collect::<Vec<_>>() {
            self.clear_limit(pid);
        }
    }

    fn enforce_limits(&mut self, caps: &BTreeMap<String, u64>) -> Vec<String> {
        let removed: Vec<String> = self.limiters.keys().filter(|name| !caps.contains_key(*name)).cloned().collect();
        for name in removed {
            if let Some(mut limiter) = self.limiters.remove(&name) {
                for pid in limiter.release() {
                    self.clear_limit(pid);
                }
            }
        }
        // Processes that exited take their limits with them.
        self.capped.retain(|pid, _| self.memory.contains_key(pid));

        let mut refused = Vec::new();
        for (name, &cap) in caps {
            let processes: Vec<ProcessUsage> = self
                .groups
                .get(name)
                .into_iter()
                .flatten()
                .filter_map(|pid| {
                    let memory = self.memory.get(pid)?;
                    Some(ProcessUsage { pid: *pid, working_set: memory.working_set, private: memory.private })
                })
                .collect();
            let changes = self.limiters.entry(name.clone()).or_default().plan(cap, &processes);
            let mut failed = false;
            for change in changes {
                match change {
                    Change::Set { pid, max } => failed |= !self.set_limit(pid, max),
                    Change::Clear { pid } => self.clear_limit(pid),
                }
            }
            if failed {
                refused.push(name.clone());
            }
        }
        refused
    }

    fn foreground_game(&mut self) -> Option<String> {
        let window = unsafe { GetForegroundWindow() };
        if window.is_null() {
            return None;
        }
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(window, &mut pid) };
        if pid == 0 || pid == self.own_pid {
            return None;
        }
        let path = process_path(pid)?;
        let exe = std::path::Path::new(&path).file_name()?.to_string_lossy().into_owned();
        let name = app_name(&exe)?;
        let lower_path = path.to_lowercase();
        let in_game_folder = GAME_FOLDERS.iter().any(|f| lower_path.contains(f))
            && !LAUNCHER_FOLDERS.iter().any(|f| lower_path.contains(f));
        let stem = exe.to_lowercase();
        let stem = stem.strip_suffix(".exe").unwrap_or(&stem);
        let looks_like_game = in_game_folder || (covers_monitor(window) && !NOT_GAMES.contains(&stem));
        looks_like_game.then_some(name)
    }

    fn is_running(&mut self, app: &str) -> bool {
        self.running.contains(app)
    }

    fn startup_entries(&mut self) -> Vec<StartupEntry> {
        startup::entries()
    }

    fn set_startup_enabled(&mut self, id: &str, enabled: bool) -> Result<(), String> {
        startup::set_enabled(id, enabled)
    }

    fn close(&mut self, app: &str) -> Result<bool, String> {
        refuse_protected(app)?;
        let windows = top_level_windows(&self.pids(app)?);
        for &window in &windows {
            unsafe { PostMessageW(window, WM_CLOSE, 0, 0) };
        }
        Ok(!windows.is_empty())
    }

    fn force_quit(&mut self, app: &str) -> Result<(), String> {
        refuse_protected(app)?;
        let ended = self
            .pids(app)?
            .into_iter()
            .filter(|&pid| {
                let Some(process) = Handle::open_process(PROCESS_TERMINATE, pid) else {
                    return false;
                };
                unsafe { TerminateProcess(process.0, 1) != 0 }
            })
            .count();
        if ended == 0 {
            return Err(format!("Windows didn't allow ending {app}"));
        }
        Ok(())
    }
}

fn refuse_protected(app: &str) -> Result<(), String> {
    if NEVER_CLOSE.contains(&app) {
        return Err(format!("{app} runs the taskbar, so it can't be closed here"));
    }
    Ok(())
}

/// Visible, unowned windows (the ones with a taskbar button) of these processes.
fn top_level_windows(pids: &[u32]) -> Vec<HWND> {
    struct Search<'a> {
        pids: &'a [u32],
        windows: Vec<HWND>,
    }

    unsafe extern "system" fn visit(window: HWND, data: LPARAM) -> BOOL {
        let search = unsafe { &mut *(data as *mut Search) };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(window, &mut pid) };
        let owned = unsafe { !GetWindow(window, GW_OWNER).is_null() };
        if search.pids.contains(&pid) && unsafe { IsWindowVisible(window) } != 0 && !owned {
            search.windows.push(window);
        }
        1 // keep enumerating
    }

    let mut search = Search { pids, windows: Vec::new() };
    unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
    search.windows
}

impl WindowsSource {
    /// Caps a process's working set at `max` bytes. Windows moves anything
    /// over it to the standby list or page file; nothing is lost.
    fn set_limit(&mut self, pid: u32, max: u64) -> bool {
        let Some(process) = Handle::open_process(PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION, pid) else {
            return false;
        };
        let original = match self.capped.get(&pid) {
            Some(&original) => original,
            None => {
                let (mut min, mut max, mut flags) = (0, 0, 0);
                if unsafe { GetProcessWorkingSetSizeEx(process.0, &mut min, &mut max, &mut flags) } == 0 {
                    return false;
                }
                (min, max)
            }
        };
        let max = usize::try_from(max).unwrap_or(usize::MAX);
        let flags = QUOTA_LIMITS_HARDWS_MAX_ENABLE | QUOTA_LIMITS_HARDWS_MIN_DISABLE;
        let ok = unsafe { SetProcessWorkingSetSizeEx(process.0, original.0.min(max / 2), max, flags) } != 0;
        if ok {
            self.capped.entry(pid).or_insert(original);
        }
        ok
    }

    /// Puts a capped process back to how Windows had it.
    fn clear_limit(&mut self, pid: u32) {
        let Some((min, max)) = self.capped.remove(&pid) else { return };
        if let Some(process) = Handle::open_process(PROCESS_SET_QUOTA, pid) {
            let flags = QUOTA_LIMITS_HARDWS_MAX_DISABLE | QUOTA_LIMITS_HARDWS_MIN_DISABLE;
            unsafe { SetProcessWorkingSetSizeEx(process.0, min, max, flags) };
        }
    }

    fn pids(&self, app: &str) -> Result<Vec<u32>, String> {
        self.groups
            .get(app)
            .cloned()
            .ok_or_else(|| format!("{app} isn't running"))
    }
}

/// Waits up to `timeout` for a process to exit (the old copy, after an update).
pub fn wait_for_exit(pid: u32, timeout: Duration) {
    if let Some(process) = Handle::open_process(PROCESS_SYNCHRONIZE, pid) {
        let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        unsafe { WaitForSingleObject(process.0, millis) };
    }
}

/// Gives this app its own identity in Windows (taskbar grouping and
/// notifications), with a display name and icon instead of showing up as
/// "Windows PowerShell" in notifications.
pub fn register_app_id(id: &str, display_name: &str, icon_png: &[u8]) {
    let id_wide = wide(id);
    unsafe { SetCurrentProcessExplicitAppUserModelID(id_wide.as_ptr()) };

    let icon = std::env::var_os("APPDATA").map(|dir| {
        std::path::Path::new(&dir).join("RAM Optimizer").join("icon.png")
    });
    if let Some(icon) = &icon {
        if let Some(dir) = icon.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(icon, icon_png);
    }
    if let Ok(key) = registry::RegKey::create(registry::Root::CurrentUser, &format!(r"Software\Classes\AppUserModelId\{id}")) {
        let _ = key.set_string("DisplayName", display_name);
        if let Some(icon) = icon {
            let _ = key.set_string("IconUri", &icon.display().to_string());
        }
    }
}

/// Starts a new copy of this program with administrator rights; Windows shows
/// its permission prompt. Returns `false` if the user declined or it failed.
pub fn relaunch_as_admin(args: &[String]) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe = wide(&exe.to_string_lossy());
    let params = wide(&args.iter().map(|a| quote_arg(a)).collect::<Vec<_>>().join(" "));
    let verb = wide("runas");
    let result = unsafe {
        ShellExecuteW(null_mut(), verb.as_ptr(), exe.as_ptr(), params.as_ptr(), null(), SW_SHOWNORMAL)
    };
    // Per the docs, values above 32 mean success.
    result as isize > 32
}

fn quote_arg(arg: &str) -> String {
    if arg.is_empty() || arg.contains([' ', '\t', '"']) {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_string()
    }
}

/// Full path of a process's executable.
fn process_path(pid: u32) -> Option<String> {
    let process = Handle::open_process(PROCESS_QUERY_LIMITED_INFORMATION, pid)?;
    let mut buffer = vec![0u16; 1024];
    let mut len = buffer.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut len) };
    (ok != 0).then(|| String::from_utf16_lossy(&buffer[..len as usize]))
}

/// Whether a window fills its whole monitor (fullscreen or borderless).
fn covers_monitor(window: HWND) -> bool {
    let mut rect: RECT = unsafe { zeroed() };
    if unsafe { GetWindowRect(window, &mut rect) } == 0 {
        return false;
    }
    let monitor = unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) };
    let mut info: MONITORINFO = unsafe { zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return false;
    }
    let screen = info.rcMonitor;
    rect.left <= screen.left && rect.top <= screen.top && rect.right >= screen.right && rect.bottom >= screen.bottom
}

/// Closes the wrapped handle on drop.
pub(crate) struct Handle(pub(crate) HANDLE);

impl Handle {
    fn open_process(access: PROCESS_ACCESS_RIGHTS, pid: u32) -> Option<Self> {
        let handle = unsafe { OpenProcess(access, 0, pid) };
        (!handle.is_null()).then_some(Self(handle))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

struct ProcessEntry {
    pid: u32,
    exe: String,
}

fn list_processes() -> Vec<ProcessEntry> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
        return Vec::new();
    }
    let snapshot = Handle(snapshot);
    let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut processes = Vec::new();
    if unsafe { Process32FirstW(snapshot.0, &mut entry) } == 0 {
        return processes;
    }
    loop {
        let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
        processes.push(ProcessEntry {
            pid: entry.th32ProcessID,
            exe: String::from_utf16_lossy(&entry.szExeFile[..len]),
        });
        if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
            return processes;
        }
    }
}

/// `PROCESS_MEMORY_COUNTERS_EX2` (Windows 10 1809+), which adds the private
/// working set. Not in windows-sys, so declared here.
#[repr(C)]
#[derive(Default)]
struct MemoryCountersEx2 {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_usage: usize,
    private_working_set_size: usize,
    shared_commit_usage: u64,
}

struct ProcessMemory {
    /// RAM used only by this process.
    private: u64,
    /// All RAM mapped by this process, including shared DLLs.
    working_set: u64,
}

fn process_memory(pid: u32) -> Option<ProcessMemory> {
    let process = Handle::open_process(PROCESS_QUERY_LIMITED_INFORMATION, pid)?;
    let mut counters = MemoryCountersEx2 {
        cb: size_of::<MemoryCountersEx2>() as u32,
        ..Default::default()
    };
    let read = |counters: &mut MemoryCountersEx2| unsafe {
        K32GetProcessMemoryInfo(
            process.0,
            counters as *mut MemoryCountersEx2 as *mut PROCESS_MEMORY_COUNTERS,
            counters.cb,
        ) != 0
    };
    if !read(&mut counters) {
        // Older Windows rejects the larger struct; fall back to the basic one.
        counters.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if !read(&mut counters) {
            return None;
        }
    }
    let working_set = counters.working_set_size as u64;
    let private = match counters.private_working_set_size {
        0 => working_set,
        bytes => bytes as u64,
    };
    Some(ProcessMemory { private, working_set })
}

fn session_of(pid: u32) -> Option<u32> {
    let mut session = 0;
    (unsafe { ProcessIdToSessionId(pid, &mut session) } != 0).then_some(session)
}

fn foreground_pid() -> Option<u32> {
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return None;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    (pid != 0).then_some(pid)
}

/// Display name for an executable, or `None` for parts of Windows.
pub(crate) fn app_name(exe: &str) -> Option<String> {
    let original = match exe.len().checked_sub(4).and_then(|i| Some((i, exe.get(i..)?))) {
        Some((i, ext)) if ext.eq_ignore_ascii_case(".exe") => &exe[..i],
        _ => exe,
    };
    let stem = original.to_lowercase();
    if SYSTEM_PROCESSES.contains(&stem.as_str()) {
        return None;
    }
    if let Some((_, alias)) = ALIASES.iter().find(|(key, _)| *key == stem) {
        return Some(alias.to_string());
    }
    let mut chars = original.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

fn memory_priority(pid: u32) -> Option<u32> {
    let process = Handle::open_process(PROCESS_QUERY_LIMITED_INFORMATION, pid)?;
    let mut info = MEMORY_PRIORITY_INFORMATION { MemoryPriority: 0 };
    let ok = unsafe {
        GetProcessInformation(
            process.0,
            ProcessMemoryPriority,
            &mut info as *mut _ as *mut c_void,
            size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
        )
    };
    (ok != 0).then_some(info.MemoryPriority)
}

fn set_memory_priority(pid: u32, priority: u32) -> bool {
    let Some(process) = Handle::open_process(PROCESS_SET_INFORMATION, pid) else {
        return false;
    };
    let info = MEMORY_PRIORITY_INFORMATION { MemoryPriority: priority };
    unsafe {
        SetProcessInformation(
            process.0,
            ProcessMemoryPriority,
            &info as *const _ as *const c_void,
            size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
        ) != 0
    }
}

fn open_own_token(access: u32) -> Option<Handle> {
    let mut token: HANDLE = null_mut();
    (unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut token) } != 0).then_some(Handle(token))
}

fn enable_privilege(name: &str) -> Result<(), String> {
    let failed = || format!("Couldn't enable {name}");
    let token = open_own_token(TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY).ok_or_else(failed)?;
    let name_wide = wide(name);
    let mut luid = LUID { LowPart: 0, HighPart: 0 };
    if unsafe { LookupPrivilegeValueW(null(), name_wide.as_ptr(), &mut luid) } == 0 {
        return Err(failed());
    }
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
    };
    let ok = unsafe { AdjustTokenPrivileges(token.0, 0, &privileges, 0, null_mut(), null_mut()) };
    // AdjustTokenPrivileges "succeeds" even when the privilege isn't held.
    if ok == 0 || unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
        return Err(failed());
    }
    Ok(())
}

pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// Reads the "Free & Zero Page List Bytes" performance counter.
struct FreeCounter {
    query: isize,
    counter: isize,
}

impl FreeCounter {
    fn new() -> Option<Self> {
        let mut query = 0;
        if unsafe { PdhOpenQueryW(null(), 0, &mut query) } != 0 {
            return None;
        }
        let path = wide("\\Memory\\Free & Zero Page List Bytes");
        let mut counter = 0;
        if unsafe { PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) } != 0 {
            unsafe { PdhCloseQuery(query) };
            return None;
        }
        Some(Self { query, counter })
    }

    fn read(&self) -> Option<u64> {
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return None;
        }
        let mut value: PDH_FMT_COUNTERVALUE = unsafe { zeroed() };
        if unsafe { PdhGetFormattedCounterValue(self.counter, PDH_FMT_LARGE, null_mut(), &mut value) } != 0 {
            return None;
        }
        u64::try_from(unsafe { value.Anonymous.largeValue }).ok()
    }
}

impl Drop for FreeCounter {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_apps() {
        assert_eq!(app_name("chrome.exe").as_deref(), Some("Chrome"));
        assert_eq!(app_name("steamwebhelper.exe").as_deref(), Some("Steam"));
        assert_eq!(app_name("notepad.exe").as_deref(), Some("Notepad"));
        assert_eq!(app_name("svchost.exe"), None);
        assert_eq!(app_name("Memory Compression"), None);
    }

    #[test]
    fn quotes_args_with_spaces() {
        assert_eq!(quote_arg("--demo"), "--demo");
        assert_eq!(quote_arg("C:\\My Games"), "\"C:\\My Games\"");
    }
}
