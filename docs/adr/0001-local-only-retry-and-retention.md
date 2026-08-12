# ADR 0001: Local-only transcription retry with 24 hour audio retention

- Status: Accepted
- Date: 2026-08-11

## Context

Hotkey transcripts could silently drop when transcription errored. The temporary WAV was deleted even on failure, leaving no source for a retry.

## Decision

Retain recorded audio for 24 hours on both transcription success and failure. Perform one automatic local retry from the retained audio. Support manual retry through `kloyce-ctl` while the audio remains available.

Retries remain local. Kloyce will not use a cloud or hosted speech-to-text fallback, and it will not use an LLM cleanup bridge. Transcript cleanup uses deterministic filler removal only.

## Consequences

- Recorded audio can remain on disk for up to 24 hours.
- The hourly daemon cleanup worker enforces the retention expiry.
- Retry attempts are bounded, so happy-path latency remains unchanged.
