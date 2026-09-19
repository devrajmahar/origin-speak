use crate::platform_shell::AutoStartState;
use std::path::{Path, PathBuf};

const AUTOSTART_FILE_NAME: &str = "origin-speak.desktop";

pub fn auto_start_supported() -> bool {
    autostart_path().is_ok()
}

pub fn auto_start_status() -> Result<AutoStartState, String> {
    let path = autostart_path()?;
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => Ok(AutoStartState::Enabled),
        Ok(_) => Err(format!(
            "Origin Speak autostart path is not a regular file: {}",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(AutoStartState::Disabled),
        Err(error) => Err(format!(
            "inspect Origin Speak autostart entry {}: {error}",
            path.display()
        )),
    }
}

pub fn set_auto_start_enabled(enabled: bool) -> Result<AutoStartState, String> {
    let path = autostart_path()?;
    if !enabled {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "remove Origin Speak autostart entry {}: {error}",
                    path.display()
                ));
            }
        }
        return Ok(AutoStartState::Disabled);
    }

    let parent = path
        .parent()
        .ok_or_else(|| "Origin Speak autostart path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "create XDG autostart directory {}: {error}",
            parent.display()
        )
    })?;

    let executable = std::env::current_exe()
        .map_err(|error| format!("resolve current Origin Speak runtime executable: {error}"))?;
    let entry = desktop_entry(&executable)?;
    let temporary = parent.join(format!(".{AUTOSTART_FILE_NAME}.tmp-{}", std::process::id()));
    std::fs::write(&temporary, entry).map_err(|error| {
        format!(
            "write temporary Origin Speak autostart entry {}: {error}",
            temporary.display()
        )
    })?;
    if let Err(error) = std::fs::rename(&temporary, &path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!(
            "activate Origin Speak autostart entry {}: {error}",
            path.display()
        ));
    }
    Ok(AutoStartState::Enabled)
}

fn autostart_path() -> Result<PathBuf, String> {
    let config_home = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(value) if Path::new(&value).is_absolute() => PathBuf::from(value),
        _ => {
            let home = std::env::var_os("HOME")
                .ok_or_else(|| "HOME is unavailable for XDG autostart".to_string())?;
            PathBuf::from(home).join(".config")
        }
    };
    Ok(config_home.join("autostart").join(AUTOSTART_FILE_NAME))
}

fn desktop_entry(executable: &Path) -> Result<String, String> {
    let executable = executable
        .to_str()
        .ok_or_else(|| "Origin Speak runtime executable path is not valid UTF-8".to_string())?;
    if executable.contains(['\n', '\r', '\0']) {
        return Err(
            "Origin Speak runtime executable path contains an unsupported character".to_string(),
        );
    }
    let mut escaped = String::with_capacity(executable.len() + 2);
    escaped.push('"');
    for character in executable.chars() {
        if matches!(character, '\\' | '"' | '`' | '$') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('"');

    Ok(format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Origin Speak\nComment=Resident voice-to-text runtime\nExec={escaped}\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_quotes_runtime_without_shell() {
        let entry =
            desktop_entry(Path::new("/home/user/My Apps/origin-runtime")).expect("desktop entry");
        assert!(entry.contains("Exec=\"/home/user/My Apps/origin-runtime\"\n"));
        assert!(entry.contains("Terminal=false\n"));
        assert!(!entry.contains("sh -c"));
    }
}
