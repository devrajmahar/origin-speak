//! Native lifecycle control for the console manager.
//!
//! Liveness and readiness are intentionally distinct. The resident publishes a
//! dedicated ready signal only after the voice runtime, global hotkey, and GPUI
//! anchor have initialized successfully.

use std::path::Path;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    Running,
    Stopped,
}

#[cfg(target_os = "windows")]
mod imp {
    use super::RuntimeState;
    use crate::bootstrap::spawn_runtime_silent;
    use std::ffi::{OsStr, c_void};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::process::Child;
    use std::time::{Duration, Instant};

    const SYNCHRONIZE: u32 = 0x0010_0000;
    const EVENT_MODIFY_STATE: u32 = 0x0002;
    const INSTANCE_MUTEX: &str = "Local\\OriginSpeak.Native.Instance.v1";
    const QUIT_EVENT: &str = "Local\\OriginSpeak.Native.Quit.v1";
    const READY_EVENT: &str = "Local\\OriginSpeak.Native.Ready.v1";
    // Legacy ListenOS named objects are probed only during migration so an
    // already-running resident can be stopped before Origin Speak starts.
    const LEGACY_INSTANCE_MUTEX: &str = "Local\\ListenOS.Native.Instance.v1";
    const LEGACY_QUIT_EVENT: &str = "Local\\ListenOS.Native.Quit.v1";
    const START_TIMEOUT: Duration = Duration::from_secs(6);
    const WAIT_OBJECT_0: u32 = 0;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenMutexW(desired_access: u32, inherit_handle: i32, name: *const u16) -> *mut c_void;
        fn OpenEventW(desired_access: u32, inherit_handle: i32, name: *const u16) -> *mut c_void;
        fn SetEvent(event: *mut c_void) -> i32;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    fn mutex_is_running(name: &str) -> bool {
        let name = wide(name);
        let handle = unsafe { OpenMutexW(SYNCHRONIZE, 0, name.as_ptr()) };
        if handle.is_null() {
            false
        } else {
            unsafe { CloseHandle(handle) };
            true
        }
    }

    fn canonical_running() -> bool {
        mutex_is_running(INSTANCE_MUTEX)
    }

    fn legacy_running() -> bool {
        mutex_is_running(LEGACY_INSTANCE_MUTEX)
    }

    pub fn status() -> RuntimeState {
        if canonical_running() || legacy_running() {
            RuntimeState::Running
        } else {
            RuntimeState::Stopped
        }
    }

    pub fn start(runtime: &Path) -> Result<bool, String> {
        if canonical_running() {
            wait_until_ready(None, START_TIMEOUT)?;
            return Ok(false);
        }
        if legacy_running() {
            stop(START_TIMEOUT)?;
        }
        let mut child = spawn_runtime_silent(runtime, &[])?;
        wait_until_ready(Some(&mut child), START_TIMEOUT)?;
        Ok(true)
    }

    pub fn ready() -> Result<bool, String> {
        let name = wide(READY_EVENT);
        let handle = unsafe { OpenEventW(SYNCHRONIZE, 0, name.as_ptr()) };
        if handle.is_null() {
            return Ok(false);
        }
        let status = unsafe { WaitForSingleObject(handle, 0) };
        unsafe { CloseHandle(handle) };
        Ok(status == WAIT_OBJECT_0)
    }

