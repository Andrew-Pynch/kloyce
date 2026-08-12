use crate::config::Config;
use crate::db;
use crate::dictionary::Dictionary;
use crate::ipc::{self, Command, Response};
use crate::job::{JobStatus, TranscriptResult, TranscriptionJob, TranscriptionMode};
use crate::learning;
use crate::media::MediaStorage;
use crate::model_catalog::{self, ModelDownloadRegistry};
use crate::platform::{audio, context, output};
use crate::transcribe;
use crate::transcript_pipeline::{TranscriptPipeline, TranscriptPipelinePolicy};
use crate::web::{self, TranscriptionEntry};

use chrono::{DateTime, Duration, Utc};
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};

const MEDIA_CLEANUP_BATCH_SIZE: usize = 100;
const MEDIA_CLEANUP_INTERVAL_SECS: u64 = 60 * 60;
const HOTKEY_FAILURE_RETRY_DELAY_SECS: u64 = 30;

type RecordingHandle = (Child, PathBuf, String, DateTime<Utc>);

#[derive(Debug, Clone)]
struct RetainedAudioMetadata {
    path: String,
    filename: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Idle,
    Recording,
    Transcribing,
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            State::Idle => write!(f, "idle"),
            State::Recording => write!(f, "recording"),
            State::Transcribing => write!(f, "transcribing"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{NewTranscriptionJob, PerJobSettings};

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "kloyce-daemon-test-{}-{}-{name}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn temp_db(dir: &Path) -> db::Db {
        db::Db::open(dir.join("kloyce.db")).unwrap()
    }

    #[test]
    fn failed_job_notification_uses_filename_and_short_first_line() {
        let long_tail = "x".repeat(240);
        let body = file_job_failed_notification_body(
            "meeting.mp4",
            &format!("\n ffmpeg failed\n{long_tail}"),
        );

        assert_eq!(body, "meeting.mp4: ffmpeg failed");
        assert!(body.len() < 180);
    }

    #[test]
    fn copy_plus_payload_includes_transcript_audio_and_metadata() {
        let timestamp = "2026-06-30T17:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let expires = "2026-07-07T17:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let payload = format_copy_plus_payload(
            &TranscriptionEntry {
                timestamp,
                duration_secs: 42,
                word_count: 4,
                text: "presentation draft pass".to_string(),
                context_tags: vec!["deck".to_string(), "practice".to_string()],
                recording_id: Some("20260630-120000.000000".to_string()),
                audio_path: Some("/tmp/kloyce-test/media/recordings/pass.mp3".to_string()),
                audio_filename: Some("pass.mp3".to_string()),
                audio_expires_at: Some(expires),
                audio_cleaned_at: None,
            },
            "[voice transcript - if anything is unclear, ask for clarification]",
        );

        assert!(payload.starts_with("```{kloyce-20260630-120000.000000.txt}\n"));
        assert!(payload.contains("presentation draft pass\n```\n\n"));
        assert!(payload.contains("audio_path: /tmp/kloyce-test/media/recordings/pass.mp3"));
        assert!(payload.contains("audio_expires_at: 2026-07-07T17:00:00+00:00"));
        assert!(payload.contains("recorded_at: 2026-06-30T17:00:00+00:00"));
        assert!(payload.contains("duration_secs: 42"));
        assert!(payload.contains("word_count: 4"));
        assert!(payload.contains("context_tags: deck, practice"));
        assert!(
            payload.ends_with("[voice transcript - if anything is unclear, ask for clarification]")
        );
    }

    #[test]
    fn copy_plus_payload_uses_longer_fence_for_backticks_and_hides_cleaned_audio() {
        let timestamp = "2026-06-30T17:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let payload = format_copy_plus_payload(
            &TranscriptionEntry {
                timestamp,
                duration_secs: 1,
                word_count: 1,
                text: "contains ``` fence".to_string(),
                context_tags: Vec::new(),
                recording_id: None,
                audio_path: Some("/tmp/deleted.mp3".to_string()),
                audio_filename: Some("deleted.mp3".to_string()),
                audio_expires_at: None,
                audio_cleaned_at: Some(timestamp),
            },
            "",
        );

