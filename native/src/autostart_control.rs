//! Native autostart control for the CLI manager.
//!
//! Windows uses the per-user Run key directly. Unix delegates normal maintenance
//! to the installed resident runtime. Uninstall has an exact Linux fallback for
//! partial installs; macOS requires the installed app bundle so SMAppService can
//! be unregistered before that bundle is removed.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoStartStatus {
    Disabled,
    Enabled,
    #[cfg(not(target_os = "windows"))]
    RequiresApproval,
    Mismatched,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoStartCleanup {
    pub removed: bool,
    pub detail: String,
}

#[cfg(any(unix, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MissingRuntimeCleanupPolicy {
    DirectPlatformCleanup,
    RequireRuntimeForPlatformCleanup,
}

#[cfg(target_os = "windows")]
mod imp {
    use super::AutoStartStatus;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ, RegCloseKey, RegDeleteValueW,
        RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };
    use windows::core::{Error as WindowsError, HRESULT, PCWSTR};

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "Origin Speak";
    const LEGACY_VALUE_NAME: &str = "ListenOS";

    struct RegistryKey(HKEY);

    impl Drop for RegistryKey {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    pub fn status(runtime: &Path) -> Result<AutoStartStatus, String> {
        let key = open_run_key(KEY_READ)?;
        let value_name = wide(VALUE_NAME);
        let mut value_type = REG_SZ;
        let mut byte_len = 0u32;
        let query = unsafe {
            RegQueryValueExW(
                key.0,
                PCWSTR(value_name.as_ptr()),
                None,
                Some(&mut value_type),
                None,
                Some(&mut byte_len),
            )
        };
        if query == ERROR_FILE_NOT_FOUND {
            return Ok(AutoStartStatus::Disabled);
        }
        check(query, "query Origin Speak login registration")?;
        if value_type != REG_SZ || byte_len < 2 {
            return Ok(AutoStartStatus::Mismatched);
        }

        let mut bytes = vec![0_u8; byte_len as usize];
        let read = unsafe {
            RegQueryValueExW(
                key.0,
                PCWSTR(value_name.as_ptr()),
                None,
                Some(&mut value_type),
                Some(bytes.as_mut_ptr()),
                Some(&mut byte_len),
            )
        };
        check(read, "read Origin Speak login registration")?;
        let units = bytes[..byte_len as usize]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .take_while(|unit| *unit != 0)
            .collect::<Vec<_>>();
        let current = String::from_utf16(&units)
            .map_err(|error| format!("decode Origin Speak login registration: {error}"))?;
        let expected = command_for_runtime(runtime)?;
        Ok(if current == expected {
            AutoStartStatus::Enabled
        } else {
            AutoStartStatus::Mismatched
        })
    }

    pub fn set(runtime: &Path, enabled: bool) -> Result<AutoStartStatus, String> {
        let key = open_run_key(KEY_WRITE)?;
        let value_name = wide(VALUE_NAME);
        if enabled {
            let command = command_for_runtime(runtime)?;
            let wide_command = wide(&command);
            let bytes = wide_command
                .iter()
                .flat_map(|unit| unit.to_le_bytes())
                .collect::<Vec<_>>();
            let status = unsafe {
                RegSetValueExW(
                    key.0,
                    PCWSTR(value_name.as_ptr()),
                    None,
                    REG_SZ,
                    Some(&bytes),
                )
            };
            check(status, "register Origin Speak runtime to launch at login")?;
        } else {
            let status = unsafe { RegDeleteValueW(key.0, PCWSTR(value_name.as_ptr())) };
            if status != ERROR_FILE_NOT_FOUND {
                check(status, "remove Origin Speak login registration")?;
            }
        }
        status(runtime)
    }

    fn command_for_runtime(runtime: &Path) -> Result<String, String> {
        let runtime = runtime
            .to_str()
            .ok_or_else(|| "Origin Speak runtime path is not valid Unicode".to_string())?;
        if runtime.contains('"') {
            return Err(
                "Origin Speak runtime path contains an unsupported quote character".to_string(),
            );
        }
        Ok(format!("\"{runtime}\""))
    }

    fn open_run_key(
        access: windows::Win32::System::Registry::REG_SAM_FLAGS,
    ) -> Result<RegistryKey, String> {
        let subkey = wide(RUN_KEY);
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
        check(status, "open current-user login registry key")?;
        Ok(RegistryKey(key))
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

    pub fn cleanup_legacy_registration() -> Result<bool, String> {
        let key = open_run_key(KEY_WRITE)?;
        let value_name = wide(LEGACY_VALUE_NAME);
        let status = unsafe { RegDeleteValueW(key.0, PCWSTR(value_name.as_ptr())) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        check(status, "remove legacy ListenOS login registration")?;
        Ok(true)
    }
}

#[cfg(unix)]
mod imp {
    use super::AutoStartStatus;
    use std::path::Path;
    use std::process::{Command, Stdio};

    pub fn status(runtime: &Path) -> Result<AutoStartStatus, String> {
        maintenance(runtime, "--maintenance-autostart-status")
    }

    pub fn set(runtime: &Path, enabled: bool) -> Result<AutoStartStatus, String> {
        maintenance(
            runtime,
            if enabled {
                "--maintenance-autostart-enable"
            } else {
                "--maintenance-autostart-disable"
            },
        )
    }

    fn maintenance(runtime: &Path, argument: &str) -> Result<AutoStartStatus, String> {
        if !runtime.is_file() {
            return Err(format!(
                "Origin Speak runtime is not installed at {}",
                runtime.display()
            ));
        }
        #[cfg(target_os = "macos")]
        crate::macos_bundle::verify_runtime_executable(runtime)?;
        let status = Command::new(runtime)
            .arg(argument)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("run Origin Speak autostart maintenance: {error}"))?;
        match status.code() {
            Some(0) => Ok(AutoStartStatus::Enabled),
            Some(10) => Ok(AutoStartStatus::Disabled),
            Some(11) => Ok(AutoStartStatus::RequiresApproval),
            Some(12) => Ok(AutoStartStatus::Mismatched),
            Some(code) => Err(format!(
                "Origin Speak autostart maintenance failed with exit code {code}"
            )),
            None => Err("Origin Speak autostart maintenance terminated unexpectedly".to_string()),
        }
    }
}

#[cfg(not(any(target_os = "windows", unix)))]
mod imp {
    use super::AutoStartStatus;
    use std::path::Path;

    pub fn status(_: &Path) -> Result<AutoStartStatus, String> {
        Err("CLI-managed native autostart is unsupported on this platform".to_string())
    }

    pub fn set(_: &Path, _: bool) -> Result<AutoStartStatus, String> {
        Err("CLI-managed native autostart is unsupported on this platform".to_string())
    }
}

pub fn status(runtime: &Path) -> Result<AutoStartStatus, String> {
    imp::status(runtime)
}

pub fn set(runtime: &Path, enabled: bool) -> Result<AutoStartStatus, String> {
    imp::set(runtime, enabled)
}

#[cfg(target_os = "windows")]
pub fn cleanup_legacy_registration() -> Result<bool, String> {
    imp::cleanup_legacy_registration()
}

pub fn disable_for_uninstall(runtime: &Path) -> Result<AutoStartCleanup, String> {
    #[cfg(target_os = "windows")]
    {
        let previous = status(runtime).unwrap_or(AutoStartStatus::Mismatched);
        let _ = set(runtime, false)?;
        return Ok(AutoStartCleanup {
            removed: !matches!(previous, AutoStartStatus::Disabled),
            detail: "Windows login Run-key registration removed or already absent".to_string(),
        });
    }

    #[cfg(unix)]
    {
        if runtime.is_file() {
            match set(runtime, false) {
                Ok(_) => {
                    return Ok(AutoStartCleanup {
                        removed: true,
                        detail: "resident runtime autostart maintenance completed".to_string(),
                    });
                }
                Err(maintenance_error) => {
                    return cleanup_after_unix_maintenance_failure(maintenance_error);
                }
            }
        }

        return match missing_runtime_cleanup_policy(std::env::consts::OS) {
            MissingRuntimeCleanupPolicy::DirectPlatformCleanup => {
                #[cfg(target_os = "linux")]
                {
                    let removed = remove_linux_autostart_entry()?;
                    Ok(AutoStartCleanup {
                        removed,
                        detail: if removed {
                            "removed exact XDG Origin Speak autostart entry after missing runtime"
                                .to_string()
                        } else {
                            "runtime and XDG Origin Speak autostart entry were already absent"
                                .to_string()
                        },
                    })
                }
                #[cfg(not(target_os = "linux"))]
                {
                    Ok(AutoStartCleanup {
                        removed: false,
                        detail:
                            "runtime missing; no direct platform autostart entry remained to remove"
                                .to_string(),
                    })
                }
            }
            MissingRuntimeCleanupPolicy::RequireRuntimeForPlatformCleanup => Err(format!(
                "cannot unregister macOS SMAppService because the installed Origin Speak runtime bundle is missing at {}; uninstall stopped before app-owned files were removed. Run 'origin setup' to restore the signed bundle, then rerun uninstall so the login item can be unregistered",
                runtime.display()
            )),
        };
    }

    #[cfg(not(any(target_os = "windows", unix)))]
    {
        let _ = runtime;
        Ok(AutoStartCleanup {
            removed: false,
            detail: "autostart cleanup is unsupported on this platform; uninstall continued"
                .to_string(),
        })
    }
}

#[cfg(unix)]
fn cleanup_after_unix_maintenance_failure(
    maintenance_error: String,
) -> Result<AutoStartCleanup, String> {
    #[cfg(target_os = "linux")]
    {
        let removed = remove_linux_autostart_entry()?;
        return Ok(AutoStartCleanup {
            removed,
            detail: if removed {
                format!(
                    "runtime autostart maintenance failed ({maintenance_error}); removed the exact owned XDG autostart entry directly"
                )
            } else {
                format!(
                    "runtime autostart maintenance failed ({maintenance_error}); no owned XDG autostart entry remained"
                )
            },
        });
    }
    #[cfg(target_os = "macos")]
    {
        Err(format!(
            "macOS SMAppService unregister failed ({maintenance_error}); uninstall stopped and the installed Origin Speak.app bundle was kept so autostart cleanup can be retried"
        ))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok(AutoStartCleanup {
            removed: false,
            detail: format!(
                "runtime autostart maintenance failed ({maintenance_error}); uninstall continued"
            ),
        })
    }
}

#[cfg(any(unix, test))]
fn missing_runtime_cleanup_policy(os: &str) -> MissingRuntimeCleanupPolicy {
    if os == "linux" {
        MissingRuntimeCleanupPolicy::DirectPlatformCleanup
    } else {
        MissingRuntimeCleanupPolicy::RequireRuntimeForPlatformCleanup
    }
}

#[cfg(target_os = "linux")]
fn remove_linux_autostart_entry() -> Result<bool, String> {
    let config_home = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(value) if Path::new(&value).is_absolute() => std::path::PathBuf::from(value),
        _ => {
            let home = std::env::var_os("HOME")
                .ok_or_else(|| "HOME is unavailable for XDG autostart cleanup".to_string())?;
            std::path::PathBuf::from(home).join(".config")
        }
    };
    let path = config_home.join("autostart").join("origin-speak.desktop");
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "inspect Origin Speak XDG autostart entry {}: {error}",
                path.display()
            ));
        }
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing to remove non-file XDG autostart path: {}",
            path.display()
        ));
    }
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing to remove symlink at Origin Speak XDG autostart path: {}",
            path.display()
        ));
    }
    let contents = std::fs::read_to_string(&path)
        .map_err(|error| format!("read XDG autostart entry {}: {error}", path.display()))?;
    if !is_owned_linux_autostart_entry(&contents) {
        return Err(format!(
            "refusing to remove unrecognized XDG autostart entry at {}",
            path.display()
        ));
    }
    std::fs::remove_file(&path)
        .map_err(|error| format!("remove XDG autostart entry {}: {error}", path.display()))?;
    Ok(true)
}

