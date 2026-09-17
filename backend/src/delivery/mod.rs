use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeliveryPhase {
    Idle,
    Preparing,
    Injecting,
    Verifying,
    Retrying,
    Succeeded,
    RecoverableFailure,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TargetSurfaceKind {
    Terminal,
    Browser,
    CodeEditor,
    TextField,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeliveryStrategy {
    CtrlV,
    CtrlShiftV,
    ShiftInsert,
    SimulatedTyping,
}

impl DeliveryStrategy {
    pub fn label(self) -> &'static str {
        match self {
            Self::CtrlV => "ctrl_v",
            Self::CtrlShiftV => "ctrl_shift_v",
            Self::ShiftInsert => "shift_insert",
            Self::SimulatedTyping => "simulated_typing",
        }
    }
}

pub struct DeliveryUpdate {
    pub phase: DeliveryPhase,
    pub surface: TargetSurfaceKind,
    pub target: Option<String>,
    pub strategy: Option<DeliveryStrategy>,
    pub attempts: u8,
    pub summary: String,
    pub recovered_to_clipboard: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryStatusSnapshot {
    pub phase: DeliveryPhase,
    pub target: Option<String>,
    pub surface: TargetSurfaceKind,
    pub strategy: Option<String>,
    pub attempts: u8,
    pub summary: String,
    pub transcript_preview: Option<String>,
    pub recovered_to_clipboard: bool,
    pub updated_at: DateTime<Utc>,
}

impl Default for DeliveryStatusSnapshot {
    fn default() -> Self {
        Self {
            phase: DeliveryPhase::Idle,
            target: None,
            surface: TargetSurfaceKind::Unknown,
            strategy: None,
            attempts: 0,
            summary: "Idle".to_string(),
            transcript_preview: None,
            recovered_to_clipboard: false,
            updated_at: Utc::now(),
        }
    }
}

pub struct DeliveryState {
    current: DeliveryStatusSnapshot,
    last_failed_text: Option<String>,
}

impl DeliveryState {
    pub fn new() -> Self {
        Self {
            current: DeliveryStatusSnapshot::default(),
            last_failed_text: None,
        }
    }

    pub fn snapshot(&self) -> DeliveryStatusSnapshot {
        self.current.clone()
    }

    pub fn reset(&mut self) {
        self.current = DeliveryStatusSnapshot::default();
        self.last_failed_text = None;
    }

    pub fn begin(&mut self, text: &str) {
        self.last_failed_text = None;
        self.current = DeliveryStatusSnapshot {
            phase: DeliveryPhase::Preparing,
            target: None,
            surface: TargetSurfaceKind::Unknown,
            strategy: None,
            attempts: 0,
            summary: "Preparing delivery".to_string(),
            transcript_preview: Some(preview_text(text)),
            recovered_to_clipboard: false,
            updated_at: Utc::now(),
        };
    }

    pub fn update(&mut self, update: DeliveryUpdate) {
        self.current.phase = update.phase;
        self.current.surface = update.surface;
        self.current.target = update.target;
        self.current.strategy = update.strategy.map(|s| s.label().to_string());
        self.current.attempts = update.attempts;
        self.current.summary = update.summary;
        self.current.recovered_to_clipboard = update.recovered_to_clipboard;
        self.current.updated_at = Utc::now();
    }

    pub fn store_failure(
        &mut self,
        text: String,
        summary: impl Into<String>,
        recovered_to_clipboard: bool,
    ) {
        self.last_failed_text = Some(text.clone());
        self.current.phase = DeliveryPhase::RecoverableFailure;
        self.current.summary = summary.into();
        self.current.transcript_preview = Some(preview_text(&text));
        self.current.recovered_to_clipboard = recovered_to_clipboard;
        self.current.updated_at = Utc::now();
    }

    pub fn last_failed_text(&self) -> Option<String> {
        self.last_failed_text.clone()
    }
}

impl Default for DeliveryState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Default)]
pub struct SurfaceSnapshot {
    pub target_label: Option<String>,
    pub process_name: Option<String>,
    pub window_title: Option<String>,
    pub window_class: Option<String>,
    pub framework_id: Option<String>,
    pub control_name: Option<String>,
    pub control_class_name: Option<String>,
    pub control_type: Option<i32>,
    pub text_snapshot: Option<String>,
}

