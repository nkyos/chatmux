use ratatui::style::Color;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub editor: EditorConfig,
    pub notifications: NotificationConfig,
    pub display: DisplayConfig,
    pub theme: ThemeConfig,
    pub upgrade: UpgradeConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct UpgradeConfig {
    pub claude_code: String,
    pub codex: String,
}

impl Default for UpgradeConfig {
    fn default() -> Self {
        Self {
            claude_code: "brew upgrade claude-code".into(),
            codex: "brew upgrade codex".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct EditorConfig {
    /// Command to open the editor. Defaults to $EDITOR or "code".
    pub command: Option<String>,
}


#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    /// Enable macOS notifications. Defaults to true.
    pub enabled: bool,
    /// Which statuses trigger notifications. Defaults to ["replied"].
    pub statuses: Vec<String>,
    /// Notification sound name. Defaults to "default".
    pub sound: String,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            statuses: vec!["replied".into()],
            sound: "default".into(),
        }
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

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub border_focused: String,
    pub border_unfocused: String,
    pub selected_fg: String,
    pub status_working: String,
    pub status_replied: String,
    pub status_read: String,
    pub status_input: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            border_focused: "cyan".into(),
            border_unfocused: "darkgray".into(),
            selected_fg: "cyan".into(),
            status_working: "blue".into(),
            status_replied: "red".into(),
            status_read: "green".into(),
            status_input: "yellow".into(),
        }
    }
}

/// Resolved theme with parsed Color values for efficient rendering.
pub struct ResolvedTheme {
    pub border_focused: Color,
    pub border_unfocused: Color,
    pub selected_fg: Color,
    pub status_working: Color,
    pub status_replied: Color,
    pub status_read: Color,
    pub status_input: Color,
}

impl ResolvedTheme {
    pub fn from_config(theme: &ThemeConfig) -> Self {
        Self {
            border_focused: parse_color(&theme.border_focused),
            border_unfocused: parse_color(&theme.border_unfocused),
            selected_fg: parse_color(&theme.selected_fg),
            status_working: parse_color(&theme.status_working),
            status_replied: parse_color(&theme.status_replied),
            status_read: parse_color(&theme.status_read),
            status_input: parse_color(&theme.status_input),
        }
    }
}

pub fn parse_color(s: &str) -> Color {
    match s.to_lowercase().as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        "white" => Color::White,
        hex if hex.starts_with('#') && hex.len() == 7 => {
            let r = u8::from_str_radix(&hex[1..3], 16).unwrap_or(255);
            let g = u8::from_str_radix(&hex[3..5], 16).unwrap_or(255);
            let b = u8::from_str_radix(&hex[5..7], 16).unwrap_or(255);
            Color::Rgb(r, g, b)
        }
        _ => Color::White,
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

        match toml::from_str(&content) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("chatmux: failed to parse config: {e}");
                Self::default()
            }
        }
    }

    /// Get the editor command split into program and arguments.
    pub fn editor_command_parts(&self) -> (String, Vec<String>) {
        let raw = self.editor_command_raw();
        match shell_words::split(&raw) {
            Ok(parts) if !parts.is_empty() => {
                let mut iter = parts.into_iter();
                (iter.next().unwrap(), iter.collect())
            }
            _ => (raw, vec![]),
        }
    }

    /// Get the raw editor command string.
    /// Priority: config > $VISUAL > $EDITOR > first found CLI (cursor, code) > "open" (Finder).
    fn editor_command_raw(&self) -> String {
        if let Some(ref cmd) = self.editor.command {
            return cmd.clone();
        }
        if let Ok(v) = std::env::var("VISUAL")
            && !v.is_empty() {
                return v;
            }
        if let Ok(v) = std::env::var("EDITOR")
            && !v.is_empty() {
                return v;
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
