use std::path::Path;
use std::process::Stdio;
use tokio::process::{Child, Command};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Play a sound file via PowerShell SoundPlayer (fire and forget).
/// Falls back to ffplay for non-wav formats.
pub fn play_sound(path: &Path) {
    if !path.exists() {
        tracing::warn!("Sound file not found: {}", path.display());
        return;
    }
    let path = path.to_owned();
    tokio::spawn(async move {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let result = if ext.eq_ignore_ascii_case("wav") {
            // PowerShell SoundPlayer handles .wav natively
            let ps_cmd = format!(
                "(New-Object Media.SoundPlayer '{}').PlaySync()",
                path.display()
            );
            Command::new("powershell")
                .args(["-NoProfile", "-Command", &ps_cmd])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
        } else {
            // Use ffplay for other formats (.ogg, .oga, .mp3, etc.)
            Command::new("ffplay")
                .args(["-nodisp", "-autoexit"])
                .arg(&path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
        };

        if let Err(e) = result {
            tracing::warn!("Failed to play sound: {e}");
        }
    });
}

/// Start recording audio via ffmpeg with DirectShow input. Returns the child process handle.
/// The child is spawned with stdin piped so we can send 'q' to stop gracefully.
pub async fn start_recording(output_path: &Path) -> Result<Child> {
    let child = Command::new("ffmpeg")
        .args([
            "-y", // overwrite output
            "-f",
            "dshow", // DirectShow input
            "-i",
            "audio=default",
            "-ar",
            "16000", // 16kHz sample rate
            "-ac",
            "1", // mono
            "-acodec",
            "pcm_s16le",
        ])
        .arg(output_path)
        .stdin(Stdio::piped()) // needed for graceful stop via 'q'
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to start ffmpeg recording: {e}"))?;

    Ok(child)
}

/// Stop a recording by sending 'q' to ffmpeg's stdin for a graceful exit.
/// Windows does not support SIGINT for child processes, so we use ffmpeg's
/// built-in quit command instead.
pub async fn stop_recording(mut child: Child) -> Result<()> {
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(b"q").await;
        drop(stdin);
    }

    let status = child.wait().await?;
    tracing::debug!("ffmpeg exited with: {status}");
    Ok(())
}
