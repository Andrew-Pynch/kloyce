use crate::cleanup;
use crate::config::{CleanupEngine, Config};
use crate::dictionary::Dictionary;
use crate::local_cleanup;
use crate::text;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct TranscriptPipeline {
    dictionary: Arc<RwLock<Dictionary>>,
}

#[derive(Debug, Clone)]
pub struct TranscriptPipelinePolicy {
    pub filler_removal: bool,
    pub cleanup_engine: CleanupEngine,
    pub claude_bin: PathBuf,
    pub claude_timeout_secs: u64,
    pub local_llm_bin: PathBuf,
    pub local_llm_model_path: PathBuf,
    pub local_llm_timeout_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessedTranscript {
    pub text: String,
    pub word_count: u64,
    pub final_bytes: usize,
    pub final_chars: usize,
    pub elapsed: Duration,
}

impl TranscriptPipeline {
    pub fn new(dictionary: Arc<RwLock<Dictionary>>) -> Self {
        Self { dictionary }
    }

    pub async fn process(
        &self,
        raw_text: &str,
        context_tags: &[String],
        policy: &TranscriptPipelinePolicy,
    ) -> ProcessedTranscript {
        let start = std::time::Instant::now();
        let cleaned = text::clean_whisper_artifacts(raw_text);
        let cleaned = if policy.filler_removal {
            text::remove_fillers(&cleaned)
        } else {
            cleaned
        };
        let dictionary_text = {
            let dictionary = self.dictionary.read().await;
            dictionary.apply(&cleaned, context_tags)
        };

        let text = apply_cleanup_engine(&dictionary_text, policy).await;
        let word_count = text.split_whitespace().count() as u64;
        let final_bytes = text.len();
        let final_chars = text.chars().count();

        ProcessedTranscript {
            text,
            word_count,
            final_bytes,
            final_chars,
            elapsed: start.elapsed(),
        }
    }
}

impl TranscriptPipelinePolicy {
    pub fn from_config(config: &Config) -> Self {
        Self {
            filler_removal: config.filler_removal,
            cleanup_engine: config.cleanup_engine,
            claude_bin: config.claude_bin.clone(),
            claude_timeout_secs: config.claude_timeout_secs,
            local_llm_bin: config.local_llm_bin.clone(),
            local_llm_model_path: config.local_llm_model_path.clone(),
            local_llm_timeout_secs: config.local_llm_timeout_secs,
        }
    }
}

async fn apply_cleanup_engine(text: &str, policy: &TranscriptPipelinePolicy) -> String {
    match policy.cleanup_engine {
        CleanupEngine::None => text.to_string(),
        CleanupEngine::Claude => {
            cleanup::cleanup_transcript(text, &policy.claude_bin, policy.claude_timeout_secs)
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!("Claude cleanup failed, using dictionary result: {error}");
                    text.to_string()
                })
        }
        CleanupEngine::Local => local_cleanup::cleanup_local(
            text,
            &policy.local_llm_bin,
            &policy.local_llm_model_path,
            policy.local_llm_timeout_secs,
        )
        .await
        .unwrap_or_else(|error| {
            tracing::warn!("Local LLM cleanup failed, using dictionary result: {error}");
            text.to_string()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "kloyce-transcript-pipeline-test-{}-{}-{name}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn dictionary_with_global(dir: &std::path::Path) -> Arc<RwLock<Dictionary>> {
        let path = dir.join("dictionary.toml");
        std::fs::write(&path, "[global]\ncloyce = \"Kloyce\"\n").unwrap();
        Arc::new(RwLock::new(Dictionary::load(&path).unwrap()))
    }

    fn policy(cleanup_engine: CleanupEngine) -> TranscriptPipelinePolicy {
        let mut config = Config::default();
        config.cleanup_engine = cleanup_engine;
        config.claude_bin = PathBuf::from("/missing/claude");
        config.local_llm_bin = PathBuf::from("/missing/llama-completion");
        config.local_llm_model_path = PathBuf::from("/missing/model.gguf");
        TranscriptPipelinePolicy::from_config(&config)
    }

    #[tokio::test]
    async fn applies_text_cleanup_fillers_dictionary_and_stats_in_order() {
        let dir = temp_dir("order");
        let pipeline = TranscriptPipeline::new(dictionary_with_global(&dir));

        let result = pipeline
            .process("Um, [Music] say cloyce", &[], &policy(CleanupEngine::None))
            .await;

        assert_eq!(result.text, "Say Kloyce");
        assert_eq!(result.word_count, 2);
        assert_eq!(result.final_bytes, "Say Kloyce".len());
        assert_eq!(result.final_chars, "Say Kloyce".chars().count());
    }

    #[tokio::test]
    async fn filler_removal_false_preserves_fillers_after_artifact_cleanup() {
        let dir = temp_dir("fillers-off");
        let pipeline = TranscriptPipeline::new(dictionary_with_global(&dir));
        let mut policy = policy(CleanupEngine::None);
        policy.filler_removal = false;

        let result = pipeline
            .process("[Music] um say cloyce", &[], &policy)
            .await;

        assert_eq!(result.text, "um say Kloyce");
    }

    #[tokio::test]
    async fn cleanup_engine_failure_falls_back_to_dictionary_output() {
        let dir = temp_dir("cleanup-failure");
        let pipeline = TranscriptPipeline::new(dictionary_with_global(&dir));

        let result = pipeline
            .process("say cloyce", &[], &policy(CleanupEngine::Claude))
            .await;

        assert_eq!(result.text, "say Kloyce");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cleanup_engine_receives_dictionary_output() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("cleanup-input");
        let pipeline = TranscriptPipeline::new(dictionary_with_global(&dir));
        let fake_claude = dir.join("claude");
        std::fs::write(
            &fake_claude,
            "#!/bin/sh\ninput=$(cat)\nprintf '{\"result\":\"cleanup saw %s\"}\\n' \"$input\"\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_claude).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_claude, permissions).unwrap();
        let mut policy = policy(CleanupEngine::Claude);
        policy.claude_bin = fake_claude;

        let result = pipeline.process("say cloyce", &[], &policy).await;

        assert_eq!(result.text, "cleanup saw say Kloyce");
    }
}
