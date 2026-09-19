#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod components;
mod overlay;
mod platform_shell;
mod runtime;
mod runtime_compute_status;
mod state;
mod theme;

use app::OriginSpeakApp;
use gpui::*;
use origin_speak_lib::AppConfig;
use runtime::RuntimeController;

fn main() {
    if let Some(request) = maintenance_autostart_request() {
        run_autostart_maintenance(request);
        return;
    }

    let is_primary = platform_shell::claim_single_instance()
        .expect("failed to establish Origin Speak single-instance ownership");
    if !is_primary {
        return;
    }
    platform_shell::start_instance_listener()
        .expect("failed to start Origin Speak resident control listener");
    let _ = runtime_compute_status::clear();
    reconcile_persisted_auto_start();

    let runtime =
        RuntimeController::new().expect("failed to initialize Origin Speak native runtime");
    let application = gpui_platform::application();
    application.run(move |cx| {
        gpui_base::init(cx);
        theme::configure_base_theme(cx);
        let mut runtime = runtime;
        runtime
            .initialize_shortcuts()
            .expect("failed to register the Origin Speak global dictation shortcut");
        let app = cx.new(|cx| OriginSpeakApp::new(runtime, cx));

        // GPUI and global-hotkey both need a live platform event loop. Keep one
        // permanently hidden anchor window instead of constructing the former
        // dashboard. The only visible surfaces are the compact status overlays.
        let bounds = Bounds::centered(None, size(px(1.0), px(1.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                focus: false,
                show: false,
                titlebar: None,
                is_movable: false,
                is_resizable: false,
                is_minimizable: false,
                window_background: WindowBackgroundAppearance::Transparent,
                window_decorations: Some(WindowDecorations::Client),
                ..WindowOptions::default()
            },
            move |_, _| app.clone(),
        )
        .expect("failed to create Origin Speak resident event-loop window");
        platform_shell::mark_runtime_ready()
            .expect("failed to publish Origin Speak runtime readiness");
    });
    let _ = runtime_compute_status::clear();
    platform_shell::cleanup_instance_control();
}

#[derive(Clone, Copy)]
enum AutoStartMaintenance {
    Enable,
    Disable,
    Status,
}

fn maintenance_autostart_request() -> Option<AutoStartMaintenance> {
    for argument in std::env::args_os().skip(1) {
        if argument == "--maintenance-autostart-enable" {
            return Some(AutoStartMaintenance::Enable);
        }
        if argument == "--maintenance-autostart-disable" {
            return Some(AutoStartMaintenance::Disable);
        }
        if argument == "--maintenance-autostart-status" {
            return Some(AutoStartMaintenance::Status);
        }
    }
    None
}

fn run_autostart_maintenance(request: AutoStartMaintenance) {
    let result = match request {
        AutoStartMaintenance::Enable => platform_shell::set_auto_start_enabled(true),
        AutoStartMaintenance::Disable => platform_shell::set_auto_start_enabled(false),
        AutoStartMaintenance::Status => platform_shell::auto_start_status(),
    };
    match result {
        Ok(platform_shell::AutoStartState::Enabled) => std::process::exit(0),
        Ok(platform_shell::AutoStartState::Disabled) => std::process::exit(10),
        Ok(platform_shell::AutoStartState::RequiresApproval) => std::process::exit(11),
        Ok(platform_shell::AutoStartState::Mismatched) => std::process::exit(12),
        Err(_error) => {
            #[cfg(debug_assertions)]
            eprintln!("[Origin Speak] autostart maintenance failed: {_error}");
            std::process::exit(1);
        }
    }
}

fn reconcile_persisted_auto_start() {
    if !platform_shell::auto_start_supported() {
        return;
    }

    let desired = AppConfig::load_from_disk().unwrap_or_default().auto_start;
    let needs_change = match platform_shell::auto_start_status() {
        Ok(platform_shell::AutoStartState::Enabled) => !desired,
        Ok(platform_shell::AutoStartState::RequiresApproval) => !desired,
        Ok(platform_shell::AutoStartState::Disabled) => desired,
        Ok(platform_shell::AutoStartState::Mismatched) => true,
        Err(_) => true,
    };
    if needs_change {
        let result = platform_shell::set_auto_start_enabled(desired);
        #[cfg(debug_assertions)]
        if let Err(error) = result {
            eprintln!("[Origin Speak] could not reconcile login autostart: {error}");
        }
        #[cfg(not(debug_assertions))]
        let _ = result;
    }
}
