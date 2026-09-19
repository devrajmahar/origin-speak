//! Bootstrap primitives shared by the console manager's setup/update flow.
//!
//! The release architecture uses a console `origin` manager and a separate
//! silent `origin-runtime`. This module avoids shell scripts at runtime: file
//! replacement and process launch use Rust/native process APIs directly.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

const COPY_BUFFER: usize = 1024 * 1024;
static SWAP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub struct TransactionalSwap {
    destination: PathBuf,
    backup: Option<PathBuf>,
    committed: bool,
}

impl TransactionalSwap {
    pub fn rollback(&mut self) -> Result<(), String> {
        if !self.committed {
            return Ok(());
        }
        if self.destination.exists() {
            fs::remove_file(&self.destination).map_err(|error| {
                format!(
                    "remove replacement {} during rollback: {error}",
                    self.destination.display()
                )
            })?;
        }
        if let Some(backup) = self.backup.as_ref() {
            fs::rename(backup, &self.destination).map_err(|error| {
                format!(
                    "restore backup {} to {}: {error}",
                    backup.display(),
                    self.destination.display()
                )
            })?;
        }
        self.committed = false;
        Ok(())
    }

    pub fn finalize(&mut self) -> Result<(), String> {
        if let Some(backup) = self.backup.take() {
            match fs::remove_file(&backup) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    self.backup = Some(backup.clone());
                    return Err(format!(
                        "remove update backup {}: {error}",
                        backup.display()
                    ));
                }
            }
        }
        self.committed = false;
        Ok(())
    }

    #[cfg(test)]
    fn backup_path(&self) -> Option<&Path> {
        self.backup.as_deref()
    }
}

pub fn runtime_path_for_manager(manager: &Path) -> Result<PathBuf, String> {
    let parent = manager
        .parent()
        .ok_or_else(|| "manager executable has no parent directory".to_string())?;
    #[cfg(target_os = "windows")]
    let name = "origin-runtime.exe";
    #[cfg(not(target_os = "windows"))]
    let name = "origin-runtime";
    Ok(parent.join(name))
}

