#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const BUNDLE_NAME: &str = "Origin Speak.app";
pub const BUNDLE_IDENTIFIER: &str = "com.originspeak.app";
pub const BUNDLE_EXECUTABLE_RELATIVE: &str = "Contents/MacOS/origin-runtime";
const STAGING_MARKER: &str = ".origin-speak-macos-bootstrap-v1";
const TRANSACTION_MARKER_PREFIX: &str = ".origin-speak-transaction-";
const TRANSACTION_MARKER_SUFFIX: &str = ".marker";
const TRANSACTION_MARKER_SCHEMA: &str = "Origin Speak macOS bundle transaction v1";
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
struct TransactionPaths {
    marker: PathBuf,
    extraction_root: PathBuf,
    staged_bundle: PathBuf,
    backup_bundle: PathBuf,
}

impl TransactionPaths {
    fn new(parent: &Path, token: &str) -> Self {
        Self {
            marker: parent.join(format!(
                "{TRANSACTION_MARKER_PREFIX}{token}{TRANSACTION_MARKER_SUFFIX}"
            )),
            extraction_root: parent.join(format!(".origin-speak-extracting-{token}")),
            staged_bundle: parent.join(format!(".origin-speak-installing-{token}.app")),
            backup_bundle: parent.join(format!(".origin-speak-backup-{token}.app")),
        }
    }
}

#[derive(Debug)]
pub struct BundleSwap {
    destination: PathBuf,
    backup: Option<PathBuf>,
    transaction_marker: Option<PathBuf>,
    committed: bool,
}

impl BundleSwap {
    pub fn rollback(&mut self) -> Result<(), String> {
        if !self.committed {
            return Ok(());
        }
        if let Some(backup) = self.backup.as_ref() {
            self.validate_backup_ownership(backup)?;
        } else if let Some(marker) = self.transaction_marker.as_ref() {
            if transaction_from_marker(marker)?.is_none() {
                return Err(format!(
                    "macOS bundle transaction marker is not owned by Origin Speak: {}",
                    marker.display()
                ));
            }
        }
        if self.destination.exists() {
            remove_tree_no_follow(&self.destination).map_err(|error| {
                format!(
                    "remove replacement macOS runtime bundle {} during rollback: {error}",
                    self.destination.display()
                )
            })?;
        }
        if let Some(backup) = self.backup.as_ref() {
            fs::rename(backup, &self.destination).map_err(|error| {
                format!(
                    "restore macOS runtime bundle backup {} to {}: {error}",
                    backup.display(),
                    self.destination.display()
                )
            })?;
        }
        self.remove_transaction_marker()?;
        self.committed = false;
        Ok(())
    }

    pub fn finalize(&mut self) -> Result<(), String> {
        if let Some(backup) = self.backup.as_ref() {
            self.validate_backup_ownership(backup)?;
            cleanup_ephemeral(backup)?;
            self.backup = None;
        }
        self.remove_transaction_marker()?;
        self.committed = false;
        Ok(())
    }

    fn validate_backup_ownership(&self, backup: &Path) -> Result<(), String> {
        if let Some(marker) = self.transaction_marker.as_ref() {
            let transaction = transaction_from_marker(marker)?.ok_or_else(|| {
                format!(
                    "macOS bundle transaction marker is not owned by Origin Speak: {}",
                    marker.display()
                )
            })?;
            if transaction.backup_bundle != backup {
                return Err(format!(
                    "macOS bundle backup is not claimed by transaction marker: {}",
                    backup.display()
                ));
            }
        }
        Ok(())
    }

    fn remove_transaction_marker(&mut self) -> Result<(), String> {
        let Some(marker) = self.transaction_marker.clone() else {
            return Ok(());
        };
        let transaction = transaction_from_marker(&marker)?.ok_or_else(|| {
            format!(
                "refusing to remove unrecognized macOS bundle transaction marker {}",
                marker.display()
            )
        })?;
        if transaction.marker != marker {
            return Err(format!(
                "macOS bundle transaction marker path changed unexpectedly: {}",
                marker.display()
            ));
        }
        fs::remove_file(&marker).map_err(|error| {
            format!(
                "remove completed macOS bundle transaction marker {}: {error}",
                marker.display()
            )
        })?;
        self.transaction_marker = None;
        Ok(())
    }
}

