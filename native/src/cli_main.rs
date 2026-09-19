mod autostart_control;
mod bootstrap;
mod cli;
mod cli_executor;
mod legacy_cleanup;
mod legacy_migration;
#[cfg(any(target_os = "macos", test))]
mod macos_bundle;
mod management;
mod path_control;
mod process_control;
mod system_capabilities;
mod update_manager;

use autostart_control::AutoStartStatus;
use cli::{
    CliError, Command, CommandResult, ConfigAction, HotkeyAction, MicAction, ModelAction,
    OutputFormat, ToggleAction, UninstallOptions, UpdateAction,
};
use cli_executor::CoreCliExecutor;
use management::DataLayout;
use origin_speak_lib::{
    State, app_data_root, get_audio_devices, get_config, get_transcription_settings,
    get_trigger_hotkey, list_local_models, normalize_hotkey_string,
};
use process_control::RuntimeState;
use semver::Version;
use std::ffi::OsString;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::{Duration, Instant};
use system_capabilities::SystemCapabilities;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(8);
const HELPER_RETRY_TIMEOUT: Duration = Duration::from_secs(20);
#[cfg(target_os = "windows")]
const UNINSTALL_COMMIT_TIMEOUT: Duration = Duration::from_secs(120);

#[cfg(target_os = "windows")]
struct ArmedUninstallHelper {
    helper_path: PathBuf,
    commit_path: PathBuf,
}

#[derive(Debug)]
struct PersistedFileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

#[derive(Debug)]
struct SetupPersistenceSnapshot {
    files: Vec<PersistedFileSnapshot>,
}

impl SetupPersistenceSnapshot {
    fn capture() -> Result<Self, String> {
        let root = app_data_root()?;
        Self::capture_from_root(&root)
    }

    fn capture_from_root(root: &Path) -> Result<Self, String> {
        let mut files = Vec::new();
        for name in [
            "config.json",
            "config.json.bak",
            "config.json.tmp",
            "transcription_settings.json",
        ] {
            let path = root.join(name);
            let contents = match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() || !metadata.is_file() {
                        return Err(format!(
                            "refusing setup persistence snapshot for non-regular app-owned file {}",
                            path.display()
                        ));
                    }
                    Some(fs::read(&path).map_err(|error| {
                        format!(
                            "read setup persistence snapshot {}: {error}",
                            path.display()
                        )
                    })?)
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(format!(
                        "inspect setup persistence file {}: {error}",
                        path.display()
                    ));
                }
            };
            files.push(PersistedFileSnapshot { path, contents });
        }
        Ok(Self { files })
    }

    fn restore(&self) -> Result<(), String> {
        for snapshot in &self.files {
            match snapshot.contents.as_ref() {
                Some(contents) => {
                    if let Some(parent) = snapshot.path.parent() {
                        fs::create_dir_all(parent).map_err(|error| {
                            format!(
                                "recreate setup persistence directory {}: {error}",
                                parent.display()
                            )
                        })?;
                    }
                    if let Ok(metadata) = fs::symlink_metadata(&snapshot.path)
                        && (metadata.file_type().is_symlink() || !metadata.is_file())
                    {
                        return Err(format!(
                            "refusing to restore setup persistence through non-regular path {}",
                            snapshot.path.display()
                        ));
                    }
                    fs::write(&snapshot.path, contents).map_err(|error| {
                        format!(
                            "restore setup persistence file {}: {error}",
                            snapshot.path.display()
                        )
                    })?;
                }
                None => match fs::symlink_metadata(&snapshot.path) {
                    Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                        fs::remove_file(&snapshot.path).map_err(|error| {
                            format!(
                                "remove setup-created persistence file {}: {error}",
                                snapshot.path.display()
                            )
                        })?;
                    }
                    Ok(_) => {
                        return Err(format!(
                            "refusing to remove non-regular setup persistence path {}",
                            snapshot.path.display()
                        ));
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!(
                            "inspect setup persistence path {} during rollback: {error}",
                            snapshot.path.display()
                        ));
                    }
                },
            }
        }
        Ok(())
    }
}

fn main() {
    let args = std::env::args_os().collect::<Vec<_>>();
    if let Some(code) = run_internal_helper(&args) {
        std::process::exit(code);
    }

    let requested_format = if args.iter().any(|arg| arg == "--json") {
        OutputFormat::Json
    } else {
        OutputFormat::Human
    };
    let request = match cli::parse(args.clone()) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("{}", error.render(requested_format));
            std::process::exit(2);
        }
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let error = CliError {
                code: "runtime_init_failed",
                message: format!("could not initialize CLI runtime: {error}"),
            };
            eprintln!("{}", error.render(request.output));
            std::process::exit(1);
        }
    };

    match runtime.block_on(execute(request.command, request.output)) {
        Ok(result) => println!("{}", result.render(request.output)),
        Err(message) => {
            let error = CliError {
                code: "command_failed",
                message,
            };
            eprintln!("{}", error.render(request.output));
            std::process::exit(1);
        }
    }
}

async fn execute(command: Command, format: OutputFormat) -> Result<CommandResult, String> {
    match command {
        Command::Help => Ok(CommandResult::success("help", cli::usage())),
        Command::Version => Ok(CommandResult::success(
            "version",
            format!("Origin Speak {}", env!("CARGO_PKG_VERSION")),
        )
        .field("version", env!("CARGO_PKG_VERSION"))),
        Command::Setup(options) => {
            if setup_needs_guidance(&options, format) {
                match guided_setup(options).await? {
                    Some(options) => setup(options).await,
                    None => Ok(CommandResult::success(
                        "setup_cancelled",
                        "Origin Speak setup cancelled; no changes were made",
                    )),
                }
            } else {
                setup(options).await
            }
        }
        Command::Status => status().await,
        Command::Doctor => doctor().await,
        Command::Config(action) => config_command(action).await,
        Command::Dictionary(action) => CoreCliExecutor::new().dictionary(&action).await,
        Command::Model(action) => model_command(action).await,
        Command::Mic(action) => mic_command(action).await,
        Command::Hotkey(action) => hotkey_command(action).await,
        Command::Autostart(action) => autostart(action).await,
        Command::Start => start(),
        Command::Stop => stop(),
        Command::Restart => restart(),
        Command::Update(action) => update(action).await,
        Command::Uninstall(options) => uninstall(options, format),
    }
}

async fn setup(options: cli::SetupOptions) -> Result<CommandResult, String> {
    let layout = DataLayout::discover()?;
    #[cfg(target_os = "macos")]
    {
        // Do this before stopping/replacing anything. Historical ListenOS macOS
        // builds used SMAppService for the main app but exposed no bounded remote
        // unregister protocol, so their bundle must be explicitly retired first.
        legacy_migration::preflight_legacy_macos_bundle()?;
        macos_bundle::recover_stale_transactions(&layout.install_root)?;
    }
    let current_exe = std::env::current_exe()
        .map_err(|error| format!("resolve current Origin Speak manager: {error}"))?;
    let runtime_state = process_control::runtime_status().ok();
    let runtime_was_running = setup_preinstall_stop_required(runtime_state);

    // The legacy NSIS executable and the new manager alias the same Windows
    // path case-insensitively. Stop the resident before replacing either binary.
    if runtime_was_running {
        process_control::stop_runtime(COMMAND_TIMEOUT)?;
    }
    setup_after_runtime_stop(&options, &layout, &current_exe, runtime_was_running).await
}

