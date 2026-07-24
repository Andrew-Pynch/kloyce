#!/usr/bin/env bash
set -euo pipefail

echo "=== Kloyce macOS Setup ==="

# 1. Check for Homebrew
echo "[1/9] Checking Homebrew..."
if ! command -v brew &>/dev/null; then
    echo "Installing Homebrew..."
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    # Add brew to PATH for this session (Apple Silicon default location)
    eval "$(/opt/homebrew/bin/brew shellenv)"
fi
echo "Homebrew OK: $(brew --version | head -1)"

# 2. Install system dependencies
echo "[2/9] Installing system dependencies..."
brew install sox cmake ffmpeg

# 3. Build whisper.cpp with Metal (Apple Silicon GPU)
echo "[3/9] Building whisper.cpp with Metal acceleration..."
WHISPER_BUILD_DIR="/tmp/whisper-cpp-build"
rm -rf "$WHISPER_BUILD_DIR"
git clone https://github.com/ggml-org/whisper.cpp "$WHISPER_BUILD_DIR"
cd "$WHISPER_BUILD_DIR"
cmake -B build -DGGML_METAL=ON -DBUILD_SHARED_LIBS=OFF
cmake --build build -j"$(sysctl -n hw.ncpu)" --config Release

# 4. Install whisper-cli
echo "[4/9] Installing whisper-cli..."
mkdir -p ~/.local/bin
install -m 755 build/bin/whisper-cli ~/.local/bin/whisper-cli
echo "Installed whisper-cli to ~/.local/bin/whisper-cli"
cd - >/dev/null

# 4b. Optional: build llama.cpp with Metal and download a local LLM for offline cleanup
# Set KLOYCE_BUILD_LLAMA=1 to enable this step (skipped by default to keep setup fast).
if [ "${KLOYCE_BUILD_LLAMA:-0}" = "1" ]; then
    echo "[opt] Building llama.cpp with Metal (local-LLM cleanup support)..."
    LLAMA_BUILD_DIR="/tmp/llama-cpp-build"
    rm -rf "$LLAMA_BUILD_DIR"
    git clone https://github.com/ggml-org/llama.cpp "$LLAMA_BUILD_DIR"
    cd "$LLAMA_BUILD_DIR"
    cmake -B build -DGGML_METAL=ON -DBUILD_SHARED_LIBS=OFF
    cmake --build build -j"$(sysctl -n hw.ncpu)" --config Release --target llama-completion
    mkdir -p ~/.local/bin
    install -m 755 build/bin/llama-completion ~/.local/bin/llama-completion
    echo "Installed llama-completion to ~/.local/bin/llama-completion"
    cd - >/dev/null

    LLM_DIR="$HOME/.local/share/kloyce/models/llm"
    mkdir -p "$LLM_DIR"
    QWEN_GGUF="$LLM_DIR/qwen2.5-1.5b-instruct-q4_k_m.gguf"
    if [ ! -f "$QWEN_GGUF" ]; then
        echo "[opt] Downloading Qwen2.5-1.5B-Instruct Q4_K_M GGUF..."
        curl -L -o "$QWEN_GGUF" \
            "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/qwen2.5-1.5b-instruct-q4_k_m.gguf"
        echo "Downloaded LLM to $QWEN_GGUF"
    else
        echo "[opt] LLM already exists, skipping download"
    fi
    echo "[opt] Local-LLM cleanup support installed."
    echo "      Set cleanup_engine = \"local\" in ~/.config/kloyce/config.toml to enable."
else
    echo ""
    echo "  TIP: Local-LLM offline cleanup is available but opt-in."
    echo "       Re-run with KLOYCE_BUILD_LLAMA=1 to build llama.cpp + download Qwen2.5-1.5B."
    echo ""
fi

# 5. Download default standard models
echo "[5/9] Downloading default standard whisper models..."
MODEL_DIR="$HOME/.local/share/kloyce/models"
mkdir -p "$MODEL_DIR"
if [ ! -f "$MODEL_DIR/ggml-small.en.bin" ]; then
    echo "  Downloading small.en (fast fallback)..."
    curl -L -o "$MODEL_DIR/ggml-small.en.bin" \
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin"
    echo "  Downloaded model to $MODEL_DIR/ggml-small.en.bin"
