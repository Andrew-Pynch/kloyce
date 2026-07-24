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
    // Try lsappinfo-based detection with PID ancestry
    if let Some((app_name, terminal_pid)) = get_active_window().await {
        if !is_terminal_app(&app_name) {
            return None;
        }

        // List tmux clients and match via PID ancestry
        if let Some(target) = match_tmux_client_by_ancestry(terminal_pid).await {
            return Some(target);
        }

        tracing::debug!(
            "PID ancestry matching failed for terminal PID {terminal_pid}, trying activity fallback"
        );
    } else {
        tracing::debug!("lsappinfo failed, trying most-recent tmux client fallback");
    }

    // Fallback: use most-recently-active tmux client
    most_recent_tmux_target().await
}

/// Match a tmux client to the focused terminal via PID ancestry.
async fn match_tmux_client_by_ancestry(terminal_pid: u32) -> Option<String> {
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
        let client_pid: u32 = parts.next()?.parse().ok()?;
        let session_name = parts.next()?;

        if is_ancestor(client_pid, terminal_pid) {
            tracing::debug!(
                "Matched tmux client PID {client_pid} (session={session_name}) to terminal PID {terminal_pid}"
            );
            return get_tmux_pane_target(session_name).await;
        }
    }

    None
}

/// Fallback: get the most-recently-active tmux client's pane target.
async fn most_recent_tmux_target() -> Option<String> {
    let output = tokio::process::Command::new("tmux")
        .args(["list-clients", "-F", "#{client_activity} #{session_name}"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // client_activity is a unix timestamp — highest = most recent
    let session_name = stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(2, ' ');
            let activity: u64 = parts.next()?.parse().ok()?;
            let session = parts.next()?;
            Some((activity, session))
        })
        .max_by_key(|(activity, _)| *activity)
        .map(|(_, session)| session.to_string())?;

    tracing::debug!("Fallback: using most-recent tmux session '{session_name}'");
    get_tmux_pane_target(&session_name).await
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
    if let Some((app_name, terminal_pid)) = get_active_window().await {
        if is_terminal_app(&app_name) {
            match resolve_terminal_context(terminal_pid).await {
                Some(tag) => Some(tag),
                None => Some("terminal".into()),
            }
        } else {
            Some(app_name.to_lowercase())
        }
    } else {
        None
    }
}

/// Get the frontmost application name and PID via lsappinfo (no Accessibility permissions needed).
async fn get_active_window() -> Option<(String, u32)> {
    // Step 1: get the ASN of the frontmost app
    let front_output = tokio::process::Command::new("lsappinfo")
        .arg("front")
        .output()
        .await
        .ok()?;

    if !front_output.status.success() {
        return None;
    }

    let asn = String::from_utf8_lossy(&front_output.stdout)
        .trim()
        .to_string();
    if asn.is_empty() {
        return None;
    }

    // Step 2: get name and PID from the ASN
    let info_output = tokio::process::Command::new("lsappinfo")
        .args(["info", "-only", "pid", "-only", "name", &asn])
        .output()
        .await
        .ok()?;

    if !info_output.status.success() {
        return None;
    }

    let info = String::from_utf8_lossy(&info_output.stdout);
    let mut name: Option<String> = None;
    let mut pid: Option<u32> = None;

    for line in info.lines() {
        let line = line.trim();
        // Parse: "LSDisplayName"="Ghostty"
        if let Some(rest) = line.strip_prefix("\"LSDisplayName\"=") {
            name = Some(rest.trim_matches('"').to_string());
        }
        // Parse: "pid"=678
        if let Some(rest) = line.strip_prefix("\"pid\"=") {
            pid = rest.parse().ok();
        }
    }

    match (name, pid) {
        (Some(n), Some(p)) => Some((n, p)),
        _ => None,
    }
}

/// Check if a process is an ancestor of another by walking the PID tree via `ps`.
fn is_ancestor(candidate_pid: u32, descendant_pid: u32) -> bool {
    let mut current = descendant_pid;
    for _ in 0..10 {
        if current == candidate_pid {
            return true;
        }
        if current <= 1 {
            return false;
        }
        match get_parent_pid(current) {
            Some(ppid) if ppid != current => current = ppid,
            _ => return false,
        }
    }
    false
}

/// Get the parent PID of a process using `ps -o ppid= -p <pid>`.
fn get_parent_pid(pid: u32) -> Option<u32> {
    let output = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

fn is_terminal_app(name: &str) -> bool {
    matches!(
        name,
        "Terminal" | "iTerm2" | "Alacritty" | "kitty" | "WezTerm" | "Hyper" | "Ghostty"
    )
}

/// Resolve terminal context using PID ancestry to match the correct tmux client.
async fn resolve_terminal_context(terminal_pid: u32) -> Option<String> {
    let output = tokio::process::Command::new("tmux")
        .args(["list-clients", "-F", "#{client_pid} #{session_name}"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Try PID ancestry matching first
    for line in stdout.lines() {
        let mut parts = line.splitn(2, ' ');
        let client_pid: u32 = match parts.next().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        let session_name = match parts.next() {
            Some(s) => s,
            None => continue,
        };

        if is_ancestor(client_pid, terminal_pid) {
            return classify_tmux_session(session_name).await;
        }
    }

    // Fallback: most-recently-active client
    let output = tokio::process::Command::new("tmux")
        .args(["list-clients", "-F", "#{client_activity} #{session_name}"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let session_name = stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(2, ' ');
            let activity: u64 = parts.next()?.parse().ok()?;
            let session = parts.next()?;
            Some((activity, session))
        })
        .max_by_key(|(activity, _)| *activity)
        .map(|(_, session)| session.to_string())?;

    classify_tmux_session(&session_name).await
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
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/unknown".into());

    if let Some(rest) = path.strip_prefix(&format!("{home}/work/")) {
        let project = rest.split('/').next().unwrap_or("unknown");
        format!("work/{project}")
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/personal/")) {
        let project = rest.split('/').next().unwrap_or("unknown");
        format!("personal/{project}")
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/Developer/")) {
        let project = rest.split('/').next().unwrap_or("unknown");
        format!("developer/{project}")
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/Projects/")) {
        let project = rest.split('/').next().unwrap_or("unknown");
        format!("projects/{project}")
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/Github/")) {
        let project = rest.split('/').next().unwrap_or("unknown");
        format!("github/{project}")
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/.config/")) {
        let project = rest.split('/').next().unwrap_or("unknown");
        format!("config/{project}")
    } else {
        std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into())
    }
}
