//! Minimal registry access: open or create a key, read, write and delete values.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegEnumValueW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ,
    KEY_WRITE, REG_BINARY, REG_OPTION_NON_VOLATILE, REG_SZ,
};

use super::wide;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Root {
    /// Settings for the signed-in user.
    CurrentUser,
    /// Settings for every user; writing needs admin rights.
    LocalMachine,
}

impl Root {
    fn handle(self) -> HKEY {
        match self {
            Root::CurrentUser => HKEY_CURRENT_USER,
            Root::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }
}

pub struct RegKey(HKEY);

impl RegKey {
    pub fn open(root: Root, path: &str) -> Option<Self> {
        let mut key = null_mut();
        let path = wide(path);
        let status = unsafe { RegOpenKeyExW(root.handle(), path.as_ptr(), 0, KEY_READ, &mut key) };
        (status == ERROR_SUCCESS).then_some(Self(key))
    }

    /// Opens for writing, creating the key if needed.
    pub fn create(root: Root, path: &str) -> Result<Self, String> {
        let mut key = null_mut();
        let path_wide = wide(path);
        let status = unsafe {
            RegCreateKeyExW(
                root.handle(),
                path_wide.as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                null(),
                &mut key,
                null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!("Couldn't open {path} (error {status})"));
        }
        Ok(Self(key))
    }

    /// Names and raw data of every value, in registry order.
    pub fn values(&self) -> Vec<(String, Vec<u8>)> {
        let mut values = Vec::new();
        for index in 0.. {
            let mut name = vec![0u16; 16_384];
            let mut name_len = name.len() as u32;
            let mut data = vec![0u8; 8_192];
            let mut data_len = data.len() as u32;
            let status = unsafe {
                RegEnumValueW(
                    self.0,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    null(),
                    null_mut(),
                    data.as_mut_ptr(),
                    &mut data_len,
                )
            };
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            if status != ERROR_SUCCESS {
                continue;
            }
            data.truncate(data_len as usize);
            values.push((String::from_utf16_lossy(&name[..name_len as usize]), data));
        }
        values
    }

    pub fn get(&self, name: &str) -> Option<Vec<u8>> {
        let name = wide(name);
        let mut data = vec![0u8; 8_192];
        let mut len = data.len() as u32;
        let status = unsafe {
            RegQueryValueExW(self.0, name.as_ptr(), null(), null_mut(), data.as_mut_ptr(), &mut len)
        };
        (status == ERROR_SUCCESS).then(|| {
            data.truncate(len as usize);
            data
        })
    }

    pub fn set_string(&self, name: &str, value: &str) -> Result<(), String> {
        let bytes: Vec<u8> = wide(value).iter().flat_map(|c| c.to_le_bytes()).collect();
        self.set(name, REG_SZ, &bytes)
    }

    pub fn set_binary(&self, name: &str, value: &[u8]) -> Result<(), String> {
        self.set(name, REG_BINARY, value)
    }

    fn set(&self, name: &str, kind: u32, bytes: &[u8]) -> Result<(), String> {
        let name_wide = wide(name);
        let status = unsafe {
            RegSetValueExW(self.0, name_wide.as_ptr(), 0, kind, bytes.as_ptr(), bytes.len() as u32)
        };
        if status != ERROR_SUCCESS {
            return Err(format!("Couldn't save {name} (error {status})"));
        }
        Ok(())
    }

    pub fn delete(&self, name: &str) {
        let name = wide(name);
        unsafe { RegDeleteValueW(self.0, name.as_ptr()) };
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

/// Decodes a REG_SZ / REG_EXPAND_SZ value.
pub fn string_value(data: &[u8]) -> String {
    let units: Vec<u16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}