pub fn runtime_executable(bundle: &Path) -> PathBuf {
    bundle.join(BUNDLE_EXECUTABLE_RELATIVE)
}

pub fn install_bundle_zip(archive: &Path, destination: &Path) -> Result<bool, String> {
    let mut swap = transactional_install_bundle_zip(archive, destination)?;
    swap.finalize()?;
    Ok(true)
}

pub fn transactional_install_bundle_zip(
    archive: &Path,
    destination: &Path,
) -> Result<BundleSwap, String> {
    validate_destination_shape(destination)?;
    let archive_metadata = fs::symlink_metadata(archive).map_err(|error| {
        format!(
            "inspect macOS runtime archive {}: {error}",
            archive.display()
        )
    })?;
    if archive_metadata.file_type().is_symlink() || !archive_metadata.is_file() {
        return Err(format!(
            "macOS runtime bundle archive must be a regular file: {}",
            archive.display()
        ));
    }

    let parent = destination
        .parent()
        .ok_or_else(|| "macOS runtime bundle has no parent directory".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create macOS runtime parent {}: {error}", parent.display()))?;

    let transaction = allocate_staging_paths(parent)?;
    let extraction_root = &transaction.extraction_root;
    let staged_bundle = &transaction.staged_bundle;
    let backup_bundle = &transaction.backup_bundle;

    let extraction = Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(archive)
        .arg(extraction_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("run /usr/bin/ditto to extract runtime bundle: {error}"))?;
    if !extraction.status.success() {
        let _ = cleanup_owned_transaction(&transaction);
        return Err(format!(
            "extract macOS runtime bundle with /usr/bin/ditto: {}",
            stderr_text(&extraction.stderr)
        ));
    }

    let extracted_bundle = match exact_extracted_bundle(&extraction_root) {
        Ok(bundle) => bundle,
        Err(error) => {
            let _ = cleanup_owned_transaction(&transaction);
            return Err(error);
        }
    };
    if let Err(error) = fs::rename(&extracted_bundle, staged_bundle) {
        let _ = cleanup_owned_transaction(&transaction);
        return Err(format!(
            "stage extracted macOS runtime bundle {}: {error}",
            staged_bundle.display()
        ));
    }
    cleanup_owned_extraction(extraction_root)?;

    if let Err(error) = verify_bundle(staged_bundle) {
        let _ = cleanup_owned_transaction(&transaction);
        return Err(error);
    }

    let had_existing = match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                let _ = cleanup_owned_transaction(&transaction);
                return Err(format!(
                    "refusing to replace non-directory macOS runtime bundle path {}",
                    destination.display()
                ));
            }
            if let Err(error) = validate_bundle_identity(destination) {
                let _ = cleanup_owned_transaction(&transaction);
                return Err(error);
            }
            true
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            let _ = cleanup_owned_transaction(&transaction);
            return Err(format!(
                "inspect installed macOS runtime bundle {}: {error}",
                destination.display()
            ));
        }
    };

    if had_existing {
        fs::rename(destination, backup_bundle).map_err(|error| {
            let _ = cleanup_owned_transaction(&transaction);
            format!(
                "move existing macOS runtime bundle to backup {}: {error}",
                backup_bundle.display()
            )
        })?;
    }

    if let Err(error) = fs::rename(staged_bundle, destination) {
        let rollback = if had_existing {
            fs::rename(backup_bundle, destination).err()
        } else {
            None
        };
        if rollback.is_none() {
            let _ = cleanup_owned_transaction(&transaction);
        }
        return Err(match rollback {
            Some(rollback) => format!(
                "install macOS runtime bundle {}: {error}; rollback also failed: {rollback}",
                destination.display()
            ),
            None => format!(
                "install macOS runtime bundle {}: {error}",
                destination.display()
            ),
        });
    }

    if let Err(error) = verify_bundle(destination) {
        let _ = remove_tree_no_follow(destination);
        let rollback = if had_existing {
            fs::rename(backup_bundle, destination).err()
        } else {
            None
        };
        if rollback.is_none() {
            let _ = cleanup_owned_transaction(&transaction);
        }
        return Err(match rollback {
            Some(rollback) => format!(
                "installed macOS runtime bundle failed verification: {error}; rollback also failed: {rollback}"
            ),
            None => format!("installed macOS runtime bundle failed verification: {error}"),
        });
    }

    Ok(BundleSwap {
        destination: destination.to_path_buf(),
        backup: had_existing.then(|| backup_bundle.clone()),
        transaction_marker: Some(transaction.marker.clone()),
        committed: true,
    })
}

