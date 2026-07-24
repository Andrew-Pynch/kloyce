use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DictionaryFile {
    #[serde(default)]
    pub global: BTreeMap<String, String>,
    #[serde(default)]
    pub context: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub meta: Meta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub last_updated: String,
}

impl Default for Meta {
    fn default() -> Self {
        Self {
            version: default_version(),
            last_updated: String::new(),
        }
    }
}

fn default_version() -> u32 {
    1
}

/// A single correction entry for merging.
#[derive(Debug, Clone)]
pub struct Correction {
    pub wrong: String,
    pub correct: String,
    /// If set, correction is context-specific (e.g. "demo/project").
    pub context: Option<String>,
}

/// Runtime dictionary with optimized lookup.
pub struct Dictionary {
    file: DictionaryFile,
    path: PathBuf,
    loaded_mtime: Option<SystemTime>,
}

impl Dictionary {
    /// Load dictionary from a TOML file. Returns empty dictionary if file doesn't exist.
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        if !path.exists() {
            tracing::info!("No dictionary file at {}, starting empty", path.display());
            return Ok(Self {
                file: DictionaryFile::default(),
                path: path.to_path_buf(),
                loaded_mtime: None,
            });
        }

        let content = std::fs::read_to_string(path)?;
        let file: DictionaryFile = toml::from_str(&content)?;
        let mtime = std::fs::metadata(path)?.modified().ok();
        let entry_count: usize =
            file.global.len() + file.context.values().map(|m| m.len()).sum::<usize>();
        tracing::info!(
            "Loaded dictionary from {} ({} entries)",
            path.display(),
            entry_count
        );

        Ok(Self {
            file,
            path: path.to_path_buf(),
            loaded_mtime: mtime,
        })
    }

    /// Create an empty dictionary at the given path.
    pub fn empty(path: PathBuf) -> Self {
        Self {
            file: DictionaryFile::default(),
            path,
            loaded_mtime: None,
        }
    }

    /// Check if the file has been modified externally and reload if so.
    pub fn reload_if_changed(&mut self) -> bool {
        let mtime = match std::fs::metadata(&self.path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return false,
        };

        if self.loaded_mtime == Some(mtime) {
            return false;
        }

        match Self::load(&self.path) {
            Ok(new) => {
                self.file = new.file;
                self.loaded_mtime = new.loaded_mtime;
                tracing::info!("Reloaded dictionary (external modification detected)");
                true
            }
            Err(e) => {
                tracing::warn!("Failed to reload dictionary: {e}");
                false
            }
        }
    }

    /// Apply corrections to text. Applies global entries plus any context-matching entries.
    /// Uses case-insensitive, word-boundary-aware matching with longest-match-first ordering.
    pub fn apply(&self, text: &str, context_tags: &[String]) -> String {
        let mut corrections: Vec<(&str, &str)> = self
            .file
            .global
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        // Add context-specific corrections
        for tag in context_tags {
            if let Some(ctx_map) = self.file.context.get(tag) {
                for (k, v) in ctx_map {
                    corrections.push((k.as_str(), v.as_str()));
                }
            }
        }

        if corrections.is_empty() {
            return text.to_string();
        }

        // Sort by wrong-text length descending (longest-match-first)
        corrections.sort_by_key(|b| std::cmp::Reverse(b.0.len()));

        let mut result = text.to_string();
        for (wrong, correct) in &corrections {
            result = replace_word_boundary_ci(&result, wrong, correct);
        }
        result
    }

    /// Merge new corrections into the dictionary. Returns count of entries actually added.
    pub fn merge_corrections(&mut self, entries: Vec<Correction>, max_entries: usize) -> usize {
        let current_count: usize =
            self.file.global.len() + self.file.context.values().map(|m| m.len()).sum::<usize>();

        let mut added = 0;
        for entry in entries {
            if current_count + added >= max_entries {
                tracing::warn!(
                    "Dictionary at max capacity ({max_entries}), skipping remaining corrections"
                );
                break;
            }

            let wrong = entry.wrong.to_lowercase();
            if wrong.is_empty() || entry.correct.is_empty() {
                continue;
            }

            if let Some(ctx) = &entry.context {
                let ctx_map = self.file.context.entry(ctx.clone()).or_default();
                if let std::collections::btree_map::Entry::Vacant(slot) = ctx_map.entry(wrong) {
                    tracing::info!(
                        "Learning: \"{}\" → \"{}\" (context: {})",
                        slot.key(),
                        entry.correct,
                        ctx
                    );
                    slot.insert(entry.correct);
                    added += 1;
                }
            } else if let std::collections::btree_map::Entry::Vacant(slot) =
                self.file.global.entry(wrong)
            {
                tracing::info!(
                    "Learning: \"{}\" → \"{}\" (global)",
                    slot.key(),
                    entry.correct
                );
                slot.insert(entry.correct);
                added += 1;
            }
        }

        if added > 0 {
            self.file.meta.last_updated = chrono::Utc::now().to_rfc3339();
        }

        added
    }

    /// Save dictionary to file atomically (write temp + rename).
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(&self.file)?;
        let tmp_path = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp_path, &content)?;
        std::fs::rename(&tmp_path, &self.path)?;

        tracing::debug!("Saved dictionary to {}", self.path.display());
        Ok(())
    }

    /// Build a whisper --prompt string from dictionary vocabulary.
    /// Extracts unique correct-side values, capped at ~200 words.
    pub fn whisper_prompt(&self) -> Option<String> {
        let mut vocab: Vec<&str> = Vec::new();

        for v in self.file.global.values() {
            for word in v.split_whitespace() {
                if !vocab.contains(&word) {
                    vocab.push(word);
                }
            }
        }
        for ctx_map in self.file.context.values() {
            for v in ctx_map.values() {
                for word in v.split_whitespace() {
                    if !vocab.contains(&word) {
                        vocab.push(word);
                    }
                }
            }
        }

        if vocab.is_empty() {
            return None;
        }

        vocab.truncate(200);
        Some(vocab.join(", "))
    }

    /// Serialize current entries for inclusion in the learning prompt.
    pub fn summarize_for_prompt(&self) -> String {
        let mut lines = Vec::new();
        for (wrong, correct) in &self.file.global {
            lines.push(format!("\"{}\" → \"{}\"", wrong, correct));
        }
        for (ctx, map) in &self.file.context {
            for (wrong, correct) in map {
                lines.push(format!(
                    "\"{}\" → \"{}\" (context: {})",
                    wrong, correct, ctx
                ));
            }
        }
        if lines.is_empty() {
            "(empty - no entries yet)".to_string()
        } else {
            lines.join("\n")
        }
    }
}

