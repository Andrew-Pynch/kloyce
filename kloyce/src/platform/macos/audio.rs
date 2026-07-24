use std::path::Path;
use tokio::process::{Child, Command};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Play a sound file via afplay (fire and forget).
pub fn play_sound(path: &Path) {
    if !path.exists() {
        tracing::warn!("Sound file not found: {}", path.display());
        return;
    }
    let path = path.to_owned();
    tracing::debug!("Playing sound: {}", path.display());
    tokio::spawn(async move {
        match Command::new("afplay").arg(&path).spawn() {
            Ok(mut child) => match child.wait().await {
                Ok(status) if !status.success() => {
                    tracing::warn!("afplay exited with: {status}");
                }
                Err(e) => tracing::warn!("afplay wait failed: {e}"),
                _ => {}
            },
            Err(e) => tracing::warn!("Failed to play sound: {e}"),
        }
    });
}

/// Start recording audio via sox's `rec` command. Returns the child process handle.
/// Records mono 16-bit signed WAV, resampled to 16kHz for whisper.cpp.
/// macOS CoreAudio typically can't capture at 16kHz natively, so we record at the
/// device's native rate and use sox's `rate` effect to resample the output.
pub async fn start_recording(output_path: &Path) -> Result<Child> {
    let child = Command::new("rec")
        .arg("-c")
        .arg("1")
        .arg("-b")
        .arg("16")
        .arg("-e")
        .arg("signed-integer")
        .arg(output_path)
        .arg("rate")
        .arg("16000")
        .spawn()
        .map_err(|e| format!("Failed to start rec (sox): {e}"))?;

    Ok(child)
}

/// Stop a recording by sending SIGINT, then waiting for clean exit.
pub async fn stop_recording(mut child: Child) -> Result<()> {
    if let Some(pid) = child.id() {
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGINT,
        )
        .map_err(|e| format!("Failed to send SIGINT to rec: {e}"))?;
    }

    let status = child.wait().await?;
    tracing::debug!("rec exited with: {status}");
    Ok(())
}
