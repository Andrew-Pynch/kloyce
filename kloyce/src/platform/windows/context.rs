use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};

pub struct ContextCollector {
    tags: Arc<Mutex<HashSet<String>>>,
    cancel_tx: Option<oneshot::Sender<()>>,
}

impl ContextCollector {
    pub fn start(poll_interval_ms: u64) -> Self {
        let tags: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let (cancel_tx, mut cancel_rx) = oneshot::channel();

        let tags_handle = tags.clone();
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_millis(poll_interval_ms));

            loop {
                interval.tick().await;
                if cancel_rx.try_recv().is_ok() {
                    break;
                }

                if let Some(tag) = poll_active_window().await {
                    let mut set = tags_handle.lock().await;
                    if set.insert(tag.clone()) {
                        tracing::debug!("Context tag added: {tag}");
                    }
                }
            }
        });

        Self {
            tags,
            cancel_tx: Some(cancel_tx),
        }
    }

    pub async fn stop(mut self) -> Vec<String> {
        if let Some(tx) = self.cancel_tx.take() {
            let _ = tx.send(());
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let set = self.tags.lock().await;
        let mut tags: Vec<String> = set.iter().cloned().collect();
        tags.sort();
        tags
    }
}

/// Capture the tmux target of the currently-focused terminal.
/// Returns None on Windows since tmux is not commonly available.
pub async fn capture_tmux_target() -> Option<String> {
    None
}

/// PowerShell script that uses inline C# to call Win32 GetForegroundWindow
/// and returns the process name of the active window.
const ACTIVE_WINDOW_SCRIPT: &str = r#"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32Focus {
    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
}
"@
$hwnd = [Win32Focus]::GetForegroundWindow()
$pid = 0
[void][Win32Focus]::GetWindowThreadProcessId($hwnd, [ref]$pid)
if ($pid -gt 0) {
    $proc = Get-Process -Id $pid -ErrorAction SilentlyContinue
    if ($proc) {
        Write-Output $proc.ProcessName
    }
}
"#;

async fn poll_active_window() -> Option<String> {
    let output = tokio::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", ACTIVE_WINDOW_SCRIPT])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        return None;
    }

    Some(name.to_lowercase())
}
