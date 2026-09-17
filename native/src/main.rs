mod app;
mod components;
mod overlay;
mod platform_shell;
mod runtime;
mod state;
mod theme;
mod views;

use app::ListenOsApp;
use gpui::*;
use runtime::RuntimeController;

fn main() {
    // A plain `cargo run -p listenos-native` intentionally builds without a
    // GPU transcription backend (the Vulkan/Metal SDKs are build-time inputs).
    // That fallback is the most common cause of "dictation is slow and the
    // Settings GPU line says CPU": the binary is fine, it was just built
    // without `--features gpu-vulkan` (Windows, needs the Vulkan SDK) or
    // `--features gpu-metal` (macOS). Release artifacts always enable the
    // matching backend. Warn once here so a CPU-only dev build is never
    // mistaken for broken GPU inference.
    #[cfg(not(any(feature = "gpu-vulkan", feature = "gpu-metal")))]
    eprintln!(
        "[ListenOS] built WITHOUT a GPU transcription backend: Whisper will run on CPU. \
        For GPU inference on Windows install the Vulkan SDK and run \
        `cargo run --manifest-path native/Cargo.toml --features gpu-vulkan`; \
        on macOS use `--features gpu-metal`. See Settings -> System for the active backend."
    );

    let is_primary = platform_shell::claim_single_instance()
        .expect("failed to establish ListenOS single-instance ownership");
    if !is_primary {
        return;
    }

    let runtime = RuntimeController::new().expect("failed to initialize ListenOS native runtime");
    let launched_minimized = platform_shell::launched_minimized();
    let launched_from_deep_link = platform_shell::launched_from_deep_link();

    let application = gpui_platform::application();
    #[cfg(target_os = "macos")]
    {
        application.on_open_urls(|_urls| platform_shell::notify_instance_activation());
        application.on_reopen(|_cx| platform_shell::notify_instance_activation());
    }

    application.run(move |cx| {
        gpui_base::init(cx);
        let app = cx.new(|cx| ListenOsApp::new(runtime, cx));
        let tray_active = app.read(cx).tray_active();
        let start_hidden = launched_minimized && tray_active && !launched_from_deep_link;
        let dashboard_bounds = Bounds::centered(None, size(px(1180.0), px(760.0)), cx);
        let dashboard_app = app.clone();
        let dashboard_window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(dashboard_bounds)),
                    focus: !start_hidden,
                    show: !start_hidden,
                    ..WindowOptions::default()
                },
                move |window, cx| {
                    platform_shell::configure_dashboard_window(window);
                    let close_app = dashboard_app.clone();
                    window.on_window_should_close(cx, move |window, cx| {
                        if close_app.read(cx).tray_active() {
                            platform_shell::hide_dashboard_window(window);
                            false
                        } else {
                            cx.quit();
                            false
                        }
                    });
                    dashboard_app
                },
            )
            .expect("failed to open ListenOS native window");
        app.update(cx, |app, _| app.attach_dashboard_window(dashboard_window));
        if !start_hidden {
            cx.activate(true);
        }
    });
}