pub fn find_sibling_runtime(manager: &Path, version: &str) -> Result<Option<PathBuf>, String> {
    let parent = manager
        .parent()
        .ok_or_else(|| "manager executable has no parent directory".to_string())?;
    let mut candidates = Vec::new();
    #[cfg(not(target_os = "macos"))]
    candidates.push(runtime_path_for_manager(manager)?);
    #[cfg(target_os = "windows")]
    candidates.push(parent.join(format!("origin-speak-runtime-{version}-windows-x86_64.exe")));
    #[cfg(target_os = "macos")]
    {
        candidates.push(parent.join(format!(
            "origin-speak-runtime-{version}-macos-universal.zip"
        )));
        candidates.push(parent.join(format!(
            "origin-speak-runtime-{version}-macos-{}.zip",
            std::env::consts::ARCH
        )));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    candidates.push(parent.join(format!(
        "origin-speak-runtime-{version}-linux-{}",
        std::env::consts::ARCH
    )));

    Ok(candidates.into_iter().find(|path| path.is_file()))
}

#[cfg(target_os = "macos")]
pub fn transactional_install_macos_runtime_bundle(
    archive: &Path,
    bundle_destination: &Path,
) -> Result<crate::macos_bundle::BundleSwap, String> {
    crate::macos_bundle::transactional_install_bundle_zip(archive, bundle_destination)
}

/// Replace one installed payload while keeping the previous live file as a
/// sibling backup. Callers may roll back after a later multi-file or health
/// check failure, or finalize only after the new installation is known-good.
pub fn transactional_replace(
    source: &Path,
    destination: &Path,
) -> Result<TransactionalSwap, String> {
    validate_regular_source(source)?;
    validate_regular_destination_or_missing(destination)?;
    let parent = destination
        .parent()
        .ok_or_else(|| "runtime destination has no parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    validate_destination_parent(parent)?;
    #[cfg(target_os = "windows")]
    crate::management::validate_known_windows_owned_root(parent)?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "runtime destination filename is not valid UTF-8".to_string())?;
    let sequence = SWAP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let token = format!("{}-{sequence}", std::process::id());
    let staged = parent.join(format!(".{file_name}.update-{token}.new"));
    let backup = parent.join(format!(".{file_name}.update-{token}.bak"));
    copy_and_flush(source, &staged)?;
    if !same_file_contents(source, &staged).unwrap_or(false) {
        let _ = fs::remove_file(&staged);
        return Err(format!(
            "staged replacement for {} did not match its source payload",
            destination.display()
        ));
    }

    let had_original = match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if metadata.is_dir() || metadata.file_type().is_symlink() {
                let _ = fs::remove_file(&staged);
                return Err(format!(
                    "refusing to replace non-regular installed payload: {}",
                    destination.display()
                ));
            }
            fs::rename(destination, &backup).map_err(|error| {
                let _ = fs::remove_file(&staged);
                format!(
                    "move existing {} to update backup: {error}",
                    destination.display()
                )
            })?;
            true
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            let _ = fs::remove_file(&staged);
            return Err(format!("inspect {}: {error}", destination.display()));
        }
    };

    if let Err(error) = fs::rename(&staged, destination) {
        if had_original {
            let restore = fs::rename(&backup, destination);
            let _ = fs::remove_file(&staged);
            return match restore {
                Ok(()) => Err(format!(
                    "install {} failed; previous payload restored: {error}",
                    destination.display()
                )),
                Err(restore_error) => Err(format!(
                    "install {} failed ({error}) and restoring its backup also failed ({restore_error})",
                    destination.display()
                )),
            };
        }
        let _ = fs::remove_file(&staged);
        return Err(format!("install {}: {error}", destination.display()));
    }

    Ok(TransactionalSwap {
        destination: destination.to_path_buf(),
        backup: had_original.then_some(backup),
        committed: true,
    })
}

pub fn spawn_runtime_silent(runtime: &Path, args: &[String]) -> Result<Child, String> {
    let mut command = Command::new(runtime);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // Prevent a console window even when a development/runtime binary was
        // accidentally built with the console subsystem.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .map_err(|error| format!("start {}: {error}", runtime.display()))
}

fn copy_and_flush(source: &Path, destination: &Path) -> Result<(), String> {
    validate_regular_source(source)?;
    let mut input =
        fs::File::open(source).map_err(|error| format!("open {}: {error}", source.display()))?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut output = options.open(destination).map_err(|error| {
        format!(
            "create exclusive staged file {}: {error}",
            destination.display()
        )
    })?;
    io::copy(&mut input, &mut output).map_err(|error| format!("copy runtime payload: {error}"))?;
    output
        .flush()
        .map_err(|error| format!("flush runtime payload: {error}"))?;
    output
        .sync_all()
        .map_err(|error| format!("sync runtime payload: {error}"))?;
    let permissions = fs::metadata(source)
        .map_err(|error| format!("read runtime payload permissions: {error}"))?
        .permissions();
    fs::set_permissions(destination, permissions)
        .map_err(|error| format!("preserve runtime payload permissions: {error}"))?;
    Ok(())
}

fn validate_regular_source(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspect source payload {}: {error}", path.display()))?;
    if !metadata.is_file() || is_link_or_reparse_point(&metadata) {
        return Err(format!(
            "refusing source payload that is not a regular file: {}",
            path.display()
        ));
    }
    Ok(())
}

fn validate_regular_destination_or_missing(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !is_link_or_reparse_point(&metadata) => Ok(()),
        Ok(_) => Err(format!(
            "refusing installed payload path that is not a regular file: {}",
            path.display()
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "inspect installed payload {}: {error}",
            path.display()
        )),
    }
}

