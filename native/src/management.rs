//! Filesystem and lifecycle primitives for the Origin Speak CLI host.
//!
//! This module intentionally stays independent from GPUI and the current desktop
//! runtime so it can be wired into `main.rs` without pulling dashboard state into
//! CLI commands.  Platform process control, autostart, and update installation
//! should be supplied by the native shell adapter; this file owns only portable
//! path discovery and safe data removal.

use origin_speak_lib::{
    app_data_root, app_local_data_root, legacy_app_data_root, legacy_app_local_data_root,
    model_storage_roots,
};
use semver::Version;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const PRODUCT_DIR: &str = "Origin Speak";
const DATA_DIR: &str = "OriginSpeak";
const UPDATE_TEMP_PREFIX: &str = "origin-speak-update-";
pub const UPDATE_STAGING_MARKER_FILE: &str = ".origin-speak-update-staging-v1";

pub fn update_staging_marker_contents(version: &str) -> String {
    format!("Origin Speak update staging v1\nversion={version}\n")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataLayout {
    /// Authoritative backend data root. This contains models, config and SQLite data.
    pub data_root: PathBuf,
    /// Exact machine-local Origin Speak data root. On Windows this is distinct from
    /// roaming data and may contain model/runtime artifacts outside `data_root`.
    pub local_data_root: PathBuf,
    /// Per-user installation root used by the CLI/bootstrap flow.
    pub install_root: PathBuf,
    /// Directory containing the installed CLI manager.
    pub manager_root: PathBuf,
    /// Installed console manager target. Setup may run from another location;
    /// uninstall removes this path only when it is the exact app-owned target.
    pub manager_path: PathBuf,
    /// Installed silent runtime target.
    pub runtime_path: PathBuf,
    /// System temporary directory where legacy/new updater payloads may be staged.
    pub temp_root: PathBuf,
    /// Current and legacy model storage directories reported by the backend.
    /// Only exact Origin Speak model leaves are eligible for removal.
    pub model_roots: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UninstallPlan {
    pub data_root: Option<PathBuf>,
    pub local_data_root: Option<PathBuf>,
    pub model_roots: Vec<PathBuf>,
    pub install_root: PathBuf,
    pub manager_root: PathBuf,
    pub manager_path: PathBuf,
    pub runtime_path: PathBuf,
    pub stale_update_paths: Vec<PathBuf>,
}

#[derive(Debug, Default)]
pub struct LegacyDataMigration {
    moves: Vec<(PathBuf, PathBuf)>,
}

impl LegacyDataMigration {
    pub fn begin() -> Result<Self, String> {
        let mut pairs = vec![
            (legacy_app_data_root()?, app_data_root()?),
            (legacy_app_local_data_root()?, app_local_data_root()?),
        ];
        pairs.dedup();

        let mut migration = Self::default();
        for (legacy, current) in pairs {
            if legacy == current || !path_exists_no_follow(&legacy)? {
                continue;
            }
            validate_owned_data_directory(&legacy, "legacy data root")?;
            if path_exists_no_follow(&current)? {
                continue;
            }
            let parent = current.parent().ok_or_else(|| {
                format!(
                    "Origin Speak data root has no parent: {}",
                    current.display()
                )
            })?;
            fs::create_dir_all(parent)
                .map_err(|error| format!("create data parent {}: {error}", parent.display()))?;
            fs::rename(&legacy, &current).map_err(|error| {
                format!(
                    "migrate legacy ListenOS data {} to Origin Speak {}: {error}",
                    legacy.display(),
                    current.display()
                )
            })?;
            migration.moves.push((legacy, current));
        }
        Ok(migration)
    }

    pub fn rollback(&mut self) -> Result<(), String> {
        for (legacy, current) in self.moves.iter().rev() {
            if path_exists_no_follow(legacy)? {
                return Err(format!(
                    "cannot roll back data migration because legacy path reappeared: {}",
                    legacy.display()
                ));
            }
            validate_owned_data_directory(current, "migrated data root")?;
            fs::rename(current, legacy).map_err(|error| {
                format!(
                    "restore migrated data {} to legacy path {}: {error}",
                    current.display(),
                    legacy.display()
                )
            })?;
        }
        self.moves.clear();
        Ok(())
    }

    pub fn migrated_count(&self) -> usize {
        self.moves.len()
    }
}

impl DataLayout {
    pub fn discover() -> Result<Self, String> {
        let install_root = install_root()?;
        let manager_root = manager_root()?;
        let runtime_path = runtime_path_for_install_root(&install_root);
        Ok(Self {
            data_root: app_data_root()?,
            local_data_root: app_local_data_root()?,
            manager_path: manager_root.join(manager_file_name()),
            runtime_path,
            install_root,
            manager_root,
            temp_root: env::temp_dir(),
            model_roots: model_storage_roots()?,
        })
    }

    pub fn uninstall_plan(&self, keep_data: bool) -> Result<UninstallPlan, String> {
        validate_data_root(&self.data_root)?;
        validate_local_data_root(&self.local_data_root)?;
        validate_install_path(&self.install_root)?;
        validate_manager_root(&self.manager_root)?;
        validate_install_file(&self.manager_path, &self.manager_root, manager_file_name())?;
        validate_runtime_path(&self.runtime_path, &self.install_root)?;
        for model_root in &self.model_roots {
            validate_model_root(model_root)?;
        }

        Ok(UninstallPlan {
            data_root: (!keep_data).then(|| self.data_root.clone()),
            local_data_root: (!keep_data && self.local_data_root != self.data_root)
                .then(|| self.local_data_root.clone()),
            model_roots: if keep_data {
                Vec::new()
            } else {
                self.model_roots.clone()
            },
            install_root: self.install_root.clone(),
            manager_root: self.manager_root.clone(),
            manager_path: self.manager_path.clone(),
            runtime_path: self.runtime_path.clone(),
            stale_update_paths: discover_stale_update_paths(&self.temp_root),
        })
    }

    pub fn update_staging_dir(&self, version: &str) -> Result<PathBuf, String> {
        if version.is_empty()
            || !version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
        {
            return Err("update version contains unsupported path characters".to_string());
        }
        Ok(self
            .temp_root
            .join(format!("origin-speak-update-{version}")))
    }

    pub fn current_manager_is_installed(&self) -> bool {
        std::env::current_exe()
            .ok()
            .and_then(|path| canonical_if_exists(&path))
            == canonical_if_exists(&self.manager_path)
    }
}

/// Remove all user-owned Origin Speak state: models, configuration, databases, and
/// other runtime data under the single backend data root. This is the default
/// uninstall behavior. `--keep-data` must be explicit to skip this operation.
pub fn purge_user_data(layout: &DataLayout) -> Result<(), String> {
    validate_data_root(&layout.data_root)?;
    validate_local_data_root(&layout.local_data_root)?;
    for model_root in &layout.model_roots {
        validate_model_root(model_root)?;
    }

    remove_tree_no_follow(&layout.data_root)
        .map_err(|error| format!("remove {}: {error}", layout.data_root.display()))?;

    if layout.local_data_root != layout.data_root {
        remove_tree_no_follow(&layout.local_data_root).map_err(|error| {
            format!(
                "remove local data root {}: {error}",
                layout.local_data_root.display()
            )
        })?;
    }

    for model_root in &layout.model_roots {
        remove_tree_no_follow(model_root)
            .map_err(|error| format!("remove model root {}: {error}", model_root.display()))?;
    }

    for path in discover_stale_update_paths(&layout.temp_root) {
        remove_tree_no_follow(&path)
            .map_err(|error| format!("remove stale update {}: {error}", path.display()))?;
    }
    Ok(())
}

pub fn remove_installed_runtime(layout: &DataLayout) -> Result<bool, String> {
    validate_runtime_path(&layout.runtime_path, &layout.install_root)?;
    #[cfg(target_os = "macos")]
    {
        return crate::macos_bundle::remove_owned_bundle(&layout.install_root);
    }
    #[cfg(not(target_os = "macos"))]
    {
        remove_install_file(&layout.runtime_path)
    }
}

pub fn remove_installed_manager(layout: &DataLayout) -> Result<bool, String> {
    validate_install_file(
        &layout.manager_path,
        &layout.manager_root,
        manager_file_name(),
    )?;
    remove_install_file(&layout.manager_path)
}

pub fn remove_empty_install_root(layout: &DataLayout) -> Result<(), String> {
    validate_install_path(&layout.install_root)?;
    #[cfg(target_os = "macos")]
    {
        if layout.install_root.exists() {
            return Err(format!(
                "macOS runtime bundle still exists after runtime removal: {}",
                layout.install_root.display()
            ));
        }
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    match fs::remove_dir(&layout.install_root) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(format!(
            "remove install directory {}: {error}",
            layout.install_root.display()
        )),
    }
}

/// Returns true when the command must not prompt on stdin. The integration
/// layer can use this together with `--yes`: destructive commands in a pipe or
/// CI environment should fail with an actionable message rather than hang.
pub fn non_interactive_terminal() -> bool {
    use std::io::IsTerminal;
    !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal()
}

fn install_root() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        return Ok(
            env_path("LOCALAPPDATA", "Windows local application-data directory")?
                .join("Programs")
                .join(PRODUCT_DIR),
        );
    }
    #[cfg(target_os = "macos")]
    {
        return Ok(home_dir()?
            .join("Applications")
            .join(crate::macos_bundle::BUNDLE_NAME));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return Ok(home_dir()?.join(".local").join("lib").join("origin-speak"));
    }
    #[allow(unreachable_code)]
    Err("Origin Speak install-directory discovery is unsupported on this platform".to_string())
}

