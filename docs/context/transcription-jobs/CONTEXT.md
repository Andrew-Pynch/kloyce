# Transcription Jobs Context

Kloyce file transcription uses durable daemon-owned jobs for uploaded or path-based source media. This context defines the vocabulary for queueing, mode selection, model management, media preparation, persistence, and file-job results.

## Language

**Transcription Job**:
A durable unit of work submitted to Kloyce to turn one source media file into one transcript result using selected transcription settings.
_Avoid_: browser queue item, upload task, one-off request

**Source Media**:
An audio or video file owned by Kloyce as the input for a **Transcription Job**.
_Avoid_: browser file, original path, upload blob

**Working Audio**:
A disposable audio file derived from **Source Media** so the selected transcription backend receives a predictable input format.
_Avoid_: converted upload, temp file when discussing job behavior, original audio

**Transcription Queue**:
The daemon-owned FIFO ordering of **Transcription Jobs** awaiting processing.
_Avoid_: browser queue, parallel batch, ad hoc worker list

**Transcription Mode**:
The selected processing path for a **Transcription Job**, either `standard` or `diarized` in V1.
_Avoid_: backend when discussing user choice, arbitrary provider, mode matrix

**Transcription Defaults**:
The daemon-persisted settings Kloyce applies when a new **Transcription Job** is created without explicit overrides.
_Avoid_: app preferences when discussing job behavior, hidden config, browser defaults

**Per-Job Settings**:
The mode, model, and diarization choices captured on a specific **Transcription Job** at submission time.
_Avoid_: temporary form state, runtime flags when discussing persisted job behavior

**Managed Whisper Model**:
A known whisper.cpp model that Kloyce can detect locally, download on demand, and use for `standard` **Transcription Mode** jobs.
_Avoid_: arbitrary model path in the dashboard, bundled model, always-installed model

**Standard Model Catalog**:
The English-first set of **Managed Whisper Models** offered for `standard` **Transcription Mode** in V1.
_Avoid_: every Whisper model, multilingual catalog, arbitrary Hugging Face model list

**Diarized Model**:
A model choice used by the advanced speaker-aware path for `diarized` **Transcription Mode** jobs.
_Avoid_: whisper.cpp model file, standard model, shared model setting

**Mode Availability**:
Whether a **Transcription Mode** can currently accept new **Transcription Jobs** on this machine.
_Avoid_: hiding unsupported mode, late runtime surprise, silent fallback

**Job Status**:
The current lifecycle state of a **Transcription Job**.
_Avoid_: daemon state, upload state, progress label

**Transcript Result**:
The completed text output produced by a successful **Transcription Job**.
_Avoid_: log text, clipboard payload, raw backend output

## Relationships

- A **Transcription Job** has exactly one **Source Media**
- A **Transcription Job** may create **Working Audio** during media preparation
- A **Transcription Job** records the transcription settings selected at submission time
- A **Transcription Job** has exactly one **Transcription Mode**
- A **Transcription Job** has exactly one **Job Status**
- A successful **Transcription Job** has exactly one **Transcript Result**
- **Per-Job Settings** override **Transcription Defaults** for one **Transcription Job**
- A **Transcription Queue** may contain many **Transcription Jobs** but V1 runs at most one job at a time
- V1 processes the **Transcription Queue** in first-in, first-out order
- V1 **Transcription Modes** are `standard` for the existing whisper.cpp path and `diarized` for the advanced speaker-aware path
- V1 downloads non-default **Managed Whisper Models** on demand rather than installing every model up front
- V1 **Standard Model Catalog** includes `tiny.en`, `base.en`, `small.en`, and `medium.en`
- `standard` and `diarized` **Transcription Modes** have independent default model settings
- Kloyce exposes **Mode Availability** so unavailable modes can be shown but disabled before job submission
- Kloyce blocks new **Transcription Jobs** for unavailable modes rather than falling back to another **Transcription Mode**
- V1 **Job Status** values are `queued`, `preparing_media`, `downloading_model`, `transcribing`, `succeeded`, `failed`, and `cancelled`
- V1 cancellation keeps the **Transcription Job** record, removes queued work from execution, and best-effort stops active media preparation, model download, or transcription
- V1 rejects cancellation for terminal **Job Status** values
- Changes to **Transcription Defaults** take effect for new **Transcription Jobs** without restarting Kloyce
- V1 keeps **Source Media** for 7 days by default after a **Transcription Job** reaches a terminal **Job Status**
- Kloyce deletes **Working Audio** after the job reaches a terminal **Job Status**
- Kloyce sends daemon-owned completion notifications for **Transcription Jobs** without requiring the submitter to stay connected
- File-upload **Transcription Jobs** persist transcript results and notify; they do not paste or inject transcript text into the focused app
- All file transcription submissions use the **Transcription Job** processor internally, including compatibility APIs that wait for completion
- Hotkey push-to-talk recordings remain latency-sensitive and do not wait behind the file **Transcription Queue** in V1
- `kloyce-ctl transcribe-file` remains synchronous by default while offering explicit queued/follow modes
- `kloyce-ctl transcribe-file-advanced` remains a compatibility path for submitting diarized file jobs
- The primary copy action copies the **Transcript Result** text only; diarized results preserve speaker labels
- Dragging a local file into the dashboard creates daemon-owned **Source Media** instead of relying on the browser to provide a local path
- The Kloyce dashboard displays the daemon-owned **Transcription Job** queue rather than maintaining a separate browser-only queue