        assert!(payload.starts_with("````{transcript.txt}\ncontains ``` fence\n````"));
        assert!(payload.contains("audio_path: unavailable"));
        assert!(payload.contains("audio_expires_at: unavailable"));
        assert!(payload.contains("context_tags: none"));
    }

    #[test]
    fn tmux_transcript_payload_is_none_for_blank_transcript() {
        // A blank final transcript must never produce a tmux payload. Sending
        // one would submit only the voice tag (or nothing) as if it were the
        // user's input.
        assert_eq!(
            format_tmux_transcript_payload("", true, "[voice transcript]"),
            None
        );
        assert_eq!(
            format_tmux_transcript_payload("   \n\t  ", true, "[voice transcript]"),
            None
        );
        assert_eq!(
            format_tmux_transcript_payload("", false, "[voice transcript]"),
            None
        );
    }

    #[test]
    fn tmux_transcript_payload_includes_voice_tag_and_transcript() {
        let payload = format_tmux_transcript_payload(
            "  fix the bug in the parser  ",
            true,
            "[voice transcript - if anything is unclear, ask for clarification]",
        );

        assert_eq!(
            payload,
            Some(
                "[voice transcript - if anything is unclear, ask for clarification] fix the bug in the parser"
                    .to_string()
            )
        );
    }

    #[test]
    fn tmux_transcript_payload_omits_tag_when_voice_tag_disabled() {
        let payload = format_tmux_transcript_payload(
            "fix the bug in the parser",
            false,
            "[voice transcript]",
        );

        assert_eq!(payload, Some("fix the bug in the parser".to_string()));
    }

    #[test]
    fn tmux_transcript_payload_falls_back_to_plain_text_when_tag_text_blank() {
        let payload = format_tmux_transcript_payload("fix the bug", true, "   ");

        assert_eq!(payload, Some("fix the bug".to_string()));
    }

    #[test]
    fn clipboard_transcript_payload_prefixes_tag_with_newline() {
        let payload = format_clipboard_transcript_payload(
            "  fix the bug in the parser  ",
            true,
            "[voice transcript - if anything is unclear, ask for clarification]",
        );

        assert_eq!(
            payload,
            "[voice transcript - if anything is unclear, ask for clarification]\nfix the bug in the parser"
                .to_string()
        );
    }

    #[test]
    fn clipboard_transcript_payload_omits_tag_when_disabled() {
        let payload = format_clipboard_transcript_payload(
            "fix the bug in the parser",
            false,
            "[voice transcript]",
        );

        assert_eq!(payload, "fix the bug in the parser".to_string());
    }

    #[test]
    fn clipboard_transcript_payload_falls_back_to_plain_text_when_tag_text_blank() {
        let payload = format_clipboard_transcript_payload("fix the bug", true, "   ");

        assert_eq!(payload, "fix the bug".to_string());
    }

    #[test]
    fn clipboard_transcript_payload_returns_raw_text_for_blank_transcript() {
        // Unlike the tmux payload, clipboard formatting never suppresses a
        // blank transcript — the caller always copies *something* to the
        // clipboard, it just isn't tag-prefixed when there's no content.
        let payload = format_clipboard_transcript_payload("   \n\t  ", true, "[voice transcript]");

        assert_eq!(payload, "   \n\t  ".to_string());
    }

    #[tokio::test]
    async fn terminal_media_cleanup_deletes_media_without_removing_job_result() {
        let dir = temp_dir("media-cleanup");
        let db = temp_db(&dir);
        let source = dir.join("media/source/clip.wav");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, b"source").unwrap();

        let job = db
            .insert_job(&NewTranscriptionJob {
                source_media_path: source.to_string_lossy().to_string(),
                source_filename: "clip.wav".to_string(),
                mode: TranscriptionMode::Standard,
                settings: PerJobSettings::standard("small.en", Vec::new()),
            })
            .unwrap();
        db.select_next_job_for_worker().unwrap().unwrap();

        let working = dir
            .join("media/working")
            .join(job.id.to_string())
            .join("working.wav");
        std::fs::create_dir_all(working.parent().unwrap()).unwrap();
        std::fs::write(&working, b"working").unwrap();
        let working_text = working.to_string_lossy().to_string();
        db.update_job_working_audio_path(job.id, &working_text)
            .unwrap();
        db.update_job_status(job.id, JobStatus::Transcribing, None)
            .unwrap();
        db.complete_job_with_result_and_retention(
            job.id,
            &TranscriptResult {
                text: "kept transcript".to_string(),
                word_count: 2,
                duration_secs: 1,
                language: None,
                speaker_count: None,
                segments: Vec::new(),
            },
            0,
        )
        .unwrap();

        let storage = MediaStorage::new(dir, PathBuf::from("ffmpeg"), PathBuf::from("ffprobe"));
        cleanup_terminal_job_media(&db, &storage, Utc::now())
            .await
            .unwrap();

        assert!(!source.exists());
        assert!(!working.exists());
        assert!(!working.parent().unwrap().exists());

        let kept = db.get_job(job.id).unwrap().unwrap();
        assert_eq!(kept.result.unwrap().text, "kept transcript");
        assert!(kept.working_audio_path.is_none());
        assert!(db
            .expired_source_media_jobs(Utc::now(), 10)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn terminal_media_cleanup_deletes_expired_hotkey_audio() {
        let dir = temp_dir("hotkey-audio-cleanup");
        let db = temp_db(&dir);
        let audio_path = dir.join("media/recordings/kloyce-test.mp3");
        std::fs::create_dir_all(audio_path.parent().unwrap()).unwrap();
        std::fs::write(&audio_path, b"audio").unwrap();
        db.insert(&TranscriptionEntry {
            timestamp: Utc::now(),
            duration_secs: 10,
            word_count: 2,
            text: "kept transcript".to_string(),
            context_tags: Vec::new(),
            recording_id: Some("test".to_string()),
            audio_path: Some(audio_path.to_string_lossy().to_string()),
            audio_filename: Some("kloyce-test.mp3".to_string()),
            audio_expires_at: Some(Utc::now() - Duration::minutes(1)),
            audio_cleaned_at: None,
        })
        .unwrap();

        let storage = MediaStorage::new(dir, PathBuf::from("ffmpeg"), PathBuf::from("ffprobe"));
        cleanup_terminal_job_media(&db, &storage, Utc::now())
            .await
            .unwrap();

        assert!(!audio_path.exists());
        let latest = db.latest_transcription().unwrap().unwrap();
        assert_eq!(latest.text, "kept transcript");
        assert!(latest.audio_cleaned_at.is_some());
        assert!(db
            .expired_transcription_audio(Utc::now(), 10)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn terminal_media_cleanup_deletes_expired_failed_hotkey_audio() {
        let dir = temp_dir("failed-hotkey-audio-cleanup");
        let db = temp_db(&dir);
        let audio_path = dir.join("media/recordings/kloyce-failed-test.mp3");
        let audio_path_str = audio_path.to_string_lossy().to_string();
        std::fs::create_dir_all(audio_path.parent().unwrap()).unwrap();
        std::fs::write(&audio_path, b"audio").unwrap();
        let failure = db
            .insert_hotkey_transcription_failure(
                "20260811-120000.000000",
                10,
                &[],
                None,
                false,
                "whisper-cli exited with status 1",
                Some(audio_path_str.as_str()),
                Some("kloyce-failed-test.mp3"),
                Some(Utc::now() - Duration::minutes(1)),
            )
            .unwrap();

        let storage = MediaStorage::new(dir, PathBuf::from("ffmpeg"), PathBuf::from("ffprobe"));
        cleanup_terminal_job_media(&db, &storage, Utc::now())
            .await
            .unwrap();

        assert!(!audio_path.exists());
        let updated = db
            .get_hotkey_transcription_failure(failure.id)
            .unwrap()
            .unwrap();
        assert!(updated.audio_cleaned_at.is_some());
        assert!(db
            .expired_hotkey_transcription_failure_audio(Utc::now(), 10)
            .unwrap()
            .is_empty());
    }
}

async fn run_transcription_job_worker(
    config: Arc<RwLock<Config>>,
    database: Arc<db::Db>,
    dictionary: Arc<RwLock<Dictionary>>,
    event_tx: broadcast::Sender<web::SseEvent>,
    data_dir: PathBuf,
) {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(1));

    loop {
        interval.tick().await;
        let job = match database.select_next_job_for_worker() {
            Ok(Some(job)) => job,
            Ok(None) => continue,
            Err(error) => {
                tracing::error!("Failed to select queued transcription job: {error}");
                continue;
            }
        };

        process_transcription_job(
            job,
            config.clone(),
            database.clone(),
            dictionary.clone(),
            event_tx.clone(),
            data_dir.clone(),
        )
        .await;
    }
}

async fn run_media_retention_cleanup_worker(
    config: Arc<RwLock<Config>>,
    database: Arc<db::Db>,
    data_dir: PathBuf,
) {
    loop {
        let config_snapshot = config.read().await.clone();
        let media_storage = MediaStorage::new(
            data_dir.clone(),
            config_snapshot.ffmpeg_bin.clone(),
            config_snapshot.ffprobe_bin.clone(),
        );

        if let Err(error) = cleanup_terminal_job_media(&database, &media_storage, Utc::now()).await
        {
            tracing::warn!("Transcription job media cleanup failed: {error}");
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(
            MEDIA_CLEANUP_INTERVAL_SECS,
        ))
        .await;
    }
}

async fn cleanup_terminal_job_media(
    database: &db::Db,
    media_storage: &MediaStorage,
    now: DateTime<Utc>,
) -> Result<(), String> {
    cleanup_terminal_working_audio(database, media_storage).await?;
    cleanup_expired_source_media(database, media_storage, now).await?;
    cleanup_expired_transcription_audio(database, media_storage, now).await?;
    cleanup_expired_hotkey_failure_audio(database, media_storage, now).await?;
    Ok(())
}

async fn cleanup_terminal_working_audio(
    database: &db::Db,
    media_storage: &MediaStorage,
) -> Result<(), String> {
    let working_jobs = database
        .terminal_jobs_with_working_audio(MEDIA_CLEANUP_BATCH_SIZE)
        .map_err(|error| format!("failed to load terminal working-audio jobs: {error}"))?;

    for job in working_jobs {
        let Some(path) = job.working_audio_path.as_deref() else {
            continue;
        };

        media_storage
            .delete_working_audio_path(Path::new(path))
            .await;
        database
            .clear_job_working_audio_path(job.id)
            .map_err(|error| {
                format!(
                    "failed to clear working audio path for job {}: {error}",
                    job.id
                )
            })?;
    }

    Ok(())
}

async fn cleanup_expired_source_media(
    database: &db::Db,
    media_storage: &MediaStorage,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let source_jobs = database
        .expired_source_media_jobs(now, MEDIA_CLEANUP_BATCH_SIZE)
        .map_err(|error| format!("failed to load expired source-media jobs: {error}"))?;

    for job in source_jobs {
        media_storage
            .delete_source_media_path(Path::new(&job.source_media_path))
            .await
            .map_err(|error| {
                format!("failed to delete source media for job {}: {error}", job.id)
            })?;
        database
            .mark_job_source_media_cleaned(job.id)
            .map_err(|error| {
                format!(
                    "failed to mark source media cleaned for job {}: {error}",
                    job.id
                )
            })?;
    }

    Ok(())
}

async fn cleanup_expired_transcription_audio(
    database: &db::Db,
    media_storage: &MediaStorage,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let audio_entries = database
        .expired_transcription_audio(now, MEDIA_CLEANUP_BATCH_SIZE)
        .map_err(|error| format!("failed to load expired hotkey audio entries: {error}"))?;

    for audio_entry in audio_entries {
        media_storage
            .delete_retained_audio_path(Path::new(&audio_entry.audio_path))
            .await
            .map_err(|error| {
                format!(
                    "failed to delete retained hotkey audio for transcription {}: {error}",
                    audio_entry.id
                )
            })?;
        database
            .mark_transcription_audio_cleaned(audio_entry.id)
            .map_err(|error| {
                format!(
                    "failed to mark hotkey audio cleaned for transcription {}: {error}",
                    audio_entry.id
                )
            })?;
    }

    Ok(())
}

async fn cleanup_expired_hotkey_failure_audio(
    database: &db::Db,
    media_storage: &MediaStorage,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let audio_entries = database
        .expired_hotkey_transcription_failure_audio(now, MEDIA_CLEANUP_BATCH_SIZE)
        .map_err(|error| format!("failed to load expired failed-hotkey audio entries: {error}"))?;

    for audio_entry in audio_entries {
        media_storage
            .delete_retained_audio_path(Path::new(&audio_entry.audio_path))
            .await
            .map_err(|error| {
                format!(
                    "failed to delete retained failed-hotkey audio for failure {}: {error}",
                    audio_entry.id
                )
            })?;
        database
            .mark_hotkey_transcription_failure_audio_cleaned(audio_entry.id)
            .map_err(|error| {
                format!(
                    "failed to mark failed-hotkey audio cleaned for failure {}: {error}",
                    audio_entry.id
                )
            })?;
    }

    Ok(())
}

async fn retain_hotkey_recording_audio(
    config: &Config,
    data_dir: &Path,
    recording_id: &str,
    wav_path: &Path,
    record_stop_time: DateTime<Utc>,
) -> Option<RetainedAudioMetadata> {
    if !config.hotkey_audio_retention_enabled {
        return None;
    }

    let media_storage = MediaStorage::new(
        data_dir.to_path_buf(),
        config.ffmpeg_bin.clone(),
        config.ffprobe_bin.clone(),
    );
    let expires_at = record_stop_time + Duration::hours(config.hotkey_audio_retention_hours as i64);

    match media_storage
        .store_hotkey_recording(recording_id, wav_path)
        .await
    {
        Ok(stored) => Some(RetainedAudioMetadata {
            path: stored.path.to_string_lossy().to_string(),
            filename: stored.filename,
            expires_at,
        }),
        Err(error) => {
            tracing::warn!(
                recording_id = %recording_id,
                wav_path = %wav_path.display(),
                "Failed to retain hotkey recording audio: {error}"
            );
            None
        }
    }
}

fn format_copy_plus_payload(entry: &TranscriptionEntry, voice_tag_text: &str) -> String {
    let fence = markdown_fence_for(&entry.text);
    let transcript_filename = transcript_filename_for_copy_plus(entry);
    let audio_path = if entry.audio_cleaned_at.is_none() {
        entry.audio_path.as_deref().unwrap_or("unavailable")
    } else {
        "unavailable"
    };
    let audio_expires_at = entry
        .audio_expires_at
        .as_ref()
        .map(DateTime::<Utc>::to_rfc3339)
        .unwrap_or_else(|| "unavailable".to_string());
    let context_tags = if entry.context_tags.is_empty() {
        "none".to_string()
    } else {
        entry.context_tags.join(", ")
    };
    let voice_note = if voice_tag_text.trim().is_empty() {
        "[voice transcript - if anything is unclear, ask for clarification]"
    } else {
        voice_tag_text.trim()
    };

    let mut payload = format!("{fence}{{{transcript_filename}}}\n");
    payload.push_str(&entry.text);
    if !entry.text.ends_with('\n') {
        payload.push('\n');
    }
    payload.push_str(&fence);
    payload.push_str("\n\n");
    payload.push_str(&format!(
        "audio_path: {audio_path}\n\
         audio_expires_at: {audio_expires_at}\n\
         recorded_at: {}\n\
         duration_secs: {}\n\
         word_count: {}\n\
         context_tags: {context_tags}\n\n\
         {voice_note}",
        entry.timestamp.to_rfc3339(),
        entry.duration_secs,
        entry.word_count,
    ));
    payload
}

/// Prefix `text` with `voice_tag_text` (joined by `separator`) when
/// `include_voice_tag` is true and the tag text is non-blank. Shared by the
/// tmux send-keys path (space-joined, single line) and the clipboard path
/// (newline-joined). Returns `None` for a blank transcript.
fn format_voice_tagged_text(
    text: &str,
    include_voice_tag: bool,
    voice_tag_text: &str,
    separator: &str,
) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    if include_voice_tag {
        let voice_tag_text = voice_tag_text.trim();
        if voice_tag_text.is_empty() {
            Some(text.to_string())
        } else {
            Some(format!("{voice_tag_text}{separator}{text}"))
        }
    } else {
        Some(text.to_string())
    }
}

