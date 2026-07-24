#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
DB_PATH="${KLOYCE_DB_PATH:-$HOME/.local/share/kloyce/kloyce.db}"
DB_BACKUP=""
BOOTSTRAP_DEPS=0

PACMAN_DEPS=(
    pipewire
    wl-clipboard
    wtype
    libnotify
    ffmpeg
    cuda
    cmake
    git
    curl
    sqlite
)

RUNTIME_COMMANDS=(
    cargo
    systemctl
    curl
    sqlite3
    ffmpeg
    ffprobe
    pw-record
    pw-play
    wl-copy
    wl-paste
    wtype
    notify-send
)

usage() {
    cat <<'EOF'
Usage: ./install/deploy.sh [--bootstrap-deps]

Deploys the Kloyce daemon, CLI, and embedded web dashboard.

Options:
  --bootstrap-deps  Install or repair Arch Linux system dependencies with sudo pacman.
  -h, --help        Show this help.
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

step() {
    printf '\n==> %s\n' "$*"
}

on_error() {
    local status=$?
    echo "error: deploy failed with status $status" >&2
    if [[ -n "$DB_BACKUP" ]]; then
        echo "database backup is available at: $DB_BACKUP" >&2
        echo "restore with: cp '$DB_BACKUP' '$DB_PATH'" >&2
    fi
    exit "$status"
}
trap on_error ERR

command_exists() {
    command -v "$1" >/dev/null 2>&1
}

parse_args() {
    for arg in "$@"; do
        case "$arg" in
            --bootstrap-deps)
                BOOTSTRAP_DEPS=1
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                usage >&2
                die "unknown argument: $arg"
                ;;
        esac
    done
}

bootstrap_deps() {
    step "Installing system dependencies"
    command_exists sudo || die "sudo is required for --bootstrap-deps"
    command_exists pacman || die "--bootstrap-deps currently supports Arch Linux via pacman"
    sudo pacman -S --needed --noconfirm "${PACMAN_DEPS[@]}"
}

verify_runtime_deps() {
    step "Verifying runtime dependencies"
    local missing=()

    for cmd in "${RUNTIME_COMMANDS[@]}"; do
        if ! command_exists "$cmd"; then
            missing+=("$cmd")
        fi
    done

    if ! command_exists whisper-cli && [[ ! -x "$HOME/.local/bin/whisper-cli" ]]; then
        missing+=("whisper-cli")
    fi

    if (( ${#missing[@]} > 0 )); then
        printf 'Missing dependencies:\n' >&2
        printf '  - %s\n' "${missing[@]}" >&2
        die "run ./install/deploy.sh --bootstrap-deps, or install the missing tools manually"
    fi
}

run_cargo_verification() {
    step "Running cargo check"
    cargo check -p kloyce -p kloyce-ctl

    step "Running cargo clippy"
    cargo clippy -p kloyce -p kloyce-ctl -- -D warnings

    step "Building daemon and CLI"
    cargo build -p kloyce -p kloyce-ctl
}

check_schema_safety() {
    step "Checking schema safety"
    local unsafe_patterns

    if unsafe_patterns="$(grep -RInEi 'DROP[[:space:]]+(TABLE|COLUMN)|ALTER[[:space:]]+TYPE|ALTER[[:space:]]+TABLE[^;]*(DROP|RENAME|ALTER[[:space:]]+COLUMN|TYPE)' kloyce/src 2>/dev/null)"; then
        echo "$unsafe_patterns" >&2
        die "potential destructive schema migration detected; stop and review before deploying"
    fi

    if [[ -f "$DB_PATH" ]]; then
        sqlite3 "$DB_PATH" ".schema transcriptions" >/tmp/kloyce-schema-transcriptions.sql
        sqlite3 "$DB_PATH" ".schema diarized_transcriptions" >/tmp/kloyce-schema-diarized-transcriptions.sql
        sqlite3 "$DB_PATH" ".schema transcription_jobs" >/tmp/kloyce-schema-transcription-jobs.sql
        echo "schema snapshots written to /tmp/kloyce-schema-*.sql"
    else
        echo "database does not exist yet; skipping live schema snapshot"
    fi
}

backup_database() {
    step "Backing up database"
    if [[ -f "$DB_PATH" ]]; then
        local timestamp
        timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
        DB_BACKUP="$DB_PATH.bak.$timestamp"
        cp "$DB_PATH" "$DB_PATH.bak"
        cp "$DB_PATH" "$DB_BACKUP"
        echo "database backup written to $DB_BACKUP"
    else
        echo "database does not exist yet; no backup needed"
    fi
}

install_binaries_and_service() {
    step "Installing kloyce daemon"
    cargo install --locked --offline --path kloyce

    step "Installing kloyce-ctl"
    cargo install --locked --offline --path kloyce-ctl

    step "Refreshing user service and keybindings"
    cargo run --release -p kloyce --bin kloyce -- install
}

restart_and_verify() {
    step "Restarting user service"
    systemctl --user daemon-reload
    systemctl --user enable --now kloyce
    systemctl --user restart kloyce

    step "Checking service status"
    systemctl --user status kloyce --no-pager

    step "Checking recent service logs"
    journalctl --user -u kloyce --no-pager -n 20

    step "Smoke testing HTTP dashboard API"
    for _ in {1..30}; do
        if curl -fsS http://127.0.0.1:9876/api/status >/tmp/kloyce-api-status.json 2>/dev/null; then
            curl -fsS http://127.0.0.1:9876/api/models/standard >/tmp/kloyce-api-models-standard.json
            echo "HTTP API smoke tests passed"
            return 0
        fi
        sleep 1
    done

    curl -fsS http://127.0.0.1:9876/api/status >/dev/null
}

main() {
    parse_args "$@"
    cd "$REPO_ROOT"

    if (( BOOTSTRAP_DEPS == 1 )); then
        bootstrap_deps
    fi

    verify_runtime_deps
    run_cargo_verification
    backup_database
    check_schema_safety
    install_binaries_and_service
    restart_and_verify

    step "Deploy complete"
}

main "$@"
