use crate::config::{Config, TranscriptionDefaultsUpdate};
use crate::daemon::{Metrics, State};
use crate::db;
use crate::dictionary::Dictionary;
use crate::job::{
    CancelJobOutcome, JobStatus, NewTranscriptionJob, PerJobSettings, TranscriptResult,
    TranscriptionJob, TranscriptionMode,
};
use crate::media::MediaStorage;
use crate::model_catalog::{self, ModelDownloadRegistry};
use crate::platform::output;

use axum::extract::{DefaultBodyLimit, Multipart, Path as AxumPath, State as AxumState};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use std::collections::VecDeque;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

const DASHBOARD_HTML: &str = include_str!("../assets/dashboard.html");
const MAX_UPLOAD_BYTES: usize = 1024 * 1024 * 1024;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TranscriptionEntry {
    pub timestamp: DateTime<Utc>,
    pub duration_secs: u64,
    pub word_count: u64,
    pub text: String,
    #[serde(default)]
    pub context_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_cleaned_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TranscriptSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub speaker: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiarizedTranscriptionEntry {
    pub timestamp: DateTime<Utc>,
    pub duration_secs: u64,
    pub file_path: String,
    pub language: String,
    pub model: String,
    pub segments: Vec<TranscriptSegment>,
    pub full_text: String,
    pub word_count: u64,
    pub speaker_count: u32,
    #[serde(default)]
    pub context_tags: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SseEvent {
    StateChange {
        state: State,
    },
    Transcription(TranscriptionEntry),
    GpuMetrics {
        gpu_name: String,
        utilization_pct: u32,
        vram_used_mb: u64,
        vram_total_mb: u64,
        temperature_c: u32,
        power_draw_w: f64,
        power_limit_w: f64,
        fan_speed_pct: u32,
        timestamp: DateTime<Utc>,
    },
    RecordingProgress {
        duration_secs: f64,
    },
    TranscriptionProgress {
        progress_pct: u32,
        elapsed_secs: f64,
    },
    #[allow(dead_code)]
    AdvancedTranscriptionProgress {
        stage: String,
        progress_pct: u32,
        elapsed_secs: f64,
    },
    #[allow(dead_code)]
    DiarizedTranscription(DiarizedTranscriptionEntry),
    #[allow(dead_code)]
    TranscriptionJobCreated(TranscriptionJob),
    #[allow(dead_code)]
    TranscriptionJobStatus {
        job_id: i64,
        status: JobStatus,
    },
    #[allow(dead_code)]
    TranscriptionJobProgress {
        job_id: i64,
        progress_pct: u32,
        elapsed_secs: f64,
    },
    #[allow(dead_code)]
    TranscriptionJobResult {
        job_id: i64,
        result: TranscriptResult,
    },
}

#[derive(Clone)]
pub struct WebState {
    pub daemon_state: Arc<RwLock<State>>,
    pub metrics: Arc<RwLock<Metrics>>,
    pub history: Arc<Mutex<VecDeque<TranscriptionEntry>>>,
    pub event_tx: broadcast::Sender<SseEvent>,
    pub gpu_latest: Arc<RwLock<Option<crate::platform::GpuMetrics>>>,
    pub config: Arc<RwLock<Config>>,
    pub model_downloads: ModelDownloadRegistry,
    pub database: Arc<db::Db>,
    #[allow(dead_code)]
    pub dictionary: Arc<RwLock<Dictionary>>,
    pub data_dir: PathBuf,
}

pub async fn start_server(state: WebState, port: u16) {
    let app = Router::new()
        .route("/", get(dashboard))
        .route("/api/status", get(api_status))
        .route("/api/history", get(api_history))
        .route("/api/events", get(api_events))
        .route("/api/gpu", get(api_gpu))
        .route(
            "/api/transcription/settings",
            get(api_transcription_settings).post(api_update_transcription_settings),
        )
        .route("/api/transcription/modes", get(api_transcription_modes))
        .route("/api/models/standard", get(api_standard_models))
        .route(
            "/api/models/standard/{model_id}/download",
            post(api_download_standard_model),
        )
        .route("/api/jobs", get(api_jobs))
        .route(
            "/api/jobs/upload",
            post(api_jobs_upload).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/api/jobs/from-path", post(api_jobs_from_path))
        .route("/api/jobs/{id}", get(api_job))
        .route("/api/jobs/{id}/cancel", post(api_cancel_job))
        .route("/api/transcribe", post(api_transcribe))
        .route("/api/transcribe-advanced", post(api_transcribe_advanced))
        .with_state(state);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    tracing::info!("Web dashboard at http://{addr}");

    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        axum::serve(listener, app).await.unwrap();
    });
}

async fn dashboard() -> impl IntoResponse {
    Html(DASHBOARD_HTML)
}

async fn api_status(AxumState(state): AxumState<WebState>) -> impl IntoResponse {
    let daemon_state = *state.daemon_state.read().await;
    let metrics = state.metrics.read().await;

    Json(serde_json::json!({
        "state": daemon_state,
        "metrics": {
            "total_transcriptions": metrics.total_transcriptions,
            "total_words": metrics.total_words,
            "started_at": metrics.started_at,
            "uptime_secs": (Utc::now() - metrics.started_at).num_seconds(),
        }
    }))
}

async fn api_history(AxumState(state): AxumState<WebState>) -> impl IntoResponse {
    let history = state.history.lock().await;
    let entries: Vec<TranscriptionEntry> = history.iter().cloned().collect();
    Json(entries)
}

async fn api_events(
    AxumState(state): AxumState<WebState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.event_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result: Result<SseEvent, _>| {
        result.ok().map(|event| {
            Ok(Event::default()
                .json_data(&event)
                .unwrap_or_else(|_| Event::default().data("error")))
        })
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn api_gpu(AxumState(state): AxumState<WebState>) -> impl IntoResponse {
    let gpu = state.gpu_latest.read().await;
    match &*gpu {
        Some(metrics) => Json(serde_json::json!(metrics)),
        None => Json(serde_json::json!({"error": "GPU metrics unavailable"})),
    }
}

#[derive(Debug, serde::Serialize)]
struct TranscriptionSettingsResponse {
    defaults: crate::config::TranscriptionDefaults,
    mode_availability: Vec<model_catalog::ModeAvailability>,
    filler_removal: bool,
    cleanup_engine: crate::config::CleanupEngine,
    hotkey_audio_retention_enabled: bool,
    hotkey_audio_retention_days: i64,
}

/// Extended settings update — superset of `TranscriptionDefaultsUpdate` that also
/// covers the pipeline toggles (`filler_removal`, `cleanup_engine`).
#[derive(Debug, serde::Deserialize, Default)]
struct TranscriptionSettingsUpdate {
    #[serde(flatten)]
    defaults: TranscriptionDefaultsUpdate,
    filler_removal: Option<bool>,
    cleanup_engine: Option<crate::config::CleanupEngine>,
    hotkey_audio_retention_enabled: Option<bool>,
    hotkey_audio_retention_days: Option<i64>,
}

async fn api_transcription_settings(AxumState(state): AxumState<WebState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    Json(TranscriptionSettingsResponse {
        defaults: config.transcription_defaults.clone(),
        mode_availability: model_catalog::mode_availability(&config),
        filler_removal: config.filler_removal,
        cleanup_engine: config.cleanup_engine,
        hotkey_audio_retention_enabled: config.hotkey_audio_retention_enabled,
        hotkey_audio_retention_days: config.hotkey_audio_retention_days,
    })
}

async fn api_transcription_modes(AxumState(state): AxumState<WebState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    Json(model_catalog::mode_availability(&config))
}

async fn api_update_transcription_settings(
    AxumState(state): AxumState<WebState>,
    Json(update): Json<TranscriptionSettingsUpdate>,
) -> (StatusCode, Json<serde_json::Value>) {
    let mut next = state.config.read().await.clone();

    if let Err(error) = next.apply_transcription_defaults_update(update.defaults) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "status": "error",
                "message": error.to_string(),
            })),
        );
    }

    if let Some(filler_removal) = update.filler_removal {
        next.filler_removal = filler_removal;
    }
    if let Some(cleanup_engine) = update.cleanup_engine {
        next.cleanup_engine = cleanup_engine;
    }
    if let Err(error) = next.apply_hotkey_audio_retention_update(
        update.hotkey_audio_retention_enabled,
        update.hotkey_audio_retention_days,
    ) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "status": "error",
                "message": error.to_string(),
            })),
        );
    }

    if let Err(error) = next.save() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to persist config.toml: {error}"),
            })),
        );
    }

    {
        let mut current = state.config.write().await;
        *current = next.clone();
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "ok",
            "defaults": next.transcription_defaults,
            "mode_availability": model_catalog::mode_availability(&next),
            "filler_removal": next.filler_removal,
            "cleanup_engine": next.cleanup_engine,
            "hotkey_audio_retention_enabled": next.hotkey_audio_retention_enabled,
            "hotkey_audio_retention_days": next.hotkey_audio_retention_days,
        })),
    )
}

