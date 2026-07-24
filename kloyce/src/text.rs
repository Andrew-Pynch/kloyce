/// Remove common filler words from transcription text.
///
/// Only whole-token matches are removed (word-boundary semantics via whitespace
/// splitting), so real words that contain filler strings as substrings — e.g.
/// "umbrella", "error", "ahead", "summer", "hummus", "mahogany" — are preserved.
///
/// Repeated-letter variants are collapsed to their canonical form in the match
/// list: "uhh" → "uh", "umm" → "um", "ahh" → "ah".
///
/// After removal, punctuation artifacts (orphaned commas, double commas, etc.)
/// are cleaned up and leading/trailing whitespace is trimmed.  Sentence-initial
/// capitalisation is restored when the original text began with an uppercase
/// letter (e.g. "Um, hello" → "Hello").
pub fn remove_fillers(text: &str) -> String {
    // Canonical filler tokens, all lowercase. Repeated-letter variants ("uhh",
    // "umm", "ahh") are listed explicitly so no runtime normalisation is needed.
    const FILLERS: &[&str] = &[
        "uh", "uhh", "um", "umm", "uhm", "erm", "er", "ah", "ahh", "mm", "hmm",
    ];

    // Use a closure (not a nested fn) so it can reference the local const above.
    let is_filler = |s: &str| -> bool {
        let lower = s.to_ascii_lowercase();
        FILLERS.contains(&lower.as_str())
    };

    const PUNCT: &[char] = &['.', ',', ';', ':', '!', '?'];

    // Process each whitespace-separated token.
    // Split off trailing (and leading) punctuation; if the inner word is a
    // filler, discard it and retain only the surrounding punctuation chars.
    let mut parts: Vec<String> = Vec::new();
    for token in text.split_whitespace() {
        let core = token.trim_end_matches(|c: char| PUNCT.contains(&c));
        let suffix = &token[core.len()..];
        let core_inner = core.trim_start_matches(|c: char| PUNCT.contains(&c));
        let prefix = &core[..core.len() - core_inner.len()];

        if is_filler(core_inner) {
            // Discard the filler; keep any surrounding punctuation as its own token.
            let retained = format!("{}{}", prefix, suffix);
            if !retained.is_empty() {
                parts.push(retained);
            }
        } else {
            parts.push(token.to_string());
        }
    }

    let joined = parts.join(" ");

    // Fix punctuation artifacts introduced by filler removal.
    // Order matters: fix doubled/orphaned commas before stripping spaces.
    let fixed = joined
        .replace(", ,", ",")
        .replace(", .", ".")
        .replace(", !", "!")
        .replace(", ?", "?")
        .replace(", ;", ";");

    // Remove any space that now appears directly before punctuation
    // (produced when a lone punct token sits at the end of the output).
    let fixed = fixed
        .replace(" ,", ",")
        .replace(" .", ".")
        .replace(" !", "!")
        .replace(" ?", "?");

    // Strip leading commas and spaces.
    let fixed = fixed.trim_start_matches([',', ' ']).to_string();

    // Collapse multiple spaces — same approach as clean_whisper_artifacts.
    let mut prev_space = false;
    let collapsed: String = fixed
        .chars()
        .filter(|c| {
            if c.is_whitespace() {
                if prev_space {
                    return false;
                }
                prev_space = true;
            } else {
                prev_space = false;
            }
            true
        })
        .collect();

    // Strip any orphaned trailing comma left after filler removal
    // (e.g. "word um," → after processing → "word," → stripped to "word").
    // STT output virtually never ends legitimately with a comma.
    let mut result = collapsed.trim().trim_end_matches(',').trim().to_string();

    // Restore sentence-initial capitalisation when the original text started
    // with a title-cased filler (e.g. "Um, hello") but NOT when the filler was
    // ALL-CAPS (e.g. "UM so …"), since that indicates emphasis rather than a
    // sentence-opening filler.  We detect title-case by checking that the second
    // character of the original text is a lowercase letter (or non-alphabetic).
    let orig_second = text.chars().nth(1);
    if let (Some(orig_first), Some(res_first)) = (text.chars().next(), result.chars().next()) {
        let is_title_case_start = orig_first.is_uppercase()
            && orig_second.is_none_or(|c| !c.is_alphabetic() || c.is_ascii_lowercase());
        if is_title_case_start && res_first.is_ascii_lowercase() {
            let mut chars = result.chars();
            let first = chars.next().unwrap();
            result = first.to_uppercase().to_string() + chars.as_str();
        }
    }

    result
}

