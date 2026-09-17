use crate::platform_shell::{AutoStartState, ShellEvent};
use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, mpsc};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, GetLastError, HANDLE,
    HINSTANCE, HWND, LPARAM, LRESULT, POINT, WAIT_OBJECT_0, WIN32_ERROR, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    GWLP_USERDATA, GetCursorPos, GetWindowLongPtrW, ICON_BIG, ICON_SMALL, IsIconic, LoadIconW,
    MF_SEPARATOR, MF_STRING, PostMessageW, RegisterClassW, RegisterWindowMessageW, SW_HIDE,
    SW_RESTORE, SW_SHOW, SendMessageW, SetForegroundWindow, SetWindowLongPtrW, ShowWindow,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP, WM_CONTEXTMENU, WM_LBUTTONUP, WM_NULL,
    WM_RBUTTONUP, WM_SETICON, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{Error as WindowsError, HRESULT, PCWSTR, w};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ListenOS";
const APP_ICON_RESOURCE_ID: usize = 1;
const TRAY_ICON_ID: u32 = 1;
const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 1;
const TRAY_OPEN_COMMAND: usize = 1;
const TRAY_QUIT_COMMAND: usize = 2;
const INSTANCE_MUTEX_NAME: windows::core::PCWSTR = w!("Local\\ListenOS.Native.Instance.v1");
const INSTANCE_EVENT_NAME: windows::core::PCWSTR = w!("Local\\ListenOS.Native.Activate.v1");

#[derive(Clone, Copy)]
struct InstanceState {
    _mutex_handle: usize,
    activation_event_handle: usize,
}

static INSTANCE_STATE: OnceLock<InstanceState> = OnceLock::new();
static INSTANCE_LISTENER_STARTED: AtomicBool = AtomicBool::new(false);

pub fn claim_single_instance() -> Result<bool, String> {
    let activation_event = unsafe { CreateEventW(None, false, false, INSTANCE_EVENT_NAME) }
        .map_err(|error| format!("create ListenOS activation event: {error}"))?;
    let mutex = match unsafe { CreateMutexW(None, false, INSTANCE_MUTEX_NAME) } {
        Ok(mutex) => mutex,
        Err(error) => {
            unsafe {
                let _ = CloseHandle(activation_event);
            }
            return Err(format!("create ListenOS instance mutex: {error}"));
        }
    };
    let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;

    if already_running {
        let signal_result = unsafe { SetEvent(activation_event) };
        unsafe {
            let _ = CloseHandle(mutex);
            let _ = CloseHandle(activation_event);
        }
        signal_result.map_err(|error| format!("activate existing ListenOS instance: {error}"))?;
        return Ok(false);
    }

    let state = InstanceState {
        _mutex_handle: mutex.0 as usize,
        activation_event_handle: activation_event.0 as usize,
    };
    INSTANCE_STATE
        .set(state)
        .map_err(|_| "ListenOS single-instance state was initialized twice".to_string())?;
    Ok(true)
}

pub fn start_instance_activation_listener() {
    if INSTANCE_LISTENER_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let Some(state) = INSTANCE_STATE.get().copied() else {
        return;
    };
    std::thread::Builder::new()
        .name("listenos-instance-activation".to_string())
        .spawn(move || {
            let event = HANDLE(state.activation_event_handle as *mut core::ffi::c_void);
            loop {
                if unsafe { WaitForSingleObject(event, u32::MAX) } != WAIT_OBJECT_0 {
                    break;
                }
                crate::platform_shell::notify_instance_activation();
            }
        })
        .ok();
}

struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

struct TrayContext {
    sender: mpsc::Sender<ShellEvent>,
    menu: windows::Win32::UI::WindowsAndMessaging::HMENU,
    icon: windows::Win32::UI::WindowsAndMessaging::HICON,
    taskbar_created: u32,
}

pub struct Tray {
    hwnd: HWND,
    context: *mut TrayContext,
}