impl SurfaceSnapshot {
    pub fn classify(&self) -> TargetSurfaceKind {
        let process = self
            .process_name
            .as_deref()
            .unwrap_or_default()
            .to_lowercase();
        let title = self
            .window_title
            .as_deref()
            .unwrap_or_default()
            .to_lowercase();
        let window_class = self
            .window_class
            .as_deref()
            .unwrap_or_default()
            .to_lowercase();
        let framework = self
            .framework_id
            .as_deref()
            .unwrap_or_default()
            .to_lowercase();
        let control_name = self
            .control_name
            .as_deref()
            .unwrap_or_default()
            .to_lowercase();
        let control_class = self
            .control_class_name
            .as_deref()
            .unwrap_or_default()
            .to_lowercase();

        let title_or_control = format!("{title} {control_name}");
        let is_terminal = [
            "windowsterminal",
            "powershell",
            "pwsh",
            "cmd",
            "wt",
            "wezterm",
            "alacritty",
            "warp",
            "mintty",
            "tabby",
            "conemu",
            "putty",
            "terminal",
            "console",
        ]
        .iter()
        .any(|keyword| {
            process.contains(keyword)
                || title_or_control.contains(keyword)
                || window_class.contains(keyword)
                || control_class.contains(keyword)
        }) || window_class.contains("consolewindowclass")
            || window_class.contains("cascadia")
            || (matches!(process.as_str(), "code" | "cursor" | "windsurf")
                && ["terminal", "powershell", "cmd", "bash", "claude"]
                    .iter()
                    .any(|keyword| title_or_control.contains(keyword)));

        if is_terminal {
            return TargetSurfaceKind::Terminal;
        }

        let is_browser = [
            "chrome", "msedge", "edge", "firefox", "brave", "opera", "arc",
        ]
        .iter()
        .any(|keyword| process.contains(keyword))
            || framework.contains("chrome")
            || framework.contains("gecko");

        if is_browser {
            return TargetSurfaceKind::Browser;
        }

        let is_editor = [
            "code",
            "cursor",
            "windsurf",
            "devenv",
            "pycharm",
            "webstorm",
            "goland",
            "idea",
            "intellij",
            "notepad++",
            "sublime",
            "zed",
        ]
        .iter()
        .any(|keyword| process.contains(keyword));

        if is_editor {
            return TargetSurfaceKind::CodeEditor;
        }

        if matches!(
            self.control_type,
            Some(50004) | Some(50030) // Edit or Document
        ) {
            return TargetSurfaceKind::TextField;
        }

        TargetSurfaceKind::Unknown
    }

    pub fn supports_readback(&self) -> bool {
        self.text_snapshot.is_some()
    }
}

pub fn preview_text(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= 64 {
        collapsed
    } else {
        let preview: String = collapsed.chars().take(64).collect();
        format!("{preview}...")
    }
}

pub fn should_use_typing_fallback(surface: TargetSurfaceKind, text: &str) -> bool {
    let length = text.chars().count();
    let has_newlines = text.contains('\n') || text.contains('\r');
    match surface {
        TargetSurfaceKind::Terminal => !has_newlines && length <= 1500,
        _ => length <= 4000,
    }
}

pub fn strategy_chain(
    surface: TargetSurfaceKind,
    snapshot: &SurfaceSnapshot,
    text: &str,
) -> Vec<DeliveryStrategy> {
    let mut strategies = match surface {
        TargetSurfaceKind::Terminal => {
            let process = snapshot
                .process_name
                .as_deref()
                .unwrap_or_default()
                .to_lowercase();
            if matches!(process.as_str(), "code" | "cursor" | "windsurf") {
                vec![DeliveryStrategy::CtrlShiftV, DeliveryStrategy::CtrlV]
            } else {
                vec![
                    DeliveryStrategy::ShiftInsert,
                    DeliveryStrategy::CtrlShiftV,
                    DeliveryStrategy::CtrlV,
                ]
            }
        }
        TargetSurfaceKind::Browser
        | TargetSurfaceKind::CodeEditor
        | TargetSurfaceKind::TextField => {
            vec![DeliveryStrategy::CtrlV]
        }
        TargetSurfaceKind::Unknown => vec![DeliveryStrategy::CtrlV],
    };

    if should_use_typing_fallback(surface, text) {
        strategies.push(DeliveryStrategy::SimulatedTyping);
    }

    strategies
}

pub fn verify_inserted_text(
    before: &SurfaceSnapshot,
    after: &SurfaceSnapshot,
    inserted: &str,
) -> bool {
    let after_text = match after.text_snapshot.as_ref() {
        Some(text) => normalize_text(text),
        None => return false,
    };
    let inserted_text = normalize_text(inserted);

    if inserted_text.is_empty() {
        return false;
    }

    let before_text = before
        .text_snapshot
        .as_ref()
        .map(|text| normalize_text(text))
        .unwrap_or_default();

    if !before_text.is_empty() && before_text == after_text {
        return false;
    }

    if !before_text.contains(&inserted_text) && after_text.contains(&inserted_text) {
        return true;
    }

    let tail = trailing_marker(&inserted_text);
    if !tail.is_empty() && !before_text.contains(&tail) && after_text.contains(&tail) {
        return true;
    }

    let delta = after_text
        .chars()
        .count()
        .saturating_sub(before_text.chars().count());
    delta >= inserted_text.chars().count().min(18)
}

