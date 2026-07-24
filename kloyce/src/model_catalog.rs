use crate::config::{Config, TranscriptionMode};
use crate::standard_models::{self, StandardModelDefinition};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

const MODEL_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

#[derive(Debug, Clone, Serialize)]
pub struct StandardModel {
    pub id: &'static str,
    pub label: &'static str,
    pub filename: &'static str,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelInstallStatus {
    Installed,
    Uninstalled,
    Downloading,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct StandardModelStatus {
    pub id: &'static str,
    pub label: &'static str,
    pub filename: &'static str,
    pub path: PathBuf,
    pub url: String,
    pub status: ModelInstallStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ModelDownloadRegistry {
    states: Arc<RwLock<HashMap<String, DownloadState>>>,
}

#[derive(Debug, Clone)]
enum DownloadState {
    Downloading,
    Error(String),
}

impl ModelDownloadRegistry {
    pub async fn start_download(&self, model_id: &str) -> bool {
        let mut states = self.states.write().await;
        if matches!(states.get(model_id), Some(DownloadState::Downloading)) {
            return false;
        }

        states.insert(model_id.to_string(), DownloadState::Downloading);
        true
    }

    pub async fn status_for(&self, model_id: &str) -> Option<(ModelInstallStatus, Option<String>)> {
        let states = self.states.read().await;
        states.get(model_id).map(|state| match state {
            DownloadState::Downloading => (ModelInstallStatus::Downloading, None),
            DownloadState::Error(message) => (ModelInstallStatus::Error, Some(message.clone())),
        })
    }

    pub async fn mark_error(&self, model_id: &str, message: String) {
        self.states
            .write()
            .await
            .insert(model_id.to_string(), DownloadState::Error(message));
    }

    pub async fn clear(&self, model_id: &str) {
        self.states.write().await.remove(model_id);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModeAvailability {
    pub mode: TranscriptionMode,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub fn standard_model_catalog() -> Vec<StandardModel> {
    standard_models::STANDARD_MODELS
        .iter()
        .map(StandardModel::from_definition)
        .collect()
}

pub fn standard_model_by_id(model_id: &str) -> Option<StandardModel> {
    standard_models::by_id(model_id).map(StandardModel::from_definition)
}

pub fn standard_model_path(config: &Config, model_id: &str) -> Option<PathBuf> {
    let model = standard_model_by_id(model_id)?;
    if model.id == "small.en" {
        Some(config.model_path.clone())
    } else {
        let dir = config
            .model_path
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Some(dir.join(model.filename))
    }
}

pub async fn standard_model_statuses(
    config: &Config,
    downloads: &ModelDownloadRegistry,
) -> Vec<StandardModelStatus> {
    let mut statuses = Vec::new();

    for model in standard_model_catalog() {
        let path =
            standard_model_path(config, model.id).unwrap_or_else(|| PathBuf::from(model.filename));
        let mut status = if path.exists() {
            ModelInstallStatus::Installed
        } else {
            ModelInstallStatus::Uninstalled
        };
        let mut error = None;

        if !matches!(status, ModelInstallStatus::Installed) {
            if let Some((download_status, download_error)) = downloads.status_for(model.id).await {
                status = download_status;
                error = download_error;
            }
        }

        statuses.push(StandardModelStatus {
            id: model.id,
            label: model.label,
            filename: model.filename,
            path,
            url: model.url,
            status,
            error,
        });
    }

    statuses
}

pub fn mode_availability(config: &Config) -> Vec<ModeAvailability> {
    vec![
        standard_mode_availability(config),
        diarized_mode_availability(config),
    ]
}

pub fn standard_mode_availability(config: &Config) -> ModeAvailability {
    let default_model = &config.transcription_defaults.default_standard_model;
    let available = standard_model_path(config, default_model)
        .map(|path| path.exists())
        .unwrap_or(false);

    ModeAvailability {
        mode: TranscriptionMode::Standard,
        available,
        reason: if available {
            None
        } else if !standard_models::is_standard_model_id(default_model) {
            Some(format!("Unknown standard model '{default_model}'"))
        } else {
            Some(format!("Standard model '{default_model}' is not installed"))
        },
    }
}

impl StandardModel {
    fn from_definition(model: &StandardModelDefinition) -> Self {
        Self {
            id: model.id,
            label: model.label,
            filename: model.filename,
            url: format!("{MODEL_BASE_URL}/{}", model.filename),
        }
    }
}

pub fn diarized_mode_availability(config: &Config) -> ModeAvailability {
    let advanced = &config.advanced_transcription;
    let python_bin = advanced.transcriber_venv.join("bin/python");

    let (available, reason) = if !advanced.enabled {
        (
            false,
            Some(
                "Advanced transcription is not enabled. Set advanced_transcription.enabled = true in config.toml"
                    .to_string(),
            ),
        )
    } else if !python_bin.exists() {
        (
            false,
            Some(format!(
                "Python venv not found at: {}",
                advanced.transcriber_venv.display()
            )),
        )
    } else {
        (true, None)
    };

    ModeAvailability {
        mode: TranscriptionMode::Diarized,
        available,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn catalog_reports_all_v1_standard_models() {
        let config = Config::default();
        let registry = ModelDownloadRegistry::default();

        let statuses = standard_model_statuses(&config, &registry).await;
        let ids: Vec<_> = statuses.iter().map(|status| status.id).collect();

        assert_eq!(
            ids,
            vec![
                "tiny.en",
                "base.en",
                "small.en",
                "medium.en",
                "large-v3-turbo",
                "large-v3-turbo-q5_0"
            ]
        );
    }

    #[tokio::test]
    async fn download_registry_tracks_one_active_download_per_model() {
        let registry = ModelDownloadRegistry::default();

        assert!(registry.start_download("small.en").await);
        assert!(!registry.start_download("small.en").await);

        registry
            .mark_error("small.en", "network error".to_string())
            .await;
        assert!(registry.start_download("small.en").await);
    }

    #[test]
    fn diarized_mode_visible_but_unavailable_when_disabled() {
        let config = Config::default();

        let availability = mode_availability(&config);
        let diarized = availability
            .iter()
            .find(|mode| mode.mode == TranscriptionMode::Diarized)
            .expect("diarized mode should be visible");

        assert!(!diarized.available);
        assert!(diarized
            .reason
            .as_ref()
            .expect("unavailable reason")
            .contains("Advanced transcription is not enabled"));
    }
}
