use std::process::Stdio;
use tokio::process::Command;

/// Set clipboard content via pbcopy.
pub async fn set_clipboard(text: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut child = Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn pbcopy: {e}"))?;

    use tokio::io::AsyncWriteExt;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).await?;
    }

    let status = child.wait().await?;
    if !status.success() {
        return Err(format!("pbcopy failed: {status}").into());
    }
    Ok(())
}

/// Send a desktop notification via osascript. Progress percentage is ignored on macOS.
pub async fn notify_progress(summary: &str, body: &str, _progress_pct: Option<u32>) {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        body.replace('\\', "\\\\").replace('"', "\\\""),
        summary.replace('\\', "\\\\").replace('"', "\\\""),
    );
    let _ = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .status()
        .await;
}

/// Send text to a tmux pane via a named tmux buffer.
pub async fn tmux_send_keys(
    text: &str,
    target: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let buffer_name = tmux_buffer_name();

    let mut load = Command::new("tmux")
        .args(["load-buffer", "-b", &buffer_name, "-"])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn tmux load-buffer: {e}"))?;

    use tokio::io::AsyncWriteExt;
    if let Some(mut stdin) = load.stdin.take() {
        stdin.write_all(text.as_bytes()).await?;
    }

    let load_status = load.wait().await?;
    if !load_status.success() {
        return Err(format!("tmux load-buffer failed: {load_status}").into());
    }

    let paste_status = Command::new("tmux")
        .args([
            "paste-buffer",
            "-p",
            "-s",
            " ",
            "-b",
            &buffer_name,
            "-t",
            target,
        ])
        .status()
        .await
        .map_err(|e| format!("Failed to spawn tmux paste-buffer: {e}"));

    let _ = Command::new("tmux")
        .args(["delete-buffer", "-b", &buffer_name])
        .status()
        .await;

    let paste_status = paste_status?;
    if !paste_status.success() {
        return Err(format!("tmux paste-buffer failed: {paste_status}").into());
    }

    Ok(())
}

fn tmux_buffer_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("kloyce-{}-{nanos}", std::process::id())
}

/// Send an Enter keypress to a tmux pane (non-literal mode).
pub async fn tmux_send_enter(target: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let status = Command::new("tmux")
        .args(["send-keys", "-t", target, "Enter"])
        .status()
        .await
        .map_err(|e| format!("Failed to spawn tmux send-keys Enter: {e}"))?;

    if !status.success() {
        return Err(format!("tmux send-keys Enter failed: {status}").into());
    }
    Ok(())
}

/// No-op on macOS (Waybar is Linux/Hyprland only).
pub fn signal_waybar() {}

/// Send a desktop notification via osascript (fire and forget).
pub fn notify(summary: &str, body: &str) {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        body.replace('\\', "\\\\").replace('"', "\\\""),
        summary.replace('\\', "\\\\").replace('"', "\\\""),
    );
    tokio::spawn(async move {
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .status()
            .await;
    });
}
