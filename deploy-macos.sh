#!/usr/bin/env bash
set -euo pipefail

# Kloyce macOS deploy script
# Usage:
#   ./deploy-macos.sh              # full deploy (daemon + app)
#   ./deploy-macos.sh --daemon-only  # fast path: daemon only
#   ./deploy-macos.sh --app-only     # tray/UI changes only
#   ./deploy-macos.sh --debug        # debug builds (faster compile)

DAEMON_ONLY=false
APP_ONLY=false
BUILD_PROFILE="release"
PROFILE_FLAG="--release"
LAUNCHCTL_LABEL="com.kloyce.daemon"
LAUNCHCTL_DOMAIN="gui/$(id -u)"
APP_NAME="Kloyce"
APP_DEST="$HOME/Applications/${APP_NAME}.app"
LOG_DIR="$HOME/Library/Logs/kloyce"
HEALTH_URL="http://127.0.0.1:9876/api/status"
HEALTH_RETRIES=5

for arg in "$@"; do
    case "$arg" in
        --daemon-only) DAEMON_ONLY=true ;;
        --app-only)    APP_ONLY=true ;;
        --debug)       BUILD_PROFILE="debug"; PROFILE_FLAG="" ;;
        *)             echo "Unknown flag: $arg"; exit 1 ;;
    esac
done

green()  { printf "\033[32m%s\033[0m\n" "$1"; }
red()    { printf "\033[31m%s\033[0m\n" "$1"; }
yellow() { printf "\033[33m%s\033[0m\n" "$1"; }
step()   { printf "\n\033[1;36m[%s] %s\033[0m\n" "$1" "$2"; }

# ── Daemon deploy ──────────────────────────────────────────────

deploy_daemon() {
    step "1/4" "Building daemon + CLI ($BUILD_PROFILE)"
    cargo build $PROFILE_FLAG -p kloyce -p kloyce-ctl

    step "2/4" "Installing binaries"
    cargo install --path kloyce --force $PROFILE_FLAG 2>&1 | tail -1
    cargo install --path kloyce-ctl --force $PROFILE_FLAG 2>&1 | tail -1

    step "3/4" "Restarting LaunchAgent"
    mkdir -p "$LOG_DIR"
    if launchctl print "$LAUNCHCTL_DOMAIN/$LAUNCHCTL_LABEL" &>/dev/null; then
        launchctl kickstart -k "$LAUNCHCTL_DOMAIN/$LAUNCHCTL_LABEL"
        green "Daemon restarted via kickstart"
    else
        yellow "LaunchAgent not loaded — bootstrapping"
        launchctl bootstrap "$LAUNCHCTL_DOMAIN" \
            "$HOME/Library/LaunchAgents/${LAUNCHCTL_LABEL}.plist" 2>/dev/null || true
        launchctl kickstart "$LAUNCHCTL_DOMAIN/$LAUNCHCTL_LABEL" 2>/dev/null || true
    fi

    step "4/4" "Health check"
    for i in $(seq 1 $HEALTH_RETRIES); do
        if curl -sf "$HEALTH_URL" > /dev/null 2>&1; then
            green "Daemon healthy (attempt $i/$HEALTH_RETRIES)"
            return 0
        fi
        sleep 1
    done
    red "Daemon failed health check after $HEALTH_RETRIES attempts"
    echo "Recent logs:"
    tail -20 "$LOG_DIR/stderr.log" 2>/dev/null || echo "(no log file found)"
    return 1
}

# ── App deploy ─────────────────────────────────────────────────

deploy_app() {
    local total=4
    [ -n "${APPLE_SIGNING_IDENTITY:-}" ] && total=5
    local n=1

    step "$n/$total" "Building Tauri app ($BUILD_PROFILE)"
    if [ "$BUILD_PROFILE" = "debug" ]; then
        cargo tauri build --target aarch64-apple-darwin --debug
    else
        cargo tauri build --target aarch64-apple-darwin
    fi

    local app_src
    if [ "$BUILD_PROFILE" = "debug" ]; then
        app_src="target/aarch64-apple-darwin/debug/bundle/macos/${APP_NAME}.app"
    else
        app_src="target/aarch64-apple-darwin/release/bundle/macos/${APP_NAME}.app"
    fi

    if [ ! -d "$app_src" ]; then
        red "Build artifact not found: $app_src"
        return 1
    fi

    if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
        n=$((n + 1))
        step "$n/$total" "Verifying code signature + notarization"
        codesign -dvv "$app_src"
        if xcrun stapler validate "$app_src" 2>/dev/null; then
            green "Notarization ticket stapled"
        else
            yellow "No notarization ticket (expected for local dev builds)"
        fi
        green "Signed with: ${APPLE_SIGNING_IDENTITY}"
    fi

    n=$((n + 1))
    step "$n/$total" "Quitting running ${APP_NAME}.app"
    osascript -e "tell application \"$APP_NAME\" to quit" 2>/dev/null || true
    sleep 1
    pkill -f "${APP_NAME}.app" 2>/dev/null || true

    n=$((n + 1))
    step "$n/$total" "Replacing app bundle"
    mkdir -p "$HOME/Applications"
    rm -rf "$APP_DEST"
    cp -R "$app_src" "$APP_DEST"
    green "Installed to $APP_DEST"

    n=$((n + 1))
    step "$n/$total" "Launching ${APP_NAME}.app"
    open "$APP_DEST"
    green "${APP_NAME}.app launched"
}

# ── Main ───────────────────────────────────────────────────────

echo "━━━ Kloyce macOS Deploy ━━━"
echo "  Profile: $BUILD_PROFILE"
echo "  Mode:    $(if $DAEMON_ONLY; then echo daemon-only; elif $APP_ONLY; then echo app-only; else echo full; fi)"

if $APP_ONLY; then
    deploy_app
elif $DAEMON_ONLY; then
    deploy_daemon
else
    deploy_daemon
    deploy_app
fi

echo ""
green "━━━ Deploy complete ━━━"
echo "  Dashboard: http://localhost:9876"
echo "  Daemon:    launchctl print $LAUNCHCTL_DOMAIN/$LAUNCHCTL_LABEL"
