//! Keeps a single copy running: launching it again just shows the window of
//! the one already in the tray.

use std::ptr::{null, null_mut};

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
/// Everyone may signal the event, including a non-admin copy signaling an
/// elevated one (the low integrity label allows writes from any level).
const SECURITY: &str = "D:(A;;GA;;;WD)S:(ML;;NW;;;LW)";

pub struct SingleInstance(Handle);

impl SingleInstance {
    /// Whether a copy is already running. With `show`, also asks it to show
    /// its window.
    pub fn find_existing(show: bool) -> bool {
        let name = wide(EVENT_NAME);
        let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
        if event.is_null() {
            return false;
        }
        let event = Handle(event);
        if show {
            unsafe { SetEvent(event.0) };
        }
        true
    }

    /// Registers this copy as the running one.
    pub fn claim() -> Option<Self> {
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
        let name = wide(EVENT_NAME);
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
        (!event.is_null()).then(|| Self(Handle(event)))
    }

    /// Whether another launch asked us to show the window since last asked.
    pub fn show_requested(&self) -> bool {
        unsafe { WaitForSingleObject(self.0 .0, 0) == WAIT_OBJECT_0 }
    }
}