async fn api_standard_models(AxumState(state): AxumState<WebState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let models = model_catalog::standard_model_statuses(&config, &state.model_downloads).await;
    Json(serde_json::json!({ "models": models }))
}

#[derive(Debug, serde::Serialize)]
struct ModelDownloadResponse {
    status: &'static str,
    model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

async fn api_download_standard_model(
    AxumState(state): AxumState<WebState>,
    AxumPath(model_id): AxumPath<String>,
) -> (StatusCode, Json<ModelDownloadResponse>) {
    let Some(model) = model_catalog::standard_model_by_id(&model_id) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ModelDownloadResponse {
                status: "error",
                model_id,
                message: Some("Unknown standard model".into()),
            }),
        );
    };

    let config = state.config.read().await.clone();
    let Some(path) = model_catalog::standard_model_path(&config, model.id) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ModelDownloadResponse {
                status: "error",
                model_id: model.id.to_string(),
                message: Some("Unknown standard model".into()),
            }),
        );
    };

    if path.exists() {
        state.model_downloads.clear(model.id).await;
        return (
            StatusCode::OK,
            Json(ModelDownloadResponse {
                status: "installed",
                model_id: model.id.to_string(),
                message: None,
            }),
        );
    }

    if !state.model_downloads.start_download(model.id).await {
        return (
            StatusCode::ACCEPTED,
            Json(ModelDownloadResponse {
                status: "downloading",
                model_id: model.id.to_string(),
                message: None,
            }),
        );
    }

    let downloads = state.model_downloads.clone();
    let model_id_for_task = model.id.to_string();
    let model_id_for_response = model.id.to_string();
    let model_url = model.url.clone();
    tokio::spawn(async move {
        match download_standard_model(&model_url, &path).await {
            Ok(()) => downloads.clear(&model_id_for_task).await,
            Err(error) => downloads.mark_error(&model_id_for_task, error).await,
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(ModelDownloadResponse {
            status: "downloading",
            model_id: model_id_for_response,
            message: None,
        }),
    )
}

async fn download_standard_model(url: &str, path: &std::path::Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("Failed to create model directory: {error}"))?;
    }

    let part_path = path.with_extension("bin.part");
    let status = tokio::process::Command::new("curl")
        .arg("-fL")
        .arg("-o")
        .arg(&part_path)
        .arg(url)
        .status()
        .await
        .map_err(|error| format!("Failed to start curl: {error}"))?;

    if !status.success() {
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(format!("curl exited with status {status}"));
    }

    tokio::fs::rename(&part_path, path)
        .await
        .map_err(|error| format!("Failed to install model: {error}"))?;
    Ok(())
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct JobSubmissionRequest {
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    mode: Option<TranscriptionMode>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    context_tags: Vec<String>,
    #[serde(default)]
    diarize: Option<bool>,
    #[serde(default)]
    min_speakers: Option<u32>,
    #[serde(default)]
    max_speakers: Option<u32>,
}