    pub fn stop(timeout: Duration) -> Result<bool, String> {
        if status() == RuntimeState::Stopped {
            return Ok(false);
        }
        for event_name in [QUIT_EVENT, LEGACY_QUIT_EVENT] {
            let name = wide(event_name);
            let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
            if event.is_null() {
                continue;
            }
            let signaled = unsafe { SetEvent(event) };
            unsafe { CloseHandle(event) };
            if signaled == 0 {
                return Err(format!(
                    "failed to signal runtime shutdown: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }

        let started = Instant::now();
        while started.elapsed() < timeout {
            if status() == RuntimeState::Stopped {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(format!(
            "runtime did not exit within {} ms",
            timeout.as_millis()
        ))
    }

    fn wait_until_ready(mut child: Option<&mut Child>, timeout: Duration) -> Result<(), String> {
        let started = Instant::now();
        loop {
            if ready()? {
                return Ok(());
            }
            if let Some(child) = child.as_deref_mut()
                && let Some(status) = child
                    .try_wait()
                    .map_err(|error| format!("inspect Origin Speak runtime process: {error}"))?
            {
                return Err(format!(
                    "runtime exited before publishing readiness ({status})"
                ));
            }
            if started.elapsed() >= timeout {
                return Err(format!(
                    "runtime did not become ready within {} ms",
                    timeout.as_millis()
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
}

#[cfg(unix)]
mod imp {
    use super::RuntimeState;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const CONTROL_TIMEOUT: Duration = Duration::from_millis(750);
    const START_TIMEOUT: Duration = Duration::from_secs(6);

    pub fn status() -> Result<RuntimeState, String> {
        Ok(if request_any(b"PING\n", b"PONG\n")? {
            RuntimeState::Running
        } else {
            RuntimeState::Stopped
        })
    }

    pub fn start(runtime: &Path) -> Result<bool, String> {
        if request_current(b"PING\n", b"PONG\n")? {
            wait_until_ready(None, START_TIMEOUT)?;
            return Ok(false);
        }
        if request_legacy(b"PING\n", b"PONG\n")? {
            stop(START_TIMEOUT)?;
        }
        #[cfg(target_os = "macos")]
        crate::macos_bundle::verify_runtime_executable(runtime)?;
        let mut child = Command::new(runtime)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("start {}: {error}", runtime.display()))?;
        wait_until_ready(Some(&mut child), START_TIMEOUT)?;
        Ok(true)
    }

    pub fn ready() -> Result<bool, String> {
        let path = origin_speak_lib::app_local_data_root()?.join("runtime.sock");
        let Some(response) = request_response_at(&path, b"READY\n", 6)? else {
            return Ok(false);
        };
        match response.as_slice() {
            b"READY\n" => Ok(true),
            b"START\n" => Ok(false),
            _ => Err(format!(
                "runtime control socket {} returned an unexpected readiness response",
                path.display()
            )),
        }
    }

    pub fn stop(timeout: Duration) -> Result<bool, String> {
        if status()? == RuntimeState::Stopped {
            return Ok(false);
        }
        let current = request_current(b"QUIT\n", b"OK\n")?;
        let legacy = request_legacy(b"QUIT\n", b"OK\n")?;
        if !current && !legacy {
            return Err(
                "runtime control socket disappeared before quit was acknowledged".to_string(),
            );
        }
        let started = std::time::Instant::now();
        while started.elapsed() < timeout {
            if status()? == RuntimeState::Stopped {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(format!(
            "runtime did not exit within {} ms",
            timeout.as_millis()
        ))
    }

    fn request_any(request: &[u8], expected: &[u8]) -> Result<bool, String> {
        Ok(request_current(request, expected)? || request_legacy(request, expected)?)
    }

    fn request_current(request: &[u8], expected: &[u8]) -> Result<bool, String> {
        let path = origin_speak_lib::app_local_data_root()?.join("runtime.sock");
        request_at(&path, request, expected)
    }

    fn request_legacy(request: &[u8], expected: &[u8]) -> Result<bool, String> {
        let path = origin_speak_lib::legacy_app_local_data_root()?.join("runtime.sock");
        request_at(&path, request, expected)
    }

    fn request_at(path: &Path, request: &[u8], expected: &[u8]) -> Result<bool, String> {
        let Some(response) = request_response_at(&path, request, expected.len())? else {
            return Ok(false);
        };
        Ok(response == expected)
    }

    fn request_response_at(
        path: &Path,
        request: &[u8],
        response_len: usize,
    ) -> Result<Option<Vec<u8>>, String> {
        let mut stream = match UnixStream::connect(path) {
            Ok(stream) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(None);
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
        let mut response = vec![0_u8; response_len];
        stream
            .read_exact(&mut response)
            .map_err(|error| format!("read runtime control response: {error}"))?;
        Ok(Some(response))
    }

    fn wait_until_ready(
        mut child: Option<&mut std::process::Child>,
        timeout: Duration,
    ) -> Result<(), String> {
        let started = Instant::now();
        loop {
            if ready()? {
                return Ok(());
            }
            if let Some(child) = child.as_deref_mut()
                && let Some(status) = child
                    .try_wait()
                    .map_err(|error| format!("inspect Origin Speak runtime process: {error}"))?
            {
                return Err(format!(
                    "runtime exited before publishing readiness ({status})"
                ));
            }
            if started.elapsed() >= timeout {
                return Err(format!(
                    "runtime did not become ready within {} ms",
                    timeout.as_millis()
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[cfg(not(any(target_os = "windows", unix)))]
mod imp {
    use super::RuntimeState;
    use std::path::Path;
    use std::time::Duration;

    pub fn status() -> Result<RuntimeState, String> {
        Err("runtime lifecycle control is unsupported on this platform".to_string())
    }

    pub fn start(_: &Path) -> Result<bool, String> {
        Err("runtime lifecycle control is unsupported on this platform".to_string())
    }

    pub fn ready() -> Result<bool, String> {
        Err("runtime readiness control is unsupported on this platform".to_string())
    }

    pub fn stop(_: Duration) -> Result<bool, String> {
        Err("runtime lifecycle control is unsupported on this platform".to_string())
    }
}

pub fn runtime_status() -> Result<RuntimeState, String> {
    #[cfg(target_os = "windows")]
    {
        Ok(imp::status())
    }
    #[cfg(not(target_os = "windows"))]
    {
        imp::status()
    }
}

pub fn start_runtime(runtime: &Path) -> Result<bool, String> {
    imp::start(runtime)
}

pub fn runtime_ready() -> Result<bool, String> {
    imp::ready()
}

pub fn stop_runtime(timeout: Duration) -> Result<bool, String> {
    imp::stop(timeout)
}

pub fn restart_runtime(runtime: &Path, timeout: Duration) -> Result<(), String> {
    stop_runtime(timeout)?;
    start_runtime(runtime)?;
    Ok(())
}
