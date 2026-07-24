# Push-To-Talk Context

Kloyce's push-to-talk path is the latency-sensitive interactive transcription flow. It records microphone audio when the user toggles recording, transcribes the capture, then inserts the resulting text into the active workflow.

## Language

**Push-To-Talk Recording**:
A microphone capture started and stopped by the interactive hotkey or control command.
_Avoid_: file job, upload, queued recording

**Daemon State**:
The live recording state machine for interactive use: `Idle`, `Recording`, and `Transcribing`.
_Avoid_: job status, upload state, queue state

**Insertion Target**:
The focused destination where interactive transcript text should go after transcription, either through clipboard/paste behavior or direct tmux pane injection.
_Avoid_: output file, transcript store, job result destination

**Toggle Command**:
An IPC command that advances the push-to-talk state machine.
_Avoid_: enqueue, submit job, upload

**Interactive Cancellation**:
Stopping an active push-to-talk recording without producing a transcript.
_Avoid_: cancelling a Transcription Job, deleting history

## Relationships

- A **Push-To-Talk Recording** is not a **Transcription Job**.
- **Push-To-Talk Recording** work must not wait behind the file **Transcription Queue**.
- The daemon owns the **Daemon State** and broadcasts state changes to observers.
- `toggle` starts recording from `Idle`, stops recording from `Recording`, and reports already-in-progress from `Transcribing`.
- `toggle_enter` follows the same recording lifecycle and asks tmux output to press Enter after insertion.
- `cancel` applies to active interactive recording behavior, not durable file job cancellation.
- Completed push-to-talk transcription preserves existing paste/tmux behavior rather than creating a file-job notification workflow.

## Example Dialogue

> **Dev:** "Should Super+R wait behind a long uploaded MP4?"
> **Domain expert:** "No - push-to-talk remains outside the file **Transcription Queue**."

> **Dev:** "Is daemon `Transcribing` the same as a file job status?"
> **Domain expert:** "No - use **Daemon State** for interactive recording and **Job Status** for durable file work."

## Flagged Ambiguities

- "state" could mean **Daemon State** or file **Job Status**. Resolve by asking whether the work is interactive push-to-talk or durable file transcription.
- "cancel" could mean interactive cancellation or file job cancellation. Do not reuse one behavior name to imply both lifecycles are identical.