#[derive(Debug, serde::Serialize)]
struct JobCreatedResponse {
    status: &'static str,
    job: TranscriptionJob,
}

#[derive(Debug, serde::Serialize)]
struct JobsResponse {
    jobs: Vec<TranscriptionJob>,
}

#[derive(Debug, serde::Serialize)]
struct JobApiErrorResponse {
    status: &'static str,
    code: &'static str,
    message: String,
}

#[derive(Debug, Clone, Copy)]
enum JobApiErrorCode {
    MissingMedia,
    UnsupportedMedia,
    UnavailableMode,
    NotFound,
    FailedJob,
    Internal,
}

#[derive(Debug)]
struct JobApiError {
    status: StatusCode,
    code: JobApiErrorCode,
    message: String,
}

impl JobApiError {
    fn new(status: StatusCode, code: JobApiErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn missing_media(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            JobApiErrorCode::MissingMedia,
            message,
        )
    }

    fn unsupported_media(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            JobApiErrorCode::UnsupportedMedia,
            message,
        )
    }

    fn unavailable_mode(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            JobApiErrorCode::UnavailableMode,
            message,
        )
    }

    fn failed_job(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            JobApiErrorCode::FailedJob,
            message,
        )
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            JobApiErrorCode::Internal,
            message,
        )
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, JobApiErrorCode::NotFound, message)
    }

    fn api_response(self) -> (StatusCode, Json<JobApiErrorResponse>) {
        (
            self.status,
            Json(JobApiErrorResponse {
                status: "error",
                code: self.code.as_str(),
                message: self.message,
            }),
        )
    }
}