async fn setup_after_runtime_stop(
    options: &cli::SetupOptions,
    layout: &DataLayout,
    current_exe: &Path,
    runtime_was_running: bool,
) -> Result<CommandResult, String> {
    if options.interactive {
        eprintln!("Installing Origin Speak manager and resident runtime...");
    }
    let mut legacy_data_migration = management::LegacyDataMigration::begin()?;
    let persistence_snapshot = match SetupPersistenceSnapshot::capture() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let _ = legacy_data_migration.rollback();
            return Err(error);
        }
    };
    let mut manager_swap: Option<bootstrap::TransactionalSwap> = None;
    #[cfg(target_os = "macos")]
    let mut runtime_swap: Option<macos_bundle::BundleSwap> = None;
    #[cfg(not(target_os = "macos"))]
    let mut runtime_swap: Option<bootstrap::TransactionalSwap> = None;
    let mut release_staging: Option<PathBuf> = None;
    let mut manager_path_added_by_setup = false;
    let mut desired_autostart = None;

    let setup_result = async {
        #[cfg(target_os = "macos")]
        {
            let bundle_parent = layout
                .install_root
                .parent()
                .ok_or_else(|| "macOS runtime bundle has no parent directory".to_string())?;
            fs::create_dir_all(bundle_parent).map_err(|error| {
                format!(
                    "create macOS Applications directory {}: {error}",
                    bundle_parent.display()
                )
            })?;
        }
        #[cfg(not(target_os = "macos"))]
        fs::create_dir_all(&layout.install_root).map_err(|error| {
            format!(
                "create install directory {}: {error}",
                layout.install_root.display()
            )
        })?;
        fs::create_dir_all(&layout.manager_root).map_err(|error| {
            format!(
                "create manager directory {}: {error}",
                layout.manager_root.display()
            )
        })?;

        let manager_installed = if same_path(current_exe, &layout.manager_path) {
            false
        } else {
            manager_swap = Some(bootstrap::transactional_replace(
                current_exe,
                &layout.manager_path,
            )?);
            true
        };
        let manager_path_registration = path_control::ensure(&layout.manager_root)?;
        manager_path_added_by_setup = manager_path_registration.changed;

        let mut runtime_installed = false;
        #[cfg(target_os = "macos")]
        {
            if let Some(sibling) =
                bootstrap::find_sibling_runtime(current_exe, env!("CARGO_PKG_VERSION"))?
            {
                runtime_swap = Some(bootstrap::transactional_install_macos_runtime_bundle(
                    &sibling,
                    &layout.install_root,
                )?);
                runtime_installed = true;
            } else if !installed_macos_bundle_matches_manager(layout) {
                let staged = stage_bootstrap_release_runtime(layout).await?;
                release_staging = Some(staged.staging);
                runtime_swap = Some(bootstrap::transactional_install_macos_runtime_bundle(
                    &staged.payload,
                    &layout.install_root,
                )?);
                runtime_installed = true;
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            if let Some(sibling) =
                bootstrap::find_sibling_runtime(current_exe, env!("CARGO_PKG_VERSION"))?
            {
                runtime_swap = Some(bootstrap::transactional_replace(
                    &sibling,
                    &layout.runtime_path,
                )?);
                runtime_installed = true;
            } else if cfg!(target_os = "windows") || !layout.runtime_path.is_file() {
                let staged = stage_bootstrap_release_runtime(layout).await?;
                release_staging = Some(staged.staging);
                runtime_swap = Some(bootstrap::transactional_replace(
                    &staged.payload,
                    &layout.runtime_path,
                )?);
                runtime_installed = true;
            }
        }

        if !layout.runtime_path.is_file() {
            return Err(format!(
                "resident runtime is not installed at {}; provide a sibling runtime payload or run a matching release manager",
                layout.runtime_path.display()
            ));
        }

        if options.interactive {
            eprintln!("Configuring local voice-to-text...");
        }
        let executor = CoreCliExecutor::new();
        let state = executor.state();
        let mut result = executor.setup(options).await?;
        let persisted = get_config(State::new(state.as_ref())).await?;
        desired_autostart = Some(persisted.auto_start);

        if options.interactive {
            eprintln!("Starting Origin Speak resident runtime...");
        }
        let started = process_control::start_runtime(&layout.runtime_path)?;

        result = result
            .field("manager_path", layout.manager_path.display().to_string())
            .field("runtime_path", layout.runtime_path.display().to_string())
            .field("manager_installed_now", manager_installed.to_string())
            .field("runtime_installed_now", runtime_installed.to_string())
            .field(
                "legacy_data_roots_migrated",
                legacy_data_migration.migrated_count().to_string(),
            )
            .field(
                "manager_on_path",
                manager_path_registration.on_path.to_string(),
            )
            .field(
                "manager_path_changed",
                manager_path_registration.changed.to_string(),
            )
            .field("runtime_started_now", started.to_string())
            .field("runtime_reloaded", runtime_was_running.to_string());
        Ok(result)
    }
    .await;

    match setup_result {
        Ok(mut result) => {
            finalize_setup_binary_swaps(&mut manager_swap, &mut runtime_swap)?;
            if let Some(staging) = release_staging {
                let _ = fs::remove_dir_all(staging);
            }
            let desired_autostart = desired_autostart.ok_or_else(|| {
                "setup completed without resolving the persisted autostart preference".to_string()
            })?;
            let native_autostart = autostart_control::set(&layout.runtime_path, desired_autostart)
                .map_err(|error| {
                    format!(
                        "Origin Speak is installed and the resident runtime is ready, but native autostart configuration failed: {error}. Run `origin autostart {}` to retry without reinstalling",
                        if desired_autostart { "enable" } else { "disable" }
                    )
                })?;
            let legacy_cleanup = legacy_migration::cleanup_after_replacement(current_exe)?;
            result = result
                .field("autostart_native", autostart_label(native_autostart))
                .field(
                    "legacy_artifacts_removed",
                    legacy_cleanup.removed.len().to_string(),
                )
                .field(
                    "legacy_cleanup_warnings",
                    legacy_cleanup.warnings.len().to_string(),
                );
            Ok(result)
        }
        Err(error) => {
            let had_binary_changes = manager_swap.is_some() || runtime_swap.is_some();
            let stop_error = match process_control::runtime_status() {
                Ok(RuntimeState::Running) => process_control::stop_runtime(COMMAND_TIMEOUT).err(),
                Ok(RuntimeState::Stopped) => None,
                Err(status_error) => Some(format!(
                    "inspect replacement runtime before rollback: {status_error}"
                )),
            };
            let rollback_error =
                rollback_setup_binary_swaps(&mut manager_swap, &mut runtime_swap).err();
            let persistence_restore_error = persistence_snapshot.restore().err();
            let data_migration_restore_error = legacy_data_migration.rollback().err();
            let restart_error = if runtime_was_running
                && stop_error.is_none()
                && rollback_error.is_none()
                && persistence_restore_error.is_none()
                && layout.runtime_path.is_file()
            {
                process_control::start_runtime(&layout.runtime_path).err()
            } else {
                None
            };
            if let Some(staging) = release_staging {
                let _ = fs::remove_dir_all(staging);
            }

            let path_restore_error = if manager_path_added_by_setup {
                path_control::remove(&layout.manager_root).err()
            } else {
                None
            };

            let mut recovery = Vec::new();
            if let Some(stop_error) = stop_error {
                recovery.push(format!(
                    "stopping the replacement runtime failed: {stop_error}"
                ));
            }
            if let Some(rollback_error) = rollback_error {
                recovery.push(format!("binary rollback failed: {rollback_error}"));
            }
            if let Some(restart_error) = restart_error {
                recovery.push(format!(
                    "restarting the previous runtime failed: {restart_error}"
                ));
            }
            if let Some(path_restore_error) = path_restore_error {
                recovery.push(format!(
                    "restoring the pre-setup PATH registration failed: {path_restore_error}"
                ));
            }
            if let Some(persistence_restore_error) = persistence_restore_error {
                recovery.push(format!(
                    "restoring the pre-setup persisted voice settings failed: {persistence_restore_error}"
                ));
            } else if runtime_was_running && recovery.is_empty() {
                recovery.push(if had_binary_changes {
                    "previous binaries were restored and the runtime was restarted".to_string()
                } else {
                    "the previous runtime was restarted".to_string()
                });
            } else if !runtime_was_running && recovery.is_empty() {
                recovery.push(if had_binary_changes {
                    "installed binary changes were rolled back".to_string()
                } else {
                    "no binary replacement was committed".to_string()
                });
            }
            if let Some(data_migration_restore_error) = data_migration_restore_error {
                recovery.push(format!(
                    "restoring migrated legacy data failed: {data_migration_restore_error}"
                ));
            }
            Err(format!("{error}; {}", recovery.join("; ")))
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn rollback_setup_binary_swaps(
    manager: &mut Option<bootstrap::TransactionalSwap>,
    runtime: &mut Option<bootstrap::TransactionalSwap>,
) -> Result<(), String> {
    let manager_result = manager.as_mut().map_or(Ok(()), |swap| swap.rollback());
    let runtime_result = runtime.as_mut().map_or(Ok(()), |swap| swap.rollback());
    combine_setup_swap_results("rollback", manager_result, runtime_result)
}

#[cfg(target_os = "macos")]
fn rollback_setup_binary_swaps(
    manager: &mut Option<bootstrap::TransactionalSwap>,
    runtime: &mut Option<macos_bundle::BundleSwap>,
) -> Result<(), String> {
    let manager_result = manager.as_mut().map_or(Ok(()), |swap| swap.rollback());
    let runtime_result = runtime.as_mut().map_or(Ok(()), |swap| swap.rollback());
    combine_setup_swap_results("rollback", manager_result, runtime_result)
}

#[cfg(not(target_os = "macos"))]
fn finalize_setup_binary_swaps(
    manager: &mut Option<bootstrap::TransactionalSwap>,
    runtime: &mut Option<bootstrap::TransactionalSwap>,
) -> Result<(), String> {
    if let Some(runtime) = runtime.as_mut() {
        runtime.finalize()?;
    }
    if let Some(manager) = manager.as_mut() {
        manager.finalize()?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn finalize_setup_binary_swaps(
    manager: &mut Option<bootstrap::TransactionalSwap>,
    runtime: &mut Option<macos_bundle::BundleSwap>,
) -> Result<(), String> {
    if let Some(runtime) = runtime.as_mut() {
        runtime.finalize()?;
    }
    if let Some(manager) = manager.as_mut() {
        manager.finalize()?;
    }
    Ok(())
}

fn combine_setup_swap_results(
    action: &str,
    manager: Result<(), String>,
    runtime: Result<(), String>,
) -> Result<(), String> {
    match (manager, runtime) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(format!("manager {action} failed: {error}")),
        (Ok(()), Err(error)) => Err(format!("runtime {action} failed: {error}")),
        (Err(manager_error), Err(runtime_error)) => Err(format!(
            "manager {action} failed ({manager_error}); runtime {action} failed ({runtime_error})"
        )),
    }
}

fn setup_preinstall_stop_required(state: Option<RuntimeState>) -> bool {
    matches!(state, Some(RuntimeState::Running))
}

fn setup_needs_guidance(options: &cli::SetupOptions, format: OutputFormat) -> bool {
    setup_needs_guidance_with_terminal(
        options,
        format,
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
    )
}

fn setup_needs_guidance_with_terminal(
    options: &cli::SetupOptions,
    format: OutputFormat,
    stdin_tty: bool,
    stdout_tty: bool,
) -> bool {
    matches!(format, OutputFormat::Human)
        && stdin_tty
        && stdout_tty
        && !options.has_explicit_choices()
        && !options.interactive
}

async fn guided_setup(mut options: cli::SetupOptions) -> Result<Option<cli::SetupOptions>, String> {
    let capabilities = SystemCapabilities::detect();
    let recommendation = capabilities.recommend_model();
    let executor = CoreCliExecutor::new();
    let state = executor.state();
    let models = list_local_models(State::new(state.as_ref())).await?;
    let config = get_config(State::new(state.as_ref())).await?;

    eprintln!("Origin Speak setup");
    eprintln!("Detected system:");
    eprintln!(
        "  CPU: {} · {} logical cores",
        capabilities.arch, capabilities.logical_cpus
    );
    eprintln!(
        "  RAM: {}",
        capabilities
            .memory_gib()
            .map(|value| format!("{value} GiB"))
            .unwrap_or_else(|| "unknown".to_string())
    );
    eprintln!("  Accelerator: {}", capabilities.accelerator);
    eprintln!("  Recommended model: {}", recommendation.model);
    eprintln!();

    let default_model_index = models
        .iter()
        .position(|model| model.id == recommendation.model)
        .unwrap_or(0);
    eprintln!("Transcription model:");
    for (index, model) in models.iter().enumerate() {
        let marker = if index == default_model_index {
            " (recommended)"
        } else {
            ""
        };
        let installed = if model.downloaded {
            " · installed"
        } else {
            ""
        };
        eprintln!(
            "  {}. {} [{}]{}{}",
            index + 1,
            model.label,
            model.id,
            marker,
            installed
        );
    }
    let model_index = prompt_index("Choose model", models.len(), default_model_index + 1)? - 1;
    options.model = Some(models[model_index].id.clone());

    let devices = get_audio_devices().await?;
    eprintln!();
    eprintln!("Microphone:");
    eprintln!("  1. System default");
    for (index, device) in devices.iter().enumerate() {
        let marker = if device.is_default {
            " (OS default)"
        } else {
            ""
        };
        eprintln!("  {}. {}{}", index + 2, device.name, marker);
    }
    let default_mic = config
        .selected_audio_device
        .as_ref()
        .and_then(|selected| devices.iter().position(|device| &device.name == selected))
        .map(|index| index + 2)
        .unwrap_or(1);
    let mic_choice = prompt_index("Choose microphone", devices.len() + 1, default_mic)?;
    let selected_mic = if mic_choice == 1 {
        None
    } else {
        Some(devices[mic_choice - 2].name.clone())
    };
    options.microphone = Some(
        selected_mic
            .clone()
            .unwrap_or_else(|| "default".to_string()),
    );

    let current_hotkey = get_trigger_hotkey(State::new(state.as_ref())).await?;
    eprintln!();
    let hotkey = loop {
        let raw = prompt_text("Dictation hotkey", &current_hotkey)?;
        match normalize_hotkey_string(&raw) {
            Ok(normalized) => break normalized,
            Err(error) => eprintln!("  {error}"),
        }
    };
    options.hotkey = Some(hotkey.clone());

    let autostart = prompt_yes_no(
        "Start Origin Speak automatically when you sign in?",
        config.auto_start,
    )?;
    options.autostart = Some(autostart);

    eprintln!();
    eprintln!("Setup summary:");
    eprintln!(
        "  Model: {}",
        options.model.as_deref().unwrap_or(recommendation.model)
    );
    eprintln!(
        "  Microphone: {}",
        selected_mic.as_deref().unwrap_or("system default")
    );
    eprintln!("  Dictation hotkey: {hotkey}");
    eprintln!(
        "  Autostart: {}",
        if autostart { "enabled" } else { "disabled" }
    );
    eprintln!("  Models and application data remain local to this user account.");
    if !prompt_yes_no("Apply these settings?", true)? {
        return Ok(None);
    }

    options.interactive = true;
    Ok(Some(options))
}

fn prompt_index(label: &str, count: usize, default: usize) -> Result<usize, String> {
    loop {
        let raw = prompt_text(label, &default.to_string())?;
        match raw.parse::<usize>() {
            Ok(value) if (1..=count).contains(&value) => return Ok(value),
            _ => eprintln!("  Enter a number from 1 to {count}."),
        }
    }
}

fn prompt_yes_no(label: &str, default: bool) -> Result<bool, String> {
    let hint = if default { "Y/n" } else { "y/N" };
    loop {
        eprint!("{label} [{hint}] ");
        io::stderr()
            .flush()
            .map_err(|error| format!("flush setup prompt: {error}"))?;
        let mut answer = String::new();
        io::stdin()
            .read_line(&mut answer)
            .map_err(|error| format!("read setup prompt: {error}"))?;
        match answer.trim().to_ascii_lowercase().as_str() {
            "" => return Ok(default),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => eprintln!("  Enter yes or no."),
        }
    }
}

fn prompt_text(label: &str, default: &str) -> Result<String, String> {
    eprint!("{label} [{default}]: ");
    io::stderr()
        .flush()
        .map_err(|error| format!("flush setup prompt: {error}"))?;
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .map_err(|error| format!("read setup prompt: {error}"))?;
    let value = value.trim();
    Ok(if value.is_empty() {
        default.to_string()
    } else {
        value.to_string()
    })
}

async fn config_command(action: ConfigAction) -> Result<CommandResult, String> {
    let mutating = matches!(action, ConfigAction::Set { .. } | ConfigAction::Reset(_));
    let result = CoreCliExecutor::new().config(&action).await?;
    restart_after_mutation(result, mutating)
}

async fn model_command(action: ModelAction) -> Result<CommandResult, String> {
    let executor = CoreCliExecutor::new();
    let restart = matches!(action, ModelAction::Select(_));
    let result = executor.model(&action).await?;
    restart_after_mutation(result, restart)
}

async fn mic_command(action: MicAction) -> Result<CommandResult, String> {
    let mutating = matches!(action, MicAction::Select(_));
    let result = CoreCliExecutor::new().mic(&action).await?;
    restart_after_mutation(result, mutating)
}

async fn hotkey_command(action: HotkeyAction) -> Result<CommandResult, String> {
    let mutating = matches!(action, HotkeyAction::Set(_));
    let result = CoreCliExecutor::new().hotkey(&action).await?;
    restart_after_mutation(result, mutating)
}

fn restart_after_mutation(result: CommandResult, mutating: bool) -> Result<CommandResult, String> {
    if !mutating {
        return Ok(result);
    }
    let layout = DataLayout::discover()?;
    if !layout.runtime_path.is_file() {
        return Ok(result.field("runtime_restarted", "false (runtime not installed)"));
    }
    match process_control::runtime_status()? {
        RuntimeState::Stopped => Ok(result.field("runtime_restarted", "false")),
        RuntimeState::Running => {
            process_control::restart_runtime(&layout.runtime_path, COMMAND_TIMEOUT)?;
            Ok(result.field("runtime_restarted", "true"))
        }
    }
}

struct StagedBootstrapRuntime {
    payload: PathBuf,
    staging: PathBuf,
}

async fn stage_bootstrap_release_runtime(
    layout: &DataLayout,
) -> Result<StagedBootstrapRuntime, String> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let manager_version = Version::parse(env!("CARGO_PKG_VERSION"))
            .map_err(|error| format!("invalid manager version: {error}"))?;
        let release = update_manager::fetch_release_for_version(&manager_version).await?;
        let staging = layout.update_staging_dir(env!("CARGO_PKG_VERSION"))?;
        let payload = update_manager::stage_runtime_payload(&release, &staging).await?;
        return Ok(StagedBootstrapRuntime { payload, staging });
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = layout;
        Err(
            "release runtime bootstrap is not integrated for this platform; use a sibling development runtime payload"
                .to_string(),
        )
    }
}

async fn status() -> Result<CommandResult, String> {
    let layout = DataLayout::discover()?;
    let executor = CoreCliExecutor::new();
    let state = executor.state();
    let config = get_config(State::new(state.as_ref())).await?;
    let transcription = get_transcription_settings(State::new(state.as_ref())).await?;
    let runtime = process_control::runtime_status()
        .map(runtime_state_label)
        .unwrap_or_else(|error| format!("unsupported: {error}"));
    let native_autostart = autostart_control::status(&layout.runtime_path)
        .map(autostart_label)
        .unwrap_or_else(|error| format!("unsupported: {error}"));
    let manager_path = path_control::status(&layout.manager_root)
        .map(|status| status.on_path.to_string())
        .unwrap_or_else(|error| format!("unknown: {error}"));

    Ok(CommandResult::success("status", "Origin Speak status")
        .field("version", env!("CARGO_PKG_VERSION"))
        .field("runtime", runtime)
        .field(
            "runtime_installed",
            layout.runtime_path.is_file().to_string(),
        )
        .field(
            "manager_installed",
            layout.manager_path.is_file().to_string(),
        )
        .field("model", transcription.model)
        .field(
            "microphone",
            config
                .selected_audio_device
                .unwrap_or_else(|| "system default".to_string()),
        )
        .field("autostart_preference", config.auto_start.to_string())
        .field("autostart_native", native_autostart)
        .field("manager_on_path", manager_path)
        .field("data_root", layout.data_root.display().to_string())
        .field("runtime_path", layout.runtime_path.display().to_string())
        .field("manager_path", layout.manager_path.display().to_string()))
}

async fn doctor() -> Result<CommandResult, String> {
    let layout = DataLayout::discover()?;
    let capabilities = SystemCapabilities::detect();
    let executor = CoreCliExecutor::new();
    let state = executor.state();
    let config_check = get_config(State::new(state.as_ref()))
        .await
        .map(|_| "ok".to_string())
        .unwrap_or_else(|error| format!("error: {error}"));
    let microphone_check = get_audio_devices()
        .await
        .map(|devices| format!("ok ({} input device(s))", devices.len()))
        .unwrap_or_else(|error| format!("error: {error}"));
    let runtime = process_control::runtime_status()
        .map(runtime_state_label)
        .unwrap_or_else(|error| format!("unsupported: {error}"));
    let autostart = autostart_control::status(&layout.runtime_path)
        .map(autostart_label)
        .unwrap_or_else(|error| format!("unsupported: {error}"));
    let manager_path = path_control::status(&layout.manager_root)
        .map(|status| status.on_path.to_string())
        .unwrap_or_else(|error| format!("unknown: {error}"));
    let recommendation = capabilities.recommend_model();

    Ok(
        CommandResult::success("doctor", "Origin Speak diagnostics complete")
            .field("os", capabilities.os)
            .field("arch", capabilities.arch)
            .field("logical_cpus", capabilities.logical_cpus.to_string())
            .field(
                "memory_gib",
                capabilities
                    .memory_gib()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
            )
            .field("accelerator", capabilities.accelerator.to_string())
            .field("recommended_model", recommendation.model)
            .field("config", config_check)
            .field("microphone", microphone_check)
            .field("runtime", runtime)
            .field("autostart", autostart)
            .field("manager_on_path", manager_path)
            .field("manager_root", layout.manager_root.display().to_string())
            .field("runtime_payload", path_check(&layout.runtime_path))
            .field("manager_install", path_check(&layout.manager_path))
            .field("data_root", layout.data_root.display().to_string()),
    )
}

async fn autostart(action: ToggleAction) -> Result<CommandResult, String> {
    let layout = DataLayout::discover()?;
    let executor = CoreCliExecutor::new();
    match action {
        ToggleAction::Status => {
            let mut result = executor.autostart(action).await?;
            result = result.field(
                "native",
                autostart_label(autostart_control::status(&layout.runtime_path)?),
            );
            Ok(result)
        }
        ToggleAction::Enable => {
            if !layout.runtime_path.is_file() {
                return Err(format!(
                    "cannot enable autostart before the resident runtime is installed at {}",
                    layout.runtime_path.display()
                ));
            }
            let native = autostart_control::set(&layout.runtime_path, true)?;
            Ok(executor
                .autostart(action)
                .await?
                .field("native", autostart_label(native)))
        }
        ToggleAction::Disable => {
            let native = autostart_control::set(&layout.runtime_path, false)?;
            Ok(executor
                .autostart(action)
                .await?
                .field("native", autostart_label(native)))
        }
    }
}

fn start() -> Result<CommandResult, String> {
    let layout = DataLayout::discover()?;
    #[cfg(target_os = "macos")]
    {
        // Refuse before current Origin Speak autostart/PATH/runtime state is
        // changed. A legacy main-app login item cannot be safely unregistered by
        // the differently branded Origin Speak main bundle.
        legacy_migration::preflight_legacy_macos_bundle()?;
        macos_bundle::recover_stale_transactions(&layout.install_root)?;
    }
    if !layout.runtime_path.is_file() {
        return Err(format!(
            "resident runtime is not installed at {}; run 'origin setup' first",
            layout.runtime_path.display()
        ));
    }
    let started = process_control::start_runtime(&layout.runtime_path)?;
    Ok(CommandResult::success(
        "runtime_started",
        if started {
            "Origin Speak runtime started"
        } else {
            "Origin Speak runtime is already running"
        },
    )
    .field("started_now", started.to_string()))
}

fn stop() -> Result<CommandResult, String> {
    let stopped = process_control::stop_runtime(COMMAND_TIMEOUT)?;
    Ok(CommandResult::success(
        "runtime_stopped",
        if stopped {
            "Origin Speak runtime stopped"
        } else {
            "Origin Speak runtime is already stopped"
        },
    )
    .field("stopped_now", stopped.to_string()))
}

fn restart() -> Result<CommandResult, String> {
    let layout = DataLayout::discover()?;
    #[cfg(target_os = "macos")]
    {
        // Refuse before disabling current Origin Speak autostart, stopping the
        // resident, or removing PATH registration. The cleanup-time preflight is
        // retained as a race fence in case the historical bundle reappears.
        legacy_migration::preflight_legacy_macos_bundle()?;
        macos_bundle::recover_stale_transactions(&layout.install_root)?;
    }
    if !layout.runtime_path.is_file() {
        return Err(format!(
            "resident runtime is not installed at {}; run 'origin setup' first",
            layout.runtime_path.display()
        ));
    }
    process_control::restart_runtime(&layout.runtime_path, COMMAND_TIMEOUT)?;
    Ok(CommandResult::success(
        "runtime_restarted",
        "Origin Speak runtime restarted",
    ))
}

async fn update(action: UpdateAction) -> Result<CommandResult, String> {
    let Some(available) = update_manager::check_for_update(env!("CARGO_PKG_VERSION"), None).await?
    else {
        return Ok(
            CommandResult::success("up_to_date", "Origin Speak is up to date")
                .field("version", env!("CARGO_PKG_VERSION")),
        );
    };

    if matches!(action, UpdateAction::Check) {
        return Ok(CommandResult::success(
            "update_available",
            format!("Origin Speak {} is available", available.version),
        )
        .field("current_version", env!("CARGO_PKG_VERSION"))
        .field("available_version", available.version.to_string()));
    }

    let layout = DataLayout::discover()?;
    let staging = layout.update_staging_dir(&available.version.to_string())?;
    let staged = update_manager::stage_update(&available, &staging).await?;
    let mut result = CommandResult::success(
        "update_staged",
        format!("Origin Speak {} update staged and verified", staged.version),
    )
    .field("manager_payload", staged.manager_path.display().to_string())
    .field("runtime_payload", staged.runtime_path.display().to_string());

    #[cfg(target_os = "windows")]
    {
        if layout.current_manager_is_installed() {
            let runtime_was_running = process_control::runtime_status()? == RuntimeState::Running;
            if runtime_was_running {
                process_control::stop_runtime(COMMAND_TIMEOUT)?;
            }
            let helper = match spawn_post_exit_helper(
                "__post-exit-update",
                &[
                    OsString::from(staged.version.to_string()),
                    staged.manager_path.as_os_str().to_os_string(),
                    staged.runtime_path.as_os_str().to_os_string(),
                    OsString::from(staged.manager_sha256.clone()),
                    OsString::from(staged.runtime_sha256.clone()),
                    OsString::from(update_runtime_state_token(runtime_was_running)),
                ],
            ) {
                Ok(helper) => helper,
                Err(error) => {
                    if runtime_was_running {
                        return match process_control::start_runtime(&layout.runtime_path) {
                            Ok(_) => Err(format!(
                                "could not start the post-exit update helper; the existing runtime was restarted: {error}"
                            )),
                            Err(restart_error) => Err(format!(
                                "could not start the post-exit update helper ({error}); restarting the existing runtime also failed ({restart_error})"
                            )),
                        };
                    }
                    return Err(error);
                }
            };
            result = result
                .field("post_exit_swap", "pending")
                .field("helper", helper.display().to_string())
                .field(
                    "runtime_restart",
                    if runtime_was_running {
                        "pending"
                    } else {
                        "preserve stopped"
                    },
                );
        } else {
            result = result.field("post_exit_swap", "not scheduled").field(
                "reason",
                "this manager is not running from the app-owned install path",
            );
        }
    }
    #[cfg(target_os = "macos")]
    {
        let runtime_was_running = process_control::runtime_status()? == RuntimeState::Running;
        if runtime_was_running {
            process_control::stop_runtime(COMMAND_TIMEOUT)?;
        }

        if let Err(error) = update_manager::verify_staged_payload(
            &staging,
            &staged.version,
            &staged.runtime_path,
            &staged.runtime_sha256,
        ) {
            return Err(macos_update_failure_after_stop(
                &layout,
                runtime_was_running,
                error,
            ));
        }
        let mut runtime_swap = match bootstrap::transactional_install_macos_runtime_bundle(
            &staged.runtime_path,
            &layout.install_root,
        ) {
            Ok(swap) => swap,
            Err(error) => {
                return Err(macos_update_failure_after_stop(
                    &layout,
                    runtime_was_running,
                    error,
                ));
            }
        };

        if let Err(error) = update_manager::verify_staged_payload(
            &staging,
            &staged.version,
            &staged.runtime_path,
            &staged.runtime_sha256,
        ) {
            let rollback = runtime_swap.rollback();
            return Err(macos_update_failure_with_rollback(
                &layout,
                runtime_was_running,
                format!("macOS runtime archive changed during apply: {error}"),
                rollback,
            ));
        }

        if let Err(error) = update_manager::verify_staged_payload(
            &staging,
            &staged.version,
            &staged.manager_path,
            &staged.manager_sha256,
        ) {
            let rollback = runtime_swap.rollback();
            return Err(macos_update_failure_with_rollback(
                &layout,
                runtime_was_running,
                error,
                rollback,
            ));
        }
        let mut manager_swap =
            match bootstrap::transactional_replace(&staged.manager_path, &layout.manager_path) {
                Ok(swap) => swap,
                Err(error) => {
                    let rollback = runtime_swap.rollback();
                    return Err(macos_update_failure_with_rollback(
                        &layout,
                        runtime_was_running,
                        format!("manager update failed: {error}"),
                        rollback,
                    ));
                }
            };

        if let Err(error) = update_manager::verify_regular_file_sha256(
            &layout.manager_path,
            &staged.manager_sha256,
            "installed manager",
        ) {
            let rollback = rollback_macos_update_pair(&mut manager_swap, &mut runtime_swap);
            return Err(macos_update_failure_with_rollback(
                &layout,
                runtime_was_running,
                error,
                rollback,
            ));
        }

        if runtime_was_running {
            if let Err(error) = process_control::start_runtime(&layout.runtime_path) {
                let stop_replacement = ensure_runtime_stopped();
                let rollback = rollback_macos_update_pair(&mut manager_swap, &mut runtime_swap);
                if let Err(stop_error) = stop_replacement {
                    return Err(match rollback {
                        Ok(()) => format!(
                            "updated macOS runtime failed readiness ({error}); stopping the replacement runtime also failed ({stop_error}); previous files were restored"
                        ),
                        Err(rollback_error) => format!(
                            "updated macOS runtime failed readiness ({error}); stopping the replacement runtime failed ({stop_error}); rollback also failed ({rollback_error})"
                        ),
                    });
                }
                return Err(macos_update_failure_with_rollback(
                    &layout,
                    true,
                    format!("updated macOS runtime failed readiness: {error}"),
                    rollback,
                ));
            }
        } else if let Err(error) = ensure_runtime_stopped() {
            let rollback = rollback_macos_update_pair(&mut manager_swap, &mut runtime_swap);
            return Err(macos_update_failure_with_rollback(
                &layout,
                false,
                format!("updated macOS runtime did not preserve stopped state: {error}"),
                rollback,
            ));
        }

        manager_swap.finalize()?;
        runtime_swap.finalize()?;
        fs::remove_dir_all(&staging).map_err(|error| {
            format!(
                "remove completed macOS update staging {}: {error}",
                staging.display()
            )
        })?;
        result = CommandResult::success(
            "update_applied",
            format!("Origin Speak {} update installed", staged.version),
        )
        .field("manager_payload", staged.manager_path.display().to_string())
        .field("runtime_payload", staged.runtime_path.display().to_string())
        .field("post_exit_swap", "not required on macOS")
        .field(
            "runtime_restart",
            if runtime_was_running {
                "complete"
            } else {
                "preserved stopped"
            },
        );
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        result = result.field(
            "post_exit_swap",
            "unsupported on this platform; verified payloads remain staged",
        );
    }
    Ok(result)
}

#[cfg(target_os = "macos")]
fn rollback_macos_update_pair(
    manager: &mut bootstrap::TransactionalSwap,
    runtime: &mut macos_bundle::BundleSwap,
) -> Result<(), String> {
    let manager_result = manager.rollback();
    let runtime_result = runtime.rollback();
    match (manager_result, runtime_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(format!("manager rollback failed: {error}")),
        (Ok(()), Err(error)) => Err(format!("runtime bundle rollback failed: {error}")),
        (Err(manager_error), Err(runtime_error)) => Err(format!(
            "manager rollback failed ({manager_error}); runtime bundle rollback failed ({runtime_error})"
        )),
    }
}

#[cfg(target_os = "macos")]
fn macos_update_failure_after_stop(
    layout: &DataLayout,
    runtime_was_running: bool,
    error: String,
) -> String {
    if !runtime_was_running {
        return error;
    }
    match process_control::start_runtime(&layout.runtime_path) {
        Ok(_) => format!("{error}; previous runtime restarted"),
        Err(restart_error) => {
            format!("{error}; restarting the previous runtime also failed ({restart_error})")
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_update_failure_with_rollback(
    layout: &DataLayout,
    runtime_was_running: bool,
    error: String,
    rollback: Result<(), String>,
) -> String {
    match rollback {
        Ok(()) => macos_update_failure_after_stop(layout, runtime_was_running, error),
        Err(rollback_error) => format!("{error}; rollback failed ({rollback_error})"),
    }
}

#[cfg(target_os = "macos")]
fn installed_macos_bundle_matches_manager(layout: &DataLayout) -> bool {
    let expected = env!("CARGO_PKG_VERSION")
        .split(|character| character == '-' || character == '+')
        .next()
        .unwrap_or(env!("CARGO_PKG_VERSION"));
    macos_bundle::verify_bundle(&layout.install_root).is_ok()
        && macos_bundle::bundle_version(&layout.install_root)
            .is_ok_and(|version| version == expected)
}

fn uninstall(options: UninstallOptions, format: OutputFormat) -> Result<CommandResult, String> {
    let layout = DataLayout::discover()?;
    cli::uninstall_requires_confirmation(
        options,
        management::non_interactive_terminal() || matches!(format, OutputFormat::Json),
    )
    .map_err(|error| error.message)?;

    if options.dry_run {
        let plan = layout.uninstall_plan(options.keep_data)?;
        return Ok(CommandResult::success(
            "uninstall_dry_run",
            "Uninstall dry run; no files were changed",
        )
        .field("manager", plan.manager_path.display().to_string())
        .field("runtime", plan.runtime_path.display().to_string())
        .field(
            "data_root",
            plan.data_root
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "kept".to_string()),
        )
        .field(
            "local_data_root",
            plan.local_data_root
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| {
                    if options.keep_data {
                        "kept".to_string()
                    } else {
                        "same as data_root".to_string()
                    }
                }),
        )
        .field("model_roots", join_paths(&plan.model_roots))
        .field("stale_update_paths", join_paths(&plan.stale_update_paths))
        .field("install_root", plan.install_root.display().to_string())
        .field("manager_root", plan.manager_root.display().to_string()));
    }

    if !options.yes && !confirm_uninstall(options.keep_data)? {
        return Ok(CommandResult::success(
            "uninstall_cancelled",
            "Uninstall cancelled",
        ));
    }

    #[cfg(target_os = "macos")]
    {
        // Refuse before current Origin Speak autostart, runtime, or PATH state
        // is changed. cleanup_after_replacement() repeats the same preflight as
        // a race fence immediately before legacy compatibility cleanup.
        legacy_migration::preflight_legacy_macos_bundle()?;
        macos_bundle::recover_stale_transactions(&layout.install_root)?;
    }

    // The running manager cannot delete itself on Windows. Prepare and start a
    // helper before any destructive work, but gate actual manager deletion on a
    // one-time commit marker written only after every irreversible uninstall
    // step below has succeeded. If cleanup fails, the helper times out and leaves
    // the manager in place so the user can retry.
    #[cfg(target_os = "windows")]
    let armed_uninstall_helper = if layout.current_manager_is_installed() {
        Some(arm_post_exit_uninstall_helper()?)
    } else {
        None
    };

    // Remove the OS login hook first so a partially completed uninstall cannot
    // launch a missing runtime on the next sign-in.
    let autostart_cleanup = autostart_control::disable_for_uninstall(&layout.runtime_path)?;
    process_control::stop_runtime(COMMAND_TIMEOUT)?;
    let path_registration = path_control::remove(&layout.manager_root)?;
    let current_exe = std::env::current_exe()
        .map_err(|error| format!("resolve current Origin Speak manager: {error}"))?;
    let legacy = legacy_migration::cleanup_after_replacement(&current_exe)?;

    if !options.keep_data {
        management::purge_user_data(&layout)?;
    }
    let runtime_removed = management::remove_installed_runtime(&layout)?;

    let mut result =
        CommandResult::success("uninstall_complete", "Origin Speak uninstall completed")
            .field("runtime_removed", runtime_removed.to_string())
            .field("data_removed", (!options.keep_data).to_string())
            .field("legacy_artifacts_removed", legacy.removed.len().to_string())
            .field("legacy_cleanup_warnings", legacy.warnings.len().to_string())
            .field("autostart_cleanup", autostart_cleanup.detail)
            .field(
                "manager_path_registration_removed",
                path_registration.changed.to_string(),
            );

    #[cfg(target_os = "windows")]
    {
        if let Some(armed) = armed_uninstall_helper {
            commit_post_exit_uninstall(&armed.commit_path)?;
            result = result
                .field("manager_removal", "pending until this command exits")
                .field("helper", armed.helper_path.display().to_string());
        } else {
            let removed = management::remove_installed_manager(&layout)?;
            management::remove_empty_install_root(&layout)?;
            result = result.field("manager_removed", removed.to_string());
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let removed = management::remove_installed_manager(&layout)?;
        management::remove_empty_install_root(&layout)?;
        result = result.field("manager_removed", removed.to_string());
    }

    Ok(result)
}

fn confirm_uninstall(keep_data: bool) -> Result<bool, String> {
    let action = if keep_data {
        "remove the installed Origin Speak manager/runtime and keep user data"
    } else {
        "remove Origin Speak, downloaded models, configuration, and runtime data"
    };
    eprint!("This will {action}. Continue? [y/N] ");
    io::stderr()
        .flush()
        .map_err(|error| format!("flush confirmation prompt: {error}"))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|error| format!("read confirmation: {error}"))?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn run_internal_helper(args: &[OsString]) -> Option<i32> {
    let mode = args.get(1)?.to_str()?;
    match mode {
        "__post-exit-uninstall" => Some(match post_exit_uninstall(&args[2..]) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("post-exit uninstall failed: {error}");
                1
            }
        }),
        "__post-exit-update" => Some(match post_exit_update(&args[2..]) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("post-exit update failed: {error}");
                1
            }
        }),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn post_exit_uninstall(args: &[OsString]) -> Result<(), String> {
    let result = post_exit_uninstall_inner(args);
    let cleanup = schedule_current_helper_delete();
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(cleanup_error)) => Err(cleanup_error),
        (Err(error), Err(cleanup_error)) => Err(format!(
            "{error}; helper cleanup scheduling also failed: {cleanup_error}"
        )),
    }
}

