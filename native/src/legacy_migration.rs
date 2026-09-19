//! Compatibility migration for installations created under the ListenOS identity.
//!
//! Every old product name in this module is intentional. Current Origin Speak
//! paths and registrations live elsewhere; this module only discovers and retires
//! exact legacy artifacts after the replacement runtime is healthy.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LegacyMigrationReport {
    pub removed: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn legacy_install_root() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| "LOCALAPPDATA is unavailable for ListenOS migration".to_string())?;
        return Ok(base.join("Programs").join("ListenOS"));
    }
    #[cfg(target_os = "macos")]
    {
        return Ok(home_dir()?.join("Applications").join("ListenOS.app"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return Ok(home_dir()?.join(".local").join("lib").join("listenos"));
    }
    #[allow(unreachable_code)]
    Err("legacy ListenOS install discovery is unsupported on this platform".to_string())
}

pub fn legacy_manager_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        return Ok(legacy_install_root()?.join("listenos.exe"));
    }
    #[cfg(unix)]
    {
        return Ok(home_dir()?.join(".local").join("bin").join("listenos"));
    }
    #[allow(unreachable_code)]
    Err("legacy ListenOS manager discovery is unsupported on this platform".to_string())
}

pub fn legacy_runtime_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        return Ok(legacy_install_root()?.join("listenos-runtime.exe"));
    }
    #[cfg(target_os = "macos")]
    {
        return Ok(legacy_install_root()?.join("Contents/MacOS/ListenOS"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return Ok(legacy_install_root()?.join("listenos-runtime"));
    }
    #[allow(unreachable_code)]
    Err("legacy ListenOS runtime discovery is unsupported on this platform".to_string())
}

pub fn validate_legacy_install_root(path: &Path) -> Result<(), String> {
    let expected = legacy_install_root()?;
    if path != expected || path.parent().is_none() {
        return Err(format!(
            "refusing unexpected legacy ListenOS install root: {}",
            path.display()
        ));
    }
    Ok(())
}

/// Refuse to mutate a historical macOS ListenOS bundle automatically.
///
/// Pre-Origin-Speak releases registered the *main application* with
/// `SMAppService::mainAppService()` and exposed no command-line maintenance
/// protocol. A different, newly branded main bundle cannot safely unregister
/// that historical main-app service on its behalf. Launching the old executable
/// with a new maintenance flag would enter its normal GPUI loop and can hang
/// migration indefinitely. Deleting the bundle without unregistering it can
/// instead strand a login-item registration.
///
/// The only ownership-safe migration is therefore to require the legacy login
/// item and bundle to be retired explicitly before Origin Speak changes any
/// current installation state. Legacy data/models remain migratable afterwards.
#[cfg(target_os = "macos")]
pub fn preflight_legacy_macos_bundle() -> Result<(), String> {
    let bundle = legacy_install_root()?;
    let metadata = match fs::symlink_metadata(&bundle) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "inspect legacy ListenOS bundle {}: {error}",
                bundle.display()
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "refusing unexpected legacy ListenOS bundle path: {}",
            bundle.display()
        ));
    }
    validate_legacy_macos_bundle(&bundle)?;
    Err(legacy_macos_manual_retirement_message(&bundle))
}

#[cfg(any(target_os = "macos", test))]
fn legacy_macos_manual_retirement_message(bundle: &Path) -> String {
    format!(
        "A pre-Origin-Speak ListenOS.app is still installed at {}. That historical build registered itself as a macOS main-app login item but has no remote unregister protocol, so Origin Speak will not launch it with unsupported flags or delete its bundle behind a potentially registered login item. In System Settings > General > Login Items, disable ListenOS if it is listed, quit ListenOS, remove the old ListenOS.app bundle, then rerun this `origin` command. Existing ListenOS models and user data can still be migrated afterwards.",
        bundle.display()
    )
}

pub fn cleanup_after_replacement(current_exe: &Path) -> Result<LegacyMigrationReport, String> {
    let mut report = LegacyMigrationReport::default();

    #[cfg(target_os = "windows")]
    {
        if crate::autostart_control::cleanup_legacy_registration()? {
            report.removed.push("HKCU Run: ListenOS".to_string());
        }
        let legacy_root = legacy_install_root()?;
        let path_registration = crate::path_control::remove(&legacy_root)?;
        if path_registration.changed {
            report
                .removed
                .push(format!("PATH: {}", legacy_root.display()));
        }
        let cleanup = crate::legacy_cleanup::cleanup(&legacy_root, current_exe)?;
        report.removed.extend(cleanup.removed);
        report.warnings.extend(cleanup.warnings);

        for path in [legacy_runtime_path()?, legacy_manager_path()?] {
            if same_path(&path, current_exe) {
                schedule_delete_on_reboot(&path)?;
                report.warnings.push(format!(
                    "legacy manager removal scheduled after exit: {}",
                    path.display()
                ));
            } else if remove_exact_regular_file(&path)? {
                report.removed.push(path.display().to_string());
            }
        }
        remove_legacy_install_root_if_empty(&legacy_root, current_exe, &mut report)?;
    }

    #[cfg(target_os = "linux")]
    {
        if remove_legacy_linux_autostart()? {
            report
                .removed
                .push("XDG autostart: listenos.desktop".to_string());
        }
        let legacy_root = legacy_install_root()?;
        for path in [legacy_runtime_path()?, legacy_manager_path()?] {
            if remove_exact_regular_file(&path)? {
                report.removed.push(path.display().to_string());
            }
        }
        remove_empty_dir(&legacy_root, &mut report)?;
    }

    #[cfg(target_os = "macos")]
    {
        // Re-check at cleanup time as well as command preflight. This closes a
        // race where the legacy bundle could reappear between preflight and the
        // final compatibility cleanup.
        preflight_legacy_macos_bundle()?;
        let manager = legacy_manager_path()?;
        if remove_exact_regular_file(&manager)? {
            report.removed.push(manager.display().to_string());
        }
    }

    let _ = current_exe;
    Ok(report)
}