fn format_tmux_transcript_payload(
    text: &str,
    include_voice_tag: bool,
    voice_tag_text: &str,
) -> Option<String> {
    format_voice_tagged_text(text, include_voice_tag, voice_tag_text, " ")
}

/// Format the transcript for clipboard output. Unlike the tmux payload, a
/// blank transcript is not suppressed here — the caller always has *some*
/// text to copy (possibly empty), it just isn't tag-prefixed.
fn format_clipboard_transcript_payload(
    text: &str,
    include_voice_tag: bool,
    voice_tag_text: &str,
) -> String {
    format_voice_tagged_text(text, include_voice_tag, voice_tag_text, "\n")
        .unwrap_or_else(|| text.to_string())
}

fn transcript_filename_for_copy_plus(entry: &TranscriptionEntry) -> String {
    entry
        .recording_id
        .as_ref()
        .map(|recording_id| format!("kloyce-{recording_id}.txt"))
        .unwrap_or_else(|| "transcript.txt".to_string())
}

fn markdown_fence_for(text: &str) -> String {
    let mut max_run = 0usize;
    let mut current_run = 0usize;
    for ch in text.chars() {
        if ch == '`' {
            current_run += 1;
            max_run = max_run.max(current_run);
        } else {
            current_run = 0;
        }
    }
    "`".repeat(max_run.max(2) + 1)
}

