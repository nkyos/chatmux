use std::process::Command;

/// Send a macOS notification when Claude responds.
pub fn notify_replied(project_name: &str) {
    let message = format!("{project_name}: Claude responded");

    // Prefer terminal-notifier for better Notification Center integration.
    let result = Command::new("terminal-notifier")
        .args([
            "-title",
            "chatmux",
            "-message",
            &message,
            "-sound",
            "default",
            "-group",
            &format!("chatmux-{project_name}"),
        ])
        .output();

    if result.is_ok() {
        return;
    }

    // Fallback to osascript.
    let script = format!(
        r#"display notification "{message}" with title "chatmux" sound name "default""#
    );
    let _ = Command::new("osascript")
        .args(["-e", &script])
        .output();
}
