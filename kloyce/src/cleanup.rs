use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const CLEANUP_PROMPT: &str = "\
You are a speech-to-text post-processor. You will receive a raw transcript from Whisper. \
Fix any words that are contextually wrong due to speech recognition errors \
(homophones, misheard words, wrong word boundaries). \
Do NOT rewrite the style, add punctuation that wasn't implied, \
add markdown formatting, or change the meaning. \
Output ONLY the corrected transcript text with no preamble, explanation, or markdown.";

/// Run the raw transcript through `claude -p` for contextual cleanup.
/// Returns the cleaned text, or an error if anything goes wrong.
pub async fn cleanup_transcript(
    raw_text: &str,
    claude_bin: &Path,
    timeout_secs: u64,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    tracing::debug!("Starting Claude cleanup pass");

    let mut child = Command::new(claude_bin)
        .arg("-p")
        .arg(CLEANUP_PROMPT)
        .arg("--output-format")
        .arg("json")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn claude: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(raw_text.as_bytes()).await?;
    }

    let output = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
        .await
        .map_err(|_| format!("Claude cleanup timed out after {timeout_secs}s"))??;

    if !output.status.success() {
        return Err(format!("claude exited with status: {}", output.status).into());
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse claude JSON output: {e}"))?;

    let result = json
        .get("result")
        .and_then(|v| v.as_str())
        .ok_or("claude JSON output missing 'result' field")?
        .trim()
        .to_string();

    if result.is_empty() {
        return Err("Claude cleanup returned empty result".into());
    }

    tracing::debug!("Claude cleanup complete ({} chars)", result.len());
    Ok(result)
}