#[cfg(target_os = "windows")]
fn post_exit_uninstall_inner(args: &[OsString]) -> Result<(), String> {
    if args.len() != 1 {
        return Err("invalid post-exit uninstall helper arguments".to_string());
    }
    let commit_path = PathBuf::from(&args[0]);
    wait_for_uninstall_commit(&commit_path, UNINSTALL_COMMIT_TIMEOUT)?;
    fs::remove_file(&commit_path).map_err(|error| {
        format!(
            "remove uninstall commit marker {}: {error}",
            commit_path.display()
        )
    })?;
    let layout = DataLayout::discover()?;
    retry_until(HELPER_RETRY_TIMEOUT, || {
        management::remove_installed_manager(&layout).map(|_| ())
    })?;
    management::remove_empty_install_root(&layout)?;
    schedule_current_helper_delete()?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn post_exit_uninstall(_: &[OsString]) -> Result<(), String> {
    Err("post-exit uninstall helper is only required on Windows".to_string())
}

#[cfg(target_os = "windows")]
fn arm_post_exit_uninstall_helper() -> Result<ArmedUninstallHelper, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let current = std::env::current_exe()
        .map_err(|error| format!("resolve current manager for uninstall helper: {error}"))?;
    let token = uuid::Uuid::new_v4();
    let temp = std::env::temp_dir();
    let helper_path = temp.join(format!("origin-speak-uninstall-helper-{token}.exe"));
    let commit_path = temp.join(format!("origin-speak-uninstall-commit-{token}.marker"));
    if helper_path.exists() || commit_path.exists() {
        return Err("refusing colliding uninstall helper paths".to_string());
    }
    fs::copy(&current, &helper_path).map_err(|error| {
        format!(
            "create post-exit uninstall helper {}: {error}",
            helper_path.display()
        )
    })?;
    let spawn = ProcessCommand::new(&helper_path)
        .arg("__post-exit-uninstall")
        .arg(&commit_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    if let Err(error) = spawn {
        let _ = fs::remove_file(&helper_path);
        return Err(format!("start post-exit uninstall helper: {error}"));
    }
    Ok(ArmedUninstallHelper {
        helper_path,
        commit_path,
    })
}

#[cfg(target_os = "windows")]
fn commit_post_exit_uninstall(path: &Path) -> Result<(), String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options
        .open(path)
        .map_err(|error| format!("create uninstall commit marker {}: {error}", path.display()))?;
    file.write_all(b"commit\n")
        .map_err(|error| format!("write uninstall commit marker: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync uninstall commit marker: {error}"))
}

