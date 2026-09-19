use crate::platform_shell::AutoStartState;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, GetLastError, HANDLE,
    WAIT_OBJECT_0, WIN32_ERROR,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject,
};
use windows::core::{Error as WindowsError, HRESULT, PCWSTR, w};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Origin Speak";
const INSTANCE_MUTEX_NAME: windows::core::PCWSTR = w!("Local\\OriginSpeak.Native.Instance.v1");
const INSTANCE_QUIT_EVENT_NAME: windows::core::PCWSTR = w!("Local\\OriginSpeak.Native.Quit.v1");
const INSTANCE_READY_EVENT_NAME: windows::core::PCWSTR = w!("Local\\OriginSpeak.Native.Ready.v1");

#[derive(Clone, Copy)]
struct InstanceState {
    _mutex_handle: usize,
    quit_event_handle: usize,
    ready_event_handle: usize,
}

static INSTANCE_STATE: OnceLock<InstanceState> = OnceLock::new();
static INSTANCE_LISTENER_STARTED: AtomicBool = AtomicBool::new(false);

pub fn claim_single_instance() -> Result<bool, String> {
    let quit_event = unsafe { CreateEventW(None, false, false, INSTANCE_QUIT_EVENT_NAME) }
        .map_err(|error| format!("create Origin Speak quit event: {error}"))?;
    let ready_event = match unsafe { CreateEventW(None, true, false, INSTANCE_READY_EVENT_NAME) } {
        Ok(event) => event,
        Err(error) => {
            unsafe {
                let _ = CloseHandle(quit_event);
            }
            return Err(format!("create Origin Speak readiness event: {error}"));
        }
    };
    let mutex = match unsafe { CreateMutexW(None, false, INSTANCE_MUTEX_NAME) } {
        Ok(mutex) => mutex,
        Err(error) => {
            unsafe {
                let _ = CloseHandle(quit_event);
                let _ = CloseHandle(ready_event);
            }
            return Err(format!("create Origin Speak instance mutex: {error}"));
        }
    };
    let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;

    if already_running {
        unsafe {
            let _ = CloseHandle(mutex);
            let _ = CloseHandle(quit_event);
            let _ = CloseHandle(ready_event);
        }
        return Ok(false);
    }

    INSTANCE_STATE
        .set(InstanceState {
            _mutex_handle: mutex.0 as usize,
            quit_event_handle: quit_event.0 as usize,
            ready_event_handle: ready_event.0 as usize,
        })
        .map_err(|_| "Origin Speak single-instance state was initialized twice".to_string())?;
    Ok(true)
}

pub fn mark_runtime_ready() -> Result<(), String> {
    let state = INSTANCE_STATE
        .get()
        .copied()
        .ok_or_else(|| "Origin Speak instance state is unavailable".to_string())?;
    let event = HANDLE(state.ready_event_handle as *mut core::ffi::c_void);
    unsafe { SetEvent(event) }
        .map_err(|error| format!("signal Origin Speak runtime readiness: {error}"))
}

pub fn start_instance_listener() -> Result<(), String> {
    if INSTANCE_LISTENER_STARTED.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let Some(state) = INSTANCE_STATE.get().copied() else {
        return Ok(());
    };
    std::thread::Builder::new()
        .name("origin-speak-instance-quit".to_string())
        .spawn(move || {
            let event = HANDLE(state.quit_event_handle as *mut core::ffi::c_void);
            if unsafe { WaitForSingleObject(event, u32::MAX) } == WAIT_OBJECT_0 {
                crate::platform_shell::notify_instance_quit();
            }
        })
        .map(|_| ())
        .map_err(|error| format!("start Origin Speak instance quit listener: {error}"))
}

struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

pub fn auto_start_supported() -> bool {
    true
}

pub fn auto_start_status() -> Result<AutoStartState, String> {
    let key = open_run_key(KEY_READ)?;
    let value_name = wide(VALUE_NAME);
    let mut value_type = REG_SZ;
    let mut byte_len = 0u32;
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
        return Ok(AutoStartState::Disabled);
    }
    check(status, "query Origin Speak login registration")?;
    if value_type != REG_SZ || byte_len == 0 {
        return Ok(AutoStartState::Mismatched);
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
    check(status, "read Origin Speak login registration")?;
    let units = bytes[..byte_len as usize]
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .take_while(|unit| *unit != 0)
        .collect::<Vec<_>>();
    let current = String::from_utf16(&units)
        .map_err(|error| format!("decode Origin Speak login registration: {error}"))?;
    let expected = login_command()?;
    Ok(if current == expected {
        AutoStartState::Enabled
    } else {
        AutoStartState::Mismatched
    })
}

pub fn set_auto_start_enabled(enabled: bool) -> Result<AutoStartState, String> {
    let key = open_run_key(KEY_WRITE)?;
    let value_name = wide(VALUE_NAME);

    if enabled {
        let command = login_command()?;
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
        check(status, "register Origin Speak to launch at login")?;
    } else {
        let status = unsafe { RegDeleteValueW(key.0, PCWSTR(value_name.as_ptr())) };
        if status != ERROR_FILE_NOT_FOUND {
            check(status, "remove Origin Speak login registration")?;
        }
    }

    auto_start_status()
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

fn login_command() -> Result<String, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("resolve current Origin Speak executable: {error}"))?;
    command_for_executable(&executable)
}

fn command_for_executable(executable: &Path) -> Result<String, String> {
    let executable = executable
        .to_str()
        .ok_or_else(|| "Origin Speak executable path is not valid Unicode".to_string())?;
    if executable.contains('"') {
        return Err(
            "Origin Speak executable path contains an unsupported quote character".to_string(),
        );
    }
    Ok(format!("\"{executable}\""))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_command_is_quoted_and_has_no_ui_flag() {
        let command = command_for_executable(Path::new(
            r"C:\Program Files\Origin Speak\origin-runtime.exe",
        ))
        .expect("valid command");
        assert_eq!(
            command,
            r#""C:\Program Files\Origin Speak\origin-runtime.exe""#
        );
    }
}
