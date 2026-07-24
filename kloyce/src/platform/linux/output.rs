use std::process::Stdio;
use tokio::process::Command;

/// Set clipboard content via wl-copy.
pub async fn set_clipboard(text: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut child = Command::new("wl-copy")
        .arg("--trim-newline")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn wl-copy: {e}"))?;

    use tokio::io::AsyncWriteExt;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).await?;
    }

    let status = child.wait().await?;
    if !status.success() {
        return Err(format!("wl-copy failed: {status}").into());
    }
    Ok(())
}

/// Notification replace ID so progress updates replace the previous notification in-place.
const NOTIFY_REPLACE_ID: &str = "91827";

/// Send a replaceable desktop notification with optional progress bar hint.
pub async fn notify_progress(summary: &str, body: &str, progress_pct: Option<u32>) {
    let mut cmd = Command::new("notify-send");
    cmd.arg("-a").arg("kloyce").arg("-r").arg(NOTIFY_REPLACE_ID);
    if let Some(pct) = progress_pct {
        cmd.arg("-h").arg(format!("int:value:{}", pct.min(100)));
    }
    cmd.arg(summary).arg(body);
    let _ = cmd.status().await;
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
        .args(paste_buffer_args(&buffer_name, target))
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

/// Build the `tmux paste-buffer` argument list. `-p` selects bracketed-paste
/// mode for applications that support it, while `-s " "` prevents tmux from
/// translating buffer linefeeds into carriage returns (literal Enter keys).
fn paste_buffer_args<'a>(buffer_name: &'a str, target: &'a str) -> [&'a str; 8] {
    [
        "paste-buffer",
        "-p",
        "-s",
        " ",
        "-b",
        buffer_name,
        "-t",
        target,
    ]
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

/// Signal Waybar to refresh the kloyce indicator module.
pub fn signal_waybar() {
    tokio::spawn(async {
        let _ = Command::new("pkill")
            .args(["-RTMIN+11", "waybar"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    });
}

/// Send a desktop notification.
pub fn notify(summary: &str, body: &str) {
    let summary = summary.to_string();
    let body = body.to_string();
    tokio::spawn(async move {
        let _ = Command::new("notify-send")
            .arg("-a")
            .arg("kloyce")
            .arg(&summary)
            .arg(&body)
            .status()
            .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_buffer_args_uses_bracketed_paste_flag() {
        let args = paste_buffer_args("kloyce-1234-56789", "session:0.0");

        assert_eq!(
            args,
            [
                "paste-buffer",
                "-p",
                "-s",
                " ",
                "-b",
                "kloyce-1234-56789",
                "-t",
                "session:0.0"
            ]
        );
        assert!(
            args.windows(2).any(|window| window == ["-s", " "]),
            "paste-buffer must replace embedded linefeeds with spaces so tmux \
             does not turn transcript newlines into raw Enter keypresses"
        );
    }
}
