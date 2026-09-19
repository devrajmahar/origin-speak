use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(unix)]
use std::sync::{Mutex, OnceLock};
#[cfg(unix)]
use std::time::Duration;

#[cfg(target_os = "linux")]
#[path = "platform_shell_linux.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "platform_shell_macos.rs"]
mod imp;
#[cfg(target_os = "windows")]
#[path = "platform_shell_windows.rs"]
mod imp;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod imp {
    use super::AutoStartState;

    pub fn auto_start_supported() -> bool {
        false
    }

    pub fn auto_start_status() -> Result<AutoStartState, String> {
        Err("auto-start is not supported on this platform".to_string())
    }

    pub fn set_auto_start_enabled(_enabled: bool) -> Result<AutoStartState, String> {
        Err("auto-start is not supported on this platform".to_string())
    }
}

static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
static RUNTIME_READY: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
static INSTANCE_LISTENER: OnceLock<Mutex<Option<UnixListener>>> = OnceLock::new();
#[cfg(unix)]
static INSTANCE_SOCKET_PATH: OnceLock<PathBuf> = OnceLock::new();
#[cfg(unix)]
const CONTROL_TIMEOUT: Duration = Duration::from_millis(500);

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnixControlResponse {
    Unavailable,
    Matched,
}

pub(crate) fn notify_instance_quit() {
    QUIT_REQUESTED.store(true, Ordering::Release);
}

pub(crate) fn take_quit_request() -> bool {
    QUIT_REQUESTED.swap(false, Ordering::AcqRel)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoStartState {
    Disabled,
    Enabled,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    RequiresApproval,
    Mismatched,
}

pub fn auto_start_supported() -> bool {
    imp::auto_start_supported()
}

pub fn auto_start_status() -> Result<AutoStartState, String> {
    imp::auto_start_status()
}

pub fn set_auto_start_enabled(enabled: bool) -> Result<AutoStartState, String> {
    imp::set_auto_start_enabled(enabled)
}

/// Publish resident readiness only after the voice runtime, global shortcut and
/// hidden GPUI anchor have all initialized successfully.
pub fn mark_runtime_ready() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        return imp::mark_runtime_ready();
    }
    #[cfg(unix)]
    {
        RUNTIME_READY.store(true, Ordering::Release);
        return Ok(());
    }
    #[cfg(not(any(target_os = "windows", unix)))]
    {
        Ok(())
    }
}

#[cfg(unix)]
fn runtime_control_socket_path() -> Result<PathBuf, String> {
    Ok(origin_speak_lib::app_local_data_root()?.join("runtime.sock"))
}

pub fn claim_single_instance() -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        imp::claim_single_instance()
    }
    #[cfg(unix)]
    {
        claim_unix_single_instance()
    }
    #[cfg(not(any(target_os = "windows", unix)))]
    {
        Ok(true)
    }
}

pub fn start_instance_listener() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        return imp::start_instance_listener();
    }
    #[cfg(unix)]
    {
        return start_unix_instance_listener();
    }
    #[cfg(not(any(target_os = "windows", unix)))]
    {
        Ok(())
    }
}

pub fn cleanup_instance_control() {
    #[cfg(unix)]
    cleanup_unix_socket();
}

#[cfg(unix)]
fn claim_unix_single_instance() -> Result<bool, String> {
    let path = runtime_control_socket_path()?;
    let parent = path
        .parent()
        .ok_or_else(|| "Origin Speak runtime control path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "create Origin Speak local-data directory {}: {error}",
            parent.display()
        )
    })?;

    if path.exists() {
        match unix_control_request_at(&path, b"PING\n", b"PONG\n")? {
            UnixControlResponse::Matched => return Ok(false),
            UnixControlResponse::Unavailable => remove_stale_unix_socket(&path)?,
        }
    }

    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(bind_error) => {
            // Another runtime may have won the bind race after our stale check.
            if matches!(
                unix_control_request_at(&path, b"PING\n", b"PONG\n"),
                Ok(UnixControlResponse::Matched)
            ) {
                return Ok(false);
            }
            return Err(format!(
                "bind Origin Speak runtime control socket {}: {bind_error}",
                path.display()
            ));
        }
    };
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(|error| {
        let _ = std::fs::remove_file(&path);
        format!(
            "restrict Origin Speak runtime control socket {}: {error}",
            path.display()
        )
    })?;

    INSTANCE_SOCKET_PATH
        .set(path)
        .map_err(|_| "Origin Speak runtime control path was initialized twice".to_string())?;
    INSTANCE_LISTENER
        .set(Mutex::new(Some(listener)))
        .map_err(|_| "Origin Speak runtime control listener was initialized twice".to_string())?;
    if let Err(error) = start_unix_instance_listener() {
        cleanup_unix_socket();
        return Err(error);
    }
    Ok(true)
}