impl JobApiErrorCode {
    fn as_str(self) -> &'static str {
        match self {
            JobApiErrorCode::MissingMedia => "missing_media",
            JobApiErrorCode::UnsupportedMedia => "unsupported_media",
            JobApiErrorCode::UnavailableMode => "unavailable_mode",
            JobApiErrorCode::NotFound => "not_found",
            JobApiErrorCode::FailedJob => "failed_job",
            JobApiErrorCode::Internal => "internal",
        }
    }
}

async fn api_jobs(AxumState(state): AxumState<WebState>) -> impl IntoResponse {
    match list_jobs(&state.database) {
        Ok(jobs) => (StatusCode::OK, Json(JobsResponse { jobs })).into_response(),
        Err(error) => JobApiError::internal(format!("Failed to list transcription jobs: {error}"))
            .api_response()
            .into_response(),
    }
}

async fn api_job(
    AxumState(state): AxumState<WebState>,
    AxumPath(id): AxumPath<i64>,
) -> impl IntoResponse {
    match state.database.get_job(id) {
        Ok(Some(job)) => (StatusCode::OK, Json(serde_json::json!({ "job": job }))).into_response(),
        Ok(None) => JobApiError::not_found(format!("Transcription job not found: {id}"))
            .api_response()
            .into_response(),
        Err(error) => JobApiError::internal(format!("Failed to get transcription job: {error}"))
            .api_response()
            .into_response(),
    }
}

async fn api_jobs_from_path(
    AxumState(state): AxumState<WebState>,
    Json(req): Json<JobSubmissionRequest>,
) -> impl IntoResponse {
    match create_job_from_path(&state, req).await {
        Ok(job) => (
            StatusCode::ACCEPTED,
            Json(JobCreatedResponse { status: "ok", job }),
        )
            .into_response(),
        Err(error) => error.api_response().into_response(),
    }
}

async fn api_jobs_upload(
    AxumState(state): AxumState<WebState>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    match create_job_from_upload(&state, &mut multipart).await {
        Ok(job) => (
            StatusCode::ACCEPTED,
            Json(JobCreatedResponse { status: "ok", job }),
        )
            .into_response(),
        Err(error) => error.api_response().into_response(),
    }
}

async fn api_cancel_job(
    AxumState(state): AxumState<WebState>,
    AxumPath(id): AxumPath<i64>,
) -> impl IntoResponse {
    match state.database.cancel_job(id) {
        Ok(CancelJobOutcome::Cancelled(job)) => {
            output::notify(
                "Kloyce",
                &format!("File transcription cancelled: {}", job.source_filename),
            );
            cancel_job_ok_response(&state.event_tx, id, job)
        }
        Ok(CancelJobOutcome::CancellationRequested(job)) => {
            cancel_job_ok_response(&state.event_tx, id, job)
        }
        Ok(CancelJobOutcome::AlreadyTerminal(job)) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "status": "error",
                "code": "failed_job",
                "message": format!("Transcription job is already terminal: {}", job.status),
                "job": job,
            })),
        )
            .into_response(),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            JobApiError::not_found(format!("Transcription job not found: {id}"))
                .api_response()
                .into_response()
        }
        Err(error) => JobApiError::internal(format!("Failed to cancel transcription job: {error}"))
            .api_response()
            .into_response(),
    }
}

fn cancel_job_ok_response(
    event_tx: &broadcast::Sender<SseEvent>,
    job_id: i64,
    job: TranscriptionJob,
) -> Response {
    let _ = event_tx.send(SseEvent::TranscriptionJobStatus {
        job_id,
        status: job.status,
    });
    (
        StatusCode::OK,
        Json(serde_json::json!({ "status": "ok", "job": job })),
    )
        .into_response()
}

async fn create_job_from_path(
    state: &WebState,
    req: JobSubmissionRequest,
) -> Result<TranscriptionJob, JobApiError> {
    let file_path = req
        .file_path
        .as_deref()
        .ok_or_else(|| JobApiError::missing_media("file_path is required"))?;
    let source_path = PathBuf::from(file_path);
    if !source_path.exists() {
        return Err(JobApiError::missing_media(format!(
            "File not found: {file_path}"
        )));
    }

    let config = state.config.read().await.clone();
    let (mode, settings) = resolve_job_settings(&config, &req)?;
    let storage = media_storage_for_state(state, &config);
    let stored = storage
        .store_source_from_path(&source_path)
        .await
        .map_err(|error| JobApiError::missing_media(error.to_string()))?;
    insert_job_from_stored_media(state, stored.path, stored.filename, mode, settings).await
}

