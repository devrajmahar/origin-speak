use crate::platform_shell::AutoStartState;
use objc2::runtime::AnyClass;
use objc2_service_management::{SMAppService, SMAppServiceStatus};

pub fn auto_start_supported() -> bool {
    AnyClass::get(c"SMAppService").is_some()
}

pub fn auto_start_status() -> Result<AutoStartState, String> {
    let service = service()?;
    status(&service)
}

pub fn set_auto_start_enabled(enabled: bool) -> Result<AutoStartState, String> {
    let service = service()?;
    let current = status(&service)?;

    if enabled {
        if current != AutoStartState::Disabled {
            return Ok(current);
        }

        if let Err(error) = unsafe { service.registerAndReturnError() } {
            let after = status(&service)?;
            if after == AutoStartState::RequiresApproval {
                return Ok(after);
            }
            return Err(format!(
                "register Origin Speak to launch at login: {error:?}"
            ));
        }
    } else {
        if current == AutoStartState::Disabled {
            return Ok(current);
        }

        unsafe { service.unregisterAndReturnError() }
            .map_err(|error| format!("remove Origin Speak login registration: {error:?}"))?;
    }

    status(&service)
}

fn service() -> Result<objc2::rc::Retained<SMAppService>, String> {
    if !auto_start_supported() {
        return Err("SMAppService requires macOS 13 or newer".to_string());
    }
    Ok(unsafe { SMAppService::mainAppService() })
}

fn status(service: &SMAppService) -> Result<AutoStartState, String> {
    let status = unsafe { service.status() };
    match status {
        SMAppServiceStatus::NotRegistered => Ok(AutoStartState::Disabled),
        SMAppServiceStatus::Enabled => Ok(AutoStartState::Enabled),
        SMAppServiceStatus::RequiresApproval => Ok(AutoStartState::RequiresApproval),
        SMAppServiceStatus::NotFound => {
            Err("macOS could not find the main-app login service for this bundle".to_string())
        }
        other => Err(format!(
            "macOS returned unknown login-service status {other:?}"
        )),
    }
}