fn trailing_marker(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let take = chars.len().min(32);
    chars[chars.len().saturating_sub(take)..].iter().collect()
}

fn normalize_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(target_os = "windows")]
pub fn capture_surface_snapshot(max_text_len: i32) -> SurfaceSnapshot {
    use std::path::Path;

    use windows::core::BSTR;
    use windows::Win32::Foundation::{BOOL, RPC_E_CHANGED_MODE};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_NATIVE,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation8, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
        IUIAutomationValuePattern, UIA_TextPatternId, UIA_ValuePatternId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };

    struct ComGuard(bool);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    fn bstr_to_string(value: windows::core::Result<BSTR>) -> Option<String> {
        value
            .ok()
            .map(|v| v.to_string())
            .filter(|s| !s.trim().is_empty())
    }

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0 == 0 {
        return SurfaceSnapshot::default();
    }

    let mut process_id = 0_u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut process_id));
    }

    let window_title = {
        let mut buf = vec![0_u16; 512];
        let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
        if len > 0 {
            Some(String::from_utf16_lossy(&buf[..len as usize]))
        } else {
            None
        }
    };

    let window_class = {
        let mut buf = vec![0_u16; 256];
        let len = unsafe { GetClassNameW(hwnd, &mut buf) };
        if len > 0 {
            Some(String::from_utf16_lossy(&buf[..len as usize]))
        } else {
            None
        }
    };

    let process_name = unsafe {
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL(0), process_id)
            .ok()
            .and_then(|handle| {
                let mut buf = vec![0_u16; 1024];
                let mut size = buf.len() as u32;
                QueryFullProcessImageNameW(
                    handle,
                    PROCESS_NAME_NATIVE,
                    windows::core::PWSTR(buf.as_mut_ptr()),
                    &mut size,
                )
                .ok()
                .map(|_| {
                    let full = String::from_utf16_lossy(&buf[..size as usize]);
                    Path::new(&full)
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .map(|stem| stem.to_string())
                        .unwrap_or(full)
                })
            })
    };

    let _com = match unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() } {
        Ok(_) => Some(ComGuard(true)),
        Err(err) if err.code() == RPC_E_CHANGED_MODE => Some(ComGuard(false)),
        Err(_) => None,
    };

    let focused_element = unsafe {
        let automation: windows::core::Result<IUIAutomation> =
            CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER);
        automation
            .and_then(|automation| automation.GetFocusedElement())
            .ok()
    };

    let mut snapshot = SurfaceSnapshot {
        target_label: process_name.clone().or_else(|| window_title.clone()),
        process_name,
        window_title,
        window_class,
        framework_id: None,
        control_name: None,
        control_class_name: None,
        control_type: None,
        text_snapshot: None,
    };

    if let Some(element) = focused_element {
        snapshot.framework_id = unsafe { bstr_to_string(element.CurrentFrameworkId()) };
        snapshot.control_name = unsafe { bstr_to_string(element.CurrentName()) };
        snapshot.control_class_name = unsafe { bstr_to_string(element.CurrentClassName()) };
        snapshot.control_type = unsafe { element.CurrentControlType().ok().map(|id| id.0) };
        snapshot.text_snapshot = read_text_snapshot(&element, max_text_len);
    }

    fn read_text_snapshot(element: &IUIAutomationElement, max_text_len: i32) -> Option<String> {
        unsafe {
            if let Ok(value_pattern) =
                element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            {
                if let Ok(value) = value_pattern.CurrentValue() {
                    let text = value.to_string();
                    if !text.trim().is_empty() {
                        return Some(text);
                    }
                }
            }

            if let Ok(text_pattern) =
                element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            {
                if let Ok(range) = text_pattern.DocumentRange() {
                    if let Ok(text) = range.GetText(max_text_len) {
                        let text = text.to_string();
                        if !text.trim().is_empty() {
                            return Some(text);
                        }
                    }
                }
            }
        }

        None
    }

    snapshot
}

#[cfg(not(target_os = "windows"))]
pub fn capture_surface_snapshot(_max_text_len: i32) -> SurfaceSnapshot {
    SurfaceSnapshot::default()
}
