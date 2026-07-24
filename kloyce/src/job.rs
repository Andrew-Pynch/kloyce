#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

pub const JOB_STATUS_SQL_CHECK: &str = "'queued', 'preparing_media', 'downloading_model', 'transcribing', 'succeeded', 'failed', 'cancelled'";
pub const TRANSCRIPTION_MODE_SQL_CHECK: &str = "'standard', 'diarized'";
pub const ACTIVE_JOB_STATUSES: [JobStatus; 3] = [
    JobStatus::PreparingMedia,
    JobStatus::DownloadingModel,
    JobStatus::Transcribing,
];
pub const TERMINAL_JOB_STATUSES: [JobStatus; 3] = [
    JobStatus::Succeeded,
    JobStatus::Failed,
    JobStatus::Cancelled,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    PreparingMedia,
    DownloadingModel,
    Transcribing,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::PreparingMedia => "preparing_media",
            JobStatus::DownloadingModel => "downloading_model",
            JobStatus::Transcribing => "transcribing",
            JobStatus::Succeeded => "succeeded",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }

    pub fn is_active(self) -> bool {
        ACTIVE_JOB_STATUSES.contains(&self)
    }

    pub fn is_terminal(self) -> bool {
        TERMINAL_JOB_STATUSES.contains(&self)
    }

    pub fn is_cancellable(self) -> bool {
        matches!(
            self,
            JobStatus::Queued
                | JobStatus::PreparingMedia
                | JobStatus::DownloadingModel
                | JobStatus::Transcribing
        )
    }

    pub fn can_transition_to(self, next: JobStatus) -> bool {
        if self == next {
            return true;
        }

        match self {
            JobStatus::Queued => matches!(next, JobStatus::PreparingMedia | JobStatus::Cancelled),
            JobStatus::PreparingMedia => matches!(
                next,
                JobStatus::DownloadingModel
                    | JobStatus::Transcribing
                    | JobStatus::Failed
                    | JobStatus::Cancelled
            ),
            JobStatus::DownloadingModel => {
                matches!(
                    next,
                    JobStatus::Transcribing | JobStatus::Failed | JobStatus::Cancelled
                )
            }
            JobStatus::Transcribing => {
                matches!(
                    next,
                    JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled
                )
            }
            JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled => false,
        }
    }
}

impl fmt::Display for JobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for JobStatus {
    type Error = InvalidJobStatus;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "queued" => Ok(JobStatus::Queued),
            "preparing_media" => Ok(JobStatus::PreparingMedia),
            "downloading_model" => Ok(JobStatus::DownloadingModel),
            "transcribing" => Ok(JobStatus::Transcribing),
            "succeeded" => Ok(JobStatus::Succeeded),
            "failed" => Ok(JobStatus::Failed),
            "cancelled" => Ok(JobStatus::Cancelled),
            other => Err(InvalidJobStatus(other.to_string())),
        }
    }
}

#[derive(Debug, Clone)]
pub struct InvalidJobStatus(String);

impl fmt::Display for InvalidJobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid transcription job status: {}", self.0)
    }
}

impl Error for InvalidJobStatus {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionMode {
    Standard,
    Diarized,
}

impl TranscriptionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            TranscriptionMode::Standard => "standard",
            TranscriptionMode::Diarized => "diarized",
        }
    }
}

impl fmt::Display for TranscriptionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for TranscriptionMode {
    type Error = InvalidTranscriptionMode;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "standard" => Ok(TranscriptionMode::Standard),
            "diarized" => Ok(TranscriptionMode::Diarized),
            other => Err(InvalidTranscriptionMode(other.to_string())),
        }
    }
}

#[derive(Debug, Clone)]
pub struct InvalidTranscriptionMode(String);

impl fmt::Display for InvalidTranscriptionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid transcription mode: {}", self.0)
    }
}

impl Error for InvalidTranscriptionMode {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerJobSettings {
    pub model: String,
    #[serde(default)]
    pub diarize: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_speakers: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_speakers: Option<u32>,
    #[serde(default)]
    pub context_tags: Vec<String>,
}

impl PerJobSettings {
    pub fn standard(model: impl Into<String>, context_tags: Vec<String>) -> Self {
        Self {
            model: model.into(),
            diarize: false,
            min_speakers: None,
            max_speakers: None,
            context_tags,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptResult {
    pub text: String,
    pub word_count: u64,
    pub duration_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionJob {
    pub id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub source_media_path: String,
    pub source_filename: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_audio_path: Option<String>,
    pub status: JobStatus,
    pub mode: TranscriptionMode,
    pub settings: PerJobSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<TranscriptResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub progress_pct: u32,
    pub cancel_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_media_retain_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTranscriptionJob {
    pub source_media_path: String,
    pub source_filename: String,
    pub mode: TranscriptionMode,
    pub settings: PerJobSettings,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CancelJobOutcome {
    Cancelled(TranscriptionJob),
    CancellationRequested(TranscriptionJob),
    AlreadyTerminal(TranscriptionJob),
}