## Example Dialogue

> **Dev:** "Should a dragged file just call the existing synchronous endpoint?"
> **Domain expert:** "No - dragging a file creates a **Transcription Job** so the daemon, dashboard, CLI, and external API callers share the same queue."

> **Dev:** "Can the dashboard submit the dropped file by local path?"
> **Domain expert:** "No - drag/drop uploads bytes into Kloyce, which stores **Source Media** for the **Transcription Job**."

> **Dev:** "Can Kloyce run several dragged files at once?"
> **Domain expert:** "No - V1 keeps one active **Transcription Job** and processes the **Transcription Queue** FIFO."

> **Dev:** "Can we pass every MP4 or MP3 straight to Whisper?"
> **Domain expert:** "No - Kloyce prepares **Working Audio** from **Source Media** before transcription when the backend needs a normalized input."

> **Dev:** "Should the UI let me pick any provider and model combination?"
> **Domain expert:** "No - V1 exposes two **Transcription Modes**: `standard` and `diarized`."

> **Dev:** "Should setup download every Whisper model?"
> **Domain expert:** "No - Kloyce installs a default **Managed Whisper Model** and downloads other managed models on demand."

> **Dev:** "Should V1 expose every Whisper multilingual and large model?"
> **Domain expert:** "No - V1 keeps the **Standard Model Catalog** English-first: `tiny.en`, `base.en`, `small.en`, and `medium.en`."

> **Dev:** "Does changing the standard Whisper default also change diarized transcription?"
> **Domain expert:** "No - `diarized` mode uses its own **Diarized Model** default."

> **Dev:** "Should the dashboard hide diarized mode when the advanced transcriber is not configured?"
> **Domain expert:** "No - it should show **Mode Availability** and disable `diarized` until setup is complete."

> **Dev:** "If `diarized` is the default but unavailable, should Kloyce quietly use `standard`?"
> **Domain expert:** "No - Kloyce blocks the submission so transcript shape never changes silently."

> **Dev:** "Should missing model be a blocked queue state?"
> **Domain expert:** "No - V1 either downloads the model as `downloading_model` or rejects the job before it enters the **Transcription Queue**."

> **Dev:** "If I cancel a running **Transcription Job**, do we keep a partial transcript?"
> **Domain expert:** "No - V1 marks it `cancelled`, stops active work best-effort, and keeps the job record without a partial transcript."

> **Dev:** "Should Kloyce keep uploaded media forever so I can retry later?"
> **Domain expert:** "No - V1 keeps **Source Media** for 7 days by default and deletes **Working Audio** when the job finishes."

> **Dev:** "Does `kloyce-ctl` need to stay running to notify when a queued job finishes?"
> **Domain expert:** "No - completion notification is daemon-owned **Transcription Job** behavior."

