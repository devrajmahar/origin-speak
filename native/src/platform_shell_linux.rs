use crate::platform_shell::AutoStartState;

pub fn auto_start_supported() -> bool {
    false
}

pub fn auto_start_status() -> Result<AutoStartState, String> {
    Err(
        "native Linux auto-start is not available until ListenOS has a packaged desktop entry"
            .to_string(),
    )
}

pub fn set_auto_start_enabled(_enabled: bool) -> Result<AutoStartState, String> {
    Err(
        "native Linux auto-start is not available until ListenOS has a packaged desktop entry"
            .to_string(),
    )
}