async fn create_job_from_upload(
    state: &WebState,
    multipart: &mut Multipart,
) -> Result<TranscriptionJob, JobApiError> {
    let mut req = JobSubmissionRequest::default();
    let mut upload_filename = None;
    let mut upload_bytes = None;

    while let Some(field) = multipart.next_field().await.map_err(|error| {
        JobApiError::unsupported_media(format!("Invalid multipart upload: {error}"))
    })? {
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" {
            upload_filename = Some(field.file_name().unwrap_or("source-media").to_string());
            let bytes = field.bytes().await.map_err(|error| {
                JobApiError::unsupported_media(format!("Failed to read upload: {error}"))
            })?;
            upload_bytes = Some(bytes);
            continue;
        }

        let value = field.text().await.map_err(|error| {
            JobApiError::unsupported_media(format!("Invalid upload field: {error}"))
        })?;
        apply_upload_field(&mut req, &name, &value)?;
    }

    let filename = upload_filename
        .as_deref()
        .ok_or_else(|| JobApiError::missing_media("file upload field is required"))?;
    let bytes = upload_bytes
        .as_deref()
        .ok_or_else(|| JobApiError::missing_media("file upload field is required"))?;

    let config = state.config.read().await.clone();
    let (mode, settings) = resolve_job_settings(&config, &req)?;
    let storage = media_storage_for_state(state, &config);
    let stored = storage
        .store_source_from_bytes(filename, bytes)
        .await
        .map_err(|error| JobApiError::missing_media(error.to_string()))?;
    insert_job_from_stored_media(state, stored.path, stored.filename, mode, settings).await
}

async fn insert_job_from_stored_media(
    state: &WebState,
    source_media_path: PathBuf,
    source_filename: String,
    mode: TranscriptionMode,
    settings: PerJobSettings,
) -> Result<TranscriptionJob, JobApiError> {
    let new_job = NewTranscriptionJob {
        source_media_path: source_media_path.to_string_lossy().to_string(),
        source_filename,
        mode,
        settings,
    };
    let job = state
        .database
        .insert_job(&new_job)
        .map_err(|error| JobApiError::internal(format!("Failed to create job: {error}")))?;
    let _ = state
        .event_tx
        .send(SseEvent::TranscriptionJobCreated(job.clone()));
    Ok(job)
}

fn resolve_job_settings(
    config: &Config,
    req: &JobSubmissionRequest,
) -> Result<(TranscriptionMode, PerJobSettings), JobApiError> {
    let mode = req.mode.unwrap_or_else(|| default_job_mode(config));

    match mode {
        TranscriptionMode::Standard => resolve_standard_job_settings(config, req),
        TranscriptionMode::Diarized => resolve_diarized_job_settings(config, req),
    }
}

fn default_job_mode(config: &Config) -> TranscriptionMode {
    match config.transcription_defaults.default_mode {
        crate::config::TranscriptionMode::Standard => TranscriptionMode::Standard,
        crate::config::TranscriptionMode::Diarized => TranscriptionMode::Diarized,
    }
}

fn resolve_standard_job_settings(
    config: &Config,
    req: &JobSubmissionRequest,
) -> Result<(TranscriptionMode, PerJobSettings), JobApiError> {
    let model = req
        .model
        .clone()
        .unwrap_or_else(|| config.transcription_defaults.default_standard_model.clone());
    let Some(model_path) = model_catalog::standard_model_path(config, &model) else {
        return Err(JobApiError::unavailable_mode(format!(
            "Unknown standard model: {model}"
        )));
    };
    if !model_path.exists() {
        return Err(JobApiError::unavailable_mode(format!(
            "Standard mode unavailable: model '{model}' is not installed"
        )));
    }

    Ok((
        TranscriptionMode::Standard,
        PerJobSettings::standard(model, req.context_tags.clone()),
    ))
}

fn resolve_diarized_job_settings(
    config: &Config,
    req: &JobSubmissionRequest,
) -> Result<(TranscriptionMode, PerJobSettings), JobApiError> {
    let availability = model_catalog::diarized_mode_availability(config);
    if !availability.available {
        return Err(JobApiError::unavailable_mode(
            availability
                .reason
                .unwrap_or_else(|| "Diarized mode is unavailable".to_string()),
        ));
    }

    let model = req
        .model
        .clone()
        .unwrap_or_else(|| config.transcription_defaults.default_diarized_model.clone());
    if model.trim().is_empty() {
        return Err(JobApiError::unavailable_mode(
            "Diarized model cannot be empty",
        ));
    }

    Ok((
        TranscriptionMode::Diarized,
        PerJobSettings {
            model,
            diarize: req.diarize.unwrap_or(true),
            min_speakers: req.min_speakers,
            max_speakers: req.max_speakers,
            context_tags: req.context_tags.clone(),
        },
    ))
}