#[cfg(unix)]
fn start_unix_instance_listener() -> Result<(), String> {
    let Some(slot) = INSTANCE_LISTENER.get() else {
        return Ok(());
    };
    let listener = match slot.lock() {
        Ok(mut guard) => guard.take(),
        Err(_) => None,
    };
    let Some(listener) = listener else {
        return Ok(());
    };

    std::thread::Builder::new()
        .name("origin-speak-instance-control".to_string())
        .spawn(move || {
            for incoming in listener.incoming() {
                let Ok(mut stream) = incoming else {
                    if QUIT_REQUESTED.load(Ordering::Acquire) {
                        break;
                    }
                    continue;
                };
                let _ = stream.set_read_timeout(Some(CONTROL_TIMEOUT));
                let _ = stream.set_write_timeout(Some(CONTROL_TIMEOUT));
                let Ok(request) = read_control_request(&mut stream) else {
                    continue;
                };
                match request.as_slice() {
                    b"PING\n" => {
                        let _ = stream.write_all(b"PONG\n");
                    }
                    b"READY\n" => {
                        let response = if RUNTIME_READY.load(Ordering::Acquire) {
                            b"READY\n".as_slice()
                        } else {
                            b"START\n".as_slice()
                        };
                        let _ = stream.write_all(response);
                    }
                    b"QUIT\n" => {
                        let _ = stream.write_all(b"OK\n");
                        let _ = stream.flush();
                        notify_instance_quit();
                        // Keep the control listener alive until the GPUI run loop
                        // has actually exited. Process-control clients use PING
                        // as the authoritative liveness signal; dropping the
                        // listener immediately after acknowledging QUIT creates a
                        // window where restart can bind a replacement socket while
                        // the old process is still unwinding, after which the old
                        // process could remove the replacement's socket during
                        // cleanup. Repeated QUIT requests are harmless because the
                        // quit flag is idempotent.
                    }
                    _ => {
                        let _ = stream.write_all(b"ERR\n");
                    }
                }
            }
        })
        .map(|_| ())
        .map_err(|error| format!("start Origin Speak runtime control listener: {error}"))
}

#[cfg(unix)]
fn read_control_request(stream: &mut UnixStream) -> std::io::Result<Vec<u8>> {
    const MAX_REQUEST_BYTES: usize = 16;
    let mut request = Vec::with_capacity(8);
    let mut byte = [0_u8; 1];
    while request.len() < MAX_REQUEST_BYTES {
        stream.read_exact(&mut byte)?;
        request.push(byte[0]);
        if byte[0] == b'\n' {
            return Ok(request);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "Origin Speak control request exceeded framing limit",
    ))
}

#[cfg(unix)]
fn unix_control_request_at(
    path: &std::path::Path,
    request: &[u8],
    expected: &[u8],
) -> Result<UnixControlResponse, String> {
    let mut stream = match UnixStream::connect(path) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(UnixControlResponse::Unavailable);
        }
        Err(error) => {
            return Err(format!(
                "connect Origin Speak runtime control socket {}: {error}",
                path.display()
            ));
        }
    };
    stream
        .set_read_timeout(Some(CONTROL_TIMEOUT))
        .map_err(|error| format!("configure runtime control read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(CONTROL_TIMEOUT))
        .map_err(|error| format!("configure runtime control write timeout: {error}"))?;
    stream
        .write_all(request)
        .map_err(|error| format!("write runtime control request: {error}"))?;
    let mut response = vec![0_u8; expected.len()];
    stream.read_exact(&mut response).map_err(|error| {
        format!(
            "runtime control socket {} accepted the connection but did not return a complete response: {error}",
            path.display()
        )
    })?;
    if response == expected {
        Ok(UnixControlResponse::Matched)
    } else {
        Err(format!(
            "runtime control socket {} returned an unexpected response",
            path.display()
        ))
    }
}

#[cfg(unix)]
fn remove_stale_unix_socket(path: &std::path::Path) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "inspect Origin Speak runtime control path {}: {error}",
                path.display()
            ));
        }
    };
    if !metadata.file_type().is_socket() {
        return Err(format!(
            "refusing to replace non-socket Origin Speak runtime control path {}",
            path.display()
        ));
    }
    std::fs::remove_file(path).map_err(|error| {
        format!(
            "remove stale Origin Speak runtime control socket {}: {error}",
            path.display()
        )
    })
}

#[cfg(unix)]
fn cleanup_unix_socket() {
    RUNTIME_READY.store(false, Ordering::Release);
    let Some(path) = INSTANCE_SOCKET_PATH.get() else {
        return;
    };
    if std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_socket())
        .unwrap_or(false)
    {
        let _ = std::fs::remove_file(path);
    }
}