#[cfg(target_os = "windows")]
fn wait_for_uninstall_commit(path: &Path, timeout: Duration) -> Result<(), String> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(format!(
                        "uninstall commit marker is not a regular file: {}",
                        path.display()
                    ));
                }
                let contents = fs::read(path)
                    .map_err(|error| format!("read uninstall commit marker: {error}"))?;
                if contents == b"commit\n" {
                    return Ok(());
                }
                return Err("uninstall commit marker contents are invalid".to_string());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                return Err(format!(
                    "inspect uninstall commit marker {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Err("uninstall helper timed out waiting for commit".to_string())
}

#[cfg(target_os = "windows")]
fn post_exit_update(args: &[OsString]) -> Result<(), String> {
    let result = post_exit_update_inner(args);
    let cleanup = schedule_current_helper_delete();
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(cleanup_error)) => Err(cleanup_error),
        (Err(error), Err(cleanup_error)) => Err(format!(
            "{error}; scheduling update-helper cleanup also failed: {cleanup_error}"
        )),
    }
}

#[cfg(target_os = "windows")]
fn post_exit_update_inner(args: &[OsString]) -> Result<(), String> {
    if args.len() != 6 {
        return Err("invalid post-exit update helper arguments".to_string());
    }
    let version_text = args[0]
        .to_str()
        .ok_or_else(|| "update version is not valid UTF-8".to_string())?;
    let version = Version::parse(version_text)
        .map_err(|error| format!("update version is invalid: {error}"))?;
    let staged_manager = PathBuf::from(&args[1]);
    let staged_runtime = PathBuf::from(&args[2]);
    let manager_sha256 = args[3]
        .to_str()
        .ok_or_else(|| "manager SHA-256 is not valid UTF-8".to_string())?;
    let runtime_sha256 = args[4]
        .to_str()
        .ok_or_else(|| "runtime SHA-256 is not valid UTF-8".to_string())?;
    let runtime_was_running = parse_update_runtime_state(&args[5])?;
    let layout = DataLayout::discover()?;
    let staging = layout.update_staging_dir(version_text)?;

    let mut runtime_swap = match retry_verified_replace(
        &staging,
        &version,
        &staged_runtime,
        runtime_sha256,
        &layout.runtime_path,
    ) {
        Ok(swap) => swap,
        Err(error) => {
            return Err(windows_update_failure_after_stop(
                &layout,
                runtime_was_running,
                error,
            ));
        }
    };
    if let Err(error) = update_manager::verify_regular_file_sha256(
        &layout.runtime_path,
        runtime_sha256,
        "installed runtime",
    ) {
        let rollback = runtime_swap.rollback();
        return Err(windows_update_failure_with_rollback(
            &layout,
            runtime_was_running,
            error,
            rollback,
        ));
    }

    let mut manager_swap = match retry_verified_replace(
        &staging,
        &version,
        &staged_manager,
        manager_sha256,
        &layout.manager_path,
    ) {
        Ok(swap) => swap,
        Err(error) => {
            let rollback = runtime_swap.rollback();
            return Err(windows_update_failure_with_rollback(
                &layout,
                runtime_was_running,
                format!("manager update failed: {error}"),
                rollback,
            ));
        }
    };
    if let Err(error) = update_manager::verify_regular_file_sha256(
        &layout.manager_path,
        manager_sha256,
        "installed manager",
    ) {
        let rollback = rollback_update_pair(&mut manager_swap, &mut runtime_swap);
        return Err(windows_update_failure_with_rollback(
            &layout,
            runtime_was_running,
            error,
            rollback,
        ));
    }

    if runtime_was_running {
        let mut child = match bootstrap::spawn_runtime_silent(&layout.runtime_path, &[]) {
            Ok(child) => child,
            Err(error) => {
                let rollback = rollback_update_pair(&mut manager_swap, &mut runtime_swap);
                return Err(windows_update_failure_with_rollback(
                    &layout,
                    true,
                    format!("updated runtime could not be started: {error}"),
                    rollback,
                ));
            }
        };

        if let Err(error) = wait_for_runtime_ready(&mut child, Duration::from_secs(6)) {
            let _ = child.kill();
            let _ = child.wait();
            let rollback = rollback_update_pair(&mut manager_swap, &mut runtime_swap);
            return Err(windows_update_failure_with_rollback(
                &layout,
                true,
                format!("updated runtime failed its readiness check: {error}"),
                rollback,
            ));
        }
    } else if let Err(error) = ensure_runtime_stopped() {
        let rollback = rollback_update_pair(&mut manager_swap, &mut runtime_swap);
        return Err(windows_update_failure_with_rollback(
            &layout,
            false,
            format!("updated runtime did not preserve stopped state: {error}"),
            rollback,
        ));
    }

    manager_swap.finalize()?;
    runtime_swap.finalize()?;
    fs::remove_dir_all(&staging).map_err(|error| {
        format!(
            "remove completed update staging {}: {error}",
            staging.display()
        )
    })?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn retry_verified_replace(
    staging: &Path,
    version: &Version,
    source: &Path,
    expected_sha256: &str,
    destination: &Path,
) -> Result<bootstrap::TransactionalSwap, String> {
    let started = Instant::now();
    loop {
        update_manager::verify_staged_payload(staging, version, source, expected_sha256)?;
        match bootstrap::transactional_replace(source, destination) {
            Ok(swap) => return Ok(swap),
            Err(error) if started.elapsed() < HELPER_RETRY_TIMEOUT => {
                let _ = error;
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_update_failure_after_stop(
    layout: &DataLayout,
    runtime_was_running: bool,
    error: String,
) -> String {
    if !runtime_was_running {
        return error;
    }
    match process_control::start_runtime(&layout.runtime_path) {
        Ok(_) => format!("{error}; previous runtime restarted"),
        Err(restart_error) => {
            format!("{error}; restarting the previous runtime also failed ({restart_error})")
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_update_failure_with_rollback(
    layout: &DataLayout,
    runtime_was_running: bool,
    error: String,
    rollback: Result<(), String>,
) -> String {
    match rollback {
        Ok(()) => windows_update_failure_after_stop(layout, runtime_was_running, error),
        Err(rollback_error) => format!("{error}; rollback failed ({rollback_error})"),
    }
}

#[cfg(target_os = "windows")]
fn rollback_update_pair(
    manager: &mut bootstrap::TransactionalSwap,
    runtime: &mut bootstrap::TransactionalSwap,
) -> Result<(), String> {
    let manager_result = manager.rollback();
    let runtime_result = runtime.rollback();
    match (manager_result, runtime_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(format!("manager rollback failed: {error}")),
        (Ok(()), Err(error)) => Err(format!("runtime rollback failed: {error}")),
        (Err(manager_error), Err(runtime_error)) => Err(format!(
            "manager rollback failed ({manager_error}); runtime rollback failed ({runtime_error})"
        )),
    }
}

#[cfg(target_os = "windows")]
fn wait_for_runtime_ready(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        if process_control::runtime_ready()? {
            return Ok(());
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("inspect updated runtime process: {error}"))?
        {
            return Err(format!(
                "updated runtime exited before becoming ready ({status})"
            ));
        }
        if started.elapsed() >= timeout {
            return Err(format!(
                "updated runtime did not become ready within {} ms",
                timeout.as_millis()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(not(target_os = "windows"))]
fn post_exit_update(_: &[OsString]) -> Result<(), String> {
    Err("post-exit update swap is not integrated on this platform".to_string())
}

#[cfg(target_os = "windows")]
fn spawn_post_exit_helper(mode: &str, args: &[OsString]) -> Result<PathBuf, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let current = std::env::current_exe()
        .map_err(|error| format!("resolve current manager for helper: {error}"))?;
    let helper = create_unique_post_exit_helper(&current, mode, &std::env::temp_dir())?;
    let mut command = ProcessCommand::new(&helper);
    command
        .arg(mode)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    if let Err(error) = command.spawn() {
        let _ = fs::remove_file(&helper);
        return Err(format!("start post-exit helper: {error}"));
    }
    Ok(helper)
}

#[cfg(target_os = "windows")]
fn create_unique_post_exit_helper(
    current: &Path,
    mode: &str,
    temp_root: &Path,
) -> Result<PathBuf, String> {
    let kind = if mode.contains("uninstall") {
        "uninstall"
    } else {
        "update"
    };
    for _ in 0..8 {
        let helper = temp_root.join(format!(
            "origin-speak-{kind}-helper-{}.exe",
            uuid::Uuid::new_v4()
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut output = match options.open(&helper) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "create exclusive post-exit helper {}: {error}",
                    helper.display()
                ));
            }
        };
        let copy_result = (|| {
            let mut input = fs::File::open(current)
                .map_err(|error| format!("open manager helper source: {error}"))?;
            io::copy(&mut input, &mut output)
                .map_err(|error| format!("copy post-exit helper: {error}"))?;
            output
                .flush()
                .map_err(|error| format!("flush post-exit helper: {error}"))?;
            output
                .sync_all()
                .map_err(|error| format!("sync post-exit helper: {error}"))?;
            Ok::<(), String>(())
        })();
        drop(output);
        if let Err(error) = copy_result {
            let _ = fs::remove_file(&helper);
            return Err(error);
        }
        let permissions = fs::metadata(current)
            .map_err(|error| format!("read manager helper permissions: {error}"))?
            .permissions();
        if let Err(error) = fs::set_permissions(&helper, permissions) {
            let _ = fs::remove_file(&helper);
            return Err(format!("set post-exit helper permissions: {error}"));
        }
        return Ok(helper);
    }
    Err("could not allocate a unique post-exit helper path".to_string())
}

#[cfg(not(target_os = "windows"))]
fn spawn_post_exit_helper(_: &str, _: &[OsString]) -> Result<PathBuf, String> {
    Err("post-exit helpers are not required on this platform".to_string())
}

fn retry_until<T>(
    timeout: Duration,
    mut operation: impl FnMut() -> Result<T, String>,
) -> Result<T, String> {
    let started = Instant::now();
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if started.elapsed() < timeout => {
                let _ = error;
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(target_os = "windows")]
fn schedule_current_helper_delete() -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    const MOVEFILE_DELAY_UNTIL_REBOOT: u32 = 0x0000_0004;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }

    let current = std::env::current_exe()
        .map_err(|error| format!("resolve helper path for cleanup: {error}"))?;
    let wide = current
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let ok = unsafe { MoveFileExW(wide.as_ptr(), ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) };
    if ok == 0 {
        return Err(format!(
            "schedule helper cleanup: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn schedule_current_helper_delete() -> Result<(), String> {
    let current = std::env::current_exe()
        .map_err(|error| format!("resolve helper path for cleanup: {error}"))?;
    fs::remove_file(current).map_err(|error| format!("remove helper: {error}"))
}

fn ensure_runtime_stopped() -> Result<(), String> {
    if process_control::runtime_status()? == RuntimeState::Running {
        process_control::stop_runtime(COMMAND_TIMEOUT)?;
    }
    match process_control::runtime_status()? {
        RuntimeState::Stopped => Ok(()),
        RuntimeState::Running => Err("runtime is still running after stop request".to_string()),
    }
}

fn update_runtime_state_token(runtime_was_running: bool) -> &'static str {
    if runtime_was_running {
        "running"
    } else {
        "stopped"
    }
}

fn parse_update_runtime_state(value: &std::ffi::OsStr) -> Result<bool, String> {
    match value.to_str() {
        Some("running") => Ok(true),
        Some("stopped") => Ok(false),
        _ => Err("invalid prior runtime state in update helper arguments".to_string()),
    }
}

fn runtime_state_label(state: RuntimeState) -> String {
    match state {
        RuntimeState::Running => "running".to_string(),
        RuntimeState::Stopped => "stopped".to_string(),
    }
}

fn autostart_label(status: AutoStartStatus) -> String {
    match status {
        AutoStartStatus::Disabled => "disabled".to_string(),
        AutoStartStatus::Enabled => "enabled".to_string(),
        #[cfg(not(target_os = "windows"))]
        AutoStartStatus::RequiresApproval => "requires approval".to_string(),
        AutoStartStatus::Mismatched => "mismatched registration".to_string(),
    }
}

fn path_check(path: &Path) -> String {
    if path.is_file() {
        "present".to_string()
    } else {
        format!("missing ({})", path.display())
    }
}

fn join_paths(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        "none".to_string()
    } else {
        paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_guidance_requires_human_tty_and_no_explicit_choices() {
        let options = cli::SetupOptions::default();
        assert!(setup_needs_guidance_with_terminal(
            &options,
            OutputFormat::Human,
            true,
            true
        ));
        assert!(!setup_needs_guidance_with_terminal(
            &options,
            OutputFormat::Json,
            true,
            true
        ));
        assert!(!setup_needs_guidance_with_terminal(
            &options,
            OutputFormat::Human,
            false,
            true
        ));

        let explicit = cli::SetupOptions {
            model: Some("tiny.en".to_string()),
            ..cli::SetupOptions::default()
        };
        assert!(!setup_needs_guidance_with_terminal(
            &explicit,
            OutputFormat::Human,
            true,
            true
        ));
    }

    #[test]
    fn setup_stops_running_resident_before_binary_replacement() {
        assert!(setup_preinstall_stop_required(Some(RuntimeState::Running)));
        assert!(!setup_preinstall_stop_required(Some(RuntimeState::Stopped)));
        assert!(!setup_preinstall_stop_required(None));
    }

    #[test]
    fn setup_persistence_snapshot_restores_existing_and_removes_new_files() {
        let root = std::env::temp_dir().join(format!(
            "origin-speak-setup-persistence-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let config = root.join("config.json");
        let settings = root.join("transcription_settings.json");
        fs::write(&config, b"old-config").unwrap();

        let snapshot = SetupPersistenceSnapshot::capture_from_root(&root).unwrap();
        fs::write(&config, b"new-config").unwrap();
        fs::write(&settings, b"new-settings").unwrap();
        fs::write(root.join("config.json.bak"), b"new-backup").unwrap();
        snapshot.restore().unwrap();

        assert_eq!(fs::read(&config).unwrap(), b"old-config");
        assert!(!settings.exists());
        assert!(!root.join("config.json.bak").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_helper_runtime_state_round_trips() {
        for running in [false, true] {
            let token = OsString::from(update_runtime_state_token(running));
            assert_eq!(parse_update_runtime_state(&token).unwrap(), running);
        }
        assert!(parse_update_runtime_state(std::ffi::OsStr::new("unknown")).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn update_helpers_use_exclusive_uuid_paths() {
        let root = std::env::temp_dir().join(format!(
            "origin-speak-update-helper-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("manager.exe");
        fs::write(&source, b"manager").unwrap();
        let first = create_unique_post_exit_helper(&source, "__post-exit-update", &root).unwrap();
        let second = create_unique_post_exit_helper(&source, "__post-exit-update", &root).unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read(&first).unwrap(), b"manager");
        assert_eq!(fs::read(&second).unwrap(), b"manager");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn uninstall_helper_commit_marker_is_explicit_and_round_trips() {
        let path = std::env::temp_dir().join(format!(
            "origin-speak-uninstall-commit-test-{}.marker",
            uuid::Uuid::new_v4()
        ));
        assert!(!path.exists());
        commit_post_exit_uninstall(&path).expect("create commit marker");
        wait_for_uninstall_commit(&path, Duration::from_millis(50))
            .expect("helper observes valid commit marker");
        fs::remove_file(path).unwrap();
    }
}