fn apply_upload_field(
    req: &mut JobSubmissionRequest,
    name: &str,
    value: &str,
) -> Result<(), JobApiError> {
    match name {
        "mode" => {
            req.mode = Some(TranscriptionMode::try_from(value).map_err(|_| {
                JobApiError::unavailable_mode(format!("Unknown transcription mode: {value}"))
            })?);
        }
        "model" => req.model = Some(value.to_string()),
        "context_tags" => req.context_tags = parse_context_tags(value)?,
        "diarize" => req.diarize = Some(parse_bool_field("diarize", value)?),
        "min_speakers" => req.min_speakers = Some(parse_u32_field("min_speakers", value)?),
        "max_speakers" => req.max_speakers = Some(parse_u32_field("max_speakers", value)?),
        _ => {}
    }
    Ok(())
}

fn parse_context_tags(value: &str) -> Result<Vec<String>, JobApiError> {
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }

    if value.trim_start().starts_with('[') {
        serde_json::from_str(value).map_err(|error| {
            JobApiError::unsupported_media(format!("Invalid context_tags JSON: {error}"))
        })
    } else {
        Ok(value
            .split(',')
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(ToOwned::to_owned)
            .collect())
    }
}

fn parse_bool_field(name: &str, value: &str) -> Result<bool, JobApiError> {
    match value {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(JobApiError::unsupported_media(format!(
            "Invalid boolean field {name}: {value}"
        ))),
    }
}

fn parse_u32_field(name: &str, value: &str) -> Result<u32, JobApiError> {
    value.parse::<u32>().map_err(|error| {
        JobApiError::unsupported_media(format!("Invalid numeric field {name}: {error}"))
    })
}

fn media_storage_for_state(state: &WebState, config: &Config) -> MediaStorage {
    MediaStorage::new(
        state.data_dir.clone(),
        config.ffmpeg_bin.clone(),
        config.ffprobe_bin.clone(),
    )
}

fn list_jobs(database: &db::Db) -> Result<Vec<TranscriptionJob>, rusqlite::Error> {
    let mut jobs = database.active_jobs()?;
    jobs.extend(database.queued_jobs(100)?);
    jobs.extend(database.recent_terminal_jobs(100)?);
    Ok(jobs)
}