pub fn verify_bundle(bundle: &Path) -> Result<(), String> {
    validate_bundle_identity(bundle)?;
    let status = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("run /usr/bin/codesign for {}: {error}", bundle.display()))?;
    if !status.status.success() {
        return Err(format!(
            "codesign verification failed for {}: {}",
            bundle.display(),
            stderr_text(&status.stderr)
        ));
    }
    Ok(())
}

pub fn validate_bundle_identity(bundle: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(bundle)
        .map_err(|error| format!("inspect macOS runtime bundle {}: {error}", bundle.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "macOS runtime bundle is not a real directory: {}",
            bundle.display()
        ));
    }

    let info = bundle.join("Contents/Info.plist");
    let executable = runtime_executable(bundle);
    require_regular_file(&info, "Info.plist")?;
    require_regular_file(&executable, "runtime executable")?;

    let identifier = plist_value(&info, "CFBundleIdentifier")?;
    if identifier != BUNDLE_IDENTIFIER {
        return Err(format!(
            "unexpected macOS runtime bundle identifier {identifier:?}; expected {BUNDLE_IDENTIFIER:?}"
        ));
    }
    let bundle_executable = plist_value(&info, "CFBundleExecutable")?;
    if bundle_executable != "origin-runtime" {
        return Err(format!(
            "unexpected macOS runtime bundle executable {bundle_executable:?}; expected \"origin-runtime\""
        ));
    }
    let package_type = plist_value(&info, "CFBundlePackageType")?;
    if package_type != "APPL" {
        return Err(format!(
            "unexpected macOS runtime bundle package type {package_type:?}; expected \"APPL\""
        ));
    }
    Ok(())
}

pub fn verify_runtime_executable(runtime: &Path) -> Result<(), String> {
    let bundle = bundle_for_runtime(runtime)?;
    verify_bundle(&bundle)
}

pub fn bundle_for_runtime(runtime: &Path) -> Result<PathBuf, String> {
    let macos = runtime
        .parent()
        .ok_or_else(|| "macOS runtime executable has no MacOS directory".to_string())?;
    let contents = macos
        .parent()
        .ok_or_else(|| "macOS runtime executable has no Contents directory".to_string())?;
    let bundle = contents
        .parent()
        .ok_or_else(|| "macOS runtime executable has no app bundle directory".to_string())?;
    if runtime_executable(bundle) != runtime
        || bundle.file_name().and_then(|name| name.to_str()) != Some(BUNDLE_NAME)
    {
        return Err(format!(
            "runtime executable is not inside the expected {BUNDLE_NAME} bundle: {}",
            runtime.display()
        ));
    }
    Ok(bundle.to_path_buf())
}

pub fn bundle_version(bundle: &Path) -> Result<String, String> {
    validate_bundle_identity(bundle)?;
    plist_value(
        &bundle.join("Contents/Info.plist"),
        "CFBundleShortVersionString",
    )
}

pub fn remove_owned_bundle(bundle: &Path) -> Result<bool, String> {
    validate_destination_shape(bundle)?;
    match fs::symlink_metadata(bundle) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!(
                    "refusing to remove non-directory macOS runtime bundle path {}",
                    bundle.display()
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "inspect macOS runtime bundle {}: {error}",
                bundle.display()
            ));
        }
    }
    validate_bundle_identity(bundle)?;
    remove_tree_no_follow(bundle)
        .map_err(|error| format!("remove macOS runtime bundle {}: {error}", bundle.display()))?;
    Ok(true)
}

fn validate_destination_shape(destination: &Path) -> Result<(), String> {
    if destination.file_name().and_then(|name| name.to_str()) != Some(BUNDLE_NAME) {
        return Err(format!(
            "refusing unexpected macOS runtime bundle path {}",
            destination.display()
        ));
    }
    if destination.parent().is_none() {
        return Err("macOS runtime bundle path has no parent directory".to_string());
    }
    Ok(())
}

