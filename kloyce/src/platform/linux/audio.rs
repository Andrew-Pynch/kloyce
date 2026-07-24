use std::path::Path;
use tokio::process::{Child, Command};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Play a sound file via pw-play (fire and forget).
pub fn play_sound(path: &Path) {
    if !path.exists() {
        tracing::warn!("Sound file not found: {}", path.display());
        return;
    }
    let path = path.to_owned();
    tokio::spawn(async move {
        match Command::new("pw-play").arg(&path).spawn() {
            Ok(mut child) => {
                let _ = child.wait().await;
            }
            Err(e) => tracing::warn!("Failed to play sound: {e}"),
        }
    });
}

/// Start recording audio via pw-record. Returns the child process handle.
pub async fn start_recording(output_path: &Path) -> Result<Child> {
    let child = Command::new("pw-record")
        .arg("--rate")
        .arg("16000")
        .arg("--channels")
        .arg("1")
        .arg("--format")
        .arg("s16")
        .arg(output_path)
        .spawn()
        .map_err(|e| format!("Failed to start pw-record: {e}"))?;

    Ok(child)
}

/// Stop a recording by sending SIGINT, then waiting for clean exit.
pub async fn stop_recording(mut child: Child) -> Result<()> {
    if let Some(pid) = child.id() {
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGINT,
        )
        .map_err(|e| format!("Failed to send SIGINT to pw-record: {e}"))?;
    }

    let status = child.wait().await?;
    tracing::debug!("pw-record exited with: {status}");
    Ok(())
}