/// Replace occurrences of `wrong` in `text` with `correct`, case-insensitively,
/// only when `wrong` sits on word boundaries.
fn replace_word_boundary_ci(text: &str, wrong: &str, correct: &str) -> String {
    if wrong.is_empty() {
        return text.to_string();
    }

    let text_lower = text.to_lowercase();
    let wrong_lower = wrong.to_lowercase();
    let mut result = String::with_capacity(text.len());
    let mut search_start = 0;

    while let Some(pos) = text_lower[search_start..].find(&wrong_lower) {
        let abs_pos = search_start + pos;
        let end_pos = abs_pos + wrong.len();

        // Check word boundaries
        let left_ok = abs_pos == 0 || !text.as_bytes()[abs_pos - 1].is_ascii_alphanumeric();
        let right_ok = end_pos >= text.len() || !text.as_bytes()[end_pos].is_ascii_alphanumeric();

        if left_ok && right_ok {
            result.push_str(&text[search_start..abs_pos]);
            result.push_str(correct);
            search_start = end_pos;
        } else {
            result.push_str(&text[search_start..abs_pos + 1]);
            search_start = abs_pos + 1;
        }
    }

    result.push_str(&text[search_start..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_word_boundary_ci() {
        assert_eq!(
            replace_word_boundary_ci("say cloyce now", "cloyce", "Kloyce"),
            "say Kloyce now"
        );
        // Should not match inside words
        assert_eq!(
            replace_word_boundary_ci("today is fine", "to", "two"),
            "today is fine"
        );
        // Case insensitive
        assert_eq!(
            replace_word_boundary_ci("say CLOYCE now", "cloyce", "Kloyce"),
            "say Kloyce now"
        );
        // Multiple occurrences
        assert_eq!(
            replace_word_boundary_ci("clod and clod", "clod", "Claude"),
            "Claude and Claude"
        );
        // At boundaries
        assert_eq!(replace_word_boundary_ci("clod", "clod", "Claude"), "Claude");
    }

    #[test]
    fn test_apply_with_context() {
        let mut dict = Dictionary::empty(PathBuf::from("/tmp/test.toml"));
        dict.file.global.insert("cloyce".into(), "Kloyce".into());
        dict.file.context.insert(
            "demo/project".into(),
            BTreeMap::from([("daemon".into(), "daemon".into())]),
        );

        // Global applies without context
        assert_eq!(dict.apply("say cloyce", &[]), "say Kloyce");

        // Context entries apply when tag matches
        let tags = vec!["demo/project".to_string()];
        assert_eq!(dict.apply("say cloyce", &tags), "say Kloyce");
    }

    #[test]
    fn test_merge_corrections() {
        let mut dict = Dictionary::empty(PathBuf::from("/tmp/test.toml"));

        let corrections = vec![
            Correction {
                wrong: "cloyce".into(),
                correct: "Kloyce".into(),
                context: None,
            },
            Correction {
                wrong: "hi per land".into(),
                correct: "Hyprland".into(),
                context: None,
            },
        ];

        let added = dict.merge_corrections(corrections, 500);
        assert_eq!(added, 2);
        assert_eq!(dict.file.global.get("cloyce").unwrap(), "Kloyce");

        // Duplicates should not be added
        let dupes = vec![Correction {
            wrong: "cloyce".into(),
            correct: "Kloyce".into(),
            context: None,
        }];
        let added = dict.merge_corrections(dupes, 500);
        assert_eq!(added, 0);
    }

    #[test]
    fn test_whisper_prompt() {
        let mut dict = Dictionary::empty(PathBuf::from("/tmp/test.toml"));
        assert_eq!(dict.whisper_prompt(), None);

        dict.file.global.insert("cloyce".into(), "Kloyce".into());
        dict.file
            .global
            .insert("hi per land".into(), "Hyprland".into());

        let prompt = dict.whisper_prompt().unwrap();
        assert!(prompt.contains("Kloyce"));
        assert!(prompt.contains("Hyprland"));
    }
}
