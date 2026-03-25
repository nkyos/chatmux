use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub editor: EditorConfig,
    pub notifications: NotificationConfig,
    pub display: DisplayConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    /// Command to open the editor. Defaults to $EDITOR or "code".
    pub command: Option<String>,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self { command: None }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    /// Enable macOS notifications. Defaults to true.
    pub enabled: bool,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    /// Sidebar width in columns. Defaults to 35.
    pub sidebar_width: u16,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self { sidebar_width: 35 }
    }
}

impl Config {
    /// Load config from ~/.config/chatmux/config.toml, falling back to defaults.
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };

        let Ok(content) = fs::read_to_string(&path) else {
            return Self::default();
        };

        toml::from_str(&content).unwrap_or_default()
    }

    /// Get the editor command.
    /// Priority: config > $VISUAL > $EDITOR > first found CLI (cursor, code) > "open" (Finder).
    pub fn editor_command(&self) -> String {
        if let Some(ref cmd) = self.editor.command {
            return cmd.clone();
        }
        if let Ok(v) = std::env::var("VISUAL") {
            if !v.is_empty() {
                return v;
            }
        }
        if let Ok(v) = std::env::var("EDITOR") {
            if !v.is_empty() {
                return v;
            }
        }
        // Auto-detect installed GUI editors.
        for candidate in &["cursor", "code", "zed"] {
            if command_exists(candidate) {
                return (*candidate).to_string();
            }
        }
        // Last resort: macOS open (opens directory in Finder).
        "open".into()
    }
}

fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("chatmux").join("config.toml"))
}

fn command_exists(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
