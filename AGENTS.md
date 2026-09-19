# Origin Speak Agent Guidance

- Origin Speak is a single-process native Rust desktop application under `native/` using application-owned UI/components on top of `gpui-base` and GPUI.
- The native application calls the Rust engine directly through the `origin_speak_lib` library. Do not add JSON-RPC, HTTP, webview, Electron, React, or Node IPC between GPUI and the Rust core.
- Keep audio capture, model inference, downloads, persistence, updates, and other blocking work off the GPUI render thread.
- Use Origin Speak-owned design tokens and explicit text/layout constraints. Native code must not depend on `gpui-kit` or `gpui-component` styled components. Use `gpui-base` as the unstyled behavior/infrastructure layer and GPUI for rendering primitives.
- Preserve unrelated working-tree changes. Validate Rust core changes with `cargo fmt --manifest-path backend/Cargo.toml -- --check` and `cargo test --manifest-path backend/Cargo.toml --locked`; validate native UI changes with `cargo fmt --manifest-path native/Cargo.toml -- --check` and `cargo check --manifest-path native/Cargo.toml --locked`.
- The supported native release paths are the Windows and macOS packaging under `native/packaging/` and the native GitHub release workflow. Native Linux packaging remains intentionally unsupported until the cross-backend overlay/input constraints are resolved.