fn manager_root() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        return install_root();
    }
    #[cfg(unix)]
    {
        return Ok(home_dir()?.join(".local").join("bin"));
    }
    #[allow(unreachable_code)]
    Err("Origin Speak manager-directory discovery is unsupported on this platform".to_string())
}

fn manager_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "origin.exe"
    } else {
        "origin"
    }
}

fn runtime_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "origin-runtime.exe"
    } else {
        "origin-runtime"
    }
}

fn runtime_path_for_install_root(install_root: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        crate::macos_bundle::runtime_executable(install_root)
    }
    #[cfg(not(target_os = "macos"))]
    {
        install_root.join(runtime_file_name())
    }
}

fn env_path(name: &str, description: &str) -> Result<PathBuf, String> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| format!("Could not resolve {description}: {name} is not set"))
}

fn home_dir() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    let candidates = ["USERPROFILE", "HOME"];
    #[cfg(not(target_os = "windows"))]
    let candidates = ["HOME", "USERPROFILE"];

    candidates
        .into_iter()
        .find_map(|name| {
            env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .ok_or_else(|| "Could not resolve the current user's home directory".to_string())
}

fn validate_data_root(path: &Path) -> Result<(), String> {
    let expected = app_data_root()?;
    if path != expected || path.file_name().and_then(|name| name.to_str()) != Some(DATA_DIR) {
        return Err(format!(
            "Refusing destructive operation on unexpected data path: {}",
            path.display()
        ));
    }
    validate_not_root_or_home(path)
}

fn validate_local_data_root(path: &Path) -> Result<(), String> {
    let expected = app_local_data_root()?;
    if path != expected || path.file_name().and_then(|name| name.to_str()) != Some(DATA_DIR) {
        return Err(format!(
            "Refusing destructive operation on unexpected local data path: {}",
            path.display()
        ));
    }
    validate_not_root_or_home(path)
}

fn validate_install_path(path: &Path) -> Result<(), String> {
    validate_not_root_or_home(path)?;
    if path != install_root()? {
        return Err(format!(
            "Refusing destructive operation on unexpected install path: {}",
            path.display()
        ));
    }
    #[cfg(target_os = "windows")]
    validate_windows_install_root_resolution(path)?;
    Ok(())
}

fn validate_manager_root(path: &Path) -> Result<(), String> {
    validate_not_root_or_home(path)?;
    if path != manager_root()? {
        return Err(format!(
            "Refusing destructive operation on unexpected manager root: {}",
            path.display()
        ));
    }
    #[cfg(target_os = "windows")]
    validate_windows_install_root_resolution(path)?;
    Ok(())
}

pub(crate) fn validate_manager_root_ownership(path: &Path) -> Result<(), String> {
    validate_manager_root(path)
}

#[cfg(target_os = "windows")]
pub(crate) fn validate_known_windows_owned_root(path: &Path) -> Result<(), String> {
    let install = install_root()?;
    let manager = manager_root()?;
    if path == install || path == manager {
        validate_windows_install_root_resolution(path)?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn validate_windows_install_root_resolution(path: &Path) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "inspect Windows install root {}: {error}",
                path.display()
            ));
        }
    };
    if !metadata.is_dir() || is_windows_reparse_point(&metadata) {
        return Err(format!(
            "Refusing Windows install root that is not a real directory or is a reparse point: {}",
            path.display()
        ));
    }

    let local_app_data = env_path("LOCALAPPDATA", "Windows local application-data directory")?;
    let resolved_local = fs::canonicalize(&local_app_data).map_err(|error| {
        format!(
            "resolve Windows local application-data root {}: {error}",
            local_app_data.display()
        )
    })?;
    let resolved_root = fs::canonicalize(path)
        .map_err(|error| format!("resolve Windows install root {}: {error}", path.display()))?;
    let relative = resolved_root.strip_prefix(&resolved_local).map_err(|_| {
        format!(
            "Refusing Windows install root that resolves outside LOCALAPPDATA: {} -> {}",
            path.display(),
            resolved_root.display()
        )
    })?;
    if !windows_install_relative_ownership_matches(relative) {
        return Err(format!(
            "Refusing Windows install root whose resolved ownership path changed: {} -> {}",
            path.display(),
            resolved_root.display()
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "windows", test))]
fn windows_install_relative_ownership_matches(relative: &Path) -> bool {
    let components = relative
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>();
    components.len() == 2
        && components[0].eq_ignore_ascii_case("Programs")
        && components[1].eq_ignore_ascii_case(PRODUCT_DIR)
}

