# Platform And Runtime Context

Kloyce runs as a local daemon with platform-specific adapters for audio, output, context tracking, GPU metrics, and install behavior. Runtime dependencies are external tools invoked by the daemon rather than services owned by Kloyce.

## Language

**Platform Adapter**:
A target-OS-specific implementation for audio, output, notifications, active-window context, GPU metrics, or install behavior.
_Avoid_: plugin, provider, sidecar

**Runtime Dependency**:
An external command or OS facility Kloyce invokes at runtime, such as `whisper-cli`, `ffmpeg`, `ffprobe`, `pw-record`, `wl-copy`, `wtype`, `notify-send`, `tmux`, or `nvidia-smi`.
_Avoid_: bundled service, internal module

**Daemon Storage**:
The local filesystem locations Kloyce owns for config, models, SQLite data, source media, and working media.
_Avoid_: browser storage, upload cache

**SQLite Store**:
The local SQLite database used for transcription history and durable file job records.
_Avoid_: remote database, dashboard state

**Service Install**:
The OS-specific auto-start configuration for the daemon.
_Avoid_: build, model download, dashboard launch

## Relationships

- **Platform Adapters** are selected at compile time with `#[cfg(target_os)]` and expose matching public APIs.
- Linux uses PipeWire, Wayland clipboard/typing tools, Hyprland context tracking, `notify-send`, and optional NVIDIA GPU metrics.
- macOS uses `rec`/sox recording, `afplay`, `pbcopy`, `osascript`, LaunchAgent install, and no implemented Metal metrics.
- Windows uses `ffmpeg` DirectShow recording, PowerShell clipboard/notifications/audio helpers, TCP IPC, Registry auto-start, and optional NVIDIA GPU metrics.
- **Runtime Dependencies** should be checked or surfaced clearly rather than silently replaced with unrelated behavior.
- **Daemon Storage** includes config under `~/.config/kloyce`, data under `~/.local/share/kloyce`, managed model files, source media, and working audio.
- **SQLite Store** migrations must preserve existing user data; destructive schema changes require explicit approval.
- **Service Install** changes must account for the daemon running as a user service or equivalent OS startup process.

## Example Dialogue

> **Dev:** "Should FFmpeg be described as a sidecar?"
> **Domain expert:** "No - FFmpeg is a **Runtime Dependency** used for media preparation and capture paths."

> **Dev:** "Can the dashboard own uploaded media paths?"
> **Domain expert:** "No - uploaded files become **Daemon Storage** as Source Media."

## Flagged Ambiguities

- "install" could mean compiling binaries, downloading models, or configuring auto-start. Use **Service Install** only for daemon startup registration.
- "storage" could mean SQLite records, source media, working audio, or config files. Name the storage kind explicitly.