pub fn recover_stale_transactions(destination: &Path) -> Result<(), String> {
    recover_stale_transactions_with(destination, verify_bundle)
}

fn recover_stale_transactions_with<F>(destination: &Path, verify: F) -> Result<(), String>
where
    F: Fn(&Path) -> Result<(), String>,
{
    validate_destination_shape(destination)?;
    let parent = destination
        .parent()
        .ok_or_else(|| "macOS runtime bundle has no parent directory".to_string())?;
    let transactions = discover_owned_transactions(parent)?;
    let owned_extractions = discover_owned_extractions(parent)?;
    if transactions.is_empty() && owned_extractions.is_empty() {
        return Ok(());
    }

    let live_exists = match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!(
                    "refusing macOS bundle recovery with non-directory live path {}",
                    destination.display()
                ));
            }
            true
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(format!(
                "inspect live macOS runtime bundle {} during recovery: {error}",
                destination.display()
            ));
        }
    };

    if live_exists {
        verify(destination).map_err(|error| {
            format!(
                "live macOS runtime bundle failed verification during stale transaction cleanup; preserving recovery artifacts: {error}"
            )
        })?;
        for transaction in &transactions {
            cleanup_owned_transaction(transaction)?;
        }
        cleanup_owned_extractions(&owned_extractions)?;
        return Ok(());
    }

    let mut verified_backups = Vec::new();
    let mut rejected_backups = Vec::new();
    for transaction in &transactions {
        let metadata = match fs::symlink_metadata(&transaction.backup_bundle) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "inspect owned macOS runtime backup {}: {error}",
                    transaction.backup_bundle.display()
                ));
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            rejected_backups.push(format!(
                "{} is not a real directory",
                transaction.backup_bundle.display()
            ));
            continue;
        }
        match verify(&transaction.backup_bundle) {
            Ok(()) => verified_backups.push(transaction.backup_bundle.clone()),
            Err(error) => rejected_backups.push(format!(
                "{} failed verification: {error}",
                transaction.backup_bundle.display()
            )),
        }
    }

    if verified_backups.len() > 1 {
        return Err(format!(
            "found multiple verified owned macOS runtime backups while {} is missing; preserved all recovery artifacts for manual inspection",
            destination.display()
        ));
    }

    if let Some(backup) = verified_backups.pop() {
        fs::rename(&backup, destination).map_err(|error| {
            format!(
                "restore verified macOS runtime backup {} to {}: {error}",
                backup.display(),
                destination.display()
            )
        })?;
        if let Err(error) = verify(destination) {
            let rollback = fs::rename(destination, &backup);
            return match rollback {
                Ok(()) => Err(format!(
                    "restored macOS runtime backup failed verification and was returned to its backup path: {error}"
                )),
                Err(rollback_error) => Err(format!(
                    "restored macOS runtime backup failed verification ({error}); returning it to {} also failed ({rollback_error})",
                    backup.display()
                )),
            };
        }
        for transaction in &transactions {
            cleanup_owned_transaction(transaction)?;
        }
        cleanup_owned_extractions(&owned_extractions)?;
        return Ok(());
    }

    if !rejected_backups.is_empty() {
        return Err(format!(
            "owned macOS runtime backup artifacts exist while {} is missing, but none passed verification; preserved them for inspection: {}",
            destination.display(),
            rejected_backups.join("; ")
        ));
    }

    for transaction in &transactions {
        cleanup_owned_transaction(transaction)?;
    }
    cleanup_owned_extractions(&owned_extractions)
}

fn discover_owned_transactions(parent: &Path) -> Result<Vec<TransactionPaths>, String> {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "inspect macOS runtime bundle parent {} for stale transactions: {error}",
                parent.display()
            ));
        }
    };
    let mut transactions = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "inspect macOS runtime bundle parent entry in {}: {error}",
                parent.display()
            )
        })?;
        let path = entry.path();
        if let Some(transaction) = transaction_from_marker(&path)? {
            transactions.push(transaction);
        }
    }
    Ok(transactions)
}

