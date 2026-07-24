use std::process::Stdio;
use tokio::process::Command;

/// Set clipboard content via PowerShell Set-Clipboard.
pub async fn set_clipboard(text: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut child = Command::new("powershell")
        .args(["-NoProfile", "-Command", "$input | Set-Clipboard"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn powershell Set-Clipboard: {e}"))?;

    use tokio::io::AsyncWriteExt;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).await?;
    }

    let status = child.wait().await?;
    if !status.success() {
        return Err(format!("PowerShell Set-Clipboard failed: {status}").into());
    }
    Ok(())
}

/// PowerShell script for Windows toast notifications using WinRT APIs.
const TOAST_SCRIPT: &str = r#"
param([string]$Title, [string]$Body)
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom, ContentType = WindowsRuntime] > $null
$template = @"
<toast>
  <visual>
    <binding template="ToastGeneric">
      <text>$Title</text>
      <text>$Body</text>
    </binding>
  </visual>
</toast>
"@
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml($template)
$toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier("Kloyce").Show($toast)
"#;

/// Send a replaceable desktop notification with optional progress percentage.
/// Windows toast notifications don't support progress bars natively,
/// so the percentage is included in the body text.
pub async fn notify_progress(summary: &str, body: &str, progress_pct: Option<u32>) {
    let full_body = match progress_pct {
        Some(pct) => format!("{body} ({pct}%)"),
        None => body.to_string(),
    };

    let _ = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            TOAST_SCRIPT,
            "-Title",
            summary,
            "-Body",
            &full_body,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

/// Send text to a tmux pane via a named tmux buffer.
/// tmux may be available on Windows via Git Bash, MSYS2, or WSL.
/// If not, this will fail and the daemon falls back to clipboard.
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

/// No-op on Windows (Waybar is Linux/Hyprland only).
pub fn signal_waybar() {}

/// Send a desktop notification (fire and forget).
pub fn notify(summary: &str, body: &str) {
    let summary = summary.to_string();
    let body = body.to_string();
    tokio::spawn(async move {
        let _ = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                TOAST_SCRIPT,
                "-Title",
                &summary,
                "-Body",
                &body,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    });
}
