//! Exact cleanup for artifacts created by the retired Windows NSIS installer.
//!
//! No parent registry hives or user folders are deleted. Every operation targets
//! a path/key/value that a historical ListenOS installer created explicitly.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LegacyCleanupReport {
    pub removed: Vec<String>,
    pub warnings: Vec<String>,
}

#[cfg(target_os = "windows")]
mod imp {
    use super::LegacyCleanupReport;
    use std::fs;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};
    use windows::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Programs, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    };
    use windows::core::{Error as WindowsError, HRESULT, PCWSTR};

    const LEGACY_KEYS: &[&str] = &[
        r"Software\ListenOS",
        r"Software\Classes\listenos",
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall\ListenOS",
    ];

    pub fn cleanup(
        install_root: &Path,
        manager_path: &Path,
    ) -> Result<LegacyCleanupReport, String> {
        crate::legacy_migration::validate_legacy_install_root(install_root)?;
        let mut report = LegacyCleanupReport::default();

        // Neutralize the historical uninstaller before touching any optional
        // registry/shortcut residue. This is the only legacy artifact that can
        // actively remove the newly installed manager. If it cannot be removed,
        // setup must fail while its binary transaction is still rollback-able.
        let uninstaller = install_root.join("Uninstall.exe");
        if remove_exact_child(install_root, &uninstaller, "Uninstall.exe")? {
            report.removed.push(uninstaller.display().to_string());
        }

        for key in LEGACY_KEYS {
            match delete_tree(key) {
                Ok(true) => report.removed.push(format!("HKCU\\{key}")),
                Ok(false) => {}
                Err(error) => report.warnings.push(error),
            }
        }

        match known_folder(&FOLDERID_Programs) {
            Ok(programs) => {
                let shortcut_dir = programs.join("ListenOS");
                for file in ["ListenOS.lnk", "Uninstall ListenOS.lnk"] {
                    let path = shortcut_dir.join(file);
                    match remove_exact_file(&path) {
                        Ok(true) => report.removed.push(path.display().to_string()),
                        Ok(false) => {}
                        Err(error) => report.warnings.push(error),
                    }
                }
                match fs::remove_dir(&shortcut_dir) {
                    Ok(()) => report.removed.push(shortcut_dir.display().to_string()),
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                        ) => {}
                    Err(error) => report.warnings.push(format!(
                        "remove legacy Start Menu directory {}: {error}",
                        shortcut_dir.display()
                    )),
                }
            }
            Err(error) => report.warnings.push(error),
        }

        match known_folder(&FOLDERID_Desktop) {
            Ok(desktop) => {
                let desktop_shortcut = desktop.join("ListenOS.lnk");
                match remove_exact_file(&desktop_shortcut) {
                    Ok(true) => report.removed.push(desktop_shortcut.display().to_string()),
                    Ok(false) => {}
                    Err(error) => report.warnings.push(error),
                }
            }
            Err(error) => report.warnings.push(error),
        }

        let legacy_executable = install_root.join("ListenOS.exe");
        if !windows_path_alias(&legacy_executable, manager_path) {
            match remove_exact_child(install_root, &legacy_executable, "ListenOS.exe") {
                Ok(true) => report.removed.push(legacy_executable.display().to_string()),
                Ok(false) => {}
                Err(error) => report.warnings.push(error),
            }
        }

        Ok(report)
    }

    fn delete_tree(subkey: &str) -> Result<bool, String> {
        let wide = wide(subkey);
        let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr())) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        check(
            status,
            &format!("remove legacy registry key HKCU\\{subkey}"),
        )?;
        Ok(true)
    }

    fn known_folder(id: &windows::core::GUID) -> Result<PathBuf, String> {
        let raw = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }
            .map_err(|error| format!("resolve Windows known folder: {error}"))?;
        let value = unsafe { raw.to_string() }
            .map_err(|error| format!("decode Windows known folder: {error}"));
        unsafe {
            CoTaskMemFree(Some(raw.0.cast()));
        }
        value.map(PathBuf::from)
    }

    fn remove_exact_child(root: &Path, path: &Path, expected_name: &str) -> Result<bool, String> {
        if path.parent() != Some(root)
            || path.file_name().and_then(|name| name.to_str()) != Some(expected_name)
        {
            return Err(format!(
                "refusing unexpected legacy install path: {}",
                path.display()
            ));
        }
        remove_exact_file(path)
    }

    fn remove_exact_file(path: &Path) -> Result<bool, String> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(format!("inspect {}: {error}", path.display())),
        };
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            return Err(format!(
                "refusing to remove legacy file because it is a directory: {}",
                path.display()
            ));
        }
        fs::remove_file(path).map_err(|error| format!("remove {}: {error}", path.display()))?;
        Ok(true)
    }

    fn wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub(super) fn windows_path_alias(left: &Path, right: &Path) -> bool {
        normalize_windows_path(left) == normalize_windows_path(right)
    }

    fn normalize_windows_path(path: &Path) -> String {
        path.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    }

    fn check(status: WIN32_ERROR, operation: &str) -> Result<(), String> {
        if status == ERROR_SUCCESS {
            return Ok(());
        }
        let error = WindowsError::from_hresult(HRESULT::from_win32(status.0));
        Err(format!("{operation}: {error}"))
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use super::LegacyCleanupReport;
    use std::path::Path;

    pub fn cleanup(_: &Path, _: &Path) -> Result<LegacyCleanupReport, String> {
        Ok(LegacyCleanupReport::default())
    }
}

pub fn cleanup(
    install_root: &std::path::Path,
    manager_path: &std::path::Path,
) -> Result<LegacyCleanupReport, String> {
    imp::cleanup(install_root, manager_path)
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::imp::windows_path_alias;
    use std::path::Path;

    #[test]
    fn legacy_executable_aliases_new_manager_case_insensitively() {
        assert!(windows_path_alias(
            Path::new(r"C:\Users\me\AppData\Local\Programs\ListenOS\ListenOS.exe"),
            Path::new(r"c:/users/me/appdata/local/programs/listenos/listenos.exe"),
        ));
    }
}