fn transaction_from_marker(marker: &Path) -> Result<Option<TransactionPaths>, String> {
    let Some(file_name) = marker.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    let Some(token) = file_name
        .strip_prefix(TRANSACTION_MARKER_PREFIX)
        .and_then(|name| name.strip_suffix(TRANSACTION_MARKER_SUFFIX))
    else {
        return Ok(None);
    };
    if !valid_transaction_token(token) {
        return Ok(None);
    }
    let metadata = match fs::symlink_metadata(marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "inspect macOS bundle transaction marker {}: {error}",
                marker.display()
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(None);
    }
    let parent = match marker.parent() {
        Some(parent) => parent,
        None => return Ok(None),
    };
    let transaction = TransactionPaths::new(parent, token);
    if transaction.marker != marker {
        return Ok(None);
    }
    let contents = fs::read_to_string(marker).map_err(|error| {
        format!(
            "read macOS bundle transaction marker {}: {error}",
            marker.display()
        )
    })?;
    if contents != transaction_marker_contents(token) {
        return Ok(None);
    }
    Ok(Some(transaction))
}

fn valid_transaction_token(token: &str) -> bool {
    let mut parts = token.split('-');
    let valid_part =
        |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(first), Some(second), Some(third), None)
            if valid_part(first) && valid_part(second) && valid_part(third)
    )
}

fn transaction_marker_contents(token: &str) -> String {
    format!(
        "{TRANSACTION_MARKER_SCHEMA}\ntoken={token}\ndestination={BUNDLE_NAME}\nextraction=.origin-speak-extracting-{token}\nstaged=.origin-speak-installing-{token}.app\nbackup=.origin-speak-backup-{token}.app\n"
    )
}

fn cleanup_owned_transaction(transaction: &TransactionPaths) -> Result<(), String> {
    let validated = transaction_from_marker(&transaction.marker)?.ok_or_else(|| {
        format!(
            "refusing to clean unrecognized macOS bundle transaction {}",
            transaction.marker.display()
        )
    })?;
    if validated.extraction_root != transaction.extraction_root
        || validated.staged_bundle != transaction.staged_bundle
        || validated.backup_bundle != transaction.backup_bundle
    {
        return Err(
            "macOS bundle transaction marker no longer matches its artifact paths".to_string(),
        );
    }

    if fs::symlink_metadata(&transaction.extraction_root).is_ok()
        && validate_staging_marker(&transaction.extraction_root).is_ok()
    {
        cleanup_owned_extraction(&transaction.extraction_root)?;
    }
    cleanup_ephemeral(&transaction.staged_bundle)?;
    cleanup_ephemeral(&transaction.backup_bundle)?;
    fs::remove_file(&transaction.marker).map_err(|error| {
        format!(
            "remove stale macOS bundle transaction marker {}: {error}",
            transaction.marker.display()
        )
    })
}

fn discover_owned_extractions(parent: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "inspect macOS runtime parent {} for stale extraction paths: {error}",
                parent.display()
            ));
        }
    };
    let mut owned = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "inspect macOS runtime parent entry in {}: {error}",
                parent.display()
            )
        })?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(token) = name.strip_prefix(".origin-speak-extracting-") else {
            continue;
        };
        if !valid_transaction_token(token) || validate_staging_marker(&path).is_err() {
            continue;
        }
        owned.push(path);
    }
    Ok(owned)
}

fn cleanup_owned_extractions(paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        if fs::symlink_metadata(path).is_ok() && validate_staging_marker(path).is_ok() {
            cleanup_owned_extraction(path)?;
        }
    }
    Ok(())
}

