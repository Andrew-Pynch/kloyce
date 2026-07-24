// v1: one-shot (reloads model each call). Follow-up: warm llama-server for sub-second latency.

use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

const LOCAL_CLEANUP_PROMPT: &str = "\
You are a speech-to-text post-processor. You will receive a raw transcript from Whisper. \
Fix any words that are contextually wrong due to speech recognition errors \
(homophones, misheard words, wrong word boundaries). \
Do NOT rewrite the style, add punctuation that wasn't implied, \
add markdown formatting, or change the meaning. \
Output ONLY the corrected transcript text with no preamble, explanation, or markdown.";

/// Wrap system + user text in the Qwen2.5 ChatML template. `llama-completion`
/// does raw completion (no chat template), so an instruct model needs the
/// template applied manually or it follows instructions poorly.
fn build_chatml_prompt(system: &str, user: &str) -> String {
    format!(
        "<|im_start|>system\n{system}<|im_end|>\n\
         <|im_start|>user\n{user}<|im_end|>\n\
         <|im_start|>assistant\n"
    )
}

/// Known end-of-generation markers that llama.cpp may append to its output.
/// `> EOF by user` is the trailer `llama-completion` prints when stdin closes.
const EOS_MARKERS: &[&str] = &[
    "<|eot_id|>",
    "</s>",
    "[end of text]",
    "<|endoftext|>",
    "> EOF by user",
];

/// Run the raw transcript through a local `llama-completion` instance for contextual cleanup.
///
/// Shells out to `llama-completion` in one-shot mode (`--no-display-prompt`),
/// captures stdout, strips EOS markers, and returns the trimmed result.
///
/// # Arguments
///
/// * `raw_text`     – the transcript text to clean up
/// * `llama_bin`    – path to the `llama-cli` binary (e.g. `~/.local/bin/llama-cli`)
/// * `model_path`   – path to the GGUF model file
/// * `timeout_secs` – maximum wall-clock seconds to wait for the subprocess
///
/// # Errors
///
/// Returns an error if:
/// - `llama_bin` or `model_path` do not exist on disk
/// - the subprocess fails to spawn or exits with a non-zero status
/// - the call exceeds `timeout_secs`
/// - the output is not valid UTF-8
/// - the cleaned result is empty after stripping markers and whitespace
pub async fn cleanup_local(
    raw_text: &str,
    llama_bin: &Path,
    model_path: &Path,
    timeout_secs: u64,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if !llama_bin.exists() {
        return Err(format!(
            "local LLM binary not found at '{}': \
             install llama.cpp (llama-completion) and set local_llm_bin in config",
            llama_bin.display()
        )
        .into());
    }
    if !model_path.exists() {
        return Err(format!(
            "local LLM model not found at '{}': \
             download a GGUF model and set local_llm_model_path in config",
            model_path.display()
        )
        .into());
    }

    let full_prompt = build_chatml_prompt(LOCAL_CLEANUP_PROMPT, raw_text);

    tracing::debug!("Starting local llama-completion cleanup pass");

    let child = Command::new(llama_bin)
        .arg("-m")
        .arg(model_path)
        .arg("--no-display-prompt")
        .arg("-n")
        .arg("256")
        .arg("-p")
        .arg(&full_prompt)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn local LLM ({}): {e}", llama_bin.display()))?;

    let output = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
        .await
        .map_err(|_| format!("local LLM cleanup timed out after {timeout_secs}s"))??;

    if !output.status.success() {
        return Err(format!("local LLM exited with status: {}", output.status).into());
    }

    let raw_stdout = std::str::from_utf8(&output.stdout)
        .map_err(|e| format!("llama-completion output was not valid UTF-8: {e}"))?;

    // Drop trailing prompt-loop artifacts: blank lines and the interactive
    // `>` / `> EOF by user` lines llama-completion prints when stdin closes.
    let cleaned: String = {
        let mut lines: Vec<&str> = raw_stdout.lines().collect();
        while let Some(last) = lines.last() {
            let t = last.trim();
            if t.is_empty() || t == ">" || t.starts_with("> EOF") {
                lines.pop();
            } else {
                break;
            }
        }
        lines.join("\n")
    };

    // Strip any trailing EOS / end-of-generation markers that llama.cpp appends.
    let mut cleaned = cleaned.trim();
    for marker in EOS_MARKERS {
        cleaned = cleaned.trim_end_matches(marker).trim();
    }

    if cleaned.is_empty() {
        return Err("local LLM cleanup returned empty result".into());
    }

    tracing::debug!(
        "Local llama-completion cleanup complete ({} chars)",
        cleaned.len()
    );
    Ok(cleaned.to_string())
}