fn validate_model_root(path: &Path) -> Result<(), String> {
    validate_not_root_or_home(path)?;
    if path.file_name().and_then(|name| name.to_str()) != Some("models") {
        return Err(format!(
            "Refusing unexpected model path: {}",
            path.display()
        ));
    }
    if !model_storage_roots()?
        .iter()
        .any(|expected| expected == path)
    {
        return Err(format!("Refusing unknown model path: {}", path.display()));
    }
    Ok(())
}

fn path_exists_no_follow(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("inspect {}: {error}", path.display())),
    }
}

fn validate_owned_data_directory(path: &Path, label: &str) -> Result<(), String> {
    validate_not_root_or_home(path)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspect {label} {}: {error}", path.display()))?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || is_windows_reparse_point(&metadata)
    {
        return Err(format!(
            "refusing {label} that is not a real directory: {}",
            path.display()
        ));
    }
    let allowed = [
        app_data_root()?,
        app_local_data_root()?,
        legacy_app_data_root()?,
        legacy_app_local_data_root()?,
    ];
    if !allowed.iter().any(|candidate| candidate == path) {
        return Err(format!("refusing unexpected {label}: {}", path.display()));
    }
    Ok(())
}

fn validate_install_file(path: &Path, root: &Path, expected_name: &str) -> Result<(), String> {
    let install_root = install_root()?;
    let manager_root = manager_root()?;
    if root == install_root {
        validate_install_path(root)?;
    } else if root == manager_root {
        validate_manager_root(root)?;
    } else {
        return Err(format!(
            "Refusing unexpected install-owned root: {}",
            root.display()
        ));
    }
    if path.parent() != Some(root)
        || path.file_name().and_then(|name| name.to_str()) != Some(expected_name)
    {
        return Err(format!(
            "Refusing unexpected install-owned path: {}",
            path.display()
        ));
    }
    Ok(())
}