fn exact_extracted_bundle(extraction_root: &Path) -> Result<PathBuf, String> {
    validate_staging_marker(extraction_root)?;
    let mut entries = fs::read_dir(extraction_root)
        .map_err(|error| format!("inspect extracted macOS runtime archive: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("inspect extracted macOS runtime archive entry: {error}"))?
        .into_iter()
        .filter(|entry| entry.file_name().to_str() != Some(STAGING_MARKER))
        .collect::<Vec<_>>();
    if entries.len() != 1 {
        return Err(format!(
            "macOS runtime archive must contain exactly one top-level {BUNDLE_NAME} bundle"
        ));
    }
    let entry = entries.pop().expect("single extracted entry");
    if entry.file_name().to_str() != Some(BUNDLE_NAME) {
        return Err(format!(
            "macOS runtime archive top-level entry must be {BUNDLE_NAME}"
        ));
    }
    Ok(entry.path())
}

fn allocate_staging_paths(parent: &Path) -> Result<TransactionPaths, String> {
    for _ in 0..64 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let token = format!("{}-{nanos}-{sequence}", std::process::id());
        let transaction = TransactionPaths::new(parent, &token);
        ensure_paths_absent([
            transaction.marker.as_path(),
            transaction.extraction_root.as_path(),
            transaction.staged_bundle.as_path(),
            transaction.backup_bundle.as_path(),
        ])?;
        let marker_result = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&transaction.marker)
            .and_then(|mut file| {
                file.write_all(transaction_marker_contents(&token).as_bytes())?;
                file.sync_all()
            });
        if let Err(error) = marker_result {
            if error.kind() == io::ErrorKind::AlreadyExists {
                continue;
            }
            return Err(format!(
                "claim macOS bundle transaction marker {}: {error}",
                transaction.marker.display()
            ));
        }
        match fs::create_dir(&transaction.extraction_root) {
            Ok(()) => {
                let marker = transaction.extraction_root.join(STAGING_MARKER);
                let marker_result = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&marker)
                    .and_then(|mut file| {
                        file.write_all(b"Origin Speak macOS bootstrap staging v1\n")?;
                        file.sync_all()
                    });
                if let Err(error) = marker_result {
                    let _ = fs::remove_file(&marker);
                    let _ = fs::remove_dir(&transaction.extraction_root);
                    let _ = fs::remove_file(&transaction.marker);
                    return Err(format!(
                        "claim macOS runtime staging directory {}: {error}",
                        transaction.extraction_root.display()
                    ));
                }
                return Ok(transaction);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(&transaction.marker);
                continue;
            }
            Err(error) => {
                let _ = fs::remove_file(&transaction.marker);
                return Err(format!(
                    "create exclusive macOS runtime staging directory {}: {error}",
                    transaction.extraction_root.display()
                ));
            }
        }
    }
    Err("could not allocate an exclusive macOS runtime staging directory".to_string())
}