/// Remove common whisper hallucination artifacts from transcription text.
pub fn clean_whisper_artifacts(text: &str) -> String {
    let mut result = text.to_string();

    // Remove bracketed annotations like [BLANK_AUDIO], [Music], [INAUDIBLE], etc.
    while let Some(start) = result.find('[') {
        if let Some(end) = result[start..].find(']') {
            let bracket_content = &result[start + 1..start + end];
            // Only remove if it looks like an annotation (ALL_CAPS, single word, or known patterns)
            if bracket_content
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_' || c == ' ')
                || bracket_content.eq_ignore_ascii_case("music")
                || bracket_content.eq_ignore_ascii_case("blank_audio")
                || bracket_content.eq_ignore_ascii_case("inaudible")
            {
                result = format!("{}{}", &result[..start], &result[start + end + 1..]);
                continue;
            }
        }
        break;
    }

    // Collapse multiple whitespace
    let mut prev_space = false;
    let collapsed: String = result
        .chars()
        .filter(|c| {
            if c.is_whitespace() {
                if prev_space {
                    return false;
                }
                prev_space = true;
            } else {
                prev_space = false;
            }
            true
        })
        .collect();

    collapsed.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- remove_fillers -----

    #[test]
    fn test_fillers_removed_basic() {
        assert_eq!(remove_fillers("um so uh testing"), "so testing");
    }

    #[test]
    fn test_all_filler_variants_removed() {
        // Every canonical filler + repeated-letter variants
        let fillers = [
            "uh", "uhh", "um", "umm", "uhm", "erm", "er", "ah", "ahh", "mm", "hmm",
        ];
        for f in &fillers {
            let input = format!("{} word", f);
            assert_eq!(
                remove_fillers(&input),
                "word",
                "expected filler '{f}' to be removed"
            );
        }
    }

    #[test]
    fn test_real_words_with_filler_substrings_preserved() {
        // "er" inside "error", "summer"; "ah" inside "ahead"; "um" inside "umbrella"/"hummus"
        let input = "umbrella error ahead summer hummus mahogany";
        assert_eq!(remove_fillers(input), input);
    }

    #[test]
    fn test_mid_sentence_filler_with_punctuation() {
        // "um," should be stripped; surrounding commas collapse cleanly
        assert_eq!(remove_fillers("I think, um, yes"), "I think, yes");
    }

    #[test]
    fn test_capitalized_filler_at_sentence_start() {
        // "Um," is the opener → removed; result should be re-capitalised
        assert_eq!(remove_fillers("Um, hello"), "Hello");
    }

    #[test]
    fn test_filler_only_input() {
        assert_eq!(remove_fillers("um uh er"), "");
    }

    #[test]
    fn test_empty_string_passthrough() {
        assert_eq!(remove_fillers(""), "");
    }

    #[test]
    fn test_no_fillers_passthrough() {
        let input = "just a normal sentence";
        assert_eq!(remove_fillers(input), input);
    }

    #[test]
    fn test_case_insensitive_removal() {
        assert_eq!(remove_fillers("UM so UH testing"), "so testing");
    }

    #[test]
    fn test_multiple_consecutive_fillers() {
        assert_eq!(remove_fillers("um uh er hmm okay"), "okay");
    }

    #[test]
    fn test_trailing_filler_with_comma_removed() {
        // Trailing orphaned comma should be cleaned up
        assert_eq!(remove_fillers("testing um,"), "testing");
    }

    #[test]
    fn test_filler_before_period_cleaned() {
        // "um." → period should not orphan
        assert_eq!(remove_fillers("I see um. Yes"), "I see. Yes");
    }
}
