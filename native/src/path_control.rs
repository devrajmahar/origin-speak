//! Per-user PATH registration for the installed Origin Speak manager.
//!
//! Windows updates only the exact Origin Speak manager directory in
//! `HKCU\Environment\Path`. Unix installs the manager in `~/.local/bin` and
//! reports whether that directory is already visible without editing shell rc files.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathRegistration {
    pub on_path: bool,
    pub changed: bool,
}

#[cfg(target_os = "windows")]
mod imp {
    use super::PathRegistration;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, LPARAM, WIN32_ERROR, WPARAM,
    };
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };
    use windows::core::{Error as WindowsError, HRESULT, PCWSTR};

    const ENVIRONMENT_KEY: &str = r"Environment";
    const PATH_VALUE: &str = "Path";

    struct RegistryKey(HKEY);

    impl Drop for RegistryKey {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    pub fn status(root: &Path) -> Result<PathRegistration, String> {
        crate::management::validate_manager_root_ownership(root)?;
        let (value, _) = read_path()?;
        Ok(PathRegistration {
            on_path: contains_root(&value, root)?,
            changed: false,
        })
    }

    pub fn ensure(root: &Path) -> Result<PathRegistration, String> {
        crate::management::validate_manager_root_ownership(root)?;
        let root = path_string(root)?;
        let (value, value_type) = read_path()?;
        if contains_normalized(&value, &root) {
            return Ok(PathRegistration {
                on_path: true,
                changed: false,
            });
        }

        let next = if value.trim().is_empty() {
            root
        } else if value.ends_with(';') {
            format!("{value}{root}")
        } else {
            format!("{value};{root}")
        };
        write_path(&next, value_type)?;
        broadcast_environment_change();
        Ok(PathRegistration {
            on_path: true,
            changed: true,
        })
    }

    pub fn remove(root: &Path) -> Result<PathRegistration, String> {
        let root = path_string(root)?;
        let (value, value_type) = read_path()?;
        let mut removed = false;
        let kept = value
            .split(';')
            .filter(|entry| {
                let matched = normalize(entry) == normalize(&root);
                removed |= matched;
                !matched
            })
            .collect::<Vec<_>>()
            .join(";");
        if removed {
            write_path(&kept, value_type)?;
            broadcast_environment_change();
        }
        Ok(PathRegistration {
            on_path: false,
            changed: removed,
        })
    }

    fn contains_root(value: &str, root: &Path) -> Result<bool, String> {
        Ok(contains_normalized(value, &path_string(root)?))
    }

    fn contains_normalized(value: &str, root: &str) -> bool {
        let root = normalize(root);
        value.split(';').any(|entry| normalize(entry) == root)
    }

    fn normalize(value: &str) -> String {
        value
            .trim()
            .trim_matches('"')
            .trim_end_matches(['\\', '/'])
            .to_ascii_lowercase()
    }

    fn path_string(path: &Path) -> Result<String, String> {
        let value = path
            .to_str()
            .ok_or_else(|| "Origin Speak manager path is not valid Unicode".to_string())?;
        if value.contains(';') {
            return Err("Origin Speak manager path contains an unsupported semicolon".to_string());
        }
        Ok(value.to_string())
    }

    fn read_path() -> Result<(String, REG_VALUE_TYPE), String> {
        let key = open_environment_key(KEY_READ)?;
        let value_name = wide(PATH_VALUE);
        let mut value_type = REG_EXPAND_SZ;
        let mut byte_len = 0_u32;
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                PCWSTR(value_name.as_ptr()),
                None,
                Some(&mut value_type),
                None,
                Some(&mut byte_len),
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok((String::new(), REG_EXPAND_SZ));
        }
        check(status, "query per-user PATH")?;
        if value_type != REG_SZ && value_type != REG_EXPAND_SZ {
            return Err("per-user PATH has an unsupported registry value type".to_string());
        }
        if byte_len == 0 {
            return Ok((String::new(), value_type));
        }

        let mut bytes = vec![0_u8; byte_len as usize];
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                PCWSTR(value_name.as_ptr()),
                None,
                Some(&mut value_type),
                Some(bytes.as_mut_ptr()),
                Some(&mut byte_len),
            )
        };
        check(status, "read per-user PATH")?;
        let units = bytes[..byte_len as usize]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .take_while(|unit| *unit != 0)
            .collect::<Vec<_>>();
        let value =
            String::from_utf16(&units).map_err(|error| format!("decode per-user PATH: {error}"))?;
        Ok((value, value_type))
    }

    fn write_path(value: &str, value_type: REG_VALUE_TYPE) -> Result<(), String> {
        let key = open_environment_key(KEY_WRITE)?;
        let value_name = wide(PATH_VALUE);
        let wide_value = wide(value);
        let bytes = wide_value
            .iter()
            .flat_map(|unit| unit.to_le_bytes())
            .collect::<Vec<_>>();
        let status = unsafe {
            RegSetValueExW(
                key.0,
                PCWSTR(value_name.as_ptr()),
                None,
                value_type,
                Some(&bytes),
            )
        };
        check(status, "write per-user PATH")
    }

    fn open_environment_key(
        access: windows::Win32::System::Registry::REG_SAM_FLAGS,
    ) -> Result<RegistryKey, String> {
        let subkey = wide(ENVIRONMENT_KEY);
        let mut key = HKEY::default();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                None,
                access,
                &mut key,
            )
        };
        check(status, "open per-user Environment registry key")?;
        Ok(RegistryKey(key))
    }

    fn broadcast_environment_change() {
        let message = wide("Environment");
        let mut result = 0_usize;
        unsafe {
            let _ = SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                WPARAM(0),
                LPARAM(message.as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                1500,
                Some(&mut result),
            );
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn check(status: WIN32_ERROR, operation: &str) -> Result<(), String> {
        if status == ERROR_SUCCESS {
            return Ok(());
        }
        let error = WindowsError::from_hresult(HRESULT::from_win32(status.0));
        Err(format!("{operation}: {error}"))
    }
}

#[cfg(unix)]
mod imp {
    use super::PathRegistration;
    use std::path::Path;

    pub fn status(root: &Path) -> Result<PathRegistration, String> {
        let on_path = std::env::var_os("PATH")
            .map(|value| std::env::split_paths(&value).any(|entry| entry == root))
            .unwrap_or(false);
        Ok(PathRegistration {
            on_path,
            changed: false,
        })
    }

    pub fn ensure(root: &Path) -> Result<PathRegistration, String> {
        status(root)
    }

    pub fn remove(root: &Path) -> Result<PathRegistration, String> {
        status(root).map(|_| PathRegistration {
            on_path: false,
            changed: false,
        })
    }
}

#[cfg(not(any(target_os = "windows", unix)))]
mod imp {
    use super::PathRegistration;
    use std::path::Path;

    pub fn status(_: &Path) -> Result<PathRegistration, String> {
        Err("manager PATH discovery is unsupported on this platform".to_string())
    }
    pub fn ensure(_: &Path) -> Result<PathRegistration, String> {
        Err("manager PATH registration is unsupported on this platform".to_string())
    }
    pub fn remove(_: &Path) -> Result<PathRegistration, String> {
        Err("manager PATH registration is unsupported on this platform".to_string())
    }
}

pub fn status(root: &Path) -> Result<PathRegistration, String> {
    imp::status(root)
}

pub fn ensure(root: &Path) -> Result<PathRegistration, String> {
    imp::ensure(root)
}

pub fn remove(root: &Path) -> Result<PathRegistration, String> {
    imp::remove(root)
}