else
    echo "  small.en already exists, skipping"
fi
if [ ! -f "$MODEL_DIR/ggml-large-v3-turbo.bin" ]; then
    echo "  Downloading large-v3-turbo (recommended default)..."
    curl -L -o "$MODEL_DIR/ggml-large-v3-turbo.bin" \
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin"
    echo "  Downloaded model to $MODEL_DIR/ggml-large-v3-turbo.bin"
else
    echo "  large-v3-turbo already exists, skipping"
fi

# 6. Create macOS config
echo "[6/9] Creating macOS config..."
CONFIG_DIR="$HOME/.config/kloyce"
mkdir -p "$CONFIG_DIR"
if [ ! -f "$CONFIG_DIR/config.toml" ]; then
    cat > "$CONFIG_DIR/config.toml" <<'TOML'
# Kloyce macOS configuration
sound_start = "/System/Library/Sounds/Ping.aiff"
sound_stop = "/System/Library/Sounds/Pop.aiff"
TOML
    echo "Created config at $CONFIG_DIR/config.toml"
else
    echo "Config already exists, skipping"
fi

# 7. Build and install kloyce daemon + CLI
echo "[7/9] Building and installing kloyce..."
SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$SCRIPT_DIR"
cargo install --path kloyce
cargo install --path kloyce-ctl

# Run install to create LaunchAgent
cargo run --release --bin kloyce -- install

# 8. Load daemon via LaunchAgent
echo "[8/9] Loading kloyce daemon..."
mkdir -p ~/Library/Logs/kloyce
PLIST="$HOME/Library/LaunchAgents/com.kloyce.daemon.plist"
if [ -f "$PLIST" ]; then
    # Unload first in case it's already loaded (ignore errors)
    launchctl unload "$PLIST" 2>/dev/null || true
    launchctl load "$PLIST"
    echo "Daemon loaded via LaunchAgent"
    # Wait for daemon to start
    sleep 2
    if curl -s http://127.0.0.1:9876/api/status >/dev/null 2>&1; then
        echo "Daemon is running and responding"
    else
        echo "Warning: Daemon loaded but not yet responding on port 9876"
        echo "  Check logs: cat ~/Library/Logs/kloyce/stderr.log"
    fi
else
    echo "Warning: LaunchAgent plist not found at $PLIST"
    echo "  Run 'kloyce install' to create it, then re-run this script"
fi

# 9. Build and install Tauri desktop app
echo "[9/9] Building Kloyce desktop app..."
if ! cargo tauri --version &>/dev/null; then
    echo "Installing tauri-cli..."
    cargo install tauri-cli
fi

cd "$SCRIPT_DIR"
cargo tauri build --target aarch64-apple-darwin

# Install .app to ~/Applications
APP_BUNDLE="target/aarch64-apple-darwin/release/bundle/macos/Kloyce.app"
if [ -d "$APP_BUNDLE" ]; then
    mkdir -p ~/Applications
    # Remove old version if present
    rm -rf ~/Applications/Kloyce.app
    cp -R "$APP_BUNDLE" ~/Applications/Kloyce.app
    echo "Installed Kloyce.app to ~/Applications/"
else
    echo "Warning: App bundle not found at $APP_BUNDLE"
    echo "  The Tauri build may have failed — check output above"
fi

echo ""
echo "=== Setup complete! ==="
echo ""
echo "Make sure ~/.local/bin is in your PATH:"
echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
echo ""
echo "What's running:"
echo "  - Kloyce daemon (LaunchAgent, auto-starts on login)"
echo "  - Dashboard: http://localhost:9876"
echo ""
echo "To launch the desktop app:"
echo "  open ~/Applications/Kloyce.app"
echo ""
echo "Permissions (macOS will prompt on first use):"
echo "  - Microphone access for your terminal app (required for recording)"
echo "  - Accessibility for context tracking (optional, degrades gracefully)"
