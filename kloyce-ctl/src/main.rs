use clap::{Parser, Subcommand, ValueEnum};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const DEFAULT_WEB_PORT: u16 = 9876;
const QUEUED_JOB_REQUEST_TIMEOUT_SECS: u64 = 30;
const JOB_REQUEST_TIMEOUT_GRACE_SECS: u64 = 30;
const STANDARD_JOB_WAIT_TIMEOUT_SECS: u64 = 24 * 60 * 60;
const DIARIZED_JOB_WAIT_TIMEOUT_SECS: u64 = 2 * 60 * 60;
const JOB_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Parser)]
#[command(name = "kloyce-ctl", about = "Control the kloyce daemon")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum TranscriptionMode {
    Standard,
    Diarized,
}

#[derive(Subcommand)]
enum Commands {
    /// Toggle recording on/off
    Toggle,
    /// Toggle recording on/off (auto-press Enter in tmux after transcription)
    ToggleEnter,
    /// Copy the latest transcript with retained-audio metadata
    CopyPlus,
    /// Get current daemon status
    Status {
        /// Output in waybar JSON format
        #[arg(long)]
        waybar: bool,
    },
    /// Cancel current recording
    Cancel,
    /// Transcribe an audio or video file
    TranscribeFile {
        /// Path to the source media file
        file: String,
        /// Context tags (repeatable, e.g. --tag screen-recorder --tag firefox)
        #[arg(short, long)]
        tag: Vec<String>,
        /// Submit the job and exit after printing job id/status
        #[arg(long, conflicts_with = "follow")]
        queue: bool,
        /// Print job status/progress until terminal, then print transcript
        #[arg(long)]
        follow: bool,
        /// Transcription mode override
        #[arg(long, value_enum)]
        mode: Option<TranscriptionMode>,
        /// Model override for the selected mode
        #[arg(short, long)]
        model: Option<String>,
        /// Use diarized mode with speaker diarization
        #[arg(long, conflicts_with = "no_diarize")]
        diarize: bool,
        /// Disable speaker diarization for diarized mode
        #[arg(long)]
        no_diarize: bool,
        /// Minimum number of speakers hint
        #[arg(long)]
        min_speakers: Option<u32>,
        /// Maximum number of speakers hint
        #[arg(long)]
        max_speakers: Option<u32>,
        /// Daemon web port
        #[arg(short, long, default_value_t = DEFAULT_WEB_PORT)]
        port: u16,
    },
    /// Transcribe an audio file with advanced diarization (speaker labels, timestamps)
    TranscribeFileAdvanced {
        /// Path to the audio file (wav, mp3, m4a, flac, ogg, etc.)
        file: String,
        /// Context tags (repeatable)
        #[arg(short, long)]
        tag: Vec<String>,
        /// Submit the job and exit after printing job id/status
        #[arg(long, conflicts_with = "follow")]
        queue: bool,
        /// Print job status/progress until terminal, then print transcript
        #[arg(long)]
        follow: bool,
        /// Whisper model size override (tiny, base, small, medium, large-v2, large-v3)
        #[arg(short, long)]
        model: Option<String>,
        /// Skip speaker diarization
        #[arg(long)]
        no_diarize: bool,
        /// Minimum number of speakers hint
        #[arg(long)]
        min_speakers: Option<u32>,
        /// Maximum number of speakers hint
        #[arg(long)]
        max_speakers: Option<u32>,
        /// Daemon web port
        #[arg(short, long, default_value_t = DEFAULT_WEB_PORT)]
        port: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobWaitMode {
    Wait,
    Queue,
    Follow,
}

impl JobWaitMode {
    fn from_flags(queue: bool, follow: bool) -> Self {
        if queue {
            Self::Queue
        } else if follow {
            Self::Follow
        } else {
            Self::Wait
        }
    }
}

#[derive(Debug, Clone)]
struct FileJobOptions {
    file: String,
    tags: Vec<String>,
    wait_mode: JobWaitMode,
    mode: TranscriptionMode,
    model: Option<String>,
    diarize: Option<bool>,
    min_speakers: Option<u32>,
    max_speakers: Option<u32>,
    port: u16,
    show_diarized_summary: bool,
}

#[derive(Debug, serde::Serialize)]
struct JobSubmissionRequest {
    file_path: String,
    mode: TranscriptionMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    context_tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diarize: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_speakers: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_speakers: Option<u32>,
}

#[derive(Debug, serde::Deserialize)]
struct JobEnvelope {
    job: TranscriptionJob,
}

#[derive(Debug, serde::Deserialize)]
struct JobCreatedResponse {
    job: TranscriptionJob,
}

#[derive(Debug, serde::Deserialize)]
struct ApiErrorResponse {
    message: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct TranscriptionJob {
    id: i64,
    status: String,
    mode: String,
    result: Option<TranscriptResult>,
    error_message: Option<String>,
    progress_pct: u32,
}

impl TranscriptionJob {
    fn is_terminal(&self) -> bool {
        matches!(self.status.as_str(), "succeeded" | "failed" | "cancelled")
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct TranscriptResult {
    text: String,
    duration_secs: u64,
    speaker_count: Option<u32>,
}

#[cfg(unix)]
fn socket_path() -> std::path::PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    std::path::PathBuf::from(runtime_dir).join("kloyce.sock")
}

#[cfg(windows)]
const IPC_PORT: u16 = 19876;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if let Some(options) = file_job_options(&cli.command) {
        transcribe_file_job(options).await;
        return;
    }

    let is_waybar = matches!(&cli.command, Commands::Status { waybar: true });

    let command = ipc_command_json(&cli.command).unwrap_or_else(|| unreachable!());

    #[cfg(unix)]
    let stream = {
        use tokio::net::UnixStream;
        let path = socket_path();
        match UnixStream::connect(&path).await {
            Ok(s) => s,
            Err(e) => {
                if is_waybar {
                    println!(r#"{{"text": "", "tooltip": "Daemon not running", "class": "idle"}}"#);
                    return;
                }
                eprintln!(
                    "Failed to connect to kloyce daemon at {}: {e}",
                    path.display()
                );
                eprintln!("Is the daemon running? Start it with: kloyce daemon");
                std::process::exit(1);
            }
        }
    };

    #[cfg(windows)]
    let stream = {
        use tokio::net::TcpStream;
        let addr = format!("127.0.0.1:{IPC_PORT}");
        match TcpStream::connect(&addr).await {
            Ok(s) => s,
            Err(e) => {
                if is_waybar {
                    println!(r#"{{"text": "", "tooltip": "Daemon not running", "class": "idle"}}"#);
                    return;
                }
                eprintln!("Failed to connect to kloyce daemon at {addr}: {e}");
                eprintln!("Is the daemon running? Start it with: kloyce daemon");
                std::process::exit(1);
            }
        }
    };

    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    if let Err(e) = writer.write_all(format!("{command}\n").as_bytes()).await {
        if is_waybar {
            println!(r#"{{"text": "", "tooltip": "Connection error", "class": "idle"}}"#);
            return;
        }
        eprintln!("Failed to send command: {e}");
        std::process::exit(1);
    }

    let mut response = String::new();
    match reader.read_line(&mut response).await {
        Ok(0) => {
            if is_waybar {
                println!(
                    r#"{{"text": "", "tooltip": "Daemon closed connection", "class": "idle"}}"#
                );
                return;
            }
            eprintln!("Daemon closed connection");
            std::process::exit(1);
        }
        Ok(_) => {
            if is_waybar {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(response.trim()) {
                    let state = json.get("state").and_then(|s| s.as_str()).unwrap_or("idle");
                    let (text, tooltip, class) = match state {
                        "recording" => ("󰍬", "Recording... (Super+R to stop)", "recording"),
                        "transcribing" => ("󰔟", "Transcribing...", "transcribing"),
                        _ => ("", "Voice input (Super+R)", "idle"),
                    };
                    println!(r#"{{"text": "{text}", "tooltip": "{tooltip}", "class": "{class}"}}"#);
                } else {
                    println!(r#"{{"text": "", "tooltip": "Parse error", "class": "idle"}}"#);
                }
            } else if let Ok(json) = serde_json::from_str::<serde_json::Value>(response.trim()) {
                if let Some(message) = json.get("message").and_then(|m| m.as_str()) {
                    println!("{message}");
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json).unwrap_or(response)
                    );
                }
            } else {
                print!("{response}");
            }
        }
        Err(e) => {
            if is_waybar {
                println!(r#"{{"text": "", "tooltip": "Read error", "class": "idle"}}"#);
                return;
            }
            eprintln!("Failed to read response: {e}");
            std::process::exit(1);
        }
    }
}

fn ipc_command_json(command: &Commands) -> Option<&'static str> {
    match command {
        Commands::Toggle => Some(r#"{"command":"toggle"}"#),
        Commands::ToggleEnter => Some(r#"{"command":"toggle_enter"}"#),
        Commands::CopyPlus => Some(r#"{"command":"copy_plus_latest"}"#),
        Commands::Status { .. } => Some(r#"{"command":"status"}"#),
        Commands::Cancel => Some(r#"{"command":"cancel"}"#),
        _ => None,
    }
}

fn file_job_options(command: &Commands) -> Option<FileJobOptions> {
    match command {
        Commands::TranscribeFile {
            file,
            tag,
            queue,
            follow,
            mode,
            model,
            diarize,
            no_diarize,
            min_speakers,
            max_speakers,
            port,
        } => {
            let selected_mode = resolve_mode(*mode, *diarize);
            Some(FileJobOptions {
                file: file.clone(),
                tags: tag.clone(),
                wait_mode: JobWaitMode::from_flags(*queue, *follow),
                mode: selected_mode,
                model: model.clone(),
                diarize: diarize_flag(*diarize, *no_diarize),
                min_speakers: *min_speakers,
                max_speakers: *max_speakers,
                port: *port,
                show_diarized_summary: selected_mode == TranscriptionMode::Diarized,
            })
        }
        Commands::TranscribeFileAdvanced {
            file,
            tag,
            queue,
            follow,
            model,
            no_diarize,
            min_speakers,
            max_speakers,
            port,
        } => Some(FileJobOptions {
            file: file.clone(),
            tags: tag.clone(),
            wait_mode: JobWaitMode::from_flags(*queue, *follow),
            mode: TranscriptionMode::Diarized,
            model: model.clone(),
            diarize: Some(!*no_diarize),
            min_speakers: *min_speakers,
            max_speakers: *max_speakers,
            port: *port,
            show_diarized_summary: true,
        }),
        _ => None,
    }
}

async fn transcribe_file_job(options: FileJobOptions) {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(request_timeout_secs(
            options.wait_mode,
            options.mode,
        )))
        .build()
        .unwrap_or_else(|e| {
            eprintln!("Failed to create HTTP client: {e}");
            std::process::exit(1);
        });

    if options.show_diarized_summary && options.wait_mode != JobWaitMode::Queue {
        eprintln!("Starting diarized transcription (this may take a while)...");
    }

    let job = submit_file_job(&client, &options).await;
    match options.wait_mode {
        JobWaitMode::Queue => print_queued_job(&job),
        JobWaitMode::Wait | JobWaitMode::Follow => {
            let terminal = wait_for_job(&client, options.port, &job, options.wait_mode).await;
            print_terminal_job(&terminal, options.show_diarized_summary);
        }
    }
}

async fn submit_file_job(client: &reqwest::Client, options: &FileJobOptions) -> TranscriptionJob {
    let url = jobs_from_path_url(options.port);
    let body = build_job_submission(options);
    let resp = match client.post(&url).json(&body).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to connect to kloyce daemon at {url}: {e}");
            eprintln!("Is the daemon running? Start it with: kloyce daemon");
            std::process::exit(1);
        }
    };

    if !resp.status().is_success() {
        print_api_error_and_exit(resp).await;
    }

    match resp.json::<JobCreatedResponse>().await {
        Ok(created) => created.job,
        Err(e) => {
            eprintln!("Failed to parse job creation response: {e}");
            std::process::exit(1);
        }
    }
}

async fn wait_for_job(
    client: &reqwest::Client,
    port: u16,
    created: &TranscriptionJob,
    wait_mode: JobWaitMode,
) -> TranscriptionJob {
    let mut last_status = String::new();
    let mut last_progress = u32::MAX;
    let timeout = std::time::Duration::from_secs(match created.mode.as_str() {
        "diarized" => DIARIZED_JOB_WAIT_TIMEOUT_SECS,
        _ => STANDARD_JOB_WAIT_TIMEOUT_SECS,
    });

    let wait = async {
        loop {
            let job = fetch_job(client, port, created.id).await;
            if wait_mode == JobWaitMode::Follow
                && (job.status != last_status || job.progress_pct != last_progress)
            {
                eprintln!("job {}: {} ({}%)", job.id, job.status, job.progress_pct);
                last_status = job.status.clone();
                last_progress = job.progress_pct;
            }
            if job.is_terminal() {
                return job;
            }
            tokio::time::sleep(JOB_POLL_INTERVAL).await;
        }
    };

    match tokio::time::timeout(timeout, wait).await {
        Ok(job) => job,
        Err(_) => {
            eprintln!(
                "Error: Timed out waiting for transcription job {} after {}s",
                created.id,
                timeout.as_secs()
            );
            std::process::exit(1);
        }
    }
}

async fn fetch_job(client: &reqwest::Client, port: u16, job_id: i64) -> TranscriptionJob {
    let url = job_url(port, job_id);
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to inspect transcription job {job_id}: {e}");
            std::process::exit(1);
        }
    };

    if !resp.status().is_success() {
        print_api_error_and_exit(resp).await;
    }

    match resp.json::<JobEnvelope>().await {
        Ok(envelope) => envelope.job,
        Err(e) => {
            eprintln!("Failed to parse job response: {e}");
            std::process::exit(1);
        }
    }
}

fn build_job_submission(options: &FileJobOptions) -> JobSubmissionRequest {
    JobSubmissionRequest {
        file_path: absolute_path_string(&options.file),
        mode: options.mode,
        model: options.model.clone(),
        context_tags: options.tags.clone(),
        diarize: options.diarize,
        min_speakers: options.min_speakers,
        max_speakers: options.max_speakers,
    }
}

fn absolute_path_string(file: &str) -> String {
    let path = std::path::Path::new(file);
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    abs_path.to_string_lossy().to_string()
}

fn resolve_mode(mode: Option<TranscriptionMode>, diarize: bool) -> TranscriptionMode {
    match (mode, diarize) {
        (Some(mode), _) => mode,
        (None, true) => TranscriptionMode::Diarized,
        (None, false) => TranscriptionMode::Standard,
    }
}

fn diarize_flag(diarize: bool, no_diarize: bool) -> Option<bool> {
    if diarize {
        Some(true)
    } else if no_diarize {
        Some(false)
    } else {
        None
    }
}

fn jobs_from_path_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/api/jobs/from-path")
}

fn job_url(port: u16, job_id: i64) -> String {
    format!("http://127.0.0.1:{port}/api/jobs/{job_id}")
}

fn request_timeout_secs(wait_mode: JobWaitMode, mode: TranscriptionMode) -> u64 {
    match wait_mode {
        JobWaitMode::Queue => QUEUED_JOB_REQUEST_TIMEOUT_SECS,
        JobWaitMode::Wait | JobWaitMode::Follow => match mode {
            TranscriptionMode::Standard => {
                STANDARD_JOB_WAIT_TIMEOUT_SECS + JOB_REQUEST_TIMEOUT_GRACE_SECS
            }
            TranscriptionMode::Diarized => {
                DIARIZED_JOB_WAIT_TIMEOUT_SECS + JOB_REQUEST_TIMEOUT_GRACE_SECS
            }
        },
    }
}

fn print_queued_job(job: &TranscriptionJob) {
    println!("job_id={} status={}", job.id, job.status);
}

fn print_terminal_job(job: &TranscriptionJob, show_diarized_summary: bool) {
    match job.status.as_str() {
        "succeeded" => {
            let Some(result) = &job.result else {
                eprintln!(
                    "Error: Transcription job {} succeeded without a result",
                    job.id
                );
                std::process::exit(1);
            };
            println!("{}", result.text);
            if show_diarized_summary {
                if let Some(speakers) = result.speaker_count {
                    eprintln!("Speakers detected: {speakers}");
                }
                eprintln!("Processing time: {}s", result.duration_secs);
            }
        }
        "failed" => {
            eprintln!(
                "Error: {}",
                job.error_message
                    .as_deref()
                    .unwrap_or("Transcription job failed")
            );
            std::process::exit(1);
        }
        "cancelled" => {
            eprintln!("Error: Transcription job was cancelled");
            std::process::exit(1);
        }
        _ => {
            eprintln!(
                "Error: Transcription job {} has not reached a terminal status: {}",
                job.id, job.status
            );
            std::process::exit(1);
        }
    }
}

async fn print_api_error_and_exit(resp: reqwest::Response) -> ! {
    let status = resp.status();
    let message = match resp.json::<ApiErrorResponse>().await {
        Ok(error) => error
            .message
            .unwrap_or_else(|| format!("HTTP {status} from kloyce daemon")),
        Err(_) => format!("HTTP {status} from kloyce daemon"),
    };
    eprintln!("Error: {message}");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> FileJobOptions {
        FileJobOptions {
            file: "/tmp/audio.wav".to_string(),
            tags: vec!["screen-recorder".to_string(), "firefox".to_string()],
            wait_mode: JobWaitMode::Wait,
            mode: TranscriptionMode::Standard,
            model: Some("small.en".to_string()),
            diarize: None,
            min_speakers: None,
            max_speakers: None,
            port: DEFAULT_WEB_PORT,
            show_diarized_summary: false,
        }
    }

    #[test]
    fn transcribe_file_submission_uses_standard_job_api_shape() {
        let submission = build_job_submission(&options());
        let json = serde_json::to_value(submission).unwrap();

        assert_eq!(json["file_path"], "/tmp/audio.wav");
        assert_eq!(json["mode"], "standard");
        assert_eq!(json["model"], "small.en");
        assert_eq!(
            json["context_tags"],
            serde_json::json!(["screen-recorder", "firefox"])
        );
        assert!(json.get("diarize").is_none());
    }

    #[test]
    fn advanced_wrapper_submission_uses_diarized_job_settings() {
        let submission = build_job_submission(&FileJobOptions {
            mode: TranscriptionMode::Diarized,
            model: Some("large-v3".to_string()),
            diarize: Some(false),
            min_speakers: Some(2),
            max_speakers: Some(4),
            show_diarized_summary: true,
            ..options()
        });
        let json = serde_json::to_value(submission).unwrap();

        assert_eq!(json["mode"], "diarized");
        assert_eq!(json["model"], "large-v3");
        assert_eq!(json["diarize"], false);
        assert_eq!(json["min_speakers"], 2);
        assert_eq!(json["max_speakers"], 4);
    }

    #[test]
    fn copy_plus_uses_copy_plus_latest_ipc_command() {
        assert_eq!(
            ipc_command_json(&Commands::CopyPlus),
            Some(r#"{"command":"copy_plus_latest"}"#)
        );
    }

    #[test]
    fn diarize_flag_without_mode_selects_diarized_mode() {
        assert_eq!(resolve_mode(None, true), TranscriptionMode::Diarized);
        assert_eq!(resolve_mode(None, false), TranscriptionMode::Standard);
        assert_eq!(
            resolve_mode(Some(TranscriptionMode::Standard), true),
            TranscriptionMode::Standard
        );
    }

    #[test]
    fn terminal_detection_matches_job_statuses() {
        let job = TranscriptionJob {
            id: 7,
            status: "succeeded".to_string(),
            mode: "standard".to_string(),
            result: Some(TranscriptResult {
                text: "hello".to_string(),
                duration_secs: 2,
                speaker_count: None,
            }),
            error_message: None,
            progress_pct: 100,
        };

        assert!(job.is_terminal());
    }
}
