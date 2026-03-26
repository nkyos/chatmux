use std::process::Command;

/// Send a macOS notification when a session status changes.
pub fn notify_status(project_name: &str, status: &str, sound: &str) {
    let message = format!("{project_name}: {status}");

    // Prefer terminal-notifier for better Notification Center integration.
    let result = Command::new("terminal-notifier")
        .args([
            "-title",
            "chatmux",
            "-message",
            &message,
            "-sound",
            sound,
            "-group",
            &format!("chatmux-{project_name}"),
        ])
        .output();

    if result.is_ok() {
        return;
    }

    // Fallback to osascript.
    let script = format!(
        r#"display notification "{message}" with title "chatmux" sound name "{sound}""#
    );
    let _ = Command::new("osascript")
        .args(["-e", &script])
        .output();
}