#[cfg(any(target_os = "linux", test))]
fn is_owned_linux_autostart_entry(contents: &str) -> bool {
    contents.lines().any(|line| line == "Name=Origin Speak")
        && contents.lines().any(|line| line == "Type=Application")
        && contents
            .lines()
            .any(|line| line.starts_with("Exec=") && line.contains("origin-runtime"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_runtime_cleanup_requires_macos_bundle_for_smappservice_unregister() {
        assert_eq!(
            missing_runtime_cleanup_policy("macos"),
            MissingRuntimeCleanupPolicy::RequireRuntimeForPlatformCleanup
        );
        assert_eq!(
            missing_runtime_cleanup_policy("linux"),
            MissingRuntimeCleanupPolicy::DirectPlatformCleanup
        );
    }

    #[test]
    fn linux_direct_cleanup_requires_recognizable_owned_entry() {
        assert!(is_owned_linux_autostart_entry(
            "[Desktop Entry]\nType=Application\nName=Origin Speak\nExec=\"/home/me/.local/lib/origin-speak/origin-runtime\"\n"
        ));
        assert!(!is_owned_linux_autostart_entry(
            "[Desktop Entry]\nType=Application\nName=Origin Speak\nExec=\"/home/me/bin/something-else\"\n"
        ));
    }
}
