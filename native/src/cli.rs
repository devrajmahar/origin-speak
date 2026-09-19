//! Command-line contract for Origin Speak management.
//!
//! Parsing and output are deliberately independent from GPUI. The future early
//! dispatch in `main.rs` can parse a management command before constructing the
//! resident UI/event loop, then route it to a small runtime/platform adapter.

use std::ffi::OsString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Human,
    Json,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub output: OutputFormat,
    pub command: Command,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Setup(SetupOptions),
    Status,
    Doctor,
    Config(ConfigAction),
    Dictionary(DictionaryAction),
    Model(ModelAction),
    Mic(MicAction),
    Hotkey(HotkeyAction),
    Autostart(ToggleAction),
    Start,
    Stop,
    Restart,
    Update(UpdateAction),
    Uninstall(UninstallOptions),
    Version,
    Help,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetupOptions {
    pub model: Option<String>,
    pub microphone: Option<String>,
    pub hotkey: Option<String>,
    pub autostart: Option<bool>,
    pub interactive: bool,
}

impl SetupOptions {
    pub fn has_explicit_choices(&self) -> bool {
        self.model.is_some()
            || self.microphone.is_some()
            || self.hotkey.is_some()
            || self.autostart.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigAction {
    List,
    Get(String),
    Set { key: String, value: String },
    Reset(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DictionaryAction {
    List,
    Add {
        word: String,
        pronunciation: Option<String>,
    },
    Update {
        id: String,
        word: String,
        pronunciation: Option<String>,
    },
    Remove(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelAction {
    List,
    Installed,
    Status,
    Select(String),
    Download(String),
    Remove(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MicAction {
    List,
    Status,
    Select(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    Show,
    Set(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleAction {
    Status,
    Enable,
    Disable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAction {
    Check,
    Apply,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UninstallOptions {
    /// False by default: models/config/runtime data are removed with the app.
    pub keep_data: bool,
    /// Required for destructive uninstall in non-interactive environments.
    pub yes: bool,
    pub dry_run: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliError {
    pub code: &'static str,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub ok: bool,
    pub code: &'static str,
    pub message: String,
    pub fields: Vec<(String, String)>,
}

impl CommandResult {
    pub fn success(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            ok: true,
            code,
            message: message.into(),
            fields: Vec::new(),
        }
    }

    pub fn field(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.push((key.into(), value.into()));
        self
    }

    pub fn render(&self, format: OutputFormat) -> String {
        match format {
            OutputFormat::Human => {
                let mut output = self.message.clone();
                for (key, value) in &self.fields {
                    output.push('\n');
                    output.push_str(key);
                    output.push_str(": ");
                    output.push_str(value);
                }
                output
            }
            OutputFormat::Json => {
                let mut fields = String::from("{");
                for (index, (key, value)) in self.fields.iter().enumerate() {
                    if index != 0 {
                        fields.push(',');
                    }
                    fields.push('"');
                    fields.push_str(&json_escape(key));
                    fields.push_str("\":\"");
                    fields.push_str(&json_escape(value));
                    fields.push('"');
                }
                fields.push('}');
                format!(
                    "{{\"ok\":{},\"code\":\"{}\",\"message\":\"{}\",\"fields\":{fields}}}",
                    self.ok,
                    json_escape(self.code),
                    json_escape(&self.message)
                )
            }
        }
    }
}

impl CliError {
    pub fn render(&self, format: OutputFormat) -> String {
        match format {
            OutputFormat::Human => format!("error: {}", self.message),
            OutputFormat::Json => format!(
                "{{\"ok\":false,\"code\":\"{}\",\"message\":\"{}\"}}",
                json_escape(self.code),
                json_escape(&self.message)
            ),
        }
    }
}

pub fn parse<I, S>(args: I) -> Result<Request, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut output = OutputFormat::Human;
    let mut raw = Vec::new();
    for value in args.into_iter().map(Into::into) {
        let value = value
            .into_string()
            .map_err(|_| error("invalid_utf8", "CLI arguments must be valid UTF-8"))?;
        if value == "--json" {
            output = OutputFormat::Json;
        } else {
            raw.push(value);
        }
    }

    if raw.first().is_some_and(|value| is_program_name(value)) {
        raw.remove(0);
    }
    let command = parse_command(&raw)?;
    Ok(Request { output, command })
}

fn is_program_name(value: &str) -> bool {
    let basename = value
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(value)
        .to_ascii_lowercase();
    matches!(basename.as_str(), "origin" | "origin.exe")
        || matches!(
            basename.as_str(),
            "origin-windows-x86_64.exe" | "origin-macos-universal"
        )
        || (basename.starts_with("origin-speak-")
            && (basename.ends_with("-windows-x86_64.exe")
                || basename.ends_with("-macos-universal")))
        // Legacy argv[0] aliases are accepted only so a renamed manager binary
        // can run setup/migration from an existing ListenOS installation path.
        || matches!(
            basename.as_str(),
            "listenos" | "listenos.exe" | "listenos-native" | "listenos-native.exe"
        )
        || (basename.starts_with("listenos-")
            && (basename.ends_with("-windows-x86_64.exe")
                || basename.ends_with("-macos-universal")))
}

pub fn usage() -> &'static str {
    "Origin Speak CLI\n\nUsage: origin [--json] <command> [options]\n\nCommands:\n  setup        Configure model, microphone, autostart, and resident runtime\n  status       Show runtime and configuration status\n  doctor       Run environment and runtime diagnostics\n  config       Read or change persisted configuration\n  dictionary   Manage speech-recognition words and pronunciations\n  model        List, install, switch, inspect, or remove local models\n  mic          List, inspect, or select microphones\n  hotkey       Show or change the dictation shortcut\n  autostart    Show, enable, or disable login launch preference\n  start        Start the resident voice host\n  stop         Stop the resident voice host\n  restart      Restart the resident voice host\n  update       Download, verify, and install the latest release\n  upgrade      Alias for 'origin update'\n  uninstall    Remove Origin Speak and user data (models included by default)\n  version      Print the installed version\n\nGlobal options:\n  --json       Emit one stable JSON object and no ANSI/progress animation\n  -h, --help   Show this help\n"
}

pub fn uninstall_requires_confirmation(
    options: UninstallOptions,
    non_interactive: bool,
) -> Result<(), CliError> {
    if options.dry_run || options.yes {
        return Ok(());
    }
    if non_interactive {
        return Err(error(
            "confirmation_required",
            "uninstall removes models, config, and runtime data by default; rerun with --yes or use --keep-data --yes",
        ));
    }
    Ok(())
}

fn parse_command(args: &[String]) -> Result<Command, CliError> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(Command::Help);
    };
    let tail = &args[1..];
    match command {
        "-h" | "--help" | "help" => Ok(Command::Help),
        "setup" => parse_setup(tail).map(Command::Setup),
        "status" => expect_empty(tail, Command::Status),
        "doctor" => expect_empty(tail, Command::Doctor),
        "config" => parse_config(tail).map(Command::Config),
        "dictionary" | "dict" => parse_dictionary(tail).map(Command::Dictionary),
        "model" => parse_model(tail).map(Command::Model),
        "mic" | "microphone" => parse_mic(tail).map(Command::Mic),
        "hotkey" => parse_hotkey(tail).map(Command::Hotkey),
        "autostart" => parse_toggle(tail).map(Command::Autostart),
        "start" => expect_empty(tail, Command::Start),
        "stop" => expect_empty(tail, Command::Stop),
        "restart" => expect_empty(tail, Command::Restart),
        "update" => parse_update(tail).map(Command::Update),
        "upgrade" => expect_empty(tail, Command::Update(UpdateAction::Apply)),
        "uninstall" => parse_uninstall(tail).map(Command::Uninstall),
        "version" | "--version" | "-V" => expect_empty(tail, Command::Version),
        other => Err(error(
            "unknown_command",
            format!("unknown command '{other}'"),
        )),
    }
}

fn parse_setup(args: &[String]) -> Result<SetupOptions, CliError> {
    let mut options = SetupOptions::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--model" => {
                options.model = Some(value_after(args, &mut index, "--model")?);
            }
            "--mic" | "--microphone" => {
                options.microphone = Some(value_after(args, &mut index, "--mic")?);
            }
            "--hotkey" => {
                options.hotkey = Some(value_after(args, &mut index, "--hotkey")?);
            }
            "--autostart" => options.autostart = Some(true),
            "--no-autostart" => options.autostart = Some(false),
            flag => {
                return Err(error(
                    "unknown_option",
                    format!("unknown setup option '{flag}'"),
                ));
            }
        }
        index += 1;
    }
    Ok(options)
}

fn parse_config(args: &[String]) -> Result<ConfigAction, CliError> {
    match args {
        [] => Ok(ConfigAction::List),
        [one] if one == "list" => Ok(ConfigAction::List),
        [op, key] if op == "get" => Ok(ConfigAction::Get(key.clone())),
        [op, key, value] if op == "set" => Ok(ConfigAction::Set {
            key: key.clone(),
            value: value.clone(),
        }),
        [op, key] if op == "reset" => Ok(ConfigAction::Reset(key.clone())),
        _ => Err(error(
            "usage",
            "usage: origin config [list|get <key>|set <key> <value>|reset <key>]",
        )),
    }
}

fn parse_dictionary(args: &[String]) -> Result<DictionaryAction, CliError> {
    match args {
        [] => Ok(DictionaryAction::List),
        [one] if one == "list" => Ok(DictionaryAction::List),
        [op, word] if op == "add" => Ok(DictionaryAction::Add {
            word: word.clone(),
            pronunciation: None,
        }),
        [op, word, pronunciation] if op == "add" => Ok(DictionaryAction::Add {
            word: word.clone(),
            pronunciation: Some(pronunciation.clone()),
        }),
        [op, id, word] if op == "update" => Ok(DictionaryAction::Update {
            id: id.clone(),
            word: word.clone(),
            pronunciation: None,
        }),
        [op, id, word, pronunciation] if op == "update" => Ok(DictionaryAction::Update {
            id: id.clone(),
            word: word.clone(),
            pronunciation: Some(pronunciation.clone()),
        }),
        [op, id] if op == "remove" || op == "delete" => Ok(DictionaryAction::Remove(id.clone())),
        _ => Err(error(
            "usage",
            "usage: origin dictionary [list|add <word> [pronunciation]|update <id> <word> [pronunciation]|remove <id>]",
        )),
    }
}

fn parse_model(args: &[String]) -> Result<ModelAction, CliError> {
    match args {
        [] => Ok(ModelAction::List),
        [one] if one == "list" => Ok(ModelAction::List),
        [one] if one == "installed" => Ok(ModelAction::Installed),
        [one] if one == "status" => Ok(ModelAction::Status),
        [op, model] if op == "select" || op == "use" || op == "switch" => {
            Ok(ModelAction::Select(model.clone()))
        }
        [op, model] if op == "download" || op == "install" => {
            Ok(ModelAction::Download(model.clone()))
        }
        [op, model] if op == "remove" || op == "delete" || op == "uninstall" => {
            Ok(ModelAction::Remove(model.clone()))
        }
        _ => Err(error(
            "usage",
            "usage: origin model [list|installed|status|use <id>|install <id>|remove <id>]",
        )),
    }
}

fn parse_mic(args: &[String]) -> Result<MicAction, CliError> {
    match args {
        [] => Ok(MicAction::List),
        [one] if one == "list" => Ok(MicAction::List),
        [one] if one == "status" => Ok(MicAction::Status),
        [op, name] if op == "select" => Ok(MicAction::Select(name.clone())),
        _ => Err(error(
            "usage",
            "usage: origin mic [list|status|select <name>]",
        )),
    }
}

fn parse_hotkey(args: &[String]) -> Result<HotkeyAction, CliError> {
    match args {
        [] => Ok(HotkeyAction::Show),
        [one] if one == "show" => Ok(HotkeyAction::Show),
        [op, chord] if op == "set" => Ok(HotkeyAction::Set(chord.clone())),
        _ => Err(error("usage", "usage: origin hotkey [show|set <chord>]")),
    }
}

fn parse_toggle(args: &[String]) -> Result<ToggleAction, CliError> {
    match args {
        [] => Ok(ToggleAction::Status),
        [one] if one == "status" => Ok(ToggleAction::Status),
        [one] if one == "enable" => Ok(ToggleAction::Enable),
        [one] if one == "disable" => Ok(ToggleAction::Disable),
        _ => Err(error(
            "usage",
            "usage: origin autostart [status|enable|disable]",
        )),
    }
}

fn parse_update(args: &[String]) -> Result<UpdateAction, CliError> {
    match args {
        [] => Ok(UpdateAction::Apply),
        [one] if one == "check" => Ok(UpdateAction::Check),
        [one] if matches!(one.as_str(), "apply" | "install" | "upgrade" | "stage") => {
            Ok(UpdateAction::Apply)
        }
        _ => Err(error("usage", "usage: origin update [check|apply]")),
    }
}

fn parse_uninstall(args: &[String]) -> Result<UninstallOptions, CliError> {
    let mut options = UninstallOptions::default();
    for arg in args {
        match arg.as_str() {
            "--keep-data" => options.keep_data = true,
            "--yes" | "-y" => options.yes = true,
            "--dry-run" => options.dry_run = true,
            flag => {
                return Err(error(
                    "unknown_option",
                    format!("unknown uninstall option '{flag}'"),
                ));
            }
        }
    }
    Ok(options)
}

fn expect_empty(args: &[String], command: Command) -> Result<Command, CliError> {
    if args.is_empty() {
        Ok(command)
    } else {
        Err(error(
            "unexpected_argument",
            format!("unexpected argument '{}'", args[0]),
        ))
    }
}

fn value_after(args: &[String], index: &mut usize, option: &str) -> Result<String, CliError> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| error("missing_value", format!("{option} requires a value")))
}

fn error(code: &'static str, message: impl Into<String>) -> CliError {
    CliError {
        code,
        message: message.into(),
    }
}

fn json_escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            ch if ch < ' ' => output.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => output.push(ch),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_json_flag_is_position_independent() {
        let request = parse(["origin", "status", "--json"]).unwrap();
        assert_eq!(request.output, OutputFormat::Json);
        assert_eq!(request.command, Command::Status);
    }

    #[test]
    fn absolute_unix_program_path_is_stripped() {
        let request = parse(["/Users/me/.local/bin/origin", "status"]).unwrap();
        assert_eq!(request.command, Command::Status);

        let request = parse(["/home/me/.local/bin/origin", "--json", "version"]).unwrap();
        assert_eq!(request.output, OutputFormat::Json);
        assert_eq!(request.command, Command::Version);
    }

    #[test]
    fn downloaded_versioned_manager_name_is_stripped() {
        let request = parse([
            r"C:\Downloads\origin-speak-0.1.21-windows-x86_64.exe",
            "setup",
            "--json",
        ])
        .unwrap();
        assert_eq!(request.output, OutputFormat::Json);
        assert!(matches!(request.command, Command::Setup(_)));
    }

    #[test]
    fn stable_release_manager_aliases_are_stripped() {
        assert_eq!(
            parse(["origin-windows-x86_64.exe", "status"])
                .unwrap()
                .command,
            Command::Status
        );
        assert_eq!(
            parse(["origin-macos-universal", "status"]).unwrap().command,
            Command::Status
        );
    }

    #[test]
    fn uninstall_deletes_data_by_default() {
        let request = parse(["origin", "uninstall", "--yes"]).unwrap();
        assert_eq!(
            request.command,
            Command::Uninstall(UninstallOptions {
                keep_data: false,
                yes: true,
                dry_run: false
            })
        );
    }

    #[test]
    fn uninstall_keep_data_must_be_explicit() {
        let request = parse(["origin", "uninstall", "--keep-data", "--yes"]).unwrap();
        assert_eq!(
            request.command,
            Command::Uninstall(UninstallOptions {
                keep_data: true,
                yes: true,
                dry_run: false
            })
        );
    }

    #[test]
    fn noninteractive_uninstall_never_prompts() {
        let options = UninstallOptions::default();
        assert_eq!(
            uninstall_requires_confirmation(options, true)
                .unwrap_err()
                .code,
            "confirmation_required"
        );
    }

    #[test]
    fn parses_management_surface() {
        assert_eq!(
            parse(["origin", "doctor"]).unwrap().command,
            Command::Doctor
        );
        assert_eq!(
            parse(["origin", "model", "download", "tiny.en"])
                .unwrap()
                .command,
            Command::Model(ModelAction::Download("tiny.en".into()))
        );
        assert_eq!(
            parse(["origin", "model", "install", "tiny.en"])
                .unwrap()
                .command,
            Command::Model(ModelAction::Download("tiny.en".into()))
        );
        assert_eq!(
            parse(["origin", "model", "use", "tiny.en"])
                .unwrap()
                .command,
            Command::Model(ModelAction::Select("tiny.en".into()))
        );
        assert_eq!(
            parse(["origin", "model", "installed"]).unwrap().command,
            Command::Model(ModelAction::Installed)
        );
        assert_eq!(
            parse(["origin", "mic", "list"]).unwrap().command,
            Command::Mic(MicAction::List)
        );
        assert_eq!(
            parse(["origin", "autostart", "enable"]).unwrap().command,
            Command::Autostart(ToggleAction::Enable)
        );
        assert_eq!(
            parse(["origin", "update", "check"]).unwrap().command,
            Command::Update(UpdateAction::Check)
        );
        assert_eq!(
            parse(["origin", "update"]).unwrap().command,
            Command::Update(UpdateAction::Apply)
        );
        assert_eq!(
            parse(["origin", "upgrade"]).unwrap().command,
            Command::Update(UpdateAction::Apply)
        );
        assert_eq!(
            parse(["origin", "dictionary", "add", "Axius", "ACK-see-us"])
                .unwrap()
                .command,
            Command::Dictionary(DictionaryAction::Add {
                word: "Axius".into(),
                pronunciation: Some("ACK-see-us".into()),
            })
        );
        assert_eq!(
            parse(["origin", "hotkey", "set", "Ctrl+Space"])
                .unwrap()
                .command,
            Command::Hotkey(HotkeyAction::Set("Ctrl+Space".into()))
        );
    }

    #[test]
    fn microphone_test_command_is_not_supported() {
        let error = parse(["origin", "mic", "test"]).unwrap_err();
        assert_eq!(error.code, "usage");
        assert!(!error.message.contains("|test"));
    }

    #[test]
    fn bare_setup_stays_noninteractive_in_parser() {
        let request = parse(["origin", "setup"]).unwrap();
        let Command::Setup(options) = request.command else {
            panic!("expected setup command");
        };
        assert!(!options.interactive);
        assert!(!options.has_explicit_choices());

        let explicit = parse(["origin", "setup", "--model", "tiny.en"])
            .unwrap()
            .command;
        let Command::Setup(options) = explicit else {
            panic!("expected setup command");
        };
        assert!(options.has_explicit_choices());
        assert!(!options.interactive);
    }

    #[test]
    fn json_output_is_single_line_and_escaped() {
        let rendered = CommandResult::success("ok", "line\n\"quoted\"")
            .field("path", "a\\b")
            .render(OutputFormat::Json);
        assert!(!rendered.contains('\n'));
        assert!(rendered.contains("line\\n\\\"quoted\\\""));
        let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON output");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["message"], "line\n\"quoted\"");
        assert_eq!(parsed["fields"]["path"], "a\\b");

        let error = CliError {
            code: "bad_input",
            message: "bad\nvalue\"".to_string(),
        }
        .render(OutputFormat::Json);
        assert!(!error.contains('\n'));
        let parsed: serde_json::Value = serde_json::from_str(&error).expect("valid error JSON");
        assert_eq!(parsed["ok"], false);
        assert_eq!(parsed["code"], "bad_input");
        assert_eq!(parsed["message"], "bad\nvalue\"");
    }
}
