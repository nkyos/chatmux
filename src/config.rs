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

    /// Get the editor command, checking config then $EDITOR then fallback.
    pub fn editor_command(&self) -> String {
        self.editor
            .command
            .clone()
            .or_else(|| std::env::var("EDITOR").ok())
            .unwrap_or_else(|| "code".into())
    }
}

fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("chatmux").join("config.toml"))
}