async fn process_transcription_job(
    job: TranscriptionJob,
    config: Arc<RwLock<Config>>,
    database: Arc<db::Db>,
    dictionary: Arc<RwLock<Dictionary>>,
    event_tx: broadcast::Sender<web::SseEvent>,
    data_dir: PathBuf,
) {
    let job_start = std::time::Instant::now();
    let config_snapshot = config.read().await.clone();
    let retention_days = config_snapshot.source_media_retention_days;
    let media_storage = MediaStorage::new(
        data_dir,
        config_snapshot.ffmpeg_bin.clone(),
        config_snapshot.ffprobe_bin.clone(),
    );

    send_job_status(&event_tx, job.id, JobStatus::PreparingMedia);
    update_job_progress(&database, &event_tx, job.id, 5, job_start);

    let source_path = PathBuf::from(&job.source_media_path);
    let working_audio = match media_storage
        .prepare_standard_working_audio(job.id, &source_path)
        .await
    {
        Ok(path) => path,
        Err(error) => {
            fail_transcription_job(
                &database,
                &event_tx,
                &media_storage,
                job.id,
                &job.source_filename,
                None,
                &error.to_string(),
                retention_days,
            )
            .await;
            return;
        }
    };

    let working_audio_text = working_audio.to_string_lossy().to_string();
    if let Err(error) = database.update_job_working_audio_path(job.id, &working_audio_text) {
        fail_transcription_job(
            &database,
            &event_tx,
            &media_storage,
            job.id,
            &job.source_filename,
            Some(&working_audio),
            &format!("Failed to persist working audio path: {error}"),
            retention_days,
        )
        .await;
        return;
    }
    update_job_progress(&database, &event_tx, job.id, 65, job_start);

    if job_cancel_requested(&database, job.id) {
        cancel_transcription_job(
            &database,
            &event_tx,
            &media_storage,
            job.id,
            &job.source_filename,
            Some(&working_audio),
            retention_days,
        )
        .await;
        return;
    }

    if matches!(job.mode, TranscriptionMode::Diarized) {
        if let Err(error) = mark_job_transcribing(&database, &event_tx, job.id) {
            fail_transcription_job(
                &database,
                &event_tx,
                &media_storage,
                job.id,
                &job.source_filename,
                Some(&working_audio),
                &error,
                retention_days,
            )
            .await;
            return;
        }
        update_job_progress(&database, &event_tx, job.id, 70, job_start);

        let result = match transcribe_diarized_job(
            job.id,
            &working_audio,
            &job.settings,
            &config_snapshot,
            &event_tx,
            job_start,
        )
        .await
        {
            Ok(result) => result,
            Err(error) => {
                fail_transcription_job(
                    &database,
                    &event_tx,
                    &media_storage,
                    job.id,
                    &job.source_filename,
                    Some(&working_audio),
                    &error,
                    retention_days,
                )
                .await;
                return;
            }
        };

        if job_cancel_requested(&database, job.id) {
            cancel_transcription_job(
                &database,
                &event_tx,
                &media_storage,
                job.id,
                &job.source_filename,
                Some(&working_audio),
                retention_days,
            )
            .await;
            return;
        }

        complete_transcription_job(
            &database,
            &event_tx,
            &media_storage,
            job.id,
            result,
            &working_audio,
            &job.source_filename,
            job.mode,
            &job.settings.model,
            &config_snapshot.sound_stop,
            retention_days,
        )
        .await;
        return;
    }

    let Some(model_path) =
        model_catalog::standard_model_path(&config_snapshot, &job.settings.model)
    else {
        fail_transcription_job(
            &database,
            &event_tx,
            &media_storage,
            job.id,
            &job.source_filename,
            Some(&working_audio),
            &format!("Unknown standard model: {}", job.settings.model),
            retention_days,
        )
        .await;
        return;
    };

    if !model_path.exists() {
        fail_transcription_job(
            &database,
            &event_tx,
            &media_storage,
            job.id,
            &job.source_filename,
            Some(&working_audio),
            &format!(
                "Standard mode unavailable: model '{}' is not installed",
                job.settings.model
            ),
            retention_days,
        )
        .await;
        return;
    }

    if let Err(error) = mark_job_transcribing(&database, &event_tx, job.id) {
        fail_transcription_job(
            &database,
            &event_tx,
            &media_storage,
            job.id,
            &job.source_filename,
            Some(&working_audio),
            &error,
            retention_days,
        )
        .await;
        return;
    }
    update_job_progress(&database, &event_tx, job.id, 70, job_start);

    let whisper_prompt = dictionary.read().await.whisper_prompt();
    let raw_text = match transcribe::transcribe(
        &working_audio,
        &model_path,
        &config_snapshot.whisper_bin,
        event_tx.clone(),
        whisper_prompt.as_deref(),
        config_snapshot.whisper_flash_attn,
        config_snapshot.whisper_threads,
        config_snapshot.whisper_beam_size,
    )
    .await
    {
        Ok(text) => text,
        Err(error) => {
            fail_transcription_job(
                &database,
                &event_tx,
                &media_storage,
                job.id,
                &job.source_filename,
                Some(&working_audio),
                &format!("Transcription failed: {error}"),
                retention_days,
            )
            .await;
            return;
        }
    };

    let pipeline = TranscriptPipeline::new(dictionary.clone());
    let pipeline_policy = TranscriptPipelinePolicy::from_config(&config_snapshot);
    let processed = pipeline
        .process(&raw_text, &job.settings.context_tags, &pipeline_policy)
        .await;

    if job_cancel_requested(&database, job.id) {
        cancel_transcription_job(
            &database,
            &event_tx,
            &media_storage,
            job.id,
            &job.source_filename,
            Some(&working_audio),
            retention_days,
        )
        .await;
        return;
    }

    let result = TranscriptResult {
        word_count: processed.word_count,
        duration_secs: job_start.elapsed().as_secs(),
        text: processed.text,
        language: None,
        speaker_count: None,
        segments: Vec::new(),
    };

    complete_transcription_job(
        &database,
        &event_tx,
        &media_storage,
        job.id,
        result,
        &working_audio,
        &job.source_filename,
        job.mode,
        &job.settings.model,
        &config_snapshot.sound_stop,
        retention_days,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn complete_transcription_job(
    database: &db::Db,
    event_tx: &broadcast::Sender<web::SseEvent>,
    media_storage: &MediaStorage,
    job_id: i64,
    result: TranscriptResult,
    working_audio: &Path,
    source_filename: &str,
    mode: TranscriptionMode,
    model: &str,
    sound_stop: &Path,
    retention_days: i64,
) {
    match database.complete_job_with_result_and_retention(job_id, &result, retention_days) {
        Ok(_) => {
            let _ = event_tx.send(web::SseEvent::TranscriptionJobResult { job_id, result });
            send_job_status(event_tx, job_id, JobStatus::Succeeded);
            output::notify(
                "Kloyce",
                &format!("File transcription completed: {source_filename} ({mode}/{model})"),
            );
            audio::play_sound(sound_stop);
        }
        Err(error) => {
            fail_transcription_job(
                database,
                event_tx,
                media_storage,
                job_id,
                source_filename,
                Some(working_audio),
                &format!("Failed to complete job: {error}"),
                retention_days,
            )
            .await;
            return;
        }
    }

    media_storage.delete_working_audio_path(working_audio).await;
}

async fn transcribe_diarized_job(
    job_id: i64,
    audio_path: &Path,
    settings: &crate::job::PerJobSettings,
    config: &Config,
    event_tx: &broadcast::Sender<web::SseEvent>,
    job_start: std::time::Instant,
) -> Result<TranscriptResult, String> {
    let advanced = &config.advanced_transcription;
    let python_bin = advanced.transcriber_venv.join("bin/python");
    if !advanced.enabled {
        return Err(
            "Advanced transcription is not enabled. Set advanced_transcription.enabled = true in config.toml"
                .to_string(),
        );
    }
    if !python_bin.exists() {
        return Err(format!(
            "Python venv not found at: {}",
            advanced.transcriber_venv.display()
        ));
    }

    let mut cmd = tokio::process::Command::new(&python_bin);
    cmd.arg("-m")
        .arg("youtube_transcriber.cli")
        .arg("transcribe-file")
        .arg(audio_path)
        .arg("--model")
        .arg(&settings.model)
        .arg("--device")
        .arg(&advanced.device)
        .arg("--json")
        .arg("--progress");

    if !settings.diarize {
        cmd.arg("--no-diarize");
    }
    if let Some(min) = settings.min_speakers {
        cmd.arg("--min-speakers").arg(min.to_string());
    }
    if let Some(max) = settings.max_speakers {
        cmd.arg("--max-speakers").arg(max.to_string());
    }

    cmd.kill_on_drop(true);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|error| format!("Failed to spawn transcriber: {error}"))?;

    let stderr = child.stderr.take();
    let progress_tx = event_tx.clone();
    let progress_handle = tokio::spawn(async move {
        if let Some(stderr) = stderr {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if let Ok(progress) = serde_json::from_str::<serde_json::Value>(&line) {
                    let pct = progress
                        .get("progress")
                        .and_then(|p| p.as_u64())
                        .unwrap_or(0) as u32;
                    let _ = progress_tx.send(web::SseEvent::TranscriptionJobProgress {
                        job_id,
                        progress_pct: pct.min(99),
                        elapsed_secs: job_start.elapsed().as_secs_f64(),
                    });
                }
            }
        }
    });

    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(advanced.timeout_secs),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            let _ = progress_handle.await;
            return Err(format!("Transcriber process error: {error}"));
        }
        Err(_) => {
            let _ = progress_handle.await;
            return Err(format!(
                "Transcription timed out after {}s",
                advanced.timeout_secs
            ));
        }
    };

    let _ = progress_handle.await;

    if !output.status.success() {
        return Err(format!(
            "Transcription failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    #[derive(serde::Deserialize)]
    struct PythonOutput {
        segments: Vec<web::TranscriptSegment>,
        #[serde(default)]
        language: String,
    }

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let json_line = stdout_str
        .lines()
        .rev()
        .find(|line| line.starts_with('{'))
        .ok_or_else(|| "No JSON output found in transcriber stdout".to_string())?;
    let parsed: PythonOutput = serde_json::from_str(json_line)
        .map_err(|error| format!("Failed to parse transcriber output: {error}"))?;
    let full_text = parsed
        .segments
        .iter()
        .map(|segment| format!("{}: {}", segment.speaker, segment.text.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    let speaker_count = parsed
        .segments
        .iter()
        .map(|segment| segment.speaker.as_str())
        .collect::<HashSet<_>>()
        .len() as u32;
    let segments = parsed
        .segments
        .into_iter()
        .filter_map(|segment| serde_json::to_value(segment).ok())
        .collect();

    Ok(TranscriptResult {
        word_count: full_text.split_whitespace().count() as u64,
        duration_secs: job_start.elapsed().as_secs(),
        text: full_text,
        language: if parsed.language.is_empty() {
            None
        } else {
            Some(parsed.language)
        },
        speaker_count: Some(speaker_count),
        segments,
    })
}

fn mark_job_transcribing(
    database: &db::Db,
    event_tx: &broadcast::Sender<web::SseEvent>,
    job_id: i64,
) -> Result<(), String> {
    database
        .update_job_status(job_id, JobStatus::Transcribing, None)
        .map_err(|error| format!("Failed to mark job transcribing: {error}"))?;
    send_job_status(event_tx, job_id, JobStatus::Transcribing);
    Ok(())
}

fn update_job_progress(
    database: &db::Db,
    event_tx: &broadcast::Sender<web::SseEvent>,
    job_id: i64,
    progress_pct: u32,
    started_at: std::time::Instant,
) {
    if let Err(error) = database.update_job_progress(job_id, progress_pct) {
        tracing::warn!(
            job_id,
            progress_pct,
            "Failed to persist job progress: {error}"
        );
    }
    let _ = event_tx.send(web::SseEvent::TranscriptionJobProgress {
        job_id,
        progress_pct,
        elapsed_secs: started_at.elapsed().as_secs_f64(),
    });
}

fn send_job_status(event_tx: &broadcast::Sender<web::SseEvent>, job_id: i64, status: JobStatus) {
    let _ = event_tx.send(web::SseEvent::TranscriptionJobStatus { job_id, status });
}

fn job_cancel_requested(database: &db::Db, job_id: i64) -> bool {
    match database.get_job(job_id) {
        Ok(Some(job)) => job.cancel_requested,
        Ok(None) => false,
        Err(error) => {
            tracing::warn!(job_id, "Failed to inspect cancellation flag: {error}");
            false
        }
    }
}

async fn cancel_transcription_job(
    database: &db::Db,
    event_tx: &broadcast::Sender<web::SseEvent>,
    media_storage: &MediaStorage,
    job_id: i64,
    source_filename: &str,
    working_audio_path: Option<&Path>,
    retention_days: i64,
) {
    if let Err(error) = database.update_job_status_with_retention(
        job_id,
        JobStatus::Cancelled,
        None,
        retention_days,
    ) {
        tracing::error!(
            job_id,
            "Failed to mark transcription job cancelled: {error}"
        );
    }
    if let Some(path) = working_audio_path {
        media_storage.delete_working_audio_path(path).await;
    }
    send_job_status(event_tx, job_id, JobStatus::Cancelled);
    output::notify(
        "Kloyce",
        &format!("File transcription cancelled: {source_filename}"),
    );
}

#[allow(clippy::too_many_arguments)]
async fn fail_transcription_job(
    database: &db::Db,
    event_tx: &broadcast::Sender<web::SseEvent>,
    media_storage: &MediaStorage,
    job_id: i64,
    source_filename: &str,
    working_audio_path: Option<&Path>,
    error_message: &str,
    retention_days: i64,
) {
    tracing::error!(job_id, "Transcription job failed: {error_message}");
    if let Err(error) = database.update_job_status_with_retention(
        job_id,
        JobStatus::Failed,
        Some(error_message),
        retention_days,
    ) {
        tracing::error!(job_id, "Failed to mark transcription job failed: {error}");
    }
    if let Some(path) = working_audio_path {
        media_storage.delete_working_audio_path(path).await;
    }
    send_job_status(event_tx, job_id, JobStatus::Failed);
    output::notify(
        "Kloyce Error",
        &file_job_failed_notification_body(source_filename, error_message),
    );
}

fn file_job_failed_notification_body(source_filename: &str, error_message: &str) -> String {
    format!("{source_filename}: {}", short_job_error(error_message))
}

fn short_job_error(error_message: &str) -> String {
    const MAX_LEN: usize = 180;
    let first_line = error_message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Transcription failed");

    if first_line.chars().count() <= MAX_LEN {
        return first_line.to_string();
    }

    let mut shortened = first_line.chars().take(MAX_LEN - 3).collect::<String>();
    shortened.push_str("...");
    shortened
}

/// Kick off a background task that waits `HOTKEY_FAILURE_RETRY_DELAY_SECS`
/// and then retries a failed hotkey transcription from its retained audio.
#[allow(clippy::too_many_arguments)]
fn schedule_hotkey_transcription_retry(
    failure_id: i64,
    config: Arc<RwLock<Config>>,
    database: Arc<db::Db>,
    dictionary: Arc<RwLock<Dictionary>>,
    event_tx: broadcast::Sender<web::SseEvent>,
    metrics: Arc<RwLock<Metrics>>,
    history: Arc<Mutex<VecDeque<TranscriptionEntry>>>,
    history_size: usize,
    data_dir: PathBuf,
) {
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(
            HOTKEY_FAILURE_RETRY_DELAY_SECS,
        ))
        .await;
        retry_hotkey_transcription(
            failure_id,
            config,
            database,
            dictionary,
            event_tx,
            metrics,
            history,
            history_size,
            data_dir,
        )
        .await;
    });
}

/// Reprocess a failed hotkey transcription from its retained audio through
/// the same transcribe + pipeline + output path used for a live recording.
/// Used both by the automatic 30s retry and by `kloyce-ctl retry`.
#[allow(clippy::too_many_arguments)]
async fn retry_hotkey_transcription(
    failure_id: i64,
    config: Arc<RwLock<Config>>,
    database: Arc<db::Db>,
    dictionary: Arc<RwLock<Dictionary>>,
    event_tx: broadcast::Sender<web::SseEvent>,
    metrics: Arc<RwLock<Metrics>>,
    history: Arc<Mutex<VecDeque<TranscriptionEntry>>>,
    history_size: usize,
    data_dir: PathBuf,
) {
    let db_handle = database.clone();
    let failure = match tokio::task::spawn_blocking(move || {
        db_handle.get_hotkey_transcription_failure(failure_id)
    })
    .await
    {
        Ok(Ok(Some(failure))) => failure,
        Ok(Ok(None)) => {
            tracing::warn!(
                failure_id,
                "Failed hotkey transcription no longer exists; skipping retry"
            );
            return;
        }
        Ok(Err(error)) => {
            tracing::error!(
                failure_id,
                "Failed to load failed hotkey transcription for retry: {error}"
            );
            return;
        }
        Err(error) => {
            tracing::error!(failure_id, "Retry lookup task crashed: {error}");
            return;
        }
    };

    if failure.resolved_at.is_some() {
        tracing::info!(
            failure_id,
            "Failed hotkey transcription already resolved; skipping retry"
        );
        return;
    }

    let Some(audio_path) = failure.audio_path.clone() else {
        tracing::warn!(
            failure_id,
            "No retained audio for failed hotkey transcription; skipping retry"
        );
        return;
    };
    let audio_path = PathBuf::from(audio_path);
    if !audio_path.exists() {
        tracing::warn!(
            failure_id,
            audio_path = %audio_path.display(),
            "Retained audio missing; skipping retry"
        );
        return;
    }

    tracing::info!(
        failure_id,
        recording_id = %failure.recording_id,
        audio_path = %audio_path.display(),
        "Retrying hotkey transcription from retained audio"
    );

    let config_snapshot = config.read().await.clone();
    let whisper_prompt = dictionary.read().await.whisper_prompt();

    let media_storage = MediaStorage::new(
        data_dir,
        config_snapshot.ffmpeg_bin.clone(),
        config_snapshot.ffprobe_bin.clone(),
    );
    let working_audio = match media_storage
        .prepare_hotkey_retry_working_audio(failure_id, &audio_path)
        .await
    {
        Ok(path) => path,
        Err(error) => {
            let error_text = error.to_string();
            tracing::error!(
                failure_id,
                "Failed to prepare retry working audio: {error_text}"
            );
            let db_handle = database.clone();
            let error_for_db = error_text.clone();
            let _ = tokio::task::spawn_blocking(move || {
                db_handle.record_hotkey_transcription_failure_retry_error(failure_id, &error_for_db)
            })
            .await;
            output::notify_progress(
                "Kloyce Error",
                &format!(
                    "Retry failed: {}. Run `kloyce-ctl retry {failure_id}` to try again.",
                    short_job_error(&error_text)
                ),
                None,
            )
            .await;
            return;
        }
    };

    let result = transcribe::transcribe(
        &working_audio,
        &config_snapshot.model_path,
        &config_snapshot.whisper_bin,
        event_tx.clone(),
        whisper_prompt.as_deref(),
        config_snapshot.whisper_flash_attn,
        config_snapshot.whisper_threads,
        config_snapshot.whisper_beam_size,
    )
    .await;
    media_storage
        .delete_working_audio_path(&working_audio)
        .await;

    match result {
        Ok(raw_text) => {
            let pipeline = TranscriptPipeline::new(dictionary.clone());
            let pipeline_policy = TranscriptPipelinePolicy::from_config(&config_snapshot);
            let processed = pipeline
                .process(&raw_text, &failure.context_tags, &pipeline_policy)
                .await;
            let word_count = processed.word_count;
            let text = processed.text;

            {
                let mut m = metrics.write().await;
                m.total_transcriptions += 1;
                m.total_words += word_count;
            }

            let entry = TranscriptionEntry {
                timestamp: Utc::now(),
                duration_secs: failure.duration_secs,
                word_count,
                text: text.clone(),
                context_tags: failure.context_tags.clone(),
                recording_id: Some(failure.recording_id.clone()),
                audio_path: Some(audio_path.to_string_lossy().to_string()),
                audio_filename: failure.audio_filename.clone(),
                audio_expires_at: failure.audio_expires_at,
                audio_cleaned_at: None,
            };
            {
                let mut h = history.lock().await;
                h.push_front(entry.clone());
                while h.len() > history_size {
                    h.pop_back();
                }
            }

            let db_handle = database.clone();
            let entry_for_db = entry.clone();
            match tokio::task::spawn_blocking(move || db_handle.insert(&entry_for_db)).await {
                Ok(Ok(())) => {
                    tracing::info!(failure_id, "Persisted retried transcription");
                }
                Ok(Err(error)) => {
                    tracing::error!(
                        failure_id,
                        "Failed to persist retried transcription: {error}"
                    );
                }
                Err(error) => {
                    tracing::error!(
                        failure_id,
                        "Persistence task for retried transcription failed: {error}"
                    );
                }
            }

            let _ = event_tx.send(web::SseEvent::Transcription(entry));

            let clipboard_text = format_clipboard_transcript_payload(
                &text,
                config_snapshot.clipboard_voice_tag,
                &config_snapshot.tmux_voice_tag_text,
            );
            let mut output_methods = Vec::new();
            let mut output_errors = Vec::new();
            match output::set_clipboard(&clipboard_text).await {
                Ok(()) => output_methods.push("copied to clipboard"),
                Err(e) => output_errors.push(format!("clipboard: {e}")),
            }

            if let Some((target, tmux_text)) =
                failure
                    .tmux_target
                    .as_ref()
                    .zip(format_tmux_transcript_payload(
                        &text,
                        config_snapshot.tmux_voice_tag,
                        &config_snapshot.tmux_voice_tag_text,
                    ))
            {
                match output::tmux_send_keys(&tmux_text, target).await {
                    Ok(()) => {
                        output_methods.push("sent to tmux pane");
                        if failure.auto_enter {
                            if let Err(e) = output::tmux_send_enter(target).await {
                                output_errors.push(format!("tmux enter: {e}"));
                            }
                        }
                    }
                    Err(e) => output_errors.push(format!("tmux: {e}")),
                }
            }

            let db_resolve = database.clone();
            match tokio::task::spawn_blocking(move || {
                db_resolve.mark_hotkey_transcription_failure_resolved(failure_id)
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::error!(
                        failure_id,
                        "Failed to mark failed hotkey transcription resolved: {error}"
                    )
                }
                Err(error) => {
                    tracing::error!(
                        failure_id,
                        "Resolve task for hotkey failure crashed: {error}"
                    )
                }
            }

            if output_methods.is_empty() {
                output::notify_progress(
                    "Kloyce",
                    &format!(
                        "Retry succeeded but delivery failed: {}",
                        output_errors.join("; ")
                    ),
                    Some(100),
                )
                .await;
            } else {
                output::notify_progress(
                    "Kloyce",
                    &format!(
                        "Retry succeeded: {word_count} words {}",
                        output_methods.join(" and ")
                    ),
                    Some(100),
                )
                .await;
            }
        }
        Err(error) => {
            let error_text = error.to_string();
            tracing::error!(
                failure_id,
                "Hotkey transcription retry failed: {error_text}"
            );
            let db_handle = database.clone();
            let error_for_db = error_text.clone();
            match tokio::task::spawn_blocking(move || {
                db_handle.record_hotkey_transcription_failure_retry_error(failure_id, &error_for_db)
            })
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(db_error)) => {
                    tracing::error!(failure_id, "Failed to record retry failure: {db_error}")
                }
                Err(join_error) => {
                    tracing::error!(
                        failure_id,
                        "Retry-failure recording task crashed: {join_error}"
                    )
                }
            }
            output::notify_progress(
                "Kloyce Error",
                &format!(
                    "Retry failed: {}. Run `kloyce-ctl retry {failure_id}` to try again.",
                    short_job_error(&error_text)
                ),
                None,
            )
            .await;
        }
    }
}

