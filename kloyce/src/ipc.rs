use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Toggle,
    ToggleEnter,
    CopyPlusLatest,
    Status,
    Cancel,
}

#[derive(Debug, Serialize, Clone)]
pub struct Response {
    pub status: &'static str,
    pub state: String,
    pub message: String,
}

#[cfg(unix)]
pub fn socket_path() -> std::path::PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    std::path::PathBuf::from(runtime_dir).join("kloyce.sock")
}

#[cfg(windows)]
pub const IPC_PORT: u16 = 19876;

/// Handle a single IPC connection. Works with any async stream (Unix socket or TCP).
async fn handle_connection<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    cmd_tx: mpsc::Sender<(Command, tokio::sync::oneshot::Sender<Response>)>,
) {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }

        match serde_json::from_str::<Command>(trimmed) {
            Ok(cmd) => {
                let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
                if cmd_tx.send((cmd, resp_tx)).await.is_err() {
                    break;
                }
                if let Ok(resp) = resp_rx.await {
                    let json = serde_json::to_string(&resp)
                        .unwrap_or_else(|_| r#"{"status":"error"}"#.into());
                    let _ = writer.write_all(format!("{json}\n").as_bytes()).await;
                }
            }
            Err(e) => {
                let resp = Response {
                    status: "error",
                    state: String::new(),
                    message: format!("Invalid command: {e}"),
                };
                let json = serde_json::to_string(&resp).unwrap_or_default();
                let _ = writer.write_all(format!("{json}\n").as_bytes()).await;
            }
        }

        line.clear();
    }
}

/// Start the IPC server. Returns a receiver for incoming commands.
/// Each command is paired with a oneshot sender for the response.
pub async fn start_server(
    cmd_tx: mpsc::Sender<(Command, tokio::sync::oneshot::Sender<Response>)>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(unix)]
    {
        use tokio::net::UnixListener;

        let path = socket_path();

        // Remove stale socket
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }

        let listener = UnixListener::bind(&path)?;
        tracing::info!("IPC listening on: {}", path.display());

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let cmd_tx = cmd_tx.clone();
                        tokio::spawn(handle_connection(stream, cmd_tx));
                    }
                    Err(e) => {
                        tracing::error!("IPC accept error: {e}");
                    }
                }
            }
        });
    }

    #[cfg(windows)]
    {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", IPC_PORT)).await?;
        tracing::info!("IPC listening on: 127.0.0.1:{}", IPC_PORT);

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let cmd_tx = cmd_tx.clone();
                        tokio::spawn(handle_connection(stream, cmd_tx));
                    }
                    Err(e) => {
                        tracing::error!("IPC accept error: {e}");
                    }
                }
            }
        });
    }

    Ok(())
}
