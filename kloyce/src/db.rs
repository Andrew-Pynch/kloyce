use crate::job::{
    CancelJobOutcome, JobStatus, NewTranscriptionJob, PerJobSettings, TranscriptResult,
    TranscriptionJob, TranscriptionMode, ACTIVE_JOB_STATUSES, JOB_STATUS_SQL_CHECK,
    TERMINAL_JOB_STATUSES, TRANSCRIPTION_MODE_SQL_CHECK,
};
use crate::web::{DiarizedTranscriptionEntry, TranscriptionEntry};

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension};
use std::path::PathBuf;
use std::sync::Mutex;

pub struct Db {
    conn: Mutex<Connection>,
}

pub const DEFAULT_SOURCE_MEDIA_RETENTION_DAYS: i64 = 7;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpiredTranscriptionAudio {
    pub id: i64,
    pub audio_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyTranscriptionFailure {
    pub id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub recording_id: String,
    pub duration_secs: u64,
    pub context_tags: Vec<String>,
    pub tmux_target: Option<String>,
    pub auto_enter: bool,
    pub error_message: String,
    pub audio_path: Option<String>,
    pub audio_filename: Option<String>,
    pub audio_expires_at: Option<DateTime<Utc>>,
    pub audio_cleaned_at: Option<DateTime<Utc>>,
    pub retry_count: i64,
    pub resolved_at: Option<DateTime<Utc>>,
}

impl Db {
    pub fn open(path: PathBuf) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS transcriptions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                duration_secs INTEGER NOT NULL,
                word_count INTEGER NOT NULL,
                text TEXT NOT NULL
            );",
        )?;

        // Migration: add context_tags column if it doesn't exist
        conn.execute_batch(
            "ALTER TABLE transcriptions ADD COLUMN context_tags TEXT NOT NULL DEFAULT '[]';",
        )
        .ok();
        conn.execute_batch("ALTER TABLE transcriptions ADD COLUMN recording_id TEXT;")
            .ok();
        conn.execute_batch("ALTER TABLE transcriptions ADD COLUMN audio_path TEXT;")
            .ok();
        conn.execute_batch("ALTER TABLE transcriptions ADD COLUMN audio_filename TEXT;")
            .ok();
        conn.execute_batch("ALTER TABLE transcriptions ADD COLUMN audio_expires_at TEXT;")
            .ok();
        conn.execute_batch("ALTER TABLE transcriptions ADD COLUMN audio_cleaned_at TEXT;")
            .ok();

        // Advanced diarized transcriptions table
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS diarized_transcriptions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                duration_secs INTEGER NOT NULL,
                file_path TEXT NOT NULL,
                language TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT '',
                segments_json TEXT NOT NULL,
                full_text TEXT NOT NULL,
                word_count INTEGER NOT NULL,
                speaker_count INTEGER NOT NULL DEFAULT 0,
                context_tags TEXT NOT NULL DEFAULT '[]'
            );",
        )?;

        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS transcription_jobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                source_media_path TEXT NOT NULL,
                source_filename TEXT NOT NULL,
                working_audio_path TEXT,
                status TEXT NOT NULL CHECK (status IN ({JOB_STATUS_SQL_CHECK})),
                mode TEXT NOT NULL CHECK (mode IN ({TRANSCRIPTION_MODE_SQL_CHECK})),
                settings_json TEXT NOT NULL,
                result_json TEXT,
                error_message TEXT,
                progress_pct INTEGER NOT NULL DEFAULT 0 CHECK (progress_pct >= 0 AND progress_pct <= 100),
                cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK (cancel_requested IN (0, 1)),
                started_at TEXT,
                completed_at TEXT,
                source_media_retain_until TEXT,
                source_media_cleaned_at TEXT
            );
            CREATE INDEX IF NOT EXISTS transcription_jobs_status_created_idx
                ON transcription_jobs (status, created_at, id);
            CREATE INDEX IF NOT EXISTS transcription_jobs_completed_idx
                ON transcription_jobs (completed_at DESC, updated_at DESC);"
        ))?;

        conn.execute_batch(
            "ALTER TABLE transcription_jobs ADD COLUMN source_media_retain_until TEXT;",
        )
        .ok();
        conn.execute_batch(
            "ALTER TABLE transcription_jobs ADD COLUMN source_media_cleaned_at TEXT;",
        )
        .ok();

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS hotkey_transcription_failures (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                recording_id TEXT NOT NULL,
                duration_secs INTEGER NOT NULL DEFAULT 0,
                context_tags TEXT NOT NULL DEFAULT '[]',
                tmux_target TEXT,
                auto_enter INTEGER NOT NULL DEFAULT 0 CHECK (auto_enter IN (0, 1)),
                error_message TEXT NOT NULL,
                audio_path TEXT,
                audio_filename TEXT,
                audio_expires_at TEXT,
                audio_cleaned_at TEXT,
                retry_count INTEGER NOT NULL DEFAULT 0,
                resolved_at TEXT
            );
            CREATE INDEX IF NOT EXISTS hotkey_transcription_failures_unresolved_idx
                ON hotkey_transcription_failures (resolved_at, created_at);",
        )?;

        tracing::info!("Opened database at {}", path.display());
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn insert(&self, entry: &TranscriptionEntry) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let tags_json = serde_json::to_string(&entry.context_tags).unwrap_or_else(|_| "[]".into());
        let audio_expires_at = entry
            .audio_expires_at
            .as_ref()
            .map(DateTime::<Utc>::to_rfc3339);
        let audio_cleaned_at = entry
            .audio_cleaned_at
            .as_ref()
            .map(DateTime::<Utc>::to_rfc3339);
        conn.execute(
            "INSERT INTO transcriptions (
                timestamp,
                duration_secs,
                word_count,
                text,
                context_tags,
                recording_id,
                audio_path,
                audio_filename,
                audio_expires_at,
                audio_cleaned_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                entry.timestamp.to_rfc3339(),
                entry.duration_secs,
                entry.word_count,
                entry.text.as_str(),
                tags_json,
                entry.recording_id.as_deref(),
                entry.audio_path.as_deref(),
                entry.audio_filename.as_deref(),
                audio_expires_at,
                audio_cleaned_at,
            ],
        )?;
        Ok(())
    }

    pub fn recent(&self, limit: usize) -> Result<Vec<TranscriptionEntry>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {TRANSCRIPTION_SELECT_COLUMNS}
             FROM transcriptions
             ORDER BY id DESC
             LIMIT ?1"
        ))?;

        let entries = stmt
            .query_map(rusqlite::params![limit as i64], row_to_transcription_entry)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    pub fn latest_transcription(&self) -> Result<Option<TranscriptionEntry>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!(
                "SELECT {TRANSCRIPTION_SELECT_COLUMNS}
                 FROM transcriptions
                 ORDER BY id DESC
                 LIMIT 1"
            ),
            [],
            row_to_transcription_entry,
        )
        .optional()
    }

    #[allow(dead_code)]
    pub fn insert_diarized(
        &self,
        entry: &DiarizedTranscriptionEntry,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let tags_json = serde_json::to_string(&entry.context_tags).unwrap_or_else(|_| "[]".into());
        let segments_json = serde_json::to_string(&entry.segments).unwrap_or_else(|_| "[]".into());
        conn.execute(
            "INSERT INTO diarized_transcriptions (timestamp, duration_secs, file_path, language, model, segments_json, full_text, word_count, speaker_count, context_tags) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                entry.timestamp.to_rfc3339(),
                entry.duration_secs,
                entry.file_path,
                entry.language,
                entry.model,
                segments_json,
                entry.full_text,
                entry.word_count,
                entry.speaker_count,
                tags_json,
            ],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn recent_diarized(
        &self,
        limit: usize,
    ) -> Result<Vec<DiarizedTranscriptionEntry>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT timestamp, duration_secs, file_path, language, model, segments_json, full_text, word_count, speaker_count, context_tags FROM diarized_transcriptions ORDER BY id DESC LIMIT ?1",
        )?;

        let entries = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                let ts_str: String = row.get(0)?;
                let timestamp: DateTime<Utc> = ts_str.parse().map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                let segments_str: String = row.get(5)?;
                let segments = serde_json::from_str(&segments_str).unwrap_or_default();
                let tags_str: String = row.get::<_, String>(9).unwrap_or_else(|_| "[]".into());
                let context_tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();
                Ok(DiarizedTranscriptionEntry {
                    timestamp,
                    duration_secs: row.get::<_, i64>(1)? as u64,
                    file_path: row.get(2)?,
                    language: row.get(3)?,
                    model: row.get(4)?,
                    segments,
                    full_text: row.get(6)?,
                    word_count: row.get::<_, i64>(7)? as u64,
                    speaker_count: row.get::<_, i64>(8)? as u32,
                    context_tags,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    pub fn total_stats(&self) -> Result<(u64, u64), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(word_count), 0) FROM transcriptions",
            [],
            |row| {
                let count: i64 = row.get(0)?;
                let words: i64 = row.get(1)?;
                Ok((count as u64, words as u64))
            },
        )
    }

    #[allow(dead_code)]
    pub fn insert_job(
        &self,
        job: &NewTranscriptionJob,
    ) -> Result<TranscriptionJob, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let settings_json = serde_json::to_string(&job.settings)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

        conn.execute(
            "INSERT INTO transcription_jobs (
                created_at,
                updated_at,
                source_media_path,
                source_filename,
                status,
                mode,
                settings_json
            ) VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                now,
                job.source_media_path,
                job.source_filename,
                JobStatus::Queued.as_str(),
                job.mode.as_str(),
                settings_json,
            ],
        )?;

        let id = conn.last_insert_rowid();
        get_job_on(&conn, id)
    }

    #[allow(dead_code)]
    pub fn get_job(&self, id: i64) -> Result<Option<TranscriptionJob>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        get_optional_job_on(&conn, id)
    }

    #[allow(dead_code)]
    pub fn queued_jobs(&self, limit: usize) -> Result<Vec<TranscriptionJob>, rusqlite::Error> {
        self.jobs_by_status(&[JobStatus::Queued], limit, "created_at ASC, id ASC")
    }

    #[allow(dead_code)]
    pub fn active_jobs(&self) -> Result<Vec<TranscriptionJob>, rusqlite::Error> {
        self.jobs_by_status(&ACTIVE_JOB_STATUSES, 16, "started_at ASC, id ASC")
    }

    #[allow(dead_code)]
    pub fn recent_terminal_jobs(
        &self,
        limit: usize,
    ) -> Result<Vec<TranscriptionJob>, rusqlite::Error> {
        self.jobs_by_status(
            &TERMINAL_JOB_STATUSES,
            limit,
            "COALESCE(completed_at, updated_at) DESC, id DESC",
        )
    }

    #[allow(dead_code)]
    pub fn select_next_job_for_worker(&self) -> Result<Option<TranscriptionJob>, rusqlite::Error> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let active_statuses = job_statuses_sql(&ACTIVE_JOB_STATUSES);

        let active_count: i64 = tx.query_row(
            &format!(
                "SELECT COUNT(*)
                 FROM transcription_jobs
                 WHERE status IN ({active_statuses})"
            ),
            [],
            |row| row.get(0),
        )?;

        if active_count > 0 {
            tx.commit()?;
            return Ok(None);
        }

        let job_id = tx
            .query_row(
                "SELECT id
                 FROM transcription_jobs
                 WHERE status = 'queued'
                 ORDER BY created_at ASC, id ASC
                 LIMIT 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;

        let Some(job_id) = job_id else {
            tx.commit()?;
            return Ok(None);
        };

        let now = Utc::now().to_rfc3339();
        tx.execute(
            "UPDATE transcription_jobs
             SET status = ?1,
                 started_at = COALESCE(started_at, ?2),
                 updated_at = ?2
             WHERE id = ?3 AND status = 'queued'",
            rusqlite::params![JobStatus::PreparingMedia.as_str(), now, job_id],
        )?;

        let job = get_job_on(&tx, job_id)?;
        tx.commit()?;
        Ok(Some(job))
    }

    #[allow(dead_code)]
    pub fn update_job_status(
        &self,
        id: i64,
        next: JobStatus,
        error_message: Option<&str>,
    ) -> Result<TranscriptionJob, rusqlite::Error> {
        self.update_job_status_with_retention(
            id,
            next,
            error_message,
            DEFAULT_SOURCE_MEDIA_RETENTION_DAYS,
        )
    }

    #[allow(dead_code)]
    pub fn update_job_status_with_retention(
        &self,
        id: i64,
        next: JobStatus,
        error_message: Option<&str>,
        retention_days: i64,
    ) -> Result<TranscriptionJob, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let current = get_job_on(&conn, id)?;
        if !current.status.can_transition_to(next) {
            return Err(rusqlite::Error::InvalidQuery);
        }

        let now = Utc::now().to_rfc3339();
        let started_at = if next.is_active() && current.started_at.is_none() {
            Some(now.as_str())
        } else {
            None
        };
        let completed_at = if next.is_terminal() {
            Some(now.as_str())
        } else {
            None
        };
        let source_media_retain_until = completed_at.map(|_| retain_until_rfc3339(retention_days));

        conn.execute(
            "UPDATE transcription_jobs
             SET status = ?1,
                 error_message = ?2,
                 updated_at = ?3,
                 started_at = COALESCE(started_at, ?4),
                 completed_at = COALESCE(completed_at, ?5),
                 source_media_retain_until = COALESCE(source_media_retain_until, ?6)
             WHERE id = ?7",
            rusqlite::params![
                next.as_str(),
                error_message,
                now,
                started_at,
                completed_at,
                source_media_retain_until,
                id,
            ],
        )?;

        get_job_on(&conn, id)
    }

    #[allow(dead_code)]
    pub fn update_job_progress(
        &self,
        id: i64,
        progress_pct: u32,
    ) -> Result<TranscriptionJob, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let pct = progress_pct.min(100);
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE transcription_jobs
             SET progress_pct = ?1, updated_at = ?2
             WHERE id = ?3",
            rusqlite::params![pct, now, id],
        )?;
        get_job_on(&conn, id)
    }

    #[allow(dead_code)]
    pub fn update_job_working_audio_path(
        &self,
        id: i64,
        working_audio_path: &str,
    ) -> Result<TranscriptionJob, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE transcription_jobs
             SET working_audio_path = ?1, updated_at = ?2
             WHERE id = ?3",
            rusqlite::params![working_audio_path, now, id],
        )?;
        get_job_on(&conn, id)
    }

    #[allow(dead_code)]
    pub fn complete_job_with_result(
        &self,
        id: i64,
        result: &TranscriptResult,
    ) -> Result<TranscriptionJob, rusqlite::Error> {
        self.complete_job_with_result_and_retention(id, result, DEFAULT_SOURCE_MEDIA_RETENTION_DAYS)
    }

    #[allow(dead_code)]
    pub fn complete_job_with_result_and_retention(
        &self,
        id: i64,
        result: &TranscriptResult,
        retention_days: i64,
    ) -> Result<TranscriptionJob, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let current = get_job_on(&conn, id)?;
        if !current.status.can_transition_to(JobStatus::Succeeded) {
            return Err(rusqlite::Error::InvalidQuery);
        }

        let now = Utc::now().to_rfc3339();
        let source_media_retain_until = retain_until_rfc3339(retention_days);
        let result_json = serde_json::to_string(result)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

        conn.execute(
            "UPDATE transcription_jobs
             SET status = ?1,
                 result_json = ?2,
                 progress_pct = 100,
                 updated_at = ?3,
                 completed_at = COALESCE(completed_at, ?3),
                 source_media_retain_until = COALESCE(source_media_retain_until, ?4)
             WHERE id = ?5",
            rusqlite::params![
                JobStatus::Succeeded.as_str(),
                result_json,
                now,
                source_media_retain_until,
                id
            ],
        )?;

        get_job_on(&conn, id)
    }

    #[allow(dead_code)]
    pub fn cancel_job(&self, id: i64) -> Result<CancelJobOutcome, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let current = get_job_on(&conn, id)?;
        if current.status.is_terminal() {
            return Ok(CancelJobOutcome::AlreadyTerminal(current));
        }

        let now = Utc::now().to_rfc3339();
        let source_media_retain_until = default_retain_until_rfc3339();
        if current.status == JobStatus::Queued {
            conn.execute(
                "UPDATE transcription_jobs
                 SET status = ?1,
                     completed_at = COALESCE(completed_at, ?2),
                     updated_at = ?2,
                     source_media_retain_until = COALESCE(source_media_retain_until, ?3)
                 WHERE id = ?4 AND status = 'queued'",
                rusqlite::params![
                    JobStatus::Cancelled.as_str(),
                    now,
                    source_media_retain_until,
                    id
                ],
            )?;
            return Ok(CancelJobOutcome::Cancelled(get_job_on(&conn, id)?));
        }

        if current.status.is_cancellable() {
            conn.execute(
                "UPDATE transcription_jobs
                 SET cancel_requested = 1, updated_at = ?1
                 WHERE id = ?2",
                rusqlite::params![now, id],
            )?;
            return Ok(CancelJobOutcome::CancellationRequested(get_job_on(
                &conn, id,
            )?));
        }

        Ok(CancelJobOutcome::AlreadyTerminal(current))
    }

    pub fn recover_interrupted_active_jobs(&self) -> Result<usize, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let source_media_retain_until = default_retain_until_rfc3339();
        let active_statuses = job_statuses_sql(&ACTIVE_JOB_STATUSES);
        conn.execute(
            &format!(
                "UPDATE transcription_jobs
                 SET status = 'failed',
                     error_message = COALESCE(error_message, 'Daemon restarted before this job reached a terminal state'),
                     updated_at = ?1,
                     completed_at = COALESCE(completed_at, ?1),
                     source_media_retain_until = COALESCE(source_media_retain_until, ?2)
                 WHERE status IN ({active_statuses})"
            ),
            rusqlite::params![now, source_media_retain_until],
        )
    }

    #[allow(dead_code)]
    pub fn terminal_jobs_with_working_audio(
        &self,
        limit: usize,
    ) -> Result<Vec<TranscriptionJob>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let terminal_statuses = job_statuses_sql(&TERMINAL_JOB_STATUSES);
        let sql = format!(
            "SELECT {JOB_SELECT_COLUMNS}
             FROM transcription_jobs
             WHERE status IN ({terminal_statuses})
               AND working_audio_path IS NOT NULL
             ORDER BY COALESCE(completed_at, updated_at) ASC, id ASC
             LIMIT ?1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let jobs = stmt
            .query_map(rusqlite::params![limit as i64], row_to_job)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(jobs)
    }

    #[allow(dead_code)]
    pub fn clear_job_working_audio_path(
        &self,
        id: i64,
    ) -> Result<TranscriptionJob, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE transcription_jobs
             SET working_audio_path = NULL, updated_at = ?1
             WHERE id = ?2",
            rusqlite::params![now, id],
        )?;
        get_job_on(&conn, id)
    }

    #[allow(dead_code)]
    pub fn expired_source_media_jobs(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<TranscriptionJob>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let terminal_statuses = job_statuses_sql(&TERMINAL_JOB_STATUSES);
        let sql = format!(
            "SELECT {JOB_SELECT_COLUMNS}
             FROM transcription_jobs
             WHERE status IN ({terminal_statuses})
               AND source_media_retain_until IS NOT NULL
               AND source_media_cleaned_at IS NULL
               AND source_media_retain_until <= ?1
             ORDER BY source_media_retain_until ASC, id ASC
             LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql)?;
        let jobs = stmt
            .query_map(
                rusqlite::params![now.to_rfc3339(), limit as i64],
                row_to_job,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(jobs)
    }

    #[allow(dead_code)]
    pub fn mark_job_source_media_cleaned(
        &self,
        id: i64,
    ) -> Result<TranscriptionJob, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE transcription_jobs
             SET source_media_cleaned_at = COALESCE(source_media_cleaned_at, ?1)
             WHERE id = ?2",
            rusqlite::params![now, id],
        )?;
        get_job_on(&conn, id)
    }

    pub fn expired_transcription_audio(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<ExpiredTranscriptionAudio>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, audio_path
             FROM transcriptions
             WHERE audio_path IS NOT NULL
               AND audio_path != ''
               AND audio_expires_at IS NOT NULL
               AND audio_cleaned_at IS NULL
               AND audio_expires_at <= ?1
             ORDER BY audio_expires_at ASC, id ASC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![now.to_rfc3339(), limit as i64], |row| {
                Ok(ExpiredTranscriptionAudio {
                    id: row.get(0)?,
                    audio_path: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn mark_transcription_audio_cleaned(&self, id: i64) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE transcriptions
             SET audio_cleaned_at = COALESCE(audio_cleaned_at, ?1)
             WHERE id = ?2",
            rusqlite::params![now, id],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_hotkey_transcription_failure(
        &self,
        recording_id: &str,
        duration_secs: u64,
        context_tags: &[String],
        tmux_target: Option<&str>,
        auto_enter: bool,
        error_message: &str,
        audio_path: Option<&str>,
        audio_filename: Option<&str>,
        audio_expires_at: Option<DateTime<Utc>>,
    ) -> Result<HotkeyTranscriptionFailure, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let tags_json = serde_json::to_string(context_tags).unwrap_or_else(|_| "[]".into());
        let audio_expires_at = audio_expires_at.map(|dt| dt.to_rfc3339());
        conn.execute(
            "INSERT INTO hotkey_transcription_failures (
                created_at,
                updated_at,
                recording_id,
                duration_secs,
                context_tags,
                tmux_target,
                auto_enter,
                error_message,
                audio_path,
                audio_filename,
                audio_expires_at
            ) VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                now,
                recording_id,
                duration_secs as i64,
                tags_json,
                tmux_target,
                auto_enter,
                error_message,
                audio_path,
                audio_filename,
                audio_expires_at,
            ],
        )?;
        let id = conn.last_insert_rowid();
        get_hotkey_transcription_failure_on(&conn, id)
    }

    pub fn get_hotkey_transcription_failure(
        &self,
        id: i64,
    ) -> Result<Option<HotkeyTranscriptionFailure>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {HOTKEY_FAILURE_SELECT_COLUMNS} FROM hotkey_transcription_failures WHERE id = ?1"
        );
        conn.query_row(
            &sql,
            rusqlite::params![id],
            row_to_hotkey_transcription_failure,
        )
        .optional()
    }

    pub fn unresolved_hotkey_transcription_failures(
        &self,
        limit: usize,
    ) -> Result<Vec<HotkeyTranscriptionFailure>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {HOTKEY_FAILURE_SELECT_COLUMNS}
             FROM hotkey_transcription_failures
             WHERE resolved_at IS NULL
             ORDER BY created_at DESC, id DESC
             LIMIT ?1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(
                rusqlite::params![limit as i64],
                row_to_hotkey_transcription_failure,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn latest_unresolved_hotkey_transcription_failure(
        &self,
    ) -> Result<Option<HotkeyTranscriptionFailure>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {HOTKEY_FAILURE_SELECT_COLUMNS}
             FROM hotkey_transcription_failures
             WHERE resolved_at IS NULL
             ORDER BY created_at DESC, id DESC
             LIMIT 1"
        );
        conn.query_row(&sql, [], row_to_hotkey_transcription_failure)
            .optional()
    }

    pub fn mark_hotkey_transcription_failure_resolved(
        &self,
        id: i64,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE hotkey_transcription_failures
             SET resolved_at = COALESCE(resolved_at, ?1), updated_at = ?1
             WHERE id = ?2",
            rusqlite::params![now, id],
        )?;
        Ok(())
    }

    pub fn record_hotkey_transcription_failure_retry_error(
        &self,
        id: i64,
        error_message: &str,
    ) -> Result<HotkeyTranscriptionFailure, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE hotkey_transcription_failures
             SET retry_count = retry_count + 1,
                 error_message = ?1,
                 updated_at = ?2
             WHERE id = ?3",
            rusqlite::params![error_message, now, id],
        )?;
        get_hotkey_transcription_failure_on(&conn, id)
    }

    pub fn expired_hotkey_transcription_failure_audio(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<ExpiredTranscriptionAudio>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, audio_path
             FROM hotkey_transcription_failures
             WHERE audio_path IS NOT NULL
               AND audio_path != ''
               AND audio_expires_at IS NOT NULL
               AND audio_cleaned_at IS NULL
               AND audio_expires_at <= ?1
             ORDER BY audio_expires_at ASC, id ASC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![now.to_rfc3339(), limit as i64], |row| {
                Ok(ExpiredTranscriptionAudio {
                    id: row.get(0)?,
                    audio_path: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn mark_hotkey_transcription_failure_audio_cleaned(
        &self,
        id: i64,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE hotkey_transcription_failures
             SET audio_cleaned_at = COALESCE(audio_cleaned_at, ?1)
             WHERE id = ?2",
            rusqlite::params![now, id],
        )?;
        Ok(())
    }

    fn jobs_by_status(
        &self,
        statuses: &[JobStatus],
        limit: usize,
        order_by: &str,
    ) -> Result<Vec<TranscriptionJob>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let status_values = job_statuses_sql(statuses);
        let sql = format!(
            "SELECT {}
             FROM transcription_jobs
             WHERE status IN ({status_values})
             ORDER BY {order_by}
             LIMIT ?1",
            JOB_SELECT_COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;
        let jobs = stmt
            .query_map(rusqlite::params![limit as i64], row_to_job)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(jobs)
    }
}

const TRANSCRIPTION_SELECT_COLUMNS: &str = "timestamp, duration_secs, word_count, text, context_tags, recording_id, audio_path, audio_filename, audio_expires_at, audio_cleaned_at";

const JOB_SELECT_COLUMNS: &str = "id, created_at, updated_at, source_media_path, source_filename, working_audio_path, status, mode, settings_json, result_json, error_message, progress_pct, cancel_requested, started_at, completed_at, source_media_retain_until";

const HOTKEY_FAILURE_SELECT_COLUMNS: &str = "id, created_at, updated_at, recording_id, duration_secs, context_tags, tmux_target, auto_enter, error_message, audio_path, audio_filename, audio_expires_at, audio_cleaned_at, retry_count, resolved_at";

mod hotkey_failure_column {
    pub const ID: usize = 0;
    pub const CREATED_AT: usize = 1;
    pub const UPDATED_AT: usize = 2;
    pub const RECORDING_ID: usize = 3;
    pub const DURATION_SECS: usize = 4;
    pub const CONTEXT_TAGS: usize = 5;
    pub const TMUX_TARGET: usize = 6;
    pub const AUTO_ENTER: usize = 7;
    pub const ERROR_MESSAGE: usize = 8;
    pub const AUDIO_PATH: usize = 9;
    pub const AUDIO_FILENAME: usize = 10;
    pub const AUDIO_EXPIRES_AT: usize = 11;
    pub const AUDIO_CLEANED_AT: usize = 12;
    pub const RETRY_COUNT: usize = 13;
    pub const RESOLVED_AT: usize = 14;
}

fn get_hotkey_transcription_failure_on(
    conn: &Connection,
    id: i64,
) -> Result<HotkeyTranscriptionFailure, rusqlite::Error> {
    conn.query_row(
        &format!("SELECT {HOTKEY_FAILURE_SELECT_COLUMNS} FROM hotkey_transcription_failures WHERE id = ?1"),
        rusqlite::params![id],
        row_to_hotkey_transcription_failure,
    )
}

fn row_to_hotkey_transcription_failure(
    row: &rusqlite::Row<'_>,
) -> Result<HotkeyTranscriptionFailure, rusqlite::Error> {
    let tags_str: String = row
        .get::<_, String>(hotkey_failure_column::CONTEXT_TAGS)
        .unwrap_or_else(|_| "[]".into());
    let context_tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();

    Ok(HotkeyTranscriptionFailure {
        id: row.get(hotkey_failure_column::ID)?,
        created_at: parse_datetime(row, hotkey_failure_column::CREATED_AT)?,
        updated_at: parse_datetime(row, hotkey_failure_column::UPDATED_AT)?,
        recording_id: row.get(hotkey_failure_column::RECORDING_ID)?,
        duration_secs: row.get::<_, i64>(hotkey_failure_column::DURATION_SECS)? as u64,
        context_tags,
        tmux_target: row.get(hotkey_failure_column::TMUX_TARGET)?,
        auto_enter: row.get(hotkey_failure_column::AUTO_ENTER)?,
        error_message: row.get(hotkey_failure_column::ERROR_MESSAGE)?,
        audio_path: row.get(hotkey_failure_column::AUDIO_PATH)?,
        audio_filename: row.get(hotkey_failure_column::AUDIO_FILENAME)?,
        audio_expires_at: parse_optional_datetime(row, hotkey_failure_column::AUDIO_EXPIRES_AT)?,
        audio_cleaned_at: parse_optional_datetime(row, hotkey_failure_column::AUDIO_CLEANED_AT)?,
        retry_count: row.get(hotkey_failure_column::RETRY_COUNT)?,
        resolved_at: parse_optional_datetime(row, hotkey_failure_column::RESOLVED_AT)?,
    })
}

mod job_column {
    pub const ID: usize = 0;
    pub const CREATED_AT: usize = 1;
    pub const UPDATED_AT: usize = 2;
    pub const SOURCE_MEDIA_PATH: usize = 3;
    pub const SOURCE_FILENAME: usize = 4;
    pub const WORKING_AUDIO_PATH: usize = 5;
    pub const STATUS: usize = 6;
    pub const MODE: usize = 7;
    pub const SETTINGS_JSON: usize = 8;
    pub const RESULT_JSON: usize = 9;
    pub const ERROR_MESSAGE: usize = 10;
    pub const PROGRESS_PCT: usize = 11;
    pub const CANCEL_REQUESTED: usize = 12;
    pub const STARTED_AT: usize = 13;
    pub const COMPLETED_AT: usize = 14;
    pub const SOURCE_MEDIA_RETAIN_UNTIL: usize = 15;
}

fn job_statuses_sql(statuses: &[JobStatus]) -> String {
    statuses
        .iter()
        .map(|status| format!("'{}'", status.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn get_optional_job_on(
    conn: &Connection,
    id: i64,
) -> Result<Option<TranscriptionJob>, rusqlite::Error> {
    conn.query_row(
        &format!("SELECT {JOB_SELECT_COLUMNS} FROM transcription_jobs WHERE id = ?1"),
        rusqlite::params![id],
        row_to_job,
    )
    .optional()
}

fn get_job_on(conn: &Connection, id: i64) -> Result<TranscriptionJob, rusqlite::Error> {
    get_optional_job_on(conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

fn row_to_job(row: &rusqlite::Row<'_>) -> Result<TranscriptionJob, rusqlite::Error> {
    let created_at = parse_datetime(row, job_column::CREATED_AT)?;
    let updated_at = parse_datetime(row, job_column::UPDATED_AT)?;
    let status_text: String = row.get(job_column::STATUS)?;
    let mode_text: String = row.get(job_column::MODE)?;
    let settings_json: String = row.get(job_column::SETTINGS_JSON)?;
    let result_json: Option<String> = row.get(job_column::RESULT_JSON)?;
    let started_at = parse_optional_datetime(row, job_column::STARTED_AT)?;
    let completed_at = parse_optional_datetime(row, job_column::COMPLETED_AT)?;
    let source_media_retain_until =
        parse_optional_datetime(row, job_column::SOURCE_MEDIA_RETAIN_UNTIL)?;

    Ok(TranscriptionJob {
        id: row.get(job_column::ID)?,
        created_at,
        updated_at,
        source_media_path: row.get(job_column::SOURCE_MEDIA_PATH)?,
        source_filename: row.get(job_column::SOURCE_FILENAME)?,
        working_audio_path: row.get(job_column::WORKING_AUDIO_PATH)?,
        status: JobStatus::try_from(status_text.as_str()).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                job_column::STATUS,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })?,
        mode: TranscriptionMode::try_from(mode_text.as_str()).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                job_column::MODE,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })?,
        settings: serde_json::from_str::<PerJobSettings>(&settings_json).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                job_column::SETTINGS_JSON,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })?,
        result: result_json
            .map(|json| {
                serde_json::from_str::<TranscriptResult>(&json).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        job_column::RESULT_JSON,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })
            })
            .transpose()?,
        error_message: row.get(job_column::ERROR_MESSAGE)?,
        progress_pct: row.get::<_, i64>(job_column::PROGRESS_PCT)? as u32,
        cancel_requested: row.get::<_, i64>(job_column::CANCEL_REQUESTED)? != 0,
        started_at,
        completed_at,
        source_media_retain_until,
    })
}

