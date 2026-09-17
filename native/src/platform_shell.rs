use std::ffi::OsStr;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellCapabilities {
    pub auto_start: bool,
    pub tray: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellEvent {
    OpenDashboard,
    Quit,
}

static INSTANCE_ACTIVATION_SENDER: OnceLock<Mutex<Option<mpsc::Sender<ShellEvent>>>> =
    OnceLock::new();
static INSTANCE_ACTIVATION_PENDING: AtomicBool = AtomicBool::new(false);

fn instance_activation_sender() -> &'static Mutex<Option<mpsc::Sender<ShellEvent>>> {
    INSTANCE_ACTIVATION_SENDER.get_or_init(|| Mutex::new(None))
}

pub(crate) fn notify_instance_activation() {
    let sender = instance_activation_sender()
        .lock()
        .ok()
        .and_then(|sender| sender.clone());
    if let Some(sender) = sender
        && sender.send(ShellEvent::OpenDashboard).is_ok()
    {
        return;
    }
    INSTANCE_ACTIVATION_PENDING.store(true, Ordering::Release);
}

fn register_instance_activation_sender(sender: &mpsc::Sender<ShellEvent>) {
    if let Ok(mut slot) = instance_activation_sender().lock() {
        *slot = Some(sender.clone());
    }
    if INSTANCE_ACTIVATION_PENDING.swap(false, Ordering::AcqRel) {
        let _ = sender.send(ShellEvent::OpenDashboard);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoStartState {
    Disabled,
    Enabled,
    RequiresApproval,
}

impl AutoStartState {
    pub const fn launches_automatically(self) -> bool {
        matches!(self, Self::Enabled)
    }

    pub const fn requested(self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellFeature {
    AutoStart,
    Tray,
}

impl fmt::Display for ShellFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AutoStart => f.write_str("auto-start"),
            Self::Tray => f.write_str("system tray"),
        }
    }
}

#[derive(Debug)]
pub enum ShellError {
    Unsupported(ShellFeature),
    OperationFailed {
        feature: ShellFeature,
        message: String,
    },
}

impl fmt::Display for ShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(feature) => write!(f, "{feature} is not supported on this platform"),
            Self::OperationFailed { feature, message } => {
                write!(f, "{feature} operation failed: {message}")
            }
        }
    }
}

impl std::error::Error for ShellError {}

/// Native desktop-shell services owned by the GPUI frontend.
pub struct ShellServices {
    event_tx: mpsc::Sender<ShellEvent>,
    events: Arc<Mutex<mpsc::Receiver<ShellEvent>>>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    tray: Option<imp::Tray>,
}

impl ShellServices {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        register_instance_activation_sender(&event_tx);
        #[cfg(target_os = "windows")]
        imp::start_instance_activation_listener();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let tray = imp::Tray::new(event_tx.clone()).ok();

        Self {
            event_tx,
            events: Arc::new(Mutex::new(event_rx)),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            tray,
        }
    }

    pub fn events(&self) -> Arc<Mutex<mpsc::Receiver<ShellEvent>>> {
        self.events.clone()
    }

    pub fn capabilities(&self) -> ShellCapabilities {
        ShellCapabilities {
            auto_start: imp::auto_start_supported(),
            tray: cfg!(any(target_os = "macos", target_os = "windows")),
        }
    }

    pub fn auto_start_status(&self) -> Result<AutoStartState, ShellError> {
        if !imp::auto_start_supported() {
            return Err(ShellError::Unsupported(ShellFeature::AutoStart));
        }

        imp::auto_start_status().map_err(|message| ShellError::OperationFailed {
            feature: ShellFeature::AutoStart,
            message,
        })
    }

    pub fn set_auto_start_enabled(&self, enabled: bool) -> Result<AutoStartState, ShellError> {
        if !imp::auto_start_supported() {
            return Err(ShellError::Unsupported(ShellFeature::AutoStart));
        }

        imp::set_auto_start_enabled(enabled).map_err(|message| ShellError::OperationFailed {
            feature: ShellFeature::AutoStart,
            message,
        })
    }

    pub fn tray_enabled(&self) -> Result<bool, ShellError> {
        if !self.capabilities().tray {
            return Err(ShellError::Unsupported(ShellFeature::Tray));
        }

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            Ok(self.tray.is_some())
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Ok(false)
        }
    }

    pub fn set_tray_enabled(&mut self, enabled: bool) -> Result<(), ShellError> {
        if !self.capabilities().tray {
            return Err(ShellError::Unsupported(ShellFeature::Tray));
        }

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            if enabled && self.tray.is_none() {
                self.tray = Some(imp::Tray::new(self.event_tx.clone()).map_err(|message| {
                    ShellError::OperationFailed {
                        feature: ShellFeature::Tray,
                        message,
                    }
                })?);
            } else if !enabled {
                self.tray = None;
            }
            Ok(())
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = enabled;
            Err(ShellError::Unsupported(ShellFeature::Tray))
        }
    }
}

impl Default for ShellServices {
    fn default() -> Self {
        Self::new()
    }
}

pub fn show_dashboard_window(window: &gpui::Window) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    imp::show_dashboard_window(window);
    window.activate_window();
}

pub fn configure_dashboard_window(window: &gpui::Window) {
    #[cfg(target_os = "windows")]
    imp::set_dashboard_window_icon(window);
    #[cfg(not(target_os = "windows"))]
    let _ = window;
}

pub fn hide_dashboard_window(window: &gpui::Window) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    imp::hide_dashboard_window(window);
}

pub fn claim_single_instance() -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        imp::claim_single_instance()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(true)
    }
}

/// Detects the native login-launch flag used to start with the dashboard hidden.
pub fn launched_minimized() -> bool {
    std::env::args_os()
        .skip(1)
        .any(|argument| argument == OsStr::new("--minimized"))
}

pub fn launched_from_deep_link() -> bool {
    std::env::args_os().skip(1).any(|argument| {
        argument
            .to_string_lossy()
            .to_ascii_lowercase()
            .starts_with("listenos://")
    })
}
