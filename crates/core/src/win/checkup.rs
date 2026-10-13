//! Collects the facts for the RAM check-up straight from Windows: memory
//! sticks from the SMBIOS firmware tables, the page file setting from the
//! registry, and memory compression from the process list. No scripts.

use windows_sys::Win32::System::SystemInformation::{GetSystemFirmwareTable, RSMB};

use super::list_processes;
use super::registry::{RegKey, Root};
use crate::smbios;

const MEMORY_MANAGEMENT: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management";

/// Fact lines for `checkup::evaluate`.
pub fn facts() -> String {
    let mut lines = smbios_table()
        .map(|table| smbios::facts(&smbios::parse(&table)))
        .unwrap_or_default();
    if let Some(automatic) = page_file_automatic() {
        lines += &format!("autopagefile|{}\n", if automatic { "True" } else { "False" });
    }
    // Windows runs a "Memory Compression" process only while compression is on.
    let compression = list_processes()
        .iter()
        .any(|p| p.exe.eq_ignore_ascii_case("Memory Compression"));
    lines += &format!("compression|{}\n", if compression { "True" } else { "False" });
    lines
}

/// The raw SMBIOS structure table.
fn smbios_table() -> Option<Vec<u8>> {
    let size = unsafe { GetSystemFirmwareTable(RSMB, 0, std::ptr::null_mut(), 0) };
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0u8; size as usize];
    let written = unsafe { GetSystemFirmwareTable(RSMB, 0, buffer.as_mut_ptr(), size) };
    if written == 0 || written > size || buffer.len() < 8 {
        return None;
    }
    // RawSMBIOSData: 4 bytes of version info, a 4-byte length, then the table.
    let length = u32::from_le_bytes(buffer[4..8].try_into().ok()?) as usize;
    let end = (8 + length).min(buffer.len());
    Some(buffer[8..end].to_vec())
}

/// Whether Windows manages the page file size. `None` when there's no page
/// file at all (the check-up spots that from the commit limit instead).
fn page_file_automatic() -> Option<bool> {
    let key = RegKey::open(Root::LocalMachine, MEMORY_MANAGEMENT)?;
    let data = key.get("PagingFiles")?;
    let units: Vec<u16> = data.as_chunks::<2>().0.iter().map(|&pair| u16::from_le_bytes(pair)).collect();
    let entries: Vec<String> = String::from_utf16_lossy(&units)
        .split('\0')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect();
    if entries.is_empty() {
        return None;
    }
    // "?:\pagefile.sys" = managed for all drives; "C:\pagefile.sys 0 0" =
    // system managed size on C:; "C:\pagefile.sys 4096 8192" = custom.
    Some(entries.iter().any(|entry| {
        entry.starts_with("?:") || entry.split_whitespace().skip(1).all(|size| size == "0")
    }))
}