impl Tray {
    pub fn new(sender: mpsc::Sender<ShellEvent>) -> Result<Self, String> {
        let module = unsafe { GetModuleHandleW(None) }
            .map_err(|error| format!("resolve ListenOS module handle: {error}"))?;
        let instance = HINSTANCE(module.0);
        let icon = load_app_icon(instance)?;
        let menu =
            unsafe { CreatePopupMenu() }.map_err(|error| format!("create tray menu: {error}"))?;

        if let Err(error) =
            unsafe { AppendMenuW(menu, MF_STRING, TRAY_OPEN_COMMAND, w!("Open Dashboard")) }
        {
            unsafe {
                let _ = DestroyMenu(menu);
            }
            return Err(format!("create tray Open Dashboard action: {error}"));
        }
        if let Err(error) = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) } {
            unsafe {
                let _ = DestroyMenu(menu);
            }
            return Err(format!("create tray separator: {error}"));
        }
        if let Err(error) = unsafe { AppendMenuW(menu, MF_STRING, TRAY_QUIT_COMMAND, w!("Quit")) } {
            unsafe {
                let _ = DestroyMenu(menu);
            }
            return Err(format!("create tray Quit action: {error}"));
        }

        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        if taskbar_created == 0 {
            unsafe {
                let _ = DestroyMenu(menu);
            }
            return Err(format!(
                "register Explorer restart message: {}",
                std::io::Error::last_os_error()
            ));
        }

        let class = WNDCLASSW {
            lpfnWndProc: Some(tray_window_proc),
            hInstance: instance,
            hIcon: icon,
            lpszClassName: w!("ListenOSTrayWindow"),
            ..Default::default()
        };
        unsafe {
            let _ = RegisterClassW(&class);
        }

        let hwnd = match unsafe {
            CreateWindowExW(
                Default::default(),
                w!("ListenOSTrayWindow"),
                w!("ListenOS Tray"),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            )
        } {
            Ok(hwnd) => hwnd,
            Err(error) => {
                unsafe {
                    let _ = DestroyMenu(menu);
                }
                return Err(format!("create tray event window: {error}"));
            }
        };

        let context = Box::into_raw(Box::new(TrayContext {
            sender,
            menu,
            icon,
            taskbar_created,
        }));
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, context as isize);
        }
        if let Err(error) = add_tray_icon(hwnd, icon) {
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(context));
                let _ = DestroyMenu(menu);
                let _ = DestroyWindow(hwnd);
            }
            return Err(error);
        }

        Ok(Self { hwnd, context })
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            remove_tray_icon(self.hwnd);
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            if !self.context.is_null() {
                let context = Box::from_raw(self.context);
                let _ = DestroyMenu(context.menu);
                self.context = std::ptr::null_mut();
            }
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn tray_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let context = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut TrayContext;
    if !context.is_null() {
        let context = unsafe { &*context };
        if message == context.taskbar_created {
            let _ = add_tray_icon(hwnd, context.icon);
            return LRESULT(0);
        }
        if message == TRAY_CALLBACK_MESSAGE {
            match lparam.0 as u32 {
                WM_LBUTTONUP => {
                    let _ = context.sender.send(ShellEvent::OpenDashboard);
                    return LRESULT(0);
                }
                WM_RBUTTONUP | WM_CONTEXTMENU => {
                    show_tray_menu(hwnd, context);
                    return LRESULT(0);
                }
                _ => {}
            }
        }
    }

    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn add_tray_icon(
    hwnd: HWND,
    icon: windows::Win32::UI::WindowsAndMessaging::HICON,
) -> Result<(), String> {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: TRAY_CALLBACK_MESSAGE,
        hIcon: icon,
        ..Default::default()
    };
    let tooltip = wide("ListenOS");
    let count = tooltip.len().min(data.szTip.len());
    data.szTip[..count].copy_from_slice(&tooltip[..count]);

    if unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
        Ok(())
    } else {
        Err(format!(
            "add ListenOS tray icon: {}",
            std::io::Error::last_os_error()
        ))
    }
}

unsafe fn remove_tray_icon(hwnd: HWND) {
    let data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        ..Default::default()
    };
    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
}

fn show_tray_menu(hwnd: HWND, context: &TrayContext) {
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) }.is_err() {
        return;
    }
    unsafe {
        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            context.menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            None,
            hwnd,
            None,
        )
        .0 as usize;
        match command {
            TRAY_OPEN_COMMAND => {
                let _ = context.sender.send(ShellEvent::OpenDashboard);
            }
            TRAY_QUIT_COMMAND => {
                let _ = context.sender.send(ShellEvent::Quit);
            }
            _ => {}
        }
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
    }
}

fn dashboard_hwnd(window: &Window) -> Option<HWND> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(HWND(handle.hwnd.get() as *mut core::ffi::c_void))
}

fn load_app_icon(
    instance: HINSTANCE,
) -> Result<windows::Win32::UI::WindowsAndMessaging::HICON, String> {
    let resource = PCWSTR(APP_ICON_RESOURCE_ID as *const u16);
    unsafe { LoadIconW(Some(instance), resource) }
        .map_err(|error| format!("load embedded ListenOS icon: {error}"))
}

pub fn set_dashboard_window_icon(window: &Window) {
    let Some(hwnd) = dashboard_hwnd(window) else {
        return;
    };
    let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
        return;
    };
    let Ok(icon) = load_app_icon(HINSTANCE(module.0)) else {
        return;
    };

    unsafe {
        let _ = SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_BIG as usize)),
            Some(LPARAM(icon.0 as isize)),
        );
        let _ = SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_SMALL as usize)),
            Some(LPARAM(icon.0 as isize)),
        );
    }
}

pub fn show_dashboard_window(window: &Window) {
    let Some(hwnd) = dashboard_hwnd(window) else {
        return;
    };
    unsafe {
        let _ = ShowWindow(
            hwnd,
            if IsIconic(hwnd).as_bool() {
                SW_RESTORE
            } else {
                SW_SHOW
            },
        );
    }
}

pub fn hide_dashboard_window(window: &Window) {
    let Some(hwnd) = dashboard_hwnd(window) else {
        return;
    };
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
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
    check(status, "query ListenOS login registration")?;

    if value_type != REG_SZ || byte_len == 0 {
        return Err("ListenOS login registration has an invalid registry value".to_string());
    }

    Ok(AutoStartState::Enabled)
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
        check(status, "register ListenOS to launch at login")?;
    } else {
        let status = unsafe { RegDeleteValueW(key.0, PCWSTR(value_name.as_ptr())) };
        if status != ERROR_FILE_NOT_FOUND {
            check(status, "remove ListenOS login registration")?;
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
        .map_err(|error| format!("resolve current ListenOS executable: {error}"))?;
    command_for_executable(&executable)
}

fn command_for_executable(executable: &Path) -> Result<String, String> {
    let executable = executable
        .to_str()
        .ok_or_else(|| "ListenOS executable path is not valid Unicode".to_string())?;
    if executable.contains('"') {
        return Err("ListenOS executable path contains an unsupported quote character".to_string());
    }
    Ok(format!("\"{executable}\" --minimized"))
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
