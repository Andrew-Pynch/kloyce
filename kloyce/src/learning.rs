use crate::dictionary::{Correction, Dictionary};
use std::path::Path;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::RwLock;

const LEARNING_PROMPT: &str = "\
You are a speech-to-text dictionary trainer. You receive a raw Whisper transcript \
and the user's current correction dictionary. Your job is to identify words or phrases \
that are likely speech recognition errors (homophones, misheard proper nouns, wrong word boundaries).

Rules:
- Only suggest corrections you are confident about
- Do NOT include entries that already exist in the dictionary
- Do NOT suggest style changes, punctuation, or rewording
- Focus on proper nouns, technical terms, and words that sound similar but are wrong
- The \"wrong\" field should be lowercase (how whisper would produce it)
- The \"correct\" field should have proper casing

Output ONLY a JSON object with this exact format:
{\"corrections\": [{\"wrong\": \"misheard text\", \"correct\": \"Correct Text\"}]}
If no corrections are needed, output: {\"corrections\": []}";

/// Run the learning agent in the background. Analyzes a transcript and updates
/// the dictionary with newly discovered corrections.
///
/// This function never returns errors to the caller - all failures are logged and swallowed.
pub async fn learn_from_transcript(
    raw_text: &str,
    context_tags: &[String],
    dictionary: Arc<RwLock<Dictionary>>,
    claude_bin: &Path,
    max_entries: usize,
) {
    // Skip very short transcripts - not enough signal to learn from
    if raw_text.split_whitespace().count() < 5 {
        tracing::debug!(
            "Skipping learning for short transcript ({} words)",
            raw_text.split_whitespace().count()
        );
        return;
    }

    let dict_summary = {
        let dict = dictionary.read().await;
        dict.summarize_for_prompt()
    };

    let user_message = format!(
        "Current dictionary entries:\n{}\n\nContext tags for this recording: {}\n\nRaw Whisper transcript:\n{}",
        dict_summary,
        if context_tags.is_empty() { "(none)".to_string() } else { context_tags.join(", ") },
        raw_text,
    );

    tracing::debug!("Starting dictionary learning from transcript");

    let result = run_claude(claude_bin, &user_message).await;

    match result {
        Ok(corrections) if corrections.is_empty() => {
            tracing::debug!("Learning agent found no new corrections");
        }
        Ok(corrections) => {
            let count = corrections.len();
            let mut dict = dictionary.write().await;
            let added = dict.merge_corrections(corrections, max_entries);
            if added > 0 {
                if let Err(e) = dict.save() {
                    tracing::error!("Failed to save dictionary after learning: {e}");
                }
            }
            tracing::info!(
                "Learning agent proposed {count} corrections, {added} new entries added"
            );
        }
        Err(e) => {
            tracing::warn!("Dictionary learning failed: {e}");
        }
    }
}

async fn run_claude(
    claude_bin: &Path,
    user_message: &str,
) -> Result<Vec<Correction>, Box<dyn std::error::Error + Send + Sync>> {
    let mut child = Command::new(claude_bin)
        .arg("-p")
        .arg(LEARNING_PROMPT)
        .arg("--output-format")
        .arg("json")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn claude for learning: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(user_message.as_bytes()).await?;
    }

    let output = tokio::time::timeout(std::time::Duration::from_secs(60), child.wait_with_output())
        .await
        .map_err(|_| "Claude learning timed out after 60s")??;

    if !output.status.success() {
        return Err(format!("claude exited with status: {}", output.status).into());
    }

    // claude -p --output-format json wraps output in {"result": "..."}
    let outer: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse claude JSON output: {e}"))?;

    let result_str = outer
        .get("result")
        .and_then(|v| v.as_str())
        .ok_or("claude JSON output missing 'result' field")?;

    // The result field contains the model's text response, which should be our JSON
    let inner: serde_json::Value = serde_json::from_str(result_str)
        .map_err(|e| format!("Failed to parse learning response JSON: {e}"))?;

    let corrections_arr = inner
        .get("corrections")
        .and_then(|v| v.as_array())
        .ok_or("Learning response missing 'corrections' array")?;

    let mut corrections = Vec::new();
    for item in corrections_arr {
        let wrong = item.get("wrong").and_then(|v| v.as_str()).unwrap_or("");
        let correct = item.get("correct").and_then(|v| v.as_str()).unwrap_or("");
        let context = item
            .get("context")
            .and_then(|v| v.as_str())
            .map(String::from);

        if !wrong.is_empty() && !correct.is_empty() {
            corrections.push(Correction {
                wrong: wrong.to_string(),
                correct: correct.to_string(),
                context,
            });
        }
    }

    Ok(corrections)
}
