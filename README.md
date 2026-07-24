# Kloyce

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![crates.io](https://img.shields.io/crates/v/kloyce.svg)](https://crates.io/crates/kloyce)

Push-to-talk speech-to-text. Press a key, speak, release, and your words appear at the cursor. Primary target is Hyprland on Arch Linux with GPU-accelerated transcription via whisper.cpp. Also supports macOS (sox, Metal) and Windows (ffmpeg, DirectShow).

## How It Works

Kloyce runs as a background daemon managed by systemd. When you press SUPER+R, the daemon starts recording audio through PipeWire. When you release the key, it stops recording and sends the audio to whisper.cpp for transcription (using your NVIDIA GPU via CUDA). The transcribed text is then either copied to your clipboard and pasted via `wtype`, or — if you were focused on a tmux pane — sent directly into that pane with `tmux send-keys`. A desktop notification confirms the result.

While all this happens, Kloyce tracks which window you were in when you started speaking. It uses this context to apply domain-specific word corrections (e.g., fixing "hyper land" to "Hyprland" when you're working in your dotfiles). Optionally, it can call Claude to clean up transcription artifacts and learn new corrections over time.

Everything is observable through a live web dashboard at `http://localhost:9876` that shows recording state, transcription progress, GPU metrics, file upload jobs, and your full transcription history.

## Install

### From crates.io

```bash
cargo install kloyce kloyce-ctl
```

### From GitHub Releases (pre-built binary)

```bash
curl -sSL https://github.com/andrew-pynch/kloyce/releases/latest/download/kloyce-installer.sh | sh
```

### Arch Linux (AUR)

```bash
yay -S kloyce-bin    # pre-built binary
yay -S kloyce-git    # build from source
```

### From source (Linux)

```bash
git clone https://github.com/andrew-pynch/kloyce.git && cd kloyce
./install/setup.sh
```

The setup script installs its managed system dependencies, builds whisper.cpp with CUDA, downloads the default `small.en` standard model, installs binaries, and configures systemd. Non-default standard models are downloaded on demand from the dashboard/API. After install, the daemon starts automatically on login.

### From source (macOS)

```bash
git clone https://github.com/andrew-pynch/kloyce.git && cd kloyce
./install/setup-macos.sh
```

Installs dependencies via Homebrew, builds whisper.cpp with Metal acceleration, downloads the default `small.en` standard model, and creates a LaunchAgent for auto-start. Non-default standard models are downloaded on demand from the dashboard/API.

## System Requirements

### Linux (Arch / Wayland)

- **PipeWire** -- `pipewire`, `pipewire-pulse` (provides `pw-record`, `pw-play`)
- **Wayland clipboard** -- `wl-clipboard` (provides `wl-copy`, `wl-paste`)
- **Wayland typing** -- `wtype` (synthetic keyboard input)
- **Notifications** -- `libnotify` (provides `notify-send`)
- **FFmpeg/ffprobe** -- `ffmpeg`, `ffprobe` for media validation and audio extraction in uploaded/path-based file jobs
- **whisper.cpp** -- with CUDA support for GPU-accelerated transcription
- **NVIDIA CUDA** -- `cuda` (for GPU-accelerated whisper.cpp)
- **Hyprland** -- for keybinding and window context tracking (`hyprctl`)

### macOS

- **sox** -- `rec` command for audio recording
- **FFmpeg/ffprobe** -- `ffmpeg`, `ffprobe` for media validation and audio extraction in uploaded/path-based file jobs
- **whisper.cpp** -- built with Metal acceleration
- **cmake** -- for building whisper.cpp

### Windows (experimental)

- **FFmpeg/ffprobe** -- `ffmpeg`, `ffprobe` for DirectShow recording plus media validation and audio extraction in file jobs
- **whisper-cli** -- transcription engine
- **PowerShell** -- clipboard, notifications, sound playback (built-in)
- **NVIDIA GPU** -- optional, for GPU-accelerated transcription

### All platforms (optional)

- **tmux** -- for direct pane injection of transcriptions
- **Claude CLI** -- for post-processing cleanup and dictionary learning

> **Note:** `cargo install` installs the Rust binaries only. You still need whisper-cli and the system dependencies above. The setup scripts (`setup.sh` for Linux, `setup-macos.sh` for macOS) handle the supported source-install path, including building whisper-cli and installing the dependencies they manage.

### Manual Build

```bash
cargo build                          # development build
cargo install --path kloyce          # install daemon
cargo install --path kloyce-ctl      # install client
```

## Usage

```bash
# Control via CLI
kloyce-ctl toggle          # start/stop recording
kloyce-ctl toggle-enter    # same, but auto-press Enter in tmux after transcription
kloyce-ctl status          # query daemon state
kloyce-ctl cancel          # cancel active recording

# Transcribe an existing audio or video file, waiting for the job by default
kloyce-ctl transcribe-file /path/to/media.mp4 --tag screen-recorder --tag firefox

# Queue and return immediately
kloyce-ctl transcribe-file /path/to/media.mp4 --queue

# Follow status/progress until terminal, then print the transcript
kloyce-ctl transcribe-file /path/to/media.mp4 --follow --mode standard --model small.en

# Submit a diarized job when advanced transcription is available
kloyce-ctl transcribe-file /path/to/meeting.wav --mode diarized --diarize --min-speakers 2

# Run daemon manually (for development)
RUST_LOG=kloyce=debug cargo run --bin kloyce -- daemon

# Install systemd service + keybinding
cargo run --bin kloyce -- install
```

The typical push-to-talk flow is still: press SUPER+R, speak, press SUPER+R again. Your transcription appears at the cursor (or in your focused tmux pane). Push-to-talk recordings stay outside the file Transcription Queue, so they do not wait behind long uploaded media jobs.

## Architecture

Cargo workspace with three crates:

- **`kloyce/`** — The daemon: audio capture, transcription, output, web dashboard, persistence
- **`kloyce-ctl/`** — Lightweight CLI client that sends commands over IPC / HTTP
- **`kloyce-app/`** — Tauri desktop app: system tray icon, global hotkeys, native dashboard window

### Daemon Modules (`kloyce/src/`)

| Module | What it does |
|--------|-------------|
| `daemon.rs` | State machine: Idle → Recording → Transcribing → Idle |
| `ipc.rs` | Unix socket (`$XDG_RUNTIME_DIR/kloyce.sock`) or TCP `127.0.0.1:19876` (Windows), newline-delimited JSON |
| `transcribe.rs` | Runs `whisper-cli`, GPU-first with CPU fallback, streams progress via SSE |
| `text.rs` | Pure text processing: removes whisper artifacts (`[BLANK_AUDIO]`, `[Music]`, etc.) |
| `config.rs` | TOML config with sensible defaults — daemon starts fine with no config file |
| `web.rs` | Axum HTTP server: dashboard, REST API, Server-Sent Events |
| `db.rs` | SQLite persistence for transcription history (WAL mode) |
| `dictionary.rs` | Context-aware word corrections with file watching for live reloads |
| `cleanup.rs` | Optional Claude-powered transcription cleanup |
| `learning.rs` | Optional Claude-powered dictionary learning from transcripts |
| `platform/` | Platform-specific audio, clipboard, notifications, context, GPU, install (see below) |

### Platform Support

Platform-specific code lives in `kloyce/src/platform/` behind `#[cfg(target_os)]` compile-time selection. Each platform exports identical public APIs.

| Capability | Linux | macOS | Windows |
|-----------|-------|-------|---------|
| Audio capture | `pw-record` (PipeWire) | `rec` (sox) | `ffmpeg` (DirectShow) |
| Audio playback | `pw-play` | `afplay` | PowerShell `SoundPlayer` / `ffplay` |
| Clipboard | `wl-copy` | `pbcopy` | PowerShell `Set-Clipboard` |
| Notifications | `notify-send` | `osascript` | WinRT toast notifications |
| Window context | `hyprctl` + `/proc/` | `osascript` | Win32 `GetForegroundWindow` |
| GPU monitoring | `nvidia-smi` | No-op | `nvidia-smi` |
| Install | systemd + Hyprland binding | LaunchAgent plist | Registry auto-start |

### IPC Protocol

Newline-delimited JSON over Unix socket (Linux/macOS) or TCP `127.0.0.1:19876` (Windows):

```
→ {"command":"toggle"}
← {"status":"ok","state":"recording","message":"Recording started"}
```

Commands: `toggle`, `toggle_enter`, `status`, `cancel`

### State Machine

```
     toggle          toggle           done
Idle ──────► Recording ──────► Transcribing ──────► Idle
                 │                    │
                 │ cancel             │ toggle
                 ▼                    ▼
               Idle              "already in progress"
```

## Web Dashboard

Available at `http://localhost:9876` while the daemon is running.

Shows real-time state (idle/recording/transcribing), recording duration with animated waveform, transcription progress bar, GPU utilization sparkline, VRAM/temperature/power/fan gauges, drag/drop file upload, the daemon-owned Transcription Queue, and a scrolling log of all transcriptions with timestamps, word counts, and context tags.

State, progress, transcription, and job events stream live via Server-Sent Events. Queue data is also available through the job REST endpoints.

**API endpoints:**

| Route | Description |
|-------|-------------|
| `GET /` | Dashboard HTML |
| `GET /api/status` | Current state, total transcriptions, word count, uptime |
| `GET /api/history` | Recent transcription entries as JSON |
| `GET /api/events` | SSE stream (state changes, progress, transcriptions, GPU metrics) |
| `GET /api/gpu` | Latest GPU metrics |
| `GET /api/transcription/settings` | Current transcription defaults and mode availability |
| `POST /api/transcription/settings` | Update default mode/model settings in `config.toml` |
| `GET /api/transcription/modes` | Availability for `standard` and `diarized` modes |
| `GET /api/models/standard` | Managed standard Whisper model catalog and install status |
| `POST /api/models/standard/{model_id}/download` | Download a managed standard model on demand |
| `GET /api/jobs` | Active, queued, and recent terminal Transcription Jobs |
| `POST /api/jobs/upload` | Upload source media bytes and enqueue a Transcription Job |
| `POST /api/jobs/from-path` | Copy a daemon-local source media path and enqueue a Transcription Job |
| `GET /api/jobs/{id}` | Inspect a Transcription Job |
| `POST /api/jobs/{id}/cancel` | Cancel a queued/running Transcription Job |
| `POST /api/transcribe` | Compatibility sync wrapper for a standard path-based job |
| `POST /api/transcribe-advanced` | Compatibility sync wrapper for a diarized path-based job |

### File Transcription API

External tools can submit audio or video files via durable Transcription Jobs. Kloyce stores source media under daemon-owned storage, validates an audio stream with `ffprobe`, prepares disposable 16 kHz mono working audio with `ffmpeg`, and processes one file job at a time through a FIFO queue. Working audio is deleted at terminal status; source media is retained for 7 days by default.

```bash
# Via CLI: submits /api/jobs/from-path, waits by default, then prints transcript text
kloyce-ctl transcribe-file /path/to/media.mp4 --tag screen-recorder

# Queue without waiting
kloyce-ctl transcribe-file /path/to/media.mp4 --queue
# Prints: job_id=123 status=queued

# Follow status/progress on stderr, then print transcript text on stdout
kloyce-ctl transcribe-file /path/to/media.mp4 --follow

# Submit a path-based async job
curl -X POST http://127.0.0.1:9876/api/jobs/from-path \
  -H 'Content-Type: application/json' \
  -d '{"file_path": "/absolute/path/to/media.mp4", "mode": "standard", "model": "small.en", "context_tags": ["screen-recorder"]}'

# Upload bytes from a browser or local client; context_tags may be JSON or comma-separated
curl -X POST http://127.0.0.1:9876/api/jobs/upload \
  -F file=@/absolute/path/to/media.mp4 \
  -F mode=standard \
  -F model=small.en \
  -F context_tags='["screen-recorder"]'

# Poll or cancel
curl http://127.0.0.1:9876/api/jobs/123
curl -X POST http://127.0.0.1:9876/api/jobs/123/cancel
```

- **Supported media** -- audio or video files with an audio stream, including MP4, MP3, and WAV.
- **Job statuses** -- `queued`, `preparing_media`, `downloading_model`, `transcribing`, `succeeded`, `failed`, `cancelled`.
- **Standard mode** -- uses whisper.cpp and managed English models: `tiny.en`, `base.en`, `small.en`, `medium.en`.
- **Diarized mode** -- uses the advanced transcription backend; it is visible but unavailable until `advanced_transcription.enabled = true` and the configured Python venv exists.
- **Compatibility APIs** -- `POST /api/transcribe` and `POST /api/transcribe-advanced` still block and return the old response shape, but internally create a Transcription Job and wait for it.

CLI flags for `transcribe-file`:

| Flag | Behavior |
|------|----------|
| `--queue` | Submit and exit after printing job id/status |
| `--follow` | Print status/progress to stderr until terminal, then print transcript |
| `--mode standard\|diarized` | Override the default Transcription Mode |
| `--model <id>` | Override the model for the selected mode |
| `--diarize` / `--no-diarize` | Enable or disable speaker diarization for diarized jobs |
| `--min-speakers <n>` / `--max-speakers <n>` | Speaker-count hints for diarized jobs |

`transcribe-file-advanced` remains as a compatibility wrapper for diarized jobs and supports `--queue`, `--follow`, `--model`, `--no-diarize`, `--min-speakers`, and `--max-speakers`.

### Managed Standard Models

Setup downloads only the default `small.en` standard model to `~/.local/share/kloyce/models/ggml-small.en.bin`. The managed standard catalog also includes `tiny.en`, `base.en`, and `medium.en`; those files are not installed up front. Use the dashboard settings UI or `POST /api/models/standard/{model_id}/download` to download them on demand before selecting them for new jobs. Existing queued/running jobs keep the mode/model settings captured when they were submitted.

## Configuration

Config lives at `~/.config/kloyce/config.toml`. All fields are optional — defaults work out of the box.

```toml
whisper_bin = "~/.local/bin/whisper-cli"
ffmpeg_bin = "ffmpeg"
ffprobe_bin = "ffprobe"
model_path = "~/.local/share/kloyce/models/ggml-small.en.bin"
web_port = 9876
history_size = 100

# Audio feedback
sound_start = "/usr/share/sounds/freedesktop/stereo/message-new-instant.oga"
sound_stop = "/usr/share/sounds/freedesktop/stereo/complete.oga"

# Output behavior
tmux_send_keys = true              # send text directly to focused tmux pane
tmux_auto_enter = false            # auto-press Enter after transcription in tmux

# File jobs and defaults
source_media_retention_days = 7

[transcription_defaults]
default_mode = "standard"
default_standard_model = "small.en"
default_diarized_model = "small"

# GPU monitoring
gpu_poll_interval_ms = 2000

# Claude integration (optional)
claude_cleanup = false             # post-process transcriptions with Claude
claude_bin = "~/.local/bin/claude"
claude_timeout_secs = 30

# Dictionary
dictionary_path = "~/.config/kloyce/dictionary.toml"
dictionary_learning = true         # auto-learn corrections via Claude
dictionary_max_entries = 500

# Context tracking
context_poll_interval_ms = 1000
```

### Correction Dictionary

Kloyce maintains a correction dictionary at `~/.config/kloyce/dictionary.toml` that fixes common Whisper misrecognitions. Corrections can be global or scoped to a context (determined by which window/project you're in when you speak):

```toml
[global]
"cloyce" = "Kloyce"
"hyper land" = "Hyprland"

[context."work/myproject"]
"tf" = "Terraform"
```

The dictionary also feeds vocabulary hints to Whisper, biasing recognition toward your known terms. If `dictionary_learning` is enabled and Claude CLI is available, new corrections are learned automatically from each transcription.

## Key Paths

| Path | Purpose |
|------|---------|
| `~/.config/kloyce/config.toml` | Configuration |
| `~/.config/kloyce/dictionary.toml` | Correction dictionary |
| `~/.local/share/kloyce/kloyce.db` | SQLite transcription history |
| `~/.local/share/kloyce/models/` | Whisper GGML models |
| `~/.local/share/kloyce/media/source/` | Daemon-owned source media for file jobs |
| `~/.local/share/kloyce/media/working/` | Disposable working audio for active file jobs |
| `$XDG_RUNTIME_DIR/kloyce.sock` | IPC socket (Linux/macOS) |
| TCP `127.0.0.1:19876` | IPC socket (Windows) |
| `~/.config/systemd/user/kloyce.service` | Systemd service (Linux) |
| `~/Library/LaunchAgents/com.kloyce.daemon.plist` | LaunchAgent (macOS) |

## Contributing

Contributions welcome! Open an issue or PR on [GitHub](https://github.com/andrew-pynch/kloyce).

## License

MIT -- see [LICENSE](LICENSE).