fn validate_destination_parent(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspect destination parent {}: {error}", path.display()))?;
    if !metadata.is_dir() || is_link_or_reparse_point(&metadata) {
        return Err(format!(
            "refusing destination parent that is not a real directory or is a reparse point: {}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn is_link_or_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(target_os = "windows"))]
fn is_link_or_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn same_file_contents(left: &Path, right: &Path) -> io::Result<bool> {
    let left_meta = fs::metadata(left)?;
    let right_meta = match fs::metadata(right) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if left_meta.len() != right_meta.len() {
        return Ok(false);
    }
    let mut left = fs::File::open(left)?;
    let mut right = fs::File::open(right)?;
    let mut left_buf = vec![0_u8; COPY_BUFFER];
    let mut right_buf = vec![0_u8; COPY_BUFFER];
    loop {
        let left_read = left.read(&mut left_buf)?;
        let right_read = right.read(&mut right_buf)?;
        if left_read != right_read || left_buf[..left_read] != right_buf[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transactional_swap_rollback_restores_original_after_later_failure() {
        let root = std::env::temp_dir().join(format!(
            "origin-speak-bootstrap-rollback-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let runtime_source = root.join("runtime-new.bin");
        let runtime_live = root.join("runtime.bin");
        let manager_source = root.join("manager-new.bin");
        let manager_live = root.join("manager.bin");
        fs::write(&runtime_source, b"new runtime").unwrap();
        fs::write(&runtime_live, b"old runtime").unwrap();
        fs::write(&manager_source, b"new manager").unwrap();
        fs::write(&manager_live, b"old manager").unwrap();

        let mut runtime = transactional_replace(&runtime_source, &runtime_live).unwrap();
        let mut manager = transactional_replace(&manager_source, &manager_live).unwrap();
        assert!(runtime.backup_path().is_some_and(Path::is_file));
        assert!(manager.backup_path().is_some_and(Path::is_file));

        // Simulate a health-gate failure after both files were replaced.
        manager.rollback().unwrap();
        runtime.rollback().unwrap();
        assert_eq!(fs::read(&runtime_live).unwrap(), b"old runtime");
        assert_eq!(fs::read(&manager_live).unwrap(), b"old manager");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn transactional_swap_finalize_removes_backup_only_after_success() {
        let root = std::env::temp_dir().join(format!(
            "origin-speak-bootstrap-finalize-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let source = root.join("new.bin");
        let live = root.join("live.bin");
        fs::write(&source, b"new").unwrap();
        fs::write(&live, b"old").unwrap();

        let mut swap = transactional_replace(&source, &live).unwrap();
        let backup = swap.backup_path().unwrap().to_path_buf();
        assert!(backup.is_file());
        swap.finalize().unwrap();
        assert!(!backup.exists());
        assert_eq!(fs::read(&live).unwrap(), b"new");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn install_refuses_non_regular_source_and_destination() {
        let root = std::env::temp_dir().join(format!(
            "origin-speak-bootstrap-regular-file-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let source_dir = root.join("source-dir");
        let source_file = root.join("source.bin");
        let destination_dir = root.join("destination-dir");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(&source_file, b"payload").unwrap();
        fs::create_dir_all(&destination_dir).unwrap();

        assert!(transactional_replace(&source_dir, &root.join("runtime.bin")).is_err());
        assert!(transactional_replace(&source_file, &destination_dir).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn destination_parent_must_be_real_directory() {
        let root = std::env::temp_dir().join(format!(
            "origin-speak-bootstrap-parent-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let directory = root.join("directory");
        let file = root.join("file");
        fs::create_dir(&directory).unwrap();
        fs::write(&file, b"not a directory").unwrap();
        assert!(validate_destination_parent(&directory).is_ok());
        assert!(validate_destination_parent(&file).is_err());
        fs::remove_dir_all(&root).unwrap();
    }
}