fn retain_until_rfc3339(retention_days: i64) -> String {
    (Utc::now() + Duration::days(retention_days.max(0))).to_rfc3339()
}

fn default_retain_until_rfc3339() -> String {
    retain_until_rfc3339(DEFAULT_SOURCE_MEDIA_RETENTION_DAYS)
}

fn row_to_transcription_entry(
    row: &rusqlite::Row<'_>,
) -> Result<TranscriptionEntry, rusqlite::Error> {
    let timestamp = parse_datetime(row, 0)?;
    let tags_str: String = row.get::<_, String>(4).unwrap_or_else(|_| "[]".into());
    let context_tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();

    Ok(TranscriptionEntry {
        timestamp,
        duration_secs: row.get::<_, i64>(1)? as u64,
        word_count: row.get::<_, i64>(2)? as u64,
        text: row.get(3)?,
        context_tags,
        recording_id: row.get(5)?,
        audio_path: row.get(6)?,
        audio_filename: row.get(7)?,
        audio_expires_at: parse_optional_datetime(row, 8)?,
        audio_cleaned_at: parse_optional_datetime(row, 9)?,
    })
}

fn parse_datetime(row: &rusqlite::Row<'_>, index: usize) -> Result<DateTime<Utc>, rusqlite::Error> {
    let value: String = row.get(index)?;
    value.parse().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn parse_optional_datetime(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> Result<Option<DateTime<Utc>>, rusqlite::Error> {
    let value: Option<String> = row.get(index)?;
    value
        .map(|timestamp| {
            timestamp.parse().map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> Db {
        let path = std::env::temp_dir().join(format!(
            "kloyce-job-test-{}-{}.db",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        Db::open(path).unwrap()
    }

    fn new_job(name: &str) -> NewTranscriptionJob {
        NewTranscriptionJob {
            source_media_path: format!("/tmp/{name}"),
            source_filename: name.to_string(),
            mode: TranscriptionMode::Standard,
            settings: PerJobSettings::standard("small.en", vec!["test".to_string()]),
        }
    }

    fn transcription_entry(
        text: &str,
        audio_path: Option<&str>,
        audio_expires_at: Option<DateTime<Utc>>,
    ) -> TranscriptionEntry {
        TranscriptionEntry {
            timestamp: Utc::now(),
            duration_secs: 12,
            word_count: text.split_whitespace().count() as u64,
            text: text.to_string(),
            context_tags: vec!["test".to_string()],
            recording_id: Some("20260630-120000.000000".to_string()),
            audio_path: audio_path.map(str::to_string),
            audio_filename: audio_path
                .and_then(|path| std::path::Path::new(path).file_name())
                .and_then(|name| name.to_str())
                .map(str::to_string),
            audio_expires_at,
            audio_cleaned_at: None,
        }
    }

    #[test]
    fn transcription_audio_metadata_persists_in_recent_history() {
        let db = temp_db();
        let expires = Utc::now() + Duration::days(7);
        let entry = transcription_entry(
            "retained audio transcript",
            Some("/tmp/kloyce-retained.mp3"),
            Some(expires),
        );

        db.insert(&entry).unwrap();

        let latest = db.latest_transcription().unwrap().unwrap();
        assert_eq!(latest.text, "retained audio transcript");
        assert_eq!(
            latest.recording_id.as_deref(),
            Some("20260630-120000.000000")
        );
        assert_eq!(
            latest.audio_path.as_deref(),
            Some("/tmp/kloyce-retained.mp3")
        );
        assert_eq!(
            latest.audio_filename.as_deref(),
            Some("kloyce-retained.mp3")
        );
        assert_eq!(latest.audio_expires_at, Some(expires));
        assert_eq!(latest.context_tags, vec!["test"]);

        let recent = db.recent(1).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].audio_path, latest.audio_path);
    }

    #[test]
    fn expired_transcription_audio_is_selected_and_marked_cleaned() {
        let db = temp_db();
        let now = Utc::now();
        db.insert(&transcription_entry(
            "expired audio",
            Some("/tmp/expired.mp3"),
            Some(now - Duration::minutes(1)),
        ))
        .unwrap();
        db.insert(&transcription_entry(
            "fresh audio",
            Some("/tmp/fresh.mp3"),
            Some(now + Duration::days(1)),
        ))
        .unwrap();

        let candidates = db.expired_transcription_audio(now, 10).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].audio_path, "/tmp/expired.mp3");

        db.mark_transcription_audio_cleaned(candidates[0].id)
            .unwrap();

        assert!(db.expired_transcription_audio(now, 10).unwrap().is_empty());
        let recent = db.recent(2).unwrap();
        let expired = recent
            .into_iter()
            .find(|entry| entry.text == "expired audio")
            .unwrap();
        assert!(expired.audio_cleaned_at.is_some());
    }

    #[test]
    fn jobs_are_inserted_queued_and_selected_fifo_one_active_at_a_time() {
        let db = temp_db();
        let first = db.insert_job(&new_job("first.wav")).unwrap();
        let second = db.insert_job(&new_job("second.wav")).unwrap();

        assert_eq!(first.status, JobStatus::Queued);
        assert_eq!(second.status, JobStatus::Queued);

        let selected = db.select_next_job_for_worker().unwrap().unwrap();
        assert_eq!(selected.id, first.id);
        assert_eq!(selected.status, JobStatus::PreparingMedia);

        assert!(db.select_next_job_for_worker().unwrap().is_none());

        let active = db.active_jobs().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, first.id);

        db.update_job_status(first.id, JobStatus::Transcribing, None)
            .unwrap();
        db.complete_job_with_result(
            first.id,
            &TranscriptResult {
                text: "hello world".to_string(),
                word_count: 2,
                duration_secs: 1,
                language: None,
                speaker_count: None,
                segments: Vec::new(),
            },
        )
        .unwrap();

        let selected = db.select_next_job_for_worker().unwrap().unwrap();
        assert_eq!(selected.id, second.id);
        assert_eq!(selected.status, JobStatus::PreparingMedia);
    }

    #[test]
    fn queued_cancellation_skips_worker_and_terminal_jobs_cannot_be_cancelled() {
        let db = temp_db();
        let queued = db.insert_job(&new_job("queued.wav")).unwrap();
        let next = db.insert_job(&new_job("next.wav")).unwrap();

        match db.cancel_job(queued.id).unwrap() {
            CancelJobOutcome::Cancelled(job) => assert_eq!(job.status, JobStatus::Cancelled),
            other => panic!("unexpected cancellation outcome: {other:?}"),
        }

        let selected = db.select_next_job_for_worker().unwrap().unwrap();
        assert_eq!(selected.id, next.id);

        match db.cancel_job(queued.id).unwrap() {
            CancelJobOutcome::AlreadyTerminal(job) => {
                assert_eq!(job.status, JobStatus::Cancelled)
            }
            other => panic!("unexpected terminal cancellation outcome: {other:?}"),
        }
    }

    #[test]
    fn active_cancellation_sets_request_flag_and_startup_recovery_fails_interrupted_jobs() {
        let db = temp_db();
        let job = db.insert_job(&new_job("active.wav")).unwrap();
        let selected = db.select_next_job_for_worker().unwrap().unwrap();
        assert_eq!(selected.id, job.id);

        match db.cancel_job(job.id).unwrap() {
            CancelJobOutcome::CancellationRequested(job) => {
                assert_eq!(job.status, JobStatus::PreparingMedia);
                assert!(job.cancel_requested);
            }
            other => panic!("unexpected active cancellation outcome: {other:?}"),
        }

        let recovered = db.recover_interrupted_active_jobs().unwrap();
        assert_eq!(recovered, 1);

        let recovered_job = db.get_job(job.id).unwrap().unwrap();
        assert_eq!(recovered_job.status, JobStatus::Failed);
        assert!(recovered_job.error_message.is_some());
        assert!(recovered_job.completed_at.is_some());
    }

    #[test]
    fn terminal_jobs_get_source_media_retention_metadata() {
        let db = temp_db();
        let job = db.insert_job(&new_job("retained.wav")).unwrap();
        let selected = db.select_next_job_for_worker().unwrap().unwrap();
        assert_eq!(selected.id, job.id);

        let failed = db
            .update_job_status(job.id, JobStatus::Failed, Some("no audio stream"))
            .unwrap();

        assert_eq!(failed.status, JobStatus::Failed);
        assert!(failed.completed_at.is_some());
        assert!(failed.source_media_retain_until.is_some());
        let retain_until = failed.source_media_retain_until.unwrap();
        let completed_at = failed.completed_at.unwrap();
        assert!(retain_until > completed_at);
    }

    #[test]
    fn expired_terminal_jobs_are_source_media_cleanup_candidates_with_results() {
        let db = temp_db();
        let job = db.insert_job(&new_job("expired-source.wav")).unwrap();
        db.select_next_job_for_worker().unwrap().unwrap();
        db.update_job_status(job.id, JobStatus::Transcribing, None)
            .unwrap();
        db.complete_job_with_result_and_retention(
            job.id,
            &TranscriptResult {
                text: "kept transcript".to_string(),
                word_count: 2,
                duration_secs: 3,
                language: None,
                speaker_count: None,
                segments: Vec::new(),
            },
            0,
        )
        .unwrap();

        let candidates = db.expired_source_media_jobs(Utc::now(), 10).unwrap();

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, job.id);
        assert_eq!(
            candidates[0]
                .result
                .as_ref()
                .map(|result| result.text.as_str()),
            Some("kept transcript")
        );

        db.mark_job_source_media_cleaned(job.id).unwrap();

        let candidates = db.expired_source_media_jobs(Utc::now(), 10).unwrap();
        assert!(candidates.is_empty());
    }

    #[test]
    fn hotkey_transcription_failure_persists_and_lists_unresolved() {
        let db = temp_db();
        let expires = Utc::now() + Duration::hours(24);
        let failure = db
            .insert_hotkey_transcription_failure(
                "20260811-090000.000000",
                12,
                &["test".to_string()],
                Some("tmux-session:0.0"),
                true,
                "whisper-cli exited with status 1",
                Some("/tmp/kloyce-failed.mp3"),
                Some("kloyce-failed.mp3"),
                Some(expires),
            )
            .unwrap();

        assert_eq!(failure.retry_count, 0);
        assert!(failure.resolved_at.is_none());
        assert_eq!(
            failure.audio_path.as_deref(),
            Some("/tmp/kloyce-failed.mp3")
        );
        assert_eq!(failure.tmux_target.as_deref(), Some("tmux-session:0.0"));
        assert!(failure.auto_enter);

        let unresolved = db.unresolved_hotkey_transcription_failures(10).unwrap();
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].id, failure.id);

        let last = db
            .latest_unresolved_hotkey_transcription_failure()
            .unwrap()
            .unwrap();
        assert_eq!(last.id, failure.id);

        let retried = db
            .record_hotkey_transcription_failure_retry_error(failure.id, "still failing")
            .unwrap();
        assert_eq!(retried.retry_count, 1);
        assert_eq!(retried.error_message, "still failing");

        db.mark_hotkey_transcription_failure_resolved(failure.id)
            .unwrap();
        assert!(db
            .unresolved_hotkey_transcription_failures(10)
            .unwrap()
            .is_empty());
        assert!(db
            .latest_unresolved_hotkey_transcription_failure()
            .unwrap()
            .is_none());

        let resolved = db
            .get_hotkey_transcription_failure(failure.id)
            .unwrap()
            .unwrap();
        assert!(resolved.resolved_at.is_some());
    }

    #[test]
    fn expired_hotkey_transcription_failure_audio_is_selected_and_marked_cleaned() {
        let db = temp_db();
        let now = Utc::now();
        let expired = db
            .insert_hotkey_transcription_failure(
                "20260811-090100.000000",
                8,
                &[],
                None,
                false,
                "transcription failed",
                Some("/tmp/expired-failure.mp3"),
                Some("expired-failure.mp3"),
                Some(now - Duration::minutes(1)),
            )
            .unwrap();
        db.insert_hotkey_transcription_failure(
            "20260811-090200.000000",
            8,
            &[],
            None,
            false,
            "transcription failed",
            Some("/tmp/fresh-failure.mp3"),
            Some("fresh-failure.mp3"),
            Some(now + Duration::hours(24)),
        )
        .unwrap();

        let candidates = db
            .expired_hotkey_transcription_failure_audio(now, 10)
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, expired.id);

        db.mark_hotkey_transcription_failure_audio_cleaned(expired.id)
            .unwrap();

        assert!(db
            .expired_hotkey_transcription_failure_audio(now, 10)
            .unwrap()
            .is_empty());
        let cleaned = db
            .get_hotkey_transcription_failure(expired.id)
            .unwrap()
            .unwrap();
        assert!(cleaned.audio_cleaned_at.is_some());
    }
}
