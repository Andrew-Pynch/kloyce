use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(runtime_dir).join("kloyce.sock")
}

pub async fn send_command(
    command: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let path = socket_path();
    let stream = tokio::net::UnixStream::connect(&path).await?;
    let (reader, mut writer) = stream.into_split();

    let json = format!("{{\"command\":\"{command}\"}}\n");
    writer.write_all(json.as_bytes()).await?;

    let mut reader = BufReader::new(reader);
    let mut response = String::new();
    reader.read_line(&mut response).await?;

    Ok(response)
}

pub async fn send_toggle() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    send_command("toggle").await
}

pub async fn send_toggle_enter() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    send_command("toggle_enter").await
}
