# Control Surfaces Context

Kloyce has several control surfaces over the daemon: the `kloyce-ctl` CLI, the embedded web dashboard, REST endpoints, Server-Sent Events, and the Tauri desktop app.

## Language

**Control Surface**:
A user-facing or integration-facing way to observe or command the daemon.
_Avoid_: backend, worker, transcription engine

**Dashboard**:
The embedded web UI served by the daemon at `http://localhost:9876`.
_Avoid_: separate frontend app, browser-owned queue

**REST API**:
The daemon HTTP interface for status, history, settings, models, and file job operations.
_Avoid_: IPC when the transport is HTTP, dashboard-only API

**SSE Stream**:
The Server-Sent Events connection that pushes daemon state, progress, GPU, transcription, and job updates to observers.
_Avoid_: polling loop, websocket

**CLI Client**:
The `kloyce-ctl` binary that sends commands over IPC and HTTP.
_Avoid_: daemon process, dashboard shell

**Desktop App**:
The Tauri app that provides tray status, global hotkeys, SSE state tracking, and a native dashboard window.
_Avoid_: daemon, web dashboard itself

## Relationships

- **Control Surfaces** coordinate with the daemon; they do not own transcription state.
- The **Dashboard** displays the daemon-owned **Transcription Queue** and live daemon state.
- The **REST API** exposes file job submission, inspection, cancellation, model catalog, settings, history, and status.
- The **SSE Stream** is the live update path for dashboard and app observers.
- The **CLI Client** sends push-to-talk commands through IPC and file-job commands through HTTP endpoints.
- `kloyce-ctl transcribe-file` remains synchronous by default while offering explicit queue and follow behavior.
- The **Desktop App** tracks daemon state rather than reimplementing recording or transcription.

## Example Dialogue

> **Dev:** "Can the dashboard keep its own queue and sync it later?"
> **Domain expert:** "No - the dashboard displays the daemon-owned **Transcription Queue**."

> **Dev:** "Should the Tauri app transcribe audio itself?"
> **Domain expert:** "No - the **Desktop App** controls and observes the daemon."

## Flagged Ambiguities

- "client" could mean CLI, dashboard JavaScript, external REST caller, or Tauri app. Name the **Control Surface** explicitly.
- "API" could mean IPC or HTTP. Use **REST API** for HTTP routes and IPC for newline-delimited daemon commands.
