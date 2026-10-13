//! Keeps a single copy running: launching it again just shows the window of
//! the one already in the tray. The installer uses the same mechanism to ask
//! the running copy to quit before replacing it.

use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{LocalFree, WAIT_OBJECT_0};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Threading::{
    CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
};

use super::{wide, Handle};

const EVENT_NAME: &str = r"Local\RAMOptimizer.ShowWindow";
const QUIT_EVENT_NAME: &str = r"Local\RAMOptimizer.Quit";
/// Everyone may signal the event, including a non-admin copy signaling an
/// elevated one (the low integrity label allows writes from any level).
const SECURITY: &str = "D:(A;;GA;;;WD)S:(ML;;NW;;;LW)";

pub struct SingleInstance {
    show: Handle,
    quit: Option<Handle>,
}

impl SingleInstance {
    /// Whether a copy is already running. With `show`, also asks it to show
    /// its window.
    pub fn find_existing(show: bool) -> bool {
        let Some(event) = open_event(EVENT_NAME) else {
            return false;
        };
        if show {
            unsafe { SetEvent(event.0) };
        }
        true
    }

    /// Asks the running copy to quit and waits up to `timeout` for it to go.
    /// Returns `false` if it's still running.
    pub fn quit_existing(timeout: Duration) -> bool {
        let Some(event) = open_event(QUIT_EVENT_NAME) else {
            return !Self::find_existing(false);
        };
        unsafe { SetEvent(event.0) };
        drop(event);
        let deadline = Instant::now() + timeout;
        while Self::find_existing(false) {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        true
    }

    /// Registers this copy as the running one.
    pub fn claim() -> Option<Self> {
        let show = create_event(EVENT_NAME)?;
        Some(Self { show, quit: create_event(QUIT_EVENT_NAME) })
    }

    /// Whether another launch asked us to show the window since last asked.
    pub fn show_requested(&self) -> bool {
        unsafe { WaitForSingleObject(self.show.0, 0) == WAIT_OBJECT_0 }
    }

    /// Whether the installer or uninstaller asked us to quit.
    pub fn quit_requested(&self) -> bool {
        self.quit.as_ref().is_some_and(|quit| unsafe { WaitForSingleObject(quit.0, 0) == WAIT_OBJECT_0 })
    }
}

fn open_event(name: &str) -> Option<Handle> {
    let name = wide(name);
    let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
    (!event.is_null()).then(|| Handle(event))
}

fn create_event(name: &str) -> Option<Handle> {
    let sddl = wide(SECURITY);
    let mut descriptor = null_mut();
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    };
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: if converted != 0 { descriptor } else { null_mut() },
        bInheritHandle: 0,
    };
    let name = wide(name);
    let event = unsafe {
        CreateEventW(
            if converted != 0 { &attributes } else { null() },
            0, // auto-reset: each signal is seen once
            0,
            name.as_ptr(),
        )
    };
    if converted != 0 {
        unsafe { LocalFree(descriptor) };
    }
    (!event.is_null()).then(|| Handle(event))
}
