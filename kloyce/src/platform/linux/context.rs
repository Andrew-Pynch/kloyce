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
        // Small delay to let the polling task finish
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let set = self.tags.lock().await;
        let mut tags: Vec<String> = set.iter().cloned().collect();
        tags.sort();
        tags
    }
}

/// Capture the tmux target (session:window.pane) of the currently-focused terminal.
/// Returns None if the focused window is not a terminal, or not running tmux.
pub async fn capture_tmux_target() -> Option<String> {
    let (class, alacritty_pid) = get_active_window().await?;

    if !class.eq_ignore_ascii_case("alacritty") {
        return None;
    }

    let output = tokio::process::Command::new("tmux")
        .args(["list-clients", "-F", "#{client_pid} #{session_name}"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        let mut parts = line.splitn(2, ' ');
        let client_pid = parts.next().and_then(|s| s.parse::<u32>().ok())?;
        let session_name = parts.next()?;

        if is_ancestor(client_pid, alacritty_pid) {
            tracing::debug!("Matched tmux client {client_pid} -> session {session_name:?}");
            return get_tmux_pane_target(session_name).await;
        }
    }

    None
}

async fn get_tmux_pane_target(session_name: &str) -> Option<String> {
    let output = tokio::process::Command::new("tmux")
        .args([
            "display-message",
            "-t",
            session_name,
            "-p",
            "#{session_name}:#{window_index}.#{pane_index}",
        ])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let target = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if target.is_empty() {
        return None;
    }

    tracing::debug!("Captured tmux target: {target}");
    Some(target)
}

async fn poll_active_window() -> Option<String> {
    let (class, pid) = get_active_window().await?;

    if class.eq_ignore_ascii_case("alacritty") {
        match resolve_alacritty_context(pid).await {
            Some(tag) => Some(tag),
            None => Some("terminal".into()),
        }
    } else {
        Some(class.to_lowercase())
    }
}

async fn get_active_window() -> Option<(String, u32)> {
    let output = tokio::process::Command::new("hyprctl")
        .args(["-j", "activewindow"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let class = json.get("class")?.as_str()?.to_string();
    let pid = json.get("pid")?.as_u64()? as u32;

    Some((class, pid))
}

async fn resolve_alacritty_context(alacritty_pid: u32) -> Option<String> {
    tracing::debug!("Resolving context for alacritty PID {alacritty_pid}");

    // Get all tmux clients and their sessions
    let output = tokio::process::Command::new("tmux")
        .args(["list-clients", "-F", "#{client_pid} #{session_name}"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        tracing::debug!("tmux list-clients failed, trying shell CWD fallback");
        return resolve_shell_cwd(alacritty_pid).await;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        let mut parts = line.splitn(2, ' ');
        let Some(client_pid) = parts.next().and_then(|s| s.parse::<u32>().ok()) else {
            tracing::debug!("Skipping unparseable tmux client line: {line:?}");
            continue;
        };
        let Some(session_name) = parts.next() else {
            tracing::debug!("Skipping tmux client line with no session name: {line:?}");
            continue;
        };

        if is_ancestor(client_pid, alacritty_pid) {
            tracing::debug!("Matched tmux client {client_pid} -> session {session_name:?}");
            return classify_tmux_session(session_name).await;
        }
    }

    // No tmux client matched — try reading CWD of alacritty's child shell
    tracing::debug!("No tmux client matched, trying shell CWD fallback");
    resolve_shell_cwd(alacritty_pid).await
}

/// Fallback: read the CWD of a direct child process (shell) of the given PID.
async fn resolve_shell_cwd(parent_pid: u32) -> Option<String> {
    let children_path = format!("/proc/{parent_pid}/task/{parent_pid}/children");
    let children_text = tokio::fs::read_to_string(&children_path).await.ok()?;

    for token in children_text.split_whitespace() {
        let Ok(child_pid) = token.parse::<u32>() else {
            continue;
        };

        let cwd = match tokio::fs::read_link(format!("/proc/{child_pid}/cwd")).await {
            Ok(p) => p,
            Err(_) => continue,
        };

        let path = cwd.to_string_lossy().to_string();
        if path == "/" || path.starts_with("/usr") {
            continue;
        }

        tracing::debug!("Shell CWD fallback: PID {child_pid} -> {path}");
        return Some(classify_path(&path));
    }

    tracing::debug!("All context resolution failed for PID {parent_pid}");
    None
}

/// Walk /proc/<pid>/stat PPid chain to check if `ancestor_pid` is an ancestor of `pid`.
fn is_ancestor(mut pid: u32, ancestor_pid: u32) -> bool {
    for _ in 0..10 {
        if pid == ancestor_pid {
            return true;
        }
        if pid <= 1 {
            return false;
        }
        let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(s) => s,
            Err(_) => return false,
        };
        // Field 2 is comm in parens, which can contain spaces like "(tmux: client)"
        // Find the last ')' to safely skip it
        let close_paren = match stat.rfind(')') {
            Some(i) => i,
            None => return false,
        };
        let fields: Vec<&str> = stat[close_paren + 2..].split_whitespace().collect();
        // Field 0 after ')' is state, field 1 is PPid
        let ppid: u32 = match fields.get(1).and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => return false,
        };
        pid = ppid;
    }
    false
}

async fn classify_tmux_session(session_name: &str) -> Option<String> {
    let output = tokio::process::Command::new("tmux")
        .args([
            "display-message",
            "-t",
            session_name,
            "-p",
            "#{pane_current_path}",
        ])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        return None;
    }

    Some(classify_path(&path))
}

fn classify_path(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();

    if let Some(rest) = path.strip_prefix(&format!("{home}/work/")) {
        let project = rest.split('/').next().unwrap_or("unknown");
        format!("work/{project}")
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/personal/")) {
        let project = rest.split('/').next().unwrap_or("unknown");
        format!("personal/{project}")
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/Github/")) {
        let project = rest.split('/').next().unwrap_or("unknown");
        format!("github/{project}")
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/projects/")) {
        let project = rest.split('/').next().unwrap_or("unknown");
        format!("projects/{project}")
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/.config/")) {
        let project = rest.split('/').next().unwrap_or("unknown");
        format!("config/{project}")
    } else {
        // Fall back to last path component
        std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into())
    }
}
