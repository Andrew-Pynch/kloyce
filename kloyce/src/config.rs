use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

pub const DEFAULT_HOTKEY_AUDIO_RETENTION_HOURS: u64 = 24;
pub const MAX_HOTKEY_AUDIO_RETENTION_HOURS: u64 = 3650 * 24;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct AdvancedTranscription {
    pub enabled: bool,
    pub transcriber_venv: PathBuf,
    pub model: String,
    pub device: String,
    pub timeout_secs: u64,
}

impl Default for AdvancedTranscription {
    fn default() -> Self {
        Self {
            enabled: false,
            transcriber_venv: PathBuf::new(),
            model: "large-v3".into(),
            device: "auto".into(),
            timeout_secs: 600,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionMode {
    #[default]
    Standard,
    Diarized,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CleanupEngine {
    #[default]
    None,
    Claude,
    Local,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct TranscriptionDefaults {
    pub default_mode: TranscriptionMode,
    pub default_standard_model: String,
    pub default_diarized_model: String,
}

impl Default for TranscriptionDefaults {
    fn default() -> Self {
        Self {
            default_mode: TranscriptionMode::Standard,
            default_standard_model: "large-v3-turbo".into(),
            default_diarized_model: AdvancedTranscription::default().model,
        }
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct TranscriptionDefaultsUpdate {
    pub default_mode: Option<TranscriptionMode>,
    pub default_standard_model: Option<String>,
    pub default_diarized_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValidationError {
    message: String,
}

impl ConfigValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ConfigValidationError {}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
pub struct Config {
    pub whisper_bin: PathBuf,
    pub ffmpeg_bin: PathBuf,
    pub ffprobe_bin: PathBuf,
    pub model_path: PathBuf,
    pub web_port: u16,
    pub sound_start: PathBuf,
    pub sound_stop: PathBuf,
    pub history_size: usize,
    pub gpu_poll_interval_ms: u64,
    pub claude_cleanup: bool,
    pub claude_bin: PathBuf,
    pub claude_timeout_secs: u64,
    pub context_poll_interval_ms: u64,
    pub dictionary_path: PathBuf,
    pub dictionary_learning: bool,
    pub dictionary_max_entries: usize,
    pub tmux_send_keys: bool,
    pub tmux_auto_enter: bool,
    pub tmux_voice_tag: bool,
    pub tmux_voice_tag_text: String,
    pub clipboard_voice_tag: bool,
    pub advanced_transcription: AdvancedTranscription,
    pub transcription_defaults: TranscriptionDefaults,
    pub source_media_retention_days: i64,
    pub hotkey_audio_retention_enabled: bool,
    pub hotkey_audio_retention_hours: u64,
    pub whisper_flash_attn: bool,
    pub whisper_threads: u32,
    pub whisper_beam_size: u32,
    pub filler_removal: bool,
    pub cleanup_engine: CleanupEngine,
    pub local_llm_bin: PathBuf,
    pub local_llm_model_path: PathBuf,
    pub local_llm_timeout_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        #[cfg(target_os = "macos")]
        {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/Users".into());
            Self {
                whisper_bin: PathBuf::from(format!("{home}/.local/bin/whisper-cli")),
                ffmpeg_bin: PathBuf::from("ffmpeg"),
                ffprobe_bin: PathBuf::from("ffprobe"),
                model_path: PathBuf::from(format!(
                    "{home}/.local/share/kloyce/models/ggml-small.en.bin"
                )),
                web_port: 9876,
                sound_start: PathBuf::from("/System/Library/Sounds/Ping.aiff"),
                sound_stop: PathBuf::from("/System/Library/Sounds/Pop.aiff"),
                history_size: 100,
                gpu_poll_interval_ms: 2000,
                claude_cleanup: false,
                claude_bin: PathBuf::from(format!("{home}/.local/bin/claude")),
                claude_timeout_secs: 30,
                context_poll_interval_ms: 1000,
                dictionary_path: PathBuf::from(format!("{home}/.config/kloyce/dictionary.toml")),
                dictionary_learning: true,
                dictionary_max_entries: 500,
                tmux_send_keys: true,
                tmux_auto_enter: false,
                tmux_voice_tag: true,
                tmux_voice_tag_text:
                    "[voice transcript - if anything is unclear, ask for clarification]".into(),
                clipboard_voice_tag: true,
                advanced_transcription: AdvancedTranscription::default(),
                transcription_defaults: TranscriptionDefaults::default(),
                source_media_retention_days: crate::db::DEFAULT_SOURCE_MEDIA_RETENTION_DAYS,
                hotkey_audio_retention_enabled: true,
                hotkey_audio_retention_hours: DEFAULT_HOTKEY_AUDIO_RETENTION_HOURS,
                whisper_flash_attn: true,
                whisper_threads: 0,
                whisper_beam_size: 0,
                filler_removal: true,
                cleanup_engine: CleanupEngine::None,
                local_llm_bin: PathBuf::from(format!("{home}/.local/bin/llama-completion")),
                local_llm_model_path: PathBuf::from(format!(
                    "{home}/.local/share/kloyce/models/llm/qwen2.5-1.5b-instruct-q4_k_m.gguf"
                )),
                local_llm_timeout_secs: 20,
            }
        }

        #[cfg(target_os = "linux")]
        {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/home".into());
            Self {
                whisper_bin: PathBuf::from(format!("{home}/.local/bin/whisper-cli")),
                ffmpeg_bin: PathBuf::from("ffmpeg"),
                ffprobe_bin: PathBuf::from("ffprobe"),
                model_path: PathBuf::from(format!(
                    "{home}/.local/share/kloyce/models/ggml-small.en.bin"
                )),
                web_port: 9876,
                sound_start: PathBuf::from(
                    "/usr/share/sounds/freedesktop/stereo/message-new-instant.oga",
                ),
                sound_stop: PathBuf::from("/usr/share/sounds/freedesktop/stereo/complete.oga"),
                history_size: 100,
                gpu_poll_interval_ms: 2000,
                claude_cleanup: false,
                claude_bin: PathBuf::from(format!("{home}/.local/bin/claude")),
                claude_timeout_secs: 30,
                context_poll_interval_ms: 1000,
                dictionary_path: PathBuf::from(format!("{home}/.config/kloyce/dictionary.toml")),
                dictionary_learning: true,
                dictionary_max_entries: 500,
                tmux_send_keys: true,
                tmux_auto_enter: false,
                tmux_voice_tag: true,
                tmux_voice_tag_text:
                    "[voice transcript - if anything is unclear, ask for clarification]".into(),
                clipboard_voice_tag: true,
                advanced_transcription: AdvancedTranscription::default(),
                transcription_defaults: TranscriptionDefaults::default(),
                source_media_retention_days: crate::db::DEFAULT_SOURCE_MEDIA_RETENTION_DAYS,
                hotkey_audio_retention_enabled: true,
                hotkey_audio_retention_hours: DEFAULT_HOTKEY_AUDIO_RETENTION_HOURS,
                whisper_flash_attn: true,
                whisper_threads: 0,
                whisper_beam_size: 0,
                filler_removal: true,
                cleanup_engine: CleanupEngine::None,
                local_llm_bin: PathBuf::from(format!("{home}/.local/bin/llama-completion")),
                local_llm_model_path: PathBuf::from(format!(
                    "{home}/.local/share/kloyce/models/llm/qwen2.5-1.5b-instruct-q4_k_m.gguf"
                )),
                local_llm_timeout_secs: 20,
            }
        }

        #[cfg(windows)]
        {
            let local_appdata = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| {
                let profile =
                    std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\Default".into());
                format!("{profile}\\AppData\\Local")
            });
            let appdata = std::env::var("APPDATA").unwrap_or_else(|_| {
                let profile =
                    std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\Default".into());
                format!("{profile}\\AppData\\Roaming")
            });
            Self {
                whisper_bin: PathBuf::from(format!("{local_appdata}\\kloyce\\whisper-cli.exe")),
                ffmpeg_bin: PathBuf::from("ffmpeg.exe"),
                ffprobe_bin: PathBuf::from("ffprobe.exe"),
                model_path: PathBuf::from(format!(
                    "{local_appdata}\\kloyce\\models\\ggml-small.en.bin"
                )),
                web_port: 9876,
                sound_start: PathBuf::from("C:\\Windows\\Media\\Windows Notify System Generic.wav"),
                sound_stop: PathBuf::from("C:\\Windows\\Media\\Windows Notify Calendar.wav"),
                history_size: 100,
                gpu_poll_interval_ms: 2000,
                claude_cleanup: false,
                claude_bin: PathBuf::from(format!("{local_appdata}\\kloyce\\claude.exe")),
                claude_timeout_secs: 30,
                context_poll_interval_ms: 1000,
                dictionary_path: PathBuf::from(format!("{appdata}\\kloyce\\dictionary.toml")),
                dictionary_learning: true,
                dictionary_max_entries: 500,
                tmux_send_keys: false,
                tmux_auto_enter: false,
                tmux_voice_tag: true,
                tmux_voice_tag_text:
                    "[voice transcript - if anything is unclear, ask for clarification]".into(),
                clipboard_voice_tag: true,
                advanced_transcription: AdvancedTranscription::default(),
                transcription_defaults: TranscriptionDefaults::default(),
                source_media_retention_days: crate::db::DEFAULT_SOURCE_MEDIA_RETENTION_DAYS,
                hotkey_audio_retention_enabled: true,
                hotkey_audio_retention_hours: DEFAULT_HOTKEY_AUDIO_RETENTION_HOURS,
                whisper_flash_attn: true,
                whisper_threads: 0,
                whisper_beam_size: 0,
                filler_removal: true,
                cleanup_engine: CleanupEngine::None,
                local_llm_bin: PathBuf::from(format!(
                    "{local_appdata}\\kloyce\\llama-completion.exe"
                )),
                local_llm_model_path: PathBuf::from(format!(
                    "{local_appdata}\\kloyce\\models\\llm\\qwen2.5-1.5b-instruct-q4_k_m.gguf"
                )),
                local_llm_timeout_secs: 20,
            }
        }
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        #[cfg(unix)]
        {
            std::env::var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| "/home".into());
                    PathBuf::from(format!("{home}/.config"))
                })
                .join("kloyce/config.toml")
        }

        #[cfg(windows)]
        {
            let appdata = std::env::var("APPDATA").unwrap_or_else(|_| {
                let profile =
                    std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\Default".into());
                format!("{profile}\\AppData\\Roaming")
            });
            PathBuf::from(appdata).join("kloyce\\config.toml")
        }
    }

    pub fn load() -> Self {
        Self::load_from_path(&Self::config_path())
    }

    pub fn load_from_path(config_path: &std::path::Path) -> Self {
        if config_path.exists() {
            let content = std::fs::read_to_string(config_path).unwrap_or_default();
            let mut config: Config = toml::from_str(&content).unwrap_or_else(|e| {
                tracing::warn!("Failed to parse config: {e}, using defaults");
                Config::default()
            });
            config.apply_legacy_default_inferences(&content);
            config.apply_cleanup_engine_backcompat();
            config.validate_cleanup_engine();
            config
        } else {
            Config::default()
        }
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.save_to_path(&Self::config_path())
    }

    pub fn save_to_path(
        &self,
        config_path: &std::path::Path,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(config_path, self.to_toml_string()?)?;
        Ok(())
    }

    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn apply_transcription_defaults_update(
        &mut self,
        update: TranscriptionDefaultsUpdate,
    ) -> Result<(), ConfigValidationError> {
        let mut next = self.transcription_defaults.clone();

        if let Some(mode) = update.default_mode {
            next.default_mode = mode;
        }
        if let Some(model) = update.default_standard_model {
            next.default_standard_model = model;
        }
        if let Some(model) = update.default_diarized_model {
            next.default_diarized_model = model;
        }

        validate_transcription_defaults(&next, &self.advanced_transcription)?;
        self.transcription_defaults = next;
        Ok(())
    }

    pub fn apply_hotkey_audio_retention_update(
        &mut self,
        enabled: Option<bool>,
        retention_hours: Option<u64>,
    ) -> Result<(), ConfigValidationError> {
        if let Some(hours) = retention_hours {
            validate_hotkey_audio_retention_hours(hours)?;
            self.hotkey_audio_retention_hours = hours;
        }
        if let Some(enabled) = enabled {
            self.hotkey_audio_retention_enabled = enabled;
        }
        Ok(())
    }

    fn apply_legacy_default_inferences(&mut self, raw_toml: &str) {
        let parsed = raw_toml.parse::<toml::Value>().ok();
        let has_defaults = parsed
            .as_ref()
            .and_then(|value| value.get("transcription_defaults"))
            .is_some();
        if has_defaults {
            return;
        }

        if let Some(model) = crate::standard_models::model_id_from_path(&self.model_path) {
            self.transcription_defaults.default_standard_model = model.to_string();
        }
        self.transcription_defaults.default_diarized_model =
            self.advanced_transcription.model.clone();
    }

    /// Back-compat: if the legacy `claude_cleanup = true` is set but `cleanup_engine` is still
    /// `None` (not explicitly configured), promote the engine to `Claude` automatically.
    fn apply_cleanup_engine_backcompat(&mut self) {
        if self.claude_cleanup && self.cleanup_engine == CleanupEngine::None {
            tracing::info!(
                "Legacy `claude_cleanup = true` detected; \
                 promoting cleanup_engine to `claude`. \
                 Set `cleanup_engine = \"claude\"` in config to silence this message."
            );
            self.cleanup_engine = CleanupEngine::Claude;
        }
    }

    /// Warn (non-fatal) when `cleanup_engine = local` is requested but the required binaries
    /// are absent.  The daemon still starts; the runtime will fall back gracefully.
    fn validate_cleanup_engine(&self) {
        if self.cleanup_engine == CleanupEngine::Local {
            if !self.local_llm_bin.exists() {
                tracing::warn!(
                    path = %self.local_llm_bin.display(),
                    "cleanup_engine is `local` but local LLM binary not found at configured path; \
                     local LLM cleanup will be skipped at runtime"
                );
            }
            if !self.local_llm_model_path.exists() {
                tracing::warn!(
                    path = %self.local_llm_model_path.display(),
                    "cleanup_engine is `local` but local LLM model not found at configured path; \
                     local LLM cleanup will be skipped at runtime"
                );
            }
        }
    }
}

pub fn validate_transcription_defaults(
    defaults: &TranscriptionDefaults,
    advanced: &AdvancedTranscription,
) -> Result<(), ConfigValidationError> {
    if !crate::standard_models::is_standard_model_id(&defaults.default_standard_model) {
        return Err(ConfigValidationError::new(format!(
            "unknown standard model '{}'",
            defaults.default_standard_model
        )));
    }

    if defaults.default_diarized_model.trim().is_empty() {
        return Err(ConfigValidationError::new(
            "default diarized model cannot be empty",
        ));
    }

    if defaults.default_mode == TranscriptionMode::Diarized && !advanced.enabled {
        return Err(ConfigValidationError::new(
            "diarized mode is unavailable because advanced transcription is not enabled",
        ));
    }

    if defaults.default_mode == TranscriptionMode::Diarized {
        let python_bin = advanced.transcriber_venv.join("bin/python");
        if !python_bin.exists() {
            return Err(ConfigValidationError::new(format!(
                "diarized mode is unavailable because Python venv was not found at {}",
                advanced.transcriber_venv.display()
            )));
        }
    }

    Ok(())
}

pub fn validate_hotkey_audio_retention_hours(hours: u64) -> Result<(), ConfigValidationError> {
    if hours > MAX_HOTKEY_AUDIO_RETENTION_HOURS {
        return Err(ConfigValidationError::new(format!(
            "hotkey audio retention hours must be between 0 and {MAX_HOTKEY_AUDIO_RETENTION_HOURS}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcription_defaults_reject_unavailable_diarized_mode() {
        let mut config = Config::default();
        config.advanced_transcription.enabled = false;

        let update = TranscriptionDefaultsUpdate {
            default_mode: Some(TranscriptionMode::Diarized),
            default_standard_model: None,
            default_diarized_model: None,
        };

        let error = config
            .apply_transcription_defaults_update(update)
            .unwrap_err();

        assert!(error.to_string().contains("diarized mode is unavailable"));
    }

    #[test]
    fn transcription_defaults_persist_to_toml() {
        let mut config = Config::default();
        let update = TranscriptionDefaultsUpdate {
            default_mode: Some(TranscriptionMode::Standard),
            default_standard_model: Some("base.en".to_string()),
            default_diarized_model: Some("medium".to_string()),
        };

        config
            .apply_transcription_defaults_update(update)
            .expect("valid defaults update");

        let toml = config.to_toml_string().expect("serialize config");

        assert!(toml.contains("[transcription_defaults]"));
        assert!(toml.contains("default_mode = \"standard\""));
        assert!(toml.contains("default_standard_model = \"base.en\""));
        assert!(toml.contains("default_diarized_model = \"medium\""));
    }

    #[test]
    fn source_media_retention_uses_database_default() {
        let config = Config::default();

        assert_eq!(
            config.source_media_retention_days,
            crate::db::DEFAULT_SOURCE_MEDIA_RETENTION_DAYS
        );
    }

    #[test]
    fn hotkey_audio_retention_defaults_to_enabled_for_twenty_four_hours() {
        let config = Config::default();

        assert!(config.hotkey_audio_retention_enabled);
        assert_eq!(
            config.hotkey_audio_retention_hours,
            DEFAULT_HOTKEY_AUDIO_RETENTION_HOURS
        );
    }

    #[test]
    fn hotkey_audio_retention_hours_are_validated() {
        let mut config = Config::default();

        config
            .apply_hotkey_audio_retention_update(Some(false), Some(0))
            .expect("zero means expire immediately");
        assert!(!config.hotkey_audio_retention_enabled);
        assert_eq!(config.hotkey_audio_retention_hours, 0);

        let error = config
            .apply_hotkey_audio_retention_update(None, Some(MAX_HOTKEY_AUDIO_RETENTION_HOURS + 1))
            .unwrap_err();

        assert!(error.to_string().contains("hotkey audio retention hours"));
    }
}
