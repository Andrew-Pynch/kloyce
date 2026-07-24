use std::path::Path;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::broadcast;

/// Run whisper.cpp on a WAV file, return the transcribed text.
/// Tries GPU first; if that fails (e.g. VRAM contention with Ollama), retries with --no-gpu.
#[allow(clippy::too_many_arguments)]
pub async fn transcribe(
    wav_path: &Path,
    model_path: &Path,
    whisper_bin: &Path,
    event_tx: broadcast::Sender<crate::web::SseEvent>,
    whisper_prompt: Option<&str>,
    flash_attn: bool,
    threads: u32,
    beam_size: u32,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if !whisper_bin.exists() {
        return Err(format!("whisper-cli not found at: {}", whisper_bin.display()).into());
    }
    if !model_path.exists() {
        return Err(format!("Model not found at: {}", model_path.display()).into());
    }
    if !wav_path.exists() {
        return Err(format!("WAV file not found at: {}", wav_path.display()).into());
    }

    // Try GPU first
    match run_whisper(
        wav_path,
        model_path,
        whisper_bin,
        event_tx.clone(),
        whisper_prompt,
        false,
        flash_attn,
        threads,
        beam_size,
    )
    .await
    {
        Ok(text) => Ok(text),
        Err(gpu_err) => {
            tracing::warn!("GPU transcription failed ({gpu_err}), retrying with CPU...");
            run_whisper(
                wav_path,
                model_path,
                whisper_bin,
                event_tx,
                whisper_prompt,
                true,
                flash_attn,
                threads,
                beam_size,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_whisper(
    wav_path: &Path,
    model_path: &Path,
    whisper_bin: &Path,
    event_tx: broadcast::Sender<crate::web::SseEvent>,
    whisper_prompt: Option<&str>,
    no_gpu: bool,
    flash_attn: bool,
    threads: u32,
    beam_size: u32,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if no_gpu {
        tracing::info!("Transcribing (CPU): {}", wav_path.display());
    } else {
        tracing::info!("Transcribing: {}", wav_path.display());
    }

    let start = std::time::Instant::now();

    let mut cmd = Command::new(whisper_bin);
    cmd.arg("-m")
        .arg(model_path)
        .arg("-f")
        .arg(wav_path)
        .arg("--no-timestamps")
        .arg("-otxt")
        .arg("--print-progress");

    if no_gpu {
        cmd.arg("--no-gpu");
    }

    if flash_attn {
        cmd.arg("-fa");
    }

    if threads > 0 {
        cmd.arg("-t").arg(threads.to_string());
    }

    if beam_size > 0 {
        cmd.arg("-bs").arg(beam_size.to_string());
    }

    if let Some(prompt) = whisper_prompt {
        cmd.arg("--prompt").arg(prompt);
    }

    let mut child = cmd
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run whisper-cli: {e}"))?;

    // Read stderr for progress updates
    if let Some(stderr) = child.stderr.take() {
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // Parse: "whisper_print_progress_callback: progress = XX%"
                if let Some(pct_str) = line
                    .strip_prefix("whisper_print_progress_callback: progress =")
                    .or_else(|| line.strip_prefix("whisper_print_progress_callback:  progress ="))
                {
                    let pct_str = pct_str.trim().trim_end_matches('%');
                    if let Ok(pct) = pct_str.parse::<u32>().map(|p| p.min(100)) {
                        let _ = event_tx.send(crate::web::SseEvent::TranscriptionProgress {
                            progress_pct: pct,
                            elapsed_secs: (start.elapsed().as_secs_f64() * 10.0).round() / 10.0,
                        });
                    }
                }
            }
        });
    }

    let status = child.wait().await?;

    if !status.success() {
        return Err(format!("whisper-cli failed ({status})").into());
    }

    // whisper.cpp with -otxt writes to <input>.txt
    let txt_path = wav_path.with_extension("wav.txt");
    let text = if txt_path.exists() {
        tokio::fs::read_to_string(&txt_path)
            .await?
            .trim()
            .to_string()
    } else {
        String::new()
    };

    // Clean up the txt file
    let _ = tokio::fs::remove_file(&txt_path).await;

    if text.is_empty() {
        return Err("Transcription produced empty result".into());
    }

    Ok(text)
}