fn ensure_paths_absent<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Result<(), String> {
    for path in paths {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(format!(
                    "refusing pre-existing macOS bootstrap staging path {}",
                    path.display()
                ));
            }
            Err(error) => {
                return Err(format!(
                    "inspect macOS bootstrap staging path {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn validate_staging_marker(extraction_root: &Path) -> Result<(), String> {
    let marker = extraction_root.join(STAGING_MARKER);
    let metadata = fs::symlink_metadata(&marker)
        .map_err(|error| format!("inspect macOS staging ownership marker: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "macOS staging ownership marker is not a regular file: {}",
            marker.display()
        ));
    }
    let contents = fs::read_to_string(&marker)
        .map_err(|error| format!("read macOS staging ownership marker: {error}"))?;
    if contents != "Origin Speak macOS bootstrap staging v1\n" {
        return Err(format!(
            "macOS staging ownership marker is invalid: {}",
            marker.display()
        ));
    }
    Ok(())
}

fn cleanup_owned_extraction(path: &Path) -> Result<(), String> {
    validate_staging_marker(path)?;
    remove_tree_no_follow(path).map_err(|error| {
        format!(
            "remove owned macOS extraction path {}: {error}",
            path.display()
        )
    })
}

fn plist_value(info: &Path, key: &str) -> Result<String, String> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(info)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("run /usr/bin/plutil for {key}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "read {key} from {}: {}",
            info.display(),
            stderr_text(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|error| format!("decode {key} from {}: {error}", info.display()))
}

fn require_regular_file(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspect macOS bundle {label} {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "macOS bundle {label} is not a regular file: {}",
            path.display()
        ));
    }
    #[cfg(unix)]
    if label == "runtime executable" {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!(
                "macOS bundle runtime executable is not executable: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn cleanup_ephemeral(path: &Path) -> Result<(), String> {
    remove_tree_no_follow(path).map_err(|error| {
        format!(
            "remove temporary macOS bundle path {}: {error}",
            path.display()
        )
    })
}

fn remove_tree_no_follow(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() {
        return fs::remove_file(path).or_else(|_| fs::remove_dir(path));
    }
    if metadata.is_file() {
        return fs::remove_file(path);
    }
    if !metadata.is_dir() {
        return fs::remove_file(path);
    }
    for entry in fs::read_dir(path)? {
        remove_tree_no_follow(&entry?.path())?;
    }
    fs::remove_dir(path)
}

fn stderr_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).trim().to_string();
    if text.is_empty() {
        "command failed without diagnostics".to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_executable_is_inside_bundle() {
        assert_eq!(
            runtime_executable(Path::new("/Users/example/Applications/Origin Speak.app")),
            Path::new("/Users/example/Applications/Origin Speak.app/Contents/MacOS/origin-runtime")
        );
    }

    #[test]
    fn bundle_is_recovered_from_runtime_path() {
        let runtime =
            Path::new("/Users/example/Applications/Origin Speak.app/Contents/MacOS/origin-runtime");
        assert_eq!(
            bundle_for_runtime(runtime).unwrap(),
            Path::new("/Users/example/Applications/Origin Speak.app")
        );
        assert!(bundle_for_runtime(Path::new("/tmp/origin-runtime")).is_err());
    }

    #[test]
    fn destination_must_be_exact_origin_speak_bundle_name() {
        assert!(
            validate_destination_shape(Path::new("/Users/example/Applications/Origin Speak.app"))
                .is_ok()
        );
        assert!(
            validate_destination_shape(Path::new("/Users/example/Applications/Other.app")).is_err()
        );
    }

    #[test]
    fn bundle_swap_rollback_restores_previous_directory() {
        let root = std::env::temp_dir().join(format!(
            "origin-speak-macos-bundle-rollback-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let live = root.join(BUNDLE_NAME);
        let backup = root.join(".origin-speak-backup-test.app");
        fs::create_dir(&live).unwrap();
        fs::write(live.join("version"), b"new").unwrap();
        fs::create_dir(&backup).unwrap();
        fs::write(backup.join("version"), b"old").unwrap();
        let mut swap = BundleSwap {
            destination: live.clone(),
            backup: Some(backup),
            transaction_marker: None,
            committed: true,
        };

        swap.rollback().unwrap();
        assert_eq!(fs::read(live.join("version")).unwrap(), b"old");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bundle_swap_finalize_removes_backup_directory() {
        let root = std::env::temp_dir().join(format!(
            "origin-speak-macos-bundle-finalize-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let live = root.join(BUNDLE_NAME);
        let backup = root.join(".origin-speak-backup-test.app");
        fs::create_dir(&live).unwrap();
        fs::create_dir(&backup).unwrap();
        fs::write(backup.join("version"), b"old").unwrap();
        let mut swap = BundleSwap {
            destination: live,
            backup: Some(backup.clone()),
            transaction_marker: None,
            committed: true,
        };

        swap.finalize().unwrap();
        assert!(!backup.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn staging_collision_is_refused_without_deleting_existing_content() {
        let root = std::env::temp_dir().join(format!(
            "origin-speak-macos-bundle-collision-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let collision = root.join(".origin-speak-installing-collision.app");
        fs::create_dir(&collision).unwrap();
        fs::write(collision.join("keep.txt"), b"unrelated").unwrap();

        assert!(ensure_paths_absent([collision.as_path()]).is_err());
        assert_eq!(fs::read(collision.join("keep.txt")).unwrap(), b"unrelated");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_transaction_recovers_exactly_one_verified_owned_backup() {
        let root = test_root("recover-owned-backup");
        let live = root.join(BUNDLE_NAME);
        let transaction = create_test_transaction(&root, "1-2-3");
        fs::create_dir(&transaction.backup_bundle).unwrap();
        fs::write(transaction.backup_bundle.join("verified"), b"old").unwrap();
        fs::create_dir(&transaction.staged_bundle).unwrap();
        fs::write(transaction.staged_bundle.join("stale"), b"new").unwrap();
        create_owned_extraction(&transaction.extraction_root);

        recover_stale_transactions_with(&live, test_bundle_verifier).unwrap();

        assert_eq!(fs::read(live.join("verified")).unwrap(), b"old");
        assert!(!transaction.backup_bundle.exists());
        assert!(!transaction.staged_bundle.exists());
        assert!(!transaction.extraction_root.exists());
        assert!(!transaction.marker.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_transaction_cleanup_keeps_verified_live_bundle() {
        let root = test_root("cleanup-with-live");
        let live = root.join(BUNDLE_NAME);
        fs::create_dir(&live).unwrap();
        fs::write(live.join("verified"), b"live").unwrap();
        let transaction = create_test_transaction(&root, "4-5-6");
        fs::create_dir(&transaction.backup_bundle).unwrap();
        fs::write(transaction.backup_bundle.join("verified"), b"old").unwrap();
        fs::create_dir(&transaction.staged_bundle).unwrap();
        create_owned_extraction(&transaction.extraction_root);

        recover_stale_transactions_with(&live, test_bundle_verifier).unwrap();

        assert_eq!(fs::read(live.join("verified")).unwrap(), b"live");
        assert!(!transaction.backup_bundle.exists());
        assert!(!transaction.staged_bundle.exists());
        assert!(!transaction.extraction_root.exists());
        assert!(!transaction.marker.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_transaction_recovery_preserves_unowned_colliding_artifacts() {
        let root = test_root("preserve-unowned-collisions");
        let live = root.join(BUNDLE_NAME);
        fs::create_dir(&live).unwrap();
        fs::write(live.join("verified"), b"live").unwrap();

        let backup = root.join(".origin-speak-backup-7-8-9.app");
        let staged = root.join(".origin-speak-installing-7-8-9.app");
        let extraction = root.join(".origin-speak-extracting-7-8-9");
        fs::create_dir(&backup).unwrap();
        fs::create_dir(&staged).unwrap();
        fs::create_dir(&extraction).unwrap();
        fs::write(backup.join("keep.txt"), b"backup").unwrap();
        fs::write(staged.join("keep.txt"), b"staged").unwrap();
        fs::write(extraction.join("keep.txt"), b"extraction").unwrap();
        let fake_marker = root.join(".origin-speak-transaction-7-8-9.marker");
        fs::write(&fake_marker, b"not Origin Speak transaction schema\n").unwrap();

        recover_stale_transactions_with(&live, test_bundle_verifier).unwrap();

        assert_eq!(fs::read(backup.join("keep.txt")).unwrap(), b"backup");
        assert_eq!(fs::read(staged.join("keep.txt")).unwrap(), b"staged");
        assert_eq!(
            fs::read(extraction.join("keep.txt")).unwrap(),
            b"extraction"
        );
        assert!(fake_marker.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_transaction_recovery_refuses_ambiguous_verified_backups() {
        let root = test_root("ambiguous-backups");
        let live = root.join(BUNDLE_NAME);
        let first = create_test_transaction(&root, "10-11-12");
        let second = create_test_transaction(&root, "13-14-15");
        for transaction in [&first, &second] {
            fs::create_dir(&transaction.backup_bundle).unwrap();
            fs::write(transaction.backup_bundle.join("verified"), b"old").unwrap();
        }

        let error = recover_stale_transactions_with(&live, test_bundle_verifier).unwrap_err();

        assert!(error.contains("multiple verified owned macOS runtime backups"));
        assert!(!live.exists());
        assert!(first.backup_bundle.exists());
        assert!(second.backup_bundle.exists());
        assert!(first.marker.exists());
        assert!(second.marker.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn archive_must_be_a_regular_file_before_any_staging_is_created() {
        let root = std::env::temp_dir().join(format!(
            "origin-speak-macos-bundle-archive-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive_directory = root.join("runtime.zip");
        fs::create_dir(&archive_directory).unwrap();
        let destination = root.join(BUNDLE_NAME);

        assert!(transactional_install_bundle_zip(&archive_directory, &destination).is_err());
        assert!(archive_directory.is_dir());
        assert!(!destination.exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn test_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "origin-speak-macos-bundle-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn create_test_transaction(root: &Path, token: &str) -> TransactionPaths {
        let transaction = TransactionPaths::new(root, token);
        fs::write(&transaction.marker, transaction_marker_contents(token)).unwrap();
        transaction
    }

    fn create_owned_extraction(path: &Path) {
        fs::create_dir(path).unwrap();
        fs::write(
            path.join(STAGING_MARKER),
            b"Origin Speak macOS bootstrap staging v1\n",
        )
        .unwrap();
    }

    fn test_bundle_verifier(path: &Path) -> Result<(), String> {
        if path.join("verified").is_file() {
            Ok(())
        } else {
            Err(format!("test bundle is not verified: {}", path.display()))
        }
    }
}