#[derive(Debug, serde::Deserialize)]
struct TranscribeRequest {
    file_path: String,
    #[serde(default)]
    context_tags: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
struct TranscribeResponse {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    word_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

async fn api_transcribe(
    AxumState(state): AxumState<WebState>,
    Json(req): Json<TranscribeRequest>,
) -> (StatusCode, Json<TranscribeResponse>) {
    let submission = JobSubmissionRequest {
        file_path: Some(req.file_path),
        mode: Some(TranscriptionMode::Standard),
        context_tags: req.context_tags,
        ..JobSubmissionRequest::default()
    };

    let job = match create_job_from_path(&state, submission).await {
        Ok(job) => job,
        Err(error) => return transcribe_error_response(error),
    };

    let timeout_secs = sync_job_wait_timeout_secs(&state, TranscriptionMode::Standard).await;
    match wait_for_terminal_job(state.database.clone(), job.id, timeout_secs).await {
        Ok(job) => transcribe_job_response(job),
        Err(error) => transcribe_error_response(error),
    }
}

#[derive(Debug, serde::Deserialize)]
struct AdvancedTranscribeRequest {
    file_path: String,
    #[serde(default)]
    context_tags: Vec<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    diarize: Option<bool>,
    #[serde(default)]
    min_speakers: Option<u32>,
    #[serde(default)]
    max_speakers: Option<u32>,
}

#[derive(Debug, serde::Serialize)]
struct AdvancedTranscribeResponse {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    segments: Option<Vec<TranscriptSegment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    full_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    word_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speaker_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

async fn api_transcribe_advanced(
    AxumState(state): AxumState<WebState>,
    Json(req): Json<AdvancedTranscribeRequest>,
) -> (StatusCode, Json<AdvancedTranscribeResponse>) {
    let submission = JobSubmissionRequest {
        file_path: Some(req.file_path),
        mode: Some(TranscriptionMode::Diarized),
        model: req.model,
        context_tags: req.context_tags,
        diarize: req.diarize,
        min_speakers: req.min_speakers,
        max_speakers: req.max_speakers,
    };

    let job = match create_job_from_path(&state, submission).await {
        Ok(job) => job,
        Err(error) => return advanced_transcribe_error_response(error),
    };

    let timeout_secs = sync_job_wait_timeout_secs(&state, TranscriptionMode::Diarized).await;
    match wait_for_terminal_job(state.database.clone(), job.id, timeout_secs).await {
        Ok(job) => advanced_transcribe_job_response(job),
        Err(error) => advanced_transcribe_error_response(error),
    }
}

async fn wait_for_terminal_job(
    database: Arc<db::Db>,
    job_id: i64,
    timeout_secs: u64,
) -> Result<TranscriptionJob, JobApiError> {
    let wait = async {
        loop {
            match database.get_job(job_id) {
                Ok(Some(job)) if job.status.is_terminal() => return Ok(job),
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err(JobApiError::new(
                        StatusCode::NOT_FOUND,
                        JobApiErrorCode::NotFound,
                        format!("Transcription job not found: {job_id}"),
                    ));
                }
                Err(error) => {
                    return Err(JobApiError::internal(format!(
                        "Failed to inspect transcription job: {error}"
                    )));
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    };

    tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), wait)
        .await
        .map_err(|_| {
            JobApiError::internal(format!(
                "Timed out waiting for transcription job {job_id} after {timeout_secs}s"
            ))
        })?
}

async fn sync_job_wait_timeout_secs(state: &WebState, mode: TranscriptionMode) -> u64 {
    let config = state.config.read().await;
    match mode {
        TranscriptionMode::Standard => 24 * 60 * 60,
        TranscriptionMode::Diarized => config.advanced_transcription.timeout_secs + 30,
    }
}

fn transcribe_job_response(job: TranscriptionJob) -> (StatusCode, Json<TranscribeResponse>) {
    match job.status {
        JobStatus::Succeeded => match job.result {
            Some(result) => (
                StatusCode::OK,
                Json(TranscribeResponse {
                    status: "ok",
                    text: Some(result.text),
                    word_count: Some(result.word_count),
                    duration_secs: Some(result.duration_secs),
                    message: None,
                }),
            ),
            None => transcribe_error_response(JobApiError::failed_job(
                "Transcription job succeeded without a result",
            )),
        },
        _ => transcribe_error_response(job_terminal_error(job)),
    }
}

fn transcribe_error_response(error: JobApiError) -> (StatusCode, Json<TranscribeResponse>) {
    (
        error.status,
        Json(TranscribeResponse {
            status: "error",
            text: None,
            word_count: None,
            duration_secs: None,
            message: Some(error.message),
        }),
    )
}

fn advanced_transcribe_job_response(
    job: TranscriptionJob,
) -> (StatusCode, Json<AdvancedTranscribeResponse>) {
    match job.status {
        JobStatus::Succeeded => match job.result {
            Some(result) => {
                let segments = result
                    .segments
                    .into_iter()
                    .filter_map(|segment| serde_json::from_value(segment).ok())
                    .collect::<Vec<TranscriptSegment>>();
                (
                    StatusCode::OK,
                    Json(AdvancedTranscribeResponse {
                        status: "ok",
                        segments: Some(segments),
                        full_text: Some(result.text),
                        language: result.language,
                        word_count: Some(result.word_count),
                        speaker_count: result.speaker_count,
                        duration_secs: Some(result.duration_secs),
                        message: None,
                    }),
                )
            }
            None => advanced_transcribe_error_response(JobApiError::failed_job(
                "Transcription job succeeded without a result",
            )),
        },
        _ => advanced_transcribe_error_response(job_terminal_error(job)),
    }
}

fn job_terminal_error(job: TranscriptionJob) -> JobApiError {
    match job.status {
        JobStatus::Failed => JobApiError::failed_job(
            job.error_message
                .unwrap_or_else(|| "Transcription failed".to_string()),
        ),
        JobStatus::Cancelled => JobApiError::failed_job("Transcription job was cancelled"),
        _ => JobApiError::internal("Transcription job has not reached a terminal status"),
    }
}

fn advanced_transcribe_error_response(
    error: JobApiError,
) -> (StatusCode, Json<AdvancedTranscribeResponse>) {
    let status = match error.code {
        JobApiErrorCode::UnavailableMode => {
            if error
                .message
                .contains("Advanced transcription is not enabled")
            {
                StatusCode::NOT_IMPLEMENTED
            } else if error.message.contains("Python venv not found") {
                StatusCode::INTERNAL_SERVER_ERROR
            } else {
                error.status
            }
        }
        _ => error.status,
    };

    (
        status,
        Json(AdvancedTranscribeResponse {
            status: "error",
            segments: None,
            full_text: None,
            language: None,
            word_count: None,
            speaker_count: None,
            duration_secs: None,
            message: Some(error.message),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> db::Db {
        let path = std::env::temp_dir().join(format!(
            "kloyce-web-test-{}-{}-{name}.db",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        db::Db::open(path).unwrap()
    }

    fn new_job(name: &str) -> NewTranscriptionJob {
        NewTranscriptionJob {
            source_media_path: format!("/tmp/{name}"),
            source_filename: name.to_string(),
            mode: TranscriptionMode::Standard,
            settings: PerJobSettings::standard("small.en", Vec::new()),
        }
    }

    #[test]
    fn standard_job_settings_use_default_standard_model_and_tags() {
        let dir = std::env::temp_dir().join(format!(
            "kloyce-web-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // model_path anchors the models directory; default model is now large-v3-turbo.
        let model_path = dir.join("ggml-small.en.bin");
        let turbo_path = dir.join("ggml-large-v3-turbo.bin");
        std::fs::write(&turbo_path, b"model").unwrap();
        let mut config = Config::default();
        config.model_path = model_path;

        let req = JobSubmissionRequest {
            mode: Some(TranscriptionMode::Standard),
            context_tags: vec!["alpha".to_string()],
            ..JobSubmissionRequest::default()
        };

        let (mode, settings) = resolve_job_settings(&config, &req).unwrap();

        assert_eq!(mode, TranscriptionMode::Standard);
        assert_eq!(settings.model, "large-v3-turbo");
        assert_eq!(settings.context_tags, vec!["alpha"]);
    }

    #[test]
    fn unavailable_standard_model_returns_typed_validation_error() {
        let mut config = Config::default();
        config.model_path = PathBuf::from("/missing/ggml-small.en.bin");
        let req = JobSubmissionRequest {
            mode: Some(TranscriptionMode::Standard),
            ..JobSubmissionRequest::default()
        };

        let error = resolve_job_settings(&config, &req).unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(matches!(error.code, JobApiErrorCode::UnavailableMode));
    }

    #[test]
    fn job_listing_includes_active_queued_and_recent_terminal_jobs() {
        let db = temp_db("listing");
        let terminal = db.insert_job(&new_job("terminal.wav")).unwrap();
        db.select_next_job_for_worker().unwrap().unwrap();
        db.update_job_status(terminal.id, JobStatus::Transcribing, None)
            .unwrap();
        db.complete_job_with_result(
            terminal.id,
            &TranscriptResult {
                text: "done".to_string(),
                word_count: 1,
                duration_secs: 1,
                language: None,
                speaker_count: None,
                segments: Vec::new(),
            },
        )
        .unwrap();

        let active = db.insert_job(&new_job("active.wav")).unwrap();
        db.select_next_job_for_worker().unwrap().unwrap();
        let queued = db.insert_job(&new_job("queued.wav")).unwrap();

        let jobs = list_jobs(&db).unwrap();
        let ids = jobs.iter().map(|job| job.id).collect::<Vec<_>>();

        assert!(ids.contains(&active.id));
        assert!(ids.contains(&queued.id));
        assert!(ids.contains(&terminal.id));
    }

    #[test]
    fn dashboard_exposes_file_job_queue_controls() {
        assert!(DASHBOARD_HTML.contains("/api/jobs/upload"));
        assert!(DASHBOARD_HTML.contains("/api/jobs/{id}/cancel"));
        assert!(DASHBOARD_HTML.contains("/api/jobs"));
        assert!(DASHBOARD_HTML.contains("copyTranscriptText"));
        assert!(DASHBOARD_HTML.contains("copy-log"));
        assert!(DASHBOARD_HTML.contains("copyPlusTranscriptText"));
        assert!(DASHBOARD_HTML.contains("copy-plus-log"));
        assert!(DASHBOARD_HTML.contains("drop-zone"));
        assert!(DASHBOARD_HTML.contains("Ctrl+Enter"));
    }

    #[test]
    fn dashboard_exposes_transcription_defaults_settings_controls() {
        assert!(DASHBOARD_HTML.contains("settings-modal"));
        assert!(DASHBOARD_HTML.contains("dashboard-settings-form"));
        assert!(DASHBOARD_HTML.contains("default-mode-select"));
        assert!(DASHBOARD_HTML.contains("default-standard-model-select"));
        assert!(DASHBOARD_HTML.contains("default-diarized-model"));
        assert!(DASHBOARD_HTML.contains("hotkey-audio-retention-check"));
        assert!(DASHBOARD_HTML.contains("hotkey-audio-retention-days"));
        assert!(DASHBOARD_HTML.contains("/api/transcription/settings"));
        assert!(DASHBOARD_HTML.contains("/api/models/standard"));
        assert!(DASHBOARD_HTML.contains("/api/models/standard/{model_id}/download"));
    }
}