fn validate_runtime_path(path: &Path, root: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        validate_install_path(root)?;
        let expected = crate::macos_bundle::runtime_executable(root);
        if path != expected {
            return Err(format!(
                "Refusing unexpected macOS runtime executable path: {}",
                path.display()
            ));
        }
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    {
        validate_install_file(path, root, runtime_file_name())
    }
}

fn validate_not_root_or_home(path: &Path) -> Result<(), String> {
    if path.parent().is_none() || path.as_os_str().is_empty() {
        return Err(format!(
            "Refusing destructive operation on filesystem root: {}",
            path.display()
        ));
    }
    if let Ok(home) = home_dir()
        && path == home
    {
        return Err("Refusing destructive operation on the user home directory".to_string());
    }
    Ok(())
}

fn discover_stale_update_paths(temp_root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(temp_root) else {
        return Vec::new();
    };
    let mut files = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            let version = name.strip_prefix(UPDATE_TEMP_PREFIX)?;
            if Version::parse(version).is_err() {
                return None;
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).ok()?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return None;
            }
            let marker = path.join(UPDATE_STAGING_MARKER_FILE);
            let marker_metadata = fs::symlink_metadata(&marker).ok()?;
            if !marker_metadata.is_file() || marker_metadata.file_type().is_symlink() {
                return None;
            }
            let contents = fs::read_to_string(marker).ok()?;
            (contents == update_staging_marker_contents(version)).then_some(path)
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn remove_install_file(path: &Path) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("inspect {}: {error}", path.display())),
    };
    if metadata.is_dir()
        && !metadata.file_type().is_symlink()
        && !is_windows_reparse_point(&metadata)
    {
        return Err(format!(
            "Refusing to remove install file because it is a directory: {}",
            path.display()
        ));
    }
    remove_link(path, false).map_err(|error| format!("remove {}: {error}", path.display()))?;
    Ok(true)
}

