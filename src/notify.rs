use std::process::Command;

/// Truncate a string to at most `max_chars` characters, appending "…" if truncated.
fn truncate(s: &str, max_chars: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_string()
    } else {
        let truncated: String = trimmed.chars().take(max_chars).collect();
        format!("{truncated}…")
    }
}

/// Extract a short snippet from the end of the reply text for notification display.
/// Takes the last non-empty lines up to `max_chars`.
fn reply_snippet(reply: &str, max_chars: usize) -> String {
    let lines: Vec<&str> = reply.lines().rev().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return String::new();
    }
    // Take lines from the end, joining them back in order.
    let mut result = String::new();
    for line in lines.iter().rev() {
        if result.is_empty() {
            result = line.trim().to_string();
        } else {
            let candidate = format!("{result} {}", line.trim());
            if candidate.chars().count() > max_chars {
                break;
            }
            result = candidate;
        }
    }
    truncate(&result, max_chars)
}

/// Send a macOS notification when a session status changes.
/// `last_reply` is an optional snippet of the agent's reply to show in the notification body.
pub fn notify_status(project_name: &str, status: &str, sound: &str, last_reply: Option<&str>) {
    let subtitle = format!("{project_name}: {status}");
    let body = last_reply
        .map(|r| reply_snippet(r, 120))
        .unwrap_or_default();

    // Prefer terminal-notifier for better Notification Center integration.
    let mut args = vec![
        "-title".to_string(),
        "chatmux".to_string(),
        "-subtitle".to_string(),
        subtitle.clone(),
        "-sound".to_string(),
        sound.to_string(),
        "-group".to_string(),
        format!("chatmux-{project_name}"),
    ];
    if !body.is_empty() {
        args.push("-message".to_string());
        args.push(body.clone());
    } else {
        args.push("-message".to_string());
        args.push(subtitle.clone());
    }

    let result = Command::new("terminal-notifier")
        .args(&args)
        .output();

    if result.is_ok() {
        return;
    }

    // Fallback to osascript.
    let display_msg = if body.is_empty() {
        subtitle
    } else {
        format!("{subtitle}\n{body}")
    };
    // Escape double quotes for AppleScript.
    let display_msg = display_msg.replace('"', r#"\""#);
    let script = format!(
        r#"display notification "{display_msg}" with title "chatmux" sound name "{sound}""#
    );
    let _ = Command::new("osascript")
        .args(["-e", &script])
        .output();
}
