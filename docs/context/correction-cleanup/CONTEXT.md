# Correction And Cleanup Context

Kloyce can improve transcript usefulness with context-aware corrections, text cleanup, optional Claude-powered cleanup, and optional dictionary learning.

## Language

**Correction Dictionary**:
The TOML configuration that maps commonly misheard text to preferred terms, globally or for a matching context.
_Avoid_: prompt database, spellcheck list, model vocabulary

**Window Context**:
The active application, title, process, and tmux information captured around a recording.
_Avoid_: browser context, job metadata, source media tags

**Whisper Prompt**:
Prompt text derived from the **Correction Dictionary** to bias whisper.cpp toward project-specific words.
_Avoid_: cleanup prompt, Claude instruction

**Text Cleanup**:
Pure local post-processing that removes known Whisper artifacts from transcript text.
_Avoid_: Claude cleanup, dictionary learning

**Claude Cleanup**:
Optional Claude-powered rewriting of transcript text after transcription.
_Avoid_: raw transcription, local artifact stripping

**Dictionary Learning**:
Optional background Claude-powered discovery of new correction candidates from transcript history.
_Avoid_: automatic mutation of transcript results, training Whisper

## Relationships

- **Window Context** helps select context-scoped **Correction Dictionary** entries.
- **Correction Dictionary** entries can feed a **Whisper Prompt** before transcription.
- **Text Cleanup** is local deterministic cleanup and should stay separate from **Claude Cleanup**.
- **Claude Cleanup** is optional and depends on external Claude CLI availability/configuration.
- **Dictionary Learning** proposes correction improvements from observed transcripts; it is not the transcription backend.
- Correction and cleanup behavior may affect push-to-talk and file job transcript text, but should not redefine queueing or daemon state.

## Example Dialogue

> **Dev:** "Is fixing 'hyper land' a model-selection feature?"
> **Domain expert:** "No - that belongs in the **Correction Dictionary**, selected using **Window Context**."

> **Dev:** "Can we call every transcript rewrite cleanup?"
> **Domain expert:** "No - distinguish deterministic **Text Cleanup** from optional **Claude Cleanup**."

## Flagged Ambiguities

- "context" can mean **Window Context**, context tags on file jobs, or agent documentation context. Name the specific concept.
- "cleanup" can mean artifact stripping or Claude-powered rewriting. Use **Text Cleanup** or **Claude Cleanup**.