#[cfg(target_os = "windows")]
fn remove_legacy_install_root_if_empty(
    root: &Path,
    current_exe: &Path,
    report: &mut LegacyMigrationReport,
) -> Result<(), String> {
    validate_legacy_install_root(root)?;
    match fs::remove_dir(root) {
        Ok(()) => report.removed.push(root.display().to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {
            if current_exe.starts_with(root) {
                schedule_delete_on_reboot(root)?;
            }
        }
        Err(error) => {
            return Err(format!(
                "remove legacy install root {}: {error}",
                root.display()
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn remove_legacy_linux_autostart() -> Result<bool, String> {
    let config_home = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(value) if Path::new(&value).is_absolute() => PathBuf::from(value),
        _ => home_dir()?.join(".config"),
    };
    let path = config_home.join("autostart").join("listenos.desktop");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "inspect legacy autostart {}: {error}",
                path.display()
            ));
        }
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing unexpected legacy ListenOS autostart path: {}",
            path.display()
        ));
    }
    let contents = fs::read_to_string(&path)
        .map_err(|error| format!("read legacy autostart {}: {error}", path.display()))?;
    let owned = contents.lines().any(|line| line == "Name=ListenOS")
        && contents.lines().any(|line| {
            line.starts_with("Exec=")
                && (line.contains("listenos-runtime") || line.contains("/listenos/"))
        });
    if !owned {
        return Err(format!(
            "refusing unrecognized legacy ListenOS autostart entry: {}",
            path.display()
        ));
    }
    fs::remove_file(&path)
        .map_err(|error| format!("remove legacy autostart {}: {error}", path.display()))?;
    Ok(true)
}

#[cfg(target_os = "macos")]
fn validate_legacy_macos_bundle(bundle: &Path) -> Result<(), String> {
    validate_legacy_install_root(bundle)?;
    let info = bundle.join("Contents/Info.plist");
    let runtime = legacy_runtime_path()?;
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
        .arg(&info)
        .output()
        .map_err(|error| format!("inspect legacy ListenOS bundle identity: {error}"))?;
    if !output.status.success()
        || String::from_utf8_lossy(&output.stdout).trim() != "com.listenos.app"
        || !runtime.is_file()
    {
        return Err(format!(
            "refusing unrecognized legacy ListenOS bundle: {}",
            bundle.display()
        ));
    }
    Ok(())
}

fn remove_exact_regular_file(path: &Path) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("inspect legacy file {}: {error}", path.display())),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || is_windows_reparse_point(&metadata)
    {
        return Err(format!(
            "refusing legacy path that is not a regular file: {}",
            path.display()
        ));
    }
    fs::remove_file(path)
        .map_err(|error| format!("remove legacy file {}: {error}", path.display()))?;
    Ok(true)
}

#[cfg(target_os = "linux")]
fn remove_empty_dir(path: &Path, report: &mut LegacyMigrationReport) -> Result<(), String> {
    validate_legacy_install_root(path)?;
    match fs::remove_dir(path) {
        Ok(()) => report.removed.push(path.display().to_string()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
            ) => {}
        Err(error) => {
            return Err(format!(
                "remove legacy directory {}: {error}",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn remove_tree_no_follow(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() {
        return fs::remove_file(path);
    }
    if metadata.is_file() {
        return fs::remove_file(path);
    }
    for entry in fs::read_dir(path)? {
        remove_tree_no_follow(&entry?.path())?;
    }
    fs::remove_dir(path)
}

#[cfg(target_os = "windows")]
fn schedule_delete_on_reboot(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    const MOVEFILE_DELAY_UNTIL_REBOOT: u32 = 0x0000_0004;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    if unsafe { MoveFileExW(wide.as_ptr(), ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) } == 0 {
        return Err(format!(
            "schedule legacy path removal {}: {}",
            path.display(),
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn same_path(left: &Path, right: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        return left
            .to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(
                right
                    .to_string_lossy()
                    .replace('/', "\\")
                    .trim_end_matches('\\'),
            );
    }
    #[cfg(not(target_os = "windows"))]
    {
        left == right
    }
}

#[cfg(unix)]
fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .ok_or_else(|| "could not resolve home directory for ListenOS migration".to_string())
}

#[cfg(target_os = "windows")]
fn is_windows_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(target_os = "windows"))]
fn is_windows_reparse_point(_: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_paths_are_distinct_from_origin_speak_paths() {
        assert_ne!(
            legacy_manager_path().unwrap(),
            crate::management::DataLayout::discover()
                .unwrap()
                .manager_path
        );
        assert_ne!(
            legacy_runtime_path().unwrap(),
            crate::management::DataLayout::discover()
                .unwrap()
                .runtime_path
        );
    }

    #[test]
    fn macos_manual_retirement_message_is_actionable_and_bounded() {
        let message = legacy_macos_manual_retirement_message(Path::new(
            "/Users/example/Applications/ListenOS.app",
        ));
        assert!(message.contains("System Settings > General > Login Items"));
        assert!(message.contains("remove the old ListenOS.app bundle"));
        assert!(message.contains("rerun this `origin` command"));
        assert!(!message.contains("--maintenance-autostart-disable"));
    }
}
