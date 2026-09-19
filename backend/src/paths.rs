use std::path::{Path, PathBuf};

const APP_DIR_NAME: &str = "OriginSpeak";
const LEGACY_APP_DIR_NAME: &str = "ListenOS";

fn roaming_root(name: &str) -> Result<PathBuf, String> {
    dirs_next::data_dir()
        .map(|base| base.join(name))
        .ok_or_else(|| "Could not find application data directory".to_string())
}

fn local_root(name: &str) -> Result<PathBuf, String> {
    if let Some(base) = dirs_next::data_local_dir() {
        return Ok(base.join(name));
    }
    roaming_root(name)
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

pub(crate) fn path_exists_no_follow(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

pub(crate) fn regular_file_exists_no_follow(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(windows)]
    let is_reparse_point = {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    };
    #[cfg(not(windows))]
    let is_reparse_point = false;

    metadata.is_file() && !metadata.file_type().is_symlink() && !is_reparse_point
}

/// Roaming/application data root used for lightweight persisted settings and databases.
pub fn app_data_root() -> Result<PathBuf, String> {
    roaming_root(APP_DIR_NAME)
}

/// Machine-local application data root used for large model artifacts.
/// Falls back to the normal application data root on platforms where a distinct
/// local-data directory is unavailable.
pub fn app_local_data_root() -> Result<PathBuf, String> {
    local_root(APP_DIR_NAME)
}

/// Previous product data root retained for migration and uninstall compatibility.
pub fn legacy_app_data_root() -> Result<PathBuf, String> {
    roaming_root(LEGACY_APP_DIR_NAME)
}

/// Previous product machine-local data root retained for model discovery and cleanup.
pub fn legacy_app_local_data_root() -> Result<PathBuf, String> {
    local_root(LEGACY_APP_DIR_NAME)
}

/// Ordered candidates for lightweight persisted files. New Origin Speak storage is
/// always first; the ListenOS roaming root remains readable during migration.
pub(crate) fn app_data_file_candidates(file_name: &str) -> Result<Vec<PathBuf>, String> {
    let mut candidates = Vec::with_capacity(2);
    push_unique(&mut candidates, app_data_root()?.join(file_name));
    push_unique(&mut candidates, legacy_app_data_root()?.join(file_name));
    Ok(candidates)
}

/// Legacy model roots owned by ListenOS. These remain explicit so migration and
/// uninstall code can distinguish them from active Origin Speak storage.
pub fn legacy_model_storage_roots() -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::with_capacity(2);
    push_unique(&mut roots, legacy_app_local_data_root()?.join("models"));
    push_unique(&mut roots, legacy_app_data_root()?.join("models"));
    Ok(roots)
}

/// Ordered model storage roots: new Origin Speak locations first, followed by
/// legacy ListenOS locations. This keeps existing model installs discoverable
/// while ensuring all new downloads land under the new active data root.
pub fn model_storage_roots() -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::with_capacity(4);
    push_unique(&mut roots, app_local_data_root()?.join("models"));
    push_unique(&mut roots, app_data_root()?.join("models"));
    for root in legacy_model_storage_roots()? {
        push_unique(&mut roots, root);
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_roots_prioritize_origin_speak_and_retain_listenos_discovery() {
        let roots = model_storage_roots().expect("model roots");
        assert!(!roots.is_empty());
        assert!(roots.len() <= 4);
        assert_eq!(
            roots[0]
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str()),
            Some(APP_DIR_NAME)
        );
        assert!(roots.iter().any(|root| {
            root.parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                == Some(LEGACY_APP_DIR_NAME)
        }));
        for root in &roots {
            assert_eq!(
                root.file_name().and_then(|name| name.to_str()),
                Some("models")
            );
            let owner = root
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str());
            assert!(matches!(
                owner,
                Some(APP_DIR_NAME) | Some(LEGACY_APP_DIR_NAME)
            ));
        }
    }

    #[test]
    fn lightweight_data_candidates_keep_origin_speak_first_and_listenos_fallback() {
        let candidates = app_data_file_candidates("config.json").expect("config candidates");
        assert_eq!(candidates[0], app_data_root().unwrap().join("config.json"));
        assert_eq!(
            candidates.last(),
            Some(&legacy_app_data_root().unwrap().join("config.json"))
        );
    }
}
