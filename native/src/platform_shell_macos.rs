use crate::platform_shell::{AutoStartState, ShellEvent};
use gpui::Window;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength, NSView,
};
use objc2_foundation::{MainThreadMarker, NSData, NSObject, NSObjectProtocol, NSSize, ns_string};
use objc2_service_management::{SMAppService, SMAppServiceStatus};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::mpsc;

#[derive(Debug)]
struct TrayTargetIvars {
    sender: mpsc::Sender<ShellEvent>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = TrayTargetIvars]
    struct TrayTarget;

    unsafe impl NSObjectProtocol for TrayTarget {}

    impl TrayTarget {
        #[unsafe(method(openDashboard:))]
        fn open_dashboard(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().sender.send(ShellEvent::OpenDashboard);
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().sender.send(ShellEvent::Quit);
        }
    }
);

impl TrayTarget {
    fn new(mtm: MainThreadMarker, sender: mpsc::Sender<ShellEvent>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TrayTargetIvars { sender });
        unsafe { msg_send![super(this), init] }
    }
}

pub struct Tray {
    status_bar: Retained<NSStatusBar>,
    status_item: Retained<NSStatusItem>,
    _icon: Retained<NSImage>,
    _menu: Retained<NSMenu>,
    _target: Retained<TrayTarget>,
}

impl Tray {
    pub fn new(sender: mpsc::Sender<ShellEvent>) -> Result<Self, String> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "macOS status item must be created on the main thread".to_string())?;
        let target = TrayTarget::new(mtm, sender);
        let status_bar = NSStatusBar::systemStatusBar();
        let status_item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
        let icon = status_bar_icon()?;
        let button = status_item
            .button(mtm)
            .ok_or_else(|| "macOS status item did not provide a button".to_string())?;
        button.setImage(Some(&icon));
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("ListenOS"));

        let open_item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                ns_string!("Open Dashboard"),
                Some(sel!(openDashboard:)),
                ns_string!(""),
            )
        };
        unsafe {
            open_item.setTarget(Some(&*target));
        }
        menu.addItem(&open_item);
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let quit_item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                ns_string!("Quit"),
                Some(sel!(quit:)),
                ns_string!(""),
            )
        };
        unsafe {
            quit_item.setTarget(Some(&*target));
        }
        menu.addItem(&quit_item);

        status_item.setMenu(Some(&menu));

        Ok(Self {
            status_bar,
            status_item,
            _icon: icon,
            _menu: menu,
            _target: target,
        })
    }
}

fn status_bar_icon() -> Result<Retained<NSImage>, String> {
    const ICON_PNG: &[u8] = include_bytes!("../assets/app-icon.png");
    let data = unsafe { NSData::dataWithBytes_length(ICON_PNG.as_ptr().cast(), ICON_PNG.len()) };
    let image = NSImage::initWithData(NSImage::alloc(), &data)
        .ok_or_else(|| "decode embedded ListenOS status-bar icon".to_string())?;
    image.setSize(NSSize::new(18.0, 18.0));
    Ok(image)
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.status_item.setMenu(None);
        self.status_bar.removeStatusItem(&self.status_item);
    }
}

fn ns_view(window: &Window) -> Option<Retained<NSView>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    unsafe { Retained::retain(handle.ns_view.as_ptr().cast()) }
}

pub fn show_dashboard_window(window: &Window) {
    let Some(view) = ns_view(window) else {
        return;
    };
    let Some(native_window) = view.window() else {
        return;
    };
    native_window.orderFront(None);
}

pub fn hide_dashboard_window(window: &Window) {
    let Some(view) = ns_view(window) else {
        return;
    };
    let Some(native_window) = view.window() else {
        return;
    };
    native_window.orderOut(None);
}

pub fn auto_start_supported() -> bool {
    // SMAppService is available on macOS 13+. Looking it up dynamically keeps
    // older supported systems from attempting an unavailable Objective-C class.
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
            return Err(format!("register ListenOS to launch at login: {error:?}"));
        }
    } else {
        if current == AutoStartState::Disabled {
            return Ok(current);
        }

        unsafe { service.unregisterAndReturnError() }
            .map_err(|error| format!("remove ListenOS login registration: {error:?}"))?;
    }

    status(&service)
}

fn service() -> Result<objc2::rc::Retained<SMAppService>, String> {
    if !auto_start_supported() {
        return Err("SMAppService requires macOS 13 or newer".to_string());
    }

    // SAFETY: The runtime class lookup above establishes that SMAppService is
    // available before the generated binding resolves and messages the class.
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