fn format_failed_transcription_age(duration: Duration) -> String {
    let total_secs = duration.num_seconds().max(0);
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    if hours > 0 {
        format!("{hours}h{minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{total_secs}s")
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Metrics {
    pub total_transcriptions: u64,
    pub total_words: u64,
    pub started_at: chrono::DateTime<Utc>,
}

pub struct Daemon {
    pub config: Arc<RwLock<Config>>,
    pub state: Arc<RwLock<State>>,
    pub metrics: Arc<RwLock<Metrics>>,
    pub history: Arc<Mutex<VecDeque<TranscriptionEntry>>>,
    pub event_tx: broadcast::Sender<web::SseEvent>,
    recording_handle: Arc<Mutex<Option<RecordingHandle>>>,
    recording_timer_cancel: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    recording_context: Arc<Mutex<Option<context::ContextCollector>>>,
    recording_tmux_target: Arc<Mutex<Option<String>>>,
    database: Arc<db::Db>,
    dictionary: Arc<RwLock<Dictionary>>,
    data_dir: PathBuf,
}

impl Daemon {
    pub fn new(config: Config) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (event_tx, _) = broadcast::channel(64);

        #[cfg(unix)]
        let data_dir = {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".local/share/kloyce")
        };
        #[cfg(windows)]
        let data_dir = {
            let local_appdata = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| {
                let profile =
                    std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\Default".into());
                format!("{profile}\\AppData\\Local")
            });
            PathBuf::from(local_appdata).join("kloyce")
        };
        let database = Arc::new(db::Db::open(data_dir.join("kloyce.db"))?);

        let dictionary = Arc::new(RwLock::new(
            Dictionary::load(&config.dictionary_path).unwrap_or_else(|e| {
                tracing::warn!("Failed to load dictionary: {e}, using empty dictionary");
                Dictionary::empty(config.dictionary_path.clone())
            }),
        ));

        let config = Arc::new(RwLock::new(config));

        Ok(Self {
            config,
            state: Arc::new(RwLock::new(State::Idle)),
            metrics: Arc::new(RwLock::new(Metrics {
                total_transcriptions: 0,
                total_words: 0,
                started_at: Utc::now(),
            })),
            history: Arc::new(Mutex::new(VecDeque::new())),
            event_tx,
            recording_handle: Arc::new(Mutex::new(None)),
            recording_timer_cancel: Arc::new(Mutex::new(None)),
            recording_context: Arc::new(Mutex::new(None)),
            recording_tmux_target: Arc::new(Mutex::new(None)),
            database,
            dictionary,
            data_dir,
        })
    }

    pub async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let startup_config = self.config.read().await.clone();
        let startup_media_storage = MediaStorage::new(
            self.data_dir.clone(),
            startup_config.ffmpeg_bin.clone(),
            startup_config.ffprobe_bin.clone(),
        );
        let recovered_jobs = self.database.recover_interrupted_active_jobs()?;
        if recovered_jobs > 0 {
            tracing::warn!(
                recovered_jobs,
                "Marked interrupted transcription jobs failed on daemon startup"
            );
        }
        startup_media_storage.delete_all_working_audio().await;

        // Load persisted metrics
        let (total_transcriptions, total_words) = self.database.total_stats()?;
        {
            let mut m = self.metrics.write().await;
            m.total_transcriptions = total_transcriptions;
            m.total_words = total_words;
        }
        tracing::info!(
            "Loaded {total_transcriptions} transcriptions ({total_words} words) from database"
        );

        // Load persisted history
        let entries = self.database.recent(startup_config.history_size)?;
        {
            let mut h = self.history.lock().await;
            *h = VecDeque::from(entries);
        }

        let (cmd_tx, mut cmd_rx) =
            mpsc::channel::<(Command, tokio::sync::oneshot::Sender<Response>)>(32);

        // Start IPC server
        ipc::start_server(cmd_tx).await?;

        // Start web server
        let gpu_latest: Arc<RwLock<Option<crate::platform::GpuMetrics>>> =
            Arc::new(RwLock::new(None));

        let web_state = web::WebState {
            daemon_state: self.state.clone(),
            metrics: self.metrics.clone(),
            history: self.history.clone(),
            event_tx: self.event_tx.clone(),
            gpu_latest: gpu_latest.clone(),
            config: self.config.clone(),
            model_downloads: ModelDownloadRegistry::default(),
            database: self.database.clone(),
            dictionary: self.dictionary.clone(),
            data_dir: self.data_dir.clone(),
        };
        web::start_server(web_state, startup_config.web_port).await;

        // Start GPU monitor
        let gpu_monitor = crate::platform::gpu::GpuMonitor::new(
            gpu_latest,
            self.event_tx.clone(),
            startup_config.gpu_poll_interval_ms,
        );
        tokio::spawn(gpu_monitor.run());

        // Start dictionary file watcher (poll mtime every 5s)
        let dict_poll = self.dictionary.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
            loop {
                interval.tick().await;
                dict_poll.write().await.reload_if_changed();
            }
        });

        tokio::spawn(run_transcription_job_worker(
            self.config.clone(),
            self.database.clone(),
            self.dictionary.clone(),
            self.event_tx.clone(),
            self.data_dir.clone(),
        ));
        tokio::spawn(run_media_retention_cleanup_worker(
            self.config.clone(),
            self.database.clone(),
            self.data_dir.clone(),
        ));

        tracing::info!("Kloyce daemon running");

        // Main command loop
        while let Some((cmd, resp_tx)) = cmd_rx.recv().await {
            let response = match cmd {
                Command::Toggle => self.handle_toggle(false).await,
                Command::ToggleEnter => self.handle_toggle(true).await,
                Command::CopyPlusLatest => self.handle_copy_plus_latest().await,
                Command::Status => {
                    let state = self.state.read().await;
                    Response {
                        status: "ok",
                        state: state.to_string(),
                        message: format!("State: {state}"),
                    }
                }
                Command::Cancel => self.handle_cancel().await,
                Command::ListFailedTranscriptions => self.handle_list_failed().await,
                Command::RetryFailedTranscription { id } => self.handle_retry_failed(id).await,
            };
            let _ = resp_tx.send(response);
        }

        Ok(())
    }

    async fn handle_copy_plus_latest(&self) -> Response {
        let current_state = self.state.read().await.to_string();
        let entry = {
            let history = self.history.lock().await;
            history.front().cloned()
        };

        let entry = match entry {
            Some(entry) => Some(entry),
            None => {
                let database = self.database.clone();
                match tokio::task::spawn_blocking(move || database.latest_transcription()).await {
                    Ok(Ok(entry)) => entry,
                    Ok(Err(error)) => {
                        tracing::error!(
                            "Failed to load latest transcription for copy plus: {error}"
                        );
                        return Response {
                            status: "error",
                            state: current_state,
                            message: format!("Failed to load latest transcript: {error}"),
                        };
                    }
                    Err(error) => {
                        tracing::error!("Latest transcription lookup task failed: {error}");
                        return Response {
                            status: "error",
                            state: current_state,
                            message: format!("Failed to load latest transcript: {error}"),
                        };
                    }
                }
            }
        };

        let Some(entry) = entry else {
            return Response {
                status: "error",
                state: current_state,
                message: "No transcriptions yet".into(),
            };
        };

        let config = self.config.read().await.clone();
        let payload = format_copy_plus_payload(&entry, &config.tmux_voice_tag_text);
        match output::set_clipboard(&payload).await {
            Ok(()) => {
                output::notify("Kloyce", "Copy+ latest transcript copied");
                Response {
                    status: "ok",
                    state: current_state,
                    message: "Copy+ latest transcript copied".into(),
                }
            }
            Err(error) => {
                tracing::error!("Failed to copy plus latest transcript: {error}");
                Response {
                    status: "error",
                    state: current_state,
                    message: format!("Failed to copy latest transcript: {error}"),
                }
            }
        }
    }

    async fn handle_toggle(&self, force_auto_enter: bool) -> Response {
        let current = *self.state.read().await;
        let config = self.config.read().await.clone();

        match current {
            State::Idle => {
                // Start recording
                let record_start_time = Utc::now();
                let recording_id = record_start_time.format("%Y%m%d-%H%M%S%.6f").to_string();
                let tmp = std::env::temp_dir().join(format!("kloyce-{recording_id}.wav"));

                tracing::info!(
                    recording_id = %recording_id,
                    force_auto_enter,
                    "Starting recording"
                );

                audio::play_sound(&config.sound_start);
                output::notify("Kloyce", "Recording started...");

                match audio::start_recording(&tmp).await {
                    Ok(child) => {
                        *self.recording_handle.lock().await =
                            Some((child, tmp, recording_id.clone(), record_start_time));
                        *self.state.write().await = State::Recording;
                        let _ = self.event_tx.send(web::SseEvent::StateChange {
                            state: State::Recording,
                        });
                        output::signal_waybar();

                        // Start recording progress timer
                        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
                        *self.recording_timer_cancel.lock().await = Some(cancel_tx);
                        let event_tx = self.event_tx.clone();
                        let start = std::time::Instant::now();
                        tokio::spawn(async move {
                            let mut interval =
                                tokio::time::interval(tokio::time::Duration::from_millis(500));
                            loop {
                                interval.tick().await;
                                if cancel_rx.try_recv().is_ok() {
                                    break;
                                }
                                let elapsed = start.elapsed().as_secs_f64();
                                let _ = event_tx.send(web::SseEvent::RecordingProgress {
                                    duration_secs: (elapsed * 10.0).round() / 10.0,
                                });
                            }
                        });

                        // Start context tracking
                        let collector =
                            context::ContextCollector::start(config.context_poll_interval_ms);
                        *self.recording_context.lock().await = Some(collector);

                        // Capture tmux target for send-keys (one-shot at recording start)
                        if config.tmux_send_keys {
                            let target = context::capture_tmux_target().await;
                            if let Some(ref t) = target {
                                tracing::info!(
                                    recording_id = %recording_id,
                                    tmux_target = %t,
                                    "Tmux target captured"
                                );
                            }
                            *self.recording_tmux_target.lock().await = target;
                        }

                        Response {
                            status: "ok",
                            state: "recording".into(),
                            message: "Recording started".into(),
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to start recording: {e}");
                        output::notify("Kloyce Error", &format!("Failed to start recording: {e}"));
                        Response {
                            status: "error",
                            state: "idle".into(),
                            message: format!("Failed to start recording: {e}"),
                        }
                    }
                }
            }
            State::Recording => {
                // Stop recording progress timer
                if let Some(cancel) = self.recording_timer_cancel.lock().await.take() {
                    let _ = cancel.send(());
                }

                // Stop context tracking and capture tags
                let context_tags =
                    if let Some(collector) = self.recording_context.lock().await.take() {
                        collector.stop().await
                    } else {
                        vec![]
                    };
                tracing::info!(context_tags = ?context_tags, "Recording context captured");

                // Stop recording and transcribe
                let handle = self.recording_handle.lock().await.take();

                if let Some((child, wav_path, recording_id, record_start_time)) = handle {
                    *self.state.write().await = State::Transcribing;
                    let _ = self.event_tx.send(web::SseEvent::StateChange {
                        state: State::Transcribing,
                    });
                    output::signal_waybar();

                    let record_stop_time = Utc::now();
                    let recording_duration_secs =
                        (record_stop_time - record_start_time).num_seconds().max(0) as u64;

                    tracing::info!(
                        recording_id = %recording_id,
                        force_auto_enter,
                        context_tags = ?context_tags,
                        "Stopping recording"
                    );
                    let stop_start = std::time::Instant::now();
                    if let Err(e) = audio::stop_recording(child).await {
                        tracing::error!(
                            recording_id = %recording_id,
                            "Failed to stop recording: {e}"
                        );
                    } else {
                        tracing::info!(
                            recording_id = %recording_id,
                            stop_ms = stop_start.elapsed().as_millis(),
                            "Recording process stopped"
                        );
                    }

                    // Transcribe in background
                    let config_arc = self.config.clone();
                    let config = config.clone();
                    let state = self.state.clone();
                    let metrics = self.metrics.clone();
                    let history = self.history.clone();
                    let event_tx = self.event_tx.clone();
                    let history_size = config.history_size;
                    let database = self.database.clone();
                    let dictionary = self.dictionary.clone();
                    let data_dir = self.data_dir.clone();
                    let tmux_target = self.recording_tmux_target.lock().await.take();
                    // The STOP command determines whether Enter is pressed
                    let auto_enter = force_auto_enter || config.tmux_auto_enter;

                    tracing::info!(
                        recording_id = %recording_id,
                        tmux_target = ?tmux_target,
                        auto_enter,
                        "Starting transcription job"
                    );

                    tokio::spawn(async move {
                        let job_start = std::time::Instant::now();
                        // Progress notification task
                        let mut progress_rx = event_tx.subscribe();
                        let progress_task = tokio::spawn(async move {
                            while let Ok(event) = progress_rx.recv().await {
                                if let web::SseEvent::TranscriptionProgress {
                                    progress_pct, ..
                                } = event
                                {
                                    output::notify_progress(
                                        "Kloyce",
                                        &format!("Transcribing... {progress_pct}%"),
                                        Some(progress_pct),
                                    )
                                    .await;
                                }
                            }
                        });

                        // Get whisper vocabulary prompt from dictionary
                        let whisper_prompt = dictionary.read().await.whisper_prompt();

                        let transcription_start = std::time::Instant::now();
                        let result = transcribe::transcribe(
                            &wav_path,
                            &config.model_path,
                            &config.whisper_bin,
                            event_tx.clone(),
                            whisper_prompt.as_deref(),
                            config.whisper_flash_attn,
                            config.whisper_threads,
                            config.whisper_beam_size,
                        )
                        .await;

                        progress_task.abort();

                        match result {
                            Ok(raw_text) => {
                                let transcription_ms = transcription_start.elapsed().as_millis();
                                tracing::info!(
                                    recording_id = %recording_id,
                                    transcription_ms,
                                    raw_bytes = raw_text.len(),
                                    raw_chars = raw_text.chars().count(),
                                    raw_words = raw_text.split_whitespace().count(),
                                    "Transcription completed"
                                );
                                let pipeline = TranscriptPipeline::new(dictionary.clone());
                                let pipeline_policy =
                                    TranscriptPipelinePolicy::from_config(&config);
                                let processed = pipeline
                                    .process(&raw_text, &context_tags, &pipeline_policy)
                                    .await;
                                let cleanup_ms = processed.elapsed.as_millis();
                                let word_count = processed.word_count;
                                tracing::info!(
                                    recording_id = %recording_id,
                                    cleanup_ms,
                                    final_bytes = processed.final_bytes,
                                    final_chars = processed.final_chars,
                                    final_words = word_count,
                                    "Final transcript prepared"
                                );
                                let text = processed.text;
                                let retained_audio = retain_hotkey_recording_audio(
                                    &config,
                                    &data_dir,
                                    &recording_id,
                                    &wav_path,
                                    record_stop_time,
                                )
                                .await;

                                // Persist successful transcription regardless of output success.
                                {
                                    let mut m = metrics.write().await;
                                    m.total_transcriptions += 1;
                                    m.total_words += word_count;
                                }

                                let entry = TranscriptionEntry {
                                    timestamp: record_stop_time,
                                    duration_secs: recording_duration_secs,
                                    word_count,
                                    text: text.clone(),
                                    context_tags: context_tags.clone(),
                                    recording_id: Some(recording_id.clone()),
                                    audio_path: retained_audio
                                        .as_ref()
                                        .map(|metadata| metadata.path.clone()),
                                    audio_filename: retained_audio
                                        .as_ref()
                                        .map(|metadata| metadata.filename.clone()),
                                    audio_expires_at: retained_audio
                                        .as_ref()
                                        .map(|metadata| metadata.expires_at),
                                    audio_cleaned_at: None,
                                };
                                {
                                    let mut h = history.lock().await;
                                    h.push_front(entry.clone());
                                    while h.len() > history_size {
                                        h.pop_back();
                                    }
                                }

                                let db_start = std::time::Instant::now();
                                let db_handle = database.clone();
                                let entry_for_db = entry.clone();
                                match tokio::task::spawn_blocking(move || {
                                    db_handle.insert(&entry_for_db)
                                })
                                .await
                                {
                                    Ok(Ok(())) => {
                                        tracing::info!(
                                            recording_id = %recording_id,
                                            db_ms = db_start.elapsed().as_millis(),
                                            "Persisted transcription"
                                        );
                                    }
                                    Ok(Err(e)) => {
                                        tracing::error!(
                                            recording_id = %recording_id,
                                            db_ms = db_start.elapsed().as_millis(),
                                            "Failed to persist transcription: {e}"
                                        );
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            recording_id = %recording_id,
                                            db_ms = db_start.elapsed().as_millis(),
                                            "Persistence task failed: {e}"
                                        );
                                    }
                                }

                                let _ = event_tx.send(web::SseEvent::Transcription(entry));

                                let clipboard_start = std::time::Instant::now();
                                let clipboard_text = format_clipboard_transcript_payload(
                                    &text,
                                    config.clipboard_voice_tag,
                                    &config.tmux_voice_tag_text,
                                );
                                let clipboard_result = output::set_clipboard(&clipboard_text).await;
                                let clipboard_ms = clipboard_start.elapsed().as_millis();
                                let mut output_methods = Vec::new();
                                let mut output_errors = Vec::new();

                                match clipboard_result {
                                    Ok(()) => {
                                        tracing::info!(
                                            recording_id = %recording_id,
                                            clipboard_ms,
                                            clipboard_bytes = clipboard_text.len(),
                                            "Copied transcription to clipboard"
                                        );
                                        output_methods.push("copied to clipboard");
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            recording_id = %recording_id,
                                            clipboard_ms,
                                            "Clipboard output failed: {e}"
                                        );
                                        output_errors.push(format!("clipboard: {e}"));
                                    }
                                }

                                if let Some((target, tmux_text)) =
                                    tmux_target.as_ref().zip(format_tmux_transcript_payload(
                                        &text,
                                        config.tmux_voice_tag,
                                        &config.tmux_voice_tag_text,
                                    ))
                                {
                                    let tmux_start = std::time::Instant::now();
                                    match output::tmux_send_keys(&tmux_text, target).await {
                                        Ok(()) => {
                                            let tmux_ms = tmux_start.elapsed().as_millis();
                                            tracing::info!(
                                                recording_id = %recording_id,
                                                tmux_target = %target,
                                                tmux_ms,
                                                tmux_bytes = tmux_text.len(),
                                                "Sent text to tmux target"
                                            );
                                            output_methods.push("sent to tmux pane");
                                            if auto_enter {
                                                let enter_start = std::time::Instant::now();
                                                if let Err(e) =
                                                    output::tmux_send_enter(target).await
                                                {
                                                    tracing::warn!(
                                                        recording_id = %recording_id,
                                                        tmux_target = %target,
                                                        enter_ms = enter_start.elapsed().as_millis(),
                                                        "tmux send Enter failed: {e}"
                                                    );
                                                    output_errors.push(format!("tmux enter: {e}"));
                                                } else {
                                                    tracing::info!(
                                                        recording_id = %recording_id,
                                                        tmux_target = %target,
                                                        enter_ms = enter_start.elapsed().as_millis(),
                                                        "Sent Enter to tmux target"
                                                    );
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            let tmux_ms = tmux_start.elapsed().as_millis();
                                            tracing::warn!(
                                                recording_id = %recording_id,
                                                tmux_target = %target,
                                                tmux_ms,
                                                "tmux output failed: {e}"
                                            );
                                            output_errors.push(format!("tmux: {e}"));
                                        }
                                    }
                                }

                                if output_methods.is_empty() {
                                    let error_summary = output_errors.join("; ");
                                    tracing::error!(
                                        recording_id = %recording_id,
                                        errors = %error_summary,
                                        "All output methods failed"
                                    );
                                    output::notify_progress(
                                        "Kloyce Error",
                                        &format!("Failed to output text: {error_summary}"),
                                        Some(100),
                                    )
                                    .await;
                                } else {
                                    let method = output_methods.join(" and ");
                                    if !output_errors.is_empty() {
                                        tracing::warn!(
                                            recording_id = %recording_id,
                                            errors = %output_errors.join("; "),
                                            "Transcription completed with output warnings"
                                        );
                                    }
                                    output::notify_progress(
                                        "Kloyce",
                                        &format!("{word_count} words {method}"),
                                        Some(100),
                                    )
                                    .await;
                                }

                                audio::play_sound(&config.sound_stop);
                                tracing::info!(
                                    recording_id = %recording_id,
                                    total_ms = job_start.elapsed().as_millis(),
                                    "Transcription job completed"
                                );

                                // Background: learn new dictionary corrections
                                if config.dictionary_learning {
                                    let dict_clone = dictionary.clone();
                                    let claude_bin = config.claude_bin.clone();
                                    let raw_for_learning = raw_text.clone();
                                    let tags_for_learning = context_tags.clone();
                                    let max_entries = config.dictionary_max_entries;
                                    tokio::spawn(async move {
                                        learning::learn_from_transcript(
                                            &raw_for_learning,
                                            &tags_for_learning,
                                            dict_clone,
                                            &claude_bin,
                                            max_entries,
                                        )
                                        .await;
                                    });
                                }
                            }
                            Err(e) => {
                                let error_text = e.to_string();
                                tracing::error!(
                                    recording_id = %recording_id,
                                    transcription_ms = transcription_start.elapsed().as_millis(),
                                    "Transcription failed: {error_text}"
                                );

                                let retained_audio = retain_hotkey_recording_audio(
                                    &config,
                                    &data_dir,
                                    &recording_id,
                                    &wav_path,
                                    record_stop_time,
                                )
                                .await;
                                let has_audio = retained_audio.is_some();

                                let db_handle = database.clone();
                                let recording_id_for_db = recording_id.clone();
                                let context_tags_for_db = context_tags.clone();
                                let tmux_target_for_db = tmux_target.clone();
                                let error_text_for_db = error_text.clone();
                                let audio_path_for_db = retained_audio
                                    .as_ref()
                                    .map(|metadata| metadata.path.clone());
                                let audio_filename_for_db = retained_audio
                                    .as_ref()
                                    .map(|metadata| metadata.filename.clone());
                                let audio_expires_at_for_db =
                                    retained_audio.as_ref().map(|metadata| metadata.expires_at);
                                let failure_record = tokio::task::spawn_blocking(move || {
                                    db_handle.insert_hotkey_transcription_failure(
                                        &recording_id_for_db,
                                        recording_duration_secs,
                                        &context_tags_for_db,
                                        tmux_target_for_db.as_deref(),
                                        auto_enter,
                                        &error_text_for_db,
                                        audio_path_for_db.as_deref(),
                                        audio_filename_for_db.as_deref(),
                                        audio_expires_at_for_db,
                                    )
                                })
                                .await;

                                match failure_record {
                                    Ok(Ok(failure)) => {
                                        tracing::info!(
                                            recording_id = %recording_id,
                                            failure_id = failure.id,
                                            audio_retained = has_audio,
                                            "Persisted failed hotkey transcription"
                                        );

                                        if has_audio {
                                            schedule_hotkey_transcription_retry(
                                                failure.id,
                                                config_arc.clone(),
                                                database.clone(),
                                                dictionary.clone(),
                                                event_tx.clone(),
                                                metrics.clone(),
                                                history.clone(),
                                                history_size,
                                                data_dir.clone(),
                                            );
                                            output::notify_progress(
                                                "Kloyce Error",
                                                &format!(
                                                    "Transcription failed: {}. Audio retained, retrying in {}s.",
                                                    short_job_error(&error_text),
                                                    HOTKEY_FAILURE_RETRY_DELAY_SECS,
                                                ),
                                                None,
                                            )
                                            .await;
                                        } else {
                                            tracing::warn!(
                                                failure_id = failure.id,
                                                "No retained audio for failed hotkey transcription; skipping automatic retry"
                                            );
                                            output::notify_progress(
                                                "Kloyce Error",
                                                &format!(
                                                    "Transcription failed: {}. Audio was not retained; see `kloyce-ctl failed`.",
                                                    short_job_error(&error_text)
                                                ),
                                                None,
                                            )
                                            .await;
                                        }
                                    }
                                    Ok(Err(db_error)) => {
                                        tracing::error!(
                                            recording_id = %recording_id,
                                            "Failed to persist failed hotkey transcription: {db_error}"
                                        );
                                        output::notify_progress(
                                            "Kloyce Error",
                                            &format!(
                                                "Transcription failed: {}",
                                                short_job_error(&error_text)
                                            ),
                                            None,
                                        )
                                        .await;
                                    }
                                    Err(join_error) => {
                                        tracing::error!(
                                            recording_id = %recording_id,
                                            "Persistence task for failed hotkey transcription crashed: {join_error}"
                                        );
                                        output::notify_progress(
                                            "Kloyce Error",
                                            &format!(
                                                "Transcription failed: {}",
                                                short_job_error(&error_text)
                                            ),
                                            None,
                                        )
                                        .await;
                                    }
                                }

                                audio::play_sound(&config.sound_stop);
                            }
                        }

                        // Clean up WAV
                        let _ = tokio::fs::remove_file(&wav_path).await;

                        *state.write().await = State::Idle;
                        let _ = event_tx.send(web::SseEvent::StateChange { state: State::Idle });
                        output::signal_waybar();
                    });

                    Response {
                        status: "ok",
                        state: "transcribing".into(),
                        message: "Recording stopped, transcribing...".into(),
                    }
                } else {
                    *self.state.write().await = State::Idle;
                    Response {
                        status: "error",
                        state: "idle".into(),
                        message: "No active recording found".into(),
                    }
                }
            }
            State::Transcribing => Response {
                status: "ok",
                state: "transcribing".into(),
                message: "Transcription in progress, please wait".into(),
            },
        }
    }

    async fn handle_list_failed(&self) -> Response {
        let current_state = self.state.read().await.to_string();
        let database = self.database.clone();
        let failures = tokio::task::spawn_blocking(move || {
            database.unresolved_hotkey_transcription_failures(50)
        })
        .await;

        let failures = match failures {
            Ok(Ok(failures)) => failures,
            Ok(Err(error)) => {
                return Response {
                    status: "error",
                    state: current_state,
                    message: format!("Failed to load failed transcriptions: {error}"),
                };
            }
            Err(error) => {
                return Response {
                    status: "error",
                    state: current_state,
                    message: format!("Failed transcriptions lookup task failed: {error}"),
                };
            }
        };

        if failures.is_empty() {
            return Response {
                status: "ok",
                state: current_state,
                message: "No failed hotkey transcriptions".into(),
            };
        }

        let now = Utc::now();
        let lines: Vec<String> = failures
            .iter()
            .map(|failure| {
                format!(
                    "id={} age={} retries={} audio={} error={}",
                    failure.id,
                    format_failed_transcription_age(now - failure.created_at),
                    failure.retry_count,
                    if failure.audio_path.is_some() {
                        "yes"
                    } else {
                        "no"
                    },
                    short_job_error(&failure.error_message),
                )
            })
            .collect();

        Response {
            status: "ok",
            state: current_state,
            message: lines.join("\n"),
        }
    }

    async fn handle_retry_failed(&self, id: Option<i64>) -> Response {
        let current_state = self.state.read().await.to_string();
        let database = self.database.clone();
        let lookup = match id {
            Some(id) => {
                let db_handle = database.clone();
                tokio::task::spawn_blocking(move || db_handle.get_hotkey_transcription_failure(id))
                    .await
            }
            None => {
                let db_handle = database.clone();
                tokio::task::spawn_blocking(move || {
                    db_handle.latest_unresolved_hotkey_transcription_failure()
                })
                .await
            }
        };

        let failure = match lookup {
            Ok(Ok(Some(failure))) => failure,
            Ok(Ok(None)) => {
                return Response {
                    status: "error",
                    state: current_state,
                    message: match id {
                        Some(id) => format!("No failed hotkey transcription found with id {id}"),
                        None => "No unresolved failed hotkey transcriptions found".into(),
                    },
                };
            }
            Ok(Err(error)) => {
                return Response {
                    status: "error",
                    state: current_state,
                    message: format!("Failed to load failed transcription: {error}"),
                };
            }
            Err(error) => {
                return Response {
                    status: "error",
                    state: current_state,
                    message: format!("Retry lookup task failed: {error}"),
                };
            }
        };

        if failure.resolved_at.is_some() {
            return Response {
                status: "error",
                state: current_state,
                message: format!(
                    "Failed hotkey transcription {} is already resolved",
                    failure.id
                ),
            };
        }
        if failure.audio_path.is_none() {
            return Response {
                status: "error",
                state: current_state,
                message: format!(
                    "Failed hotkey transcription {} has no retained audio to retry",
                    failure.id
                ),
            };
        }

        let failure_id = failure.id;
        let history_size = self.config.read().await.history_size;
        tokio::spawn(retry_hotkey_transcription(
            failure_id,
            self.config.clone(),
            self.database.clone(),
            self.dictionary.clone(),
            self.event_tx.clone(),
            self.metrics.clone(),
            self.history.clone(),
            history_size,
            self.data_dir.clone(),
        ));

        Response {
            status: "ok",
            state: current_state,
            message: format!("Retrying failed hotkey transcription {failure_id}"),
        }
    }

    async fn handle_cancel(&self) -> Response {
        let current = *self.state.read().await;
        match current {
            State::Recording => {
                // Stop recording progress timer
                if let Some(cancel) = self.recording_timer_cancel.lock().await.take() {
                    let _ = cancel.send(());
                }
                // Stop context tracking
                if let Some(collector) = self.recording_context.lock().await.take() {
                    let _ = collector.stop().await;
                }
                // Clear tmux target
                *self.recording_tmux_target.lock().await = None;
                let handle = self.recording_handle.lock().await.take();
                if let Some((child, wav_path, recording_id, _record_start_time)) = handle {
                    tracing::info!(
                        recording_id = %recording_id,
                        "Cancelling active recording"
                    );
                    let _ = audio::stop_recording(child).await;
                    let _ = tokio::fs::remove_file(&wav_path).await;
                }
                *self.state.write().await = State::Idle;
                let _ = self
                    .event_tx
                    .send(web::SseEvent::StateChange { state: State::Idle });
                output::signal_waybar();
                output::notify("Kloyce", "Recording cancelled");
                Response {
                    status: "ok",
                    state: "idle".into(),
                    message: "Recording cancelled".into(),
                }
            }
            _ => Response {
                status: "ok",
                state: current.to_string(),
                message: "Nothing to cancel".into(),
            },
        }
    }
}