fn canonical_if_exists(path: &Path) -> Option<PathBuf> {
    fs::canonicalize(path)
        .ok()
        .or_else(|| Some(path.to_path_buf()))
}

fn remove_tree_no_follow(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };

    if metadata.file_type().is_symlink() || is_windows_reparse_point(&metadata) {
        return remove_link(path, metadata.is_dir());
    }
    if metadata.is_file() {
        return fs::remove_file(path);
    }
    if !metadata.is_dir() {
        return fs::remove_file(path);
    }

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        remove_tree_no_follow(&entry.path())?;
    }
    fs::remove_dir(path)
}

fn remove_link(path: &Path, directory: bool) -> io::Result<()> {
    if directory {
        fs::remove_dir(path).or_else(|_| fs::remove_file(path))
    } else {
        fs::remove_file(path).or_else(|_| fs::remove_dir(path))
    }
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
    fn uninstall_removes_data_by_default() {
        let base = env::temp_dir().join("origin-speak-management-test-base");
        let layout = DataLayout {
            data_root: app_data_root().unwrap(),
            local_data_root: app_local_data_root().unwrap(),
            install_root: install_root().unwrap(),
            manager_root: manager_root().unwrap(),
            manager_path: manager_root().unwrap().join(manager_file_name()),
            runtime_path: runtime_path_for_install_root(&install_root().unwrap()),
            temp_root: base,
            model_roots: model_storage_roots().unwrap(),
        };
        let plan = layout.uninstall_plan(false).unwrap();
        assert_eq!(plan.data_root.as_deref(), Some(layout.data_root.as_path()));
        assert_eq!(
            plan.local_data_root.as_deref(),
            (layout.local_data_root != layout.data_root)
                .then_some(layout.local_data_root.as_path())
        );
    }

    #[test]
    fn keep_data_is_explicit_opt_out() {
        let layout = DataLayout::discover().unwrap();
        let plan = layout.uninstall_plan(true).unwrap();
        assert!(plan.data_root.is_none());
        assert!(plan.local_data_root.is_none());
    }

    #[test]
    fn unexpected_data_path_is_rejected() {
        let wrong = app_data_root().unwrap().with_file_name("SomethingElse");
        assert!(validate_data_root(&wrong).is_err());
    }

    #[test]
    fn runtime_and_manager_are_scoped_to_owned_roots() {
        let layout = DataLayout::discover().unwrap();
        assert_eq!(
            layout.manager_path.parent(),
            Some(layout.manager_root.as_path())
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            layout.runtime_path,
            layout.install_root.join("Contents/MacOS/origin-runtime")
        );
        #[cfg(not(target_os = "macos"))]
        assert_eq!(
            layout.runtime_path.parent(),
            Some(layout.install_root.as_path())
        );
    }

    #[test]
    fn colliding_update_directory_without_marker_is_preserved() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "origin-speak-management-staging-test-{}-{nonce}",
            std::process::id()
        ));
        let unrelated = root.join("origin-speak-update-1.2.3");
        let owned = root.join("origin-speak-update-2.3.4");
        fs::create_dir_all(&unrelated).unwrap();
        fs::write(unrelated.join("keep.txt"), b"not Origin Speak-owned").unwrap();
        fs::create_dir_all(&owned).unwrap();
        fs::write(
            owned.join(UPDATE_STAGING_MARKER_FILE),
            update_staging_marker_contents("2.3.4"),
        )
        .unwrap();

        let discovered = discover_stale_update_paths(&root);
        assert_eq!(discovered, vec![owned.clone()]);
        for path in discovered {
            remove_tree_no_follow(&path).unwrap();
        }
        assert!(unrelated.join("keep.txt").is_file());
        assert!(!owned.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn windows_install_relative_ownership_is_exact() {
        assert!(windows_install_relative_ownership_matches(Path::new(
            "Programs/Origin Speak"
        )));
        assert!(windows_install_relative_ownership_matches(Path::new(
            "programs/origin speak"
        )));
        assert!(!windows_install_relative_ownership_matches(Path::new(
            "Other/Origin Speak"
        )));
        assert!(!windows_install_relative_ownership_matches(Path::new(
            "Programs/Origin Speak/Nested"
        )));
    }
}