> **Dev:** "Should a completed file upload paste the transcript wherever my cursor is?"
> **Domain expert:** "No - file-upload **Transcription Jobs** persist results and notify, but never auto-paste."

> **Dev:** "Should the copy button include filename, model, and timestamps?"
> **Domain expert:** "No - the primary copy action copies **Transcript Result** text only, preserving speaker labels for diarized results."

> **Dev:** "Should the old synchronous file API keep separate transcription logic?"
> **Domain expert:** "No - file APIs should use the **Transcription Job** processor internally; compatibility endpoints can wait for the job."

> **Dev:** "Should Super+R wait behind a long uploaded MP4?"
> **Domain expert:** "No - hotkey push-to-talk remains outside the file **Transcription Queue** in V1."

> **Dev:** "Should `kloyce-ctl transcribe-file` become async by default?"
> **Domain expert:** "No - it stays synchronous for compatibility, with explicit queued and follow modes for **Transcription Jobs**."

> **Dev:** "If I change the defaults after queuing a file, does the queued job change?"
> **Domain expert:** "No - the job keeps the **Per-Job Settings** captured when it was submitted."

> **Dev:** "Do I need to restart Kloyce after changing default mode or model?"
> **Domain expert:** "No - changed **Transcription Defaults** apply to newly submitted **Transcription Jobs** immediately."

## Flagged Ambiguities

- "processing queue" could mean browser-only UI state or daemon-owned durable work. Resolved: use **Transcription Job** for queued daemon work and show that queue in the dashboard.
- "file" could mean a browser-selected file, an absolute path supplied by another local process, or daemon-owned **Source Media**. Resolved: drag/drop creates **Source Media**; path-based API requests remain a separate submission route.
- "queue" could imply parallel workers or priority scheduling. Resolved: V1 **Transcription Queue** is single-active FIFO.
- "converted file" could mean a user-visible output or an internal derived artifact. Resolved: use **Working Audio** for disposable derived input.
- "sidecar" was used loosely for FFmpeg. Resolved: avoid "sidecar" here; media preparation uses an external runtime dependency rather than a companion service.
- "mode" could mean low-level backend, model, or provider. Resolved: **Transcription Mode** is the user-facing processing path, with `standard` and `diarized` in V1.
- "settings" could mean daemon defaults, upload form choices, or low-level config. Resolved: use **Transcription Defaults** for daemon-persisted defaults and **Per-Job Settings** for captured job overrides.
- "model selector" could mean arbitrary local paths or known downloadable models. Resolved: V1 standard mode uses **Managed Whisper Models**.
- "Whisper models" could mean every upstream whisper.cpp model. Resolved: V1 uses an English-first **Standard Model Catalog**.
- "model" could mean a whisper.cpp file or the advanced diarization model name. Resolved: use **Managed Whisper Model** for `standard` and **Diarized Model** for `diarized`.
- "unavailable mode" could mean hidden UI, late failure, or automatic fallback. Resolved: expose **Mode Availability** and block submission for unavailable modes.
- "state" could mean daemon recording state, upload form state, or queue state. Resolved: use **Job Status** for **Transcription Job** lifecycle.
- "cancel" could mean deleting the job, stopping only queued work, or preserving partial output. Resolved: V1 keeps the job record, best-effort stops active work, and does not preserve partial transcripts.
- "retention" could mean transcript history, source media, or derived media. Resolved: V1 keeps **Source Media** for 7 days by default and deletes **Working Audio** at terminal status.
- "notify" could mean client-side CLI notification or daemon-owned job notification. Resolved: V1 completion notifications are sent by the daemon.
- "output" could mean hotkey insertion, clipboard copy, persisted result, or desktop notification. Resolved: file-upload **Transcription Jobs** persist and notify, while hotkey recordings keep existing insertion behavior.
- "copy transcript" could include metadata or only text. Resolved: primary copy action copies **Transcript Result** text only.
- "file transcription API" could mean old synchronous endpoints or new async job endpoints. Resolved: all file submissions use the **Transcription Job** processor internally, with compatibility endpoints allowed to wait.
- "`kloyce-ctl transcribe-file`" could mean old blocking CLI behavior or new async submission. Resolved: default remains synchronous; async behavior is opt-in.
