#!/usr/bin/env bash
set -euo pipefail

echo "=== Kloyce Setup ==="

# Install system dependencies
echo "[1/5] Installing system dependencies..."
sudo pacman -S --needed --noconfirm wtype cuda cmake ffmpeg

# Build whisper.cpp with CUDA
echo "[2/5] Building whisper.cpp with CUDA..."
export PATH="/opt/cuda/bin:$PATH"
WHISPER_BUILD_DIR="/tmp/whisper-cpp-build"
rm -rf "$WHISPER_BUILD_DIR"
git clone https://github.com/ggml-org/whisper.cpp "$WHISPER_BUILD_DIR"
cd "$WHISPER_BUILD_DIR"
cmake -B build -DGGML_CUDA=1 -DCMAKE_CUDA_ARCHITECTURES="89-real" -DCUDAToolkit_ROOT=/opt/cuda -DBUILD_SHARED_LIBS=OFF
cmake --build build -j"$(nproc)" --config Release
install -Dm755 build/bin/whisper-cli ~/.local/bin/whisper-cli
echo "Installed whisper-cli to ~/.local/bin/whisper-cli"
cd -

# Optional: build llama.cpp with CUDA and download a local LLM for offline cleanup
# Set KLOYCE_BUILD_LLAMA=1 to enable this step (skipped by default to keep setup fast).
if [ "${KLOYCE_BUILD_LLAMA:-0}" = "1" ]; then
    echo "[opt] Building llama.cpp with CUDA (local-LLM cleanup support)..."
    LLAMA_BUILD_DIR="/tmp/llama-cpp-build"
    rm -rf "$LLAMA_BUILD_DIR"
    git clone https://github.com/ggml-org/llama.cpp "$LLAMA_BUILD_DIR"
    cd "$LLAMA_BUILD_DIR"
    cmake -B build -DGGML_CUDA=1 -DCMAKE_CUDA_ARCHITECTURES="89-real" -DCUDAToolkit_ROOT=/opt/cuda -DBUILD_SHARED_LIBS=OFF
    cmake --build build -j"$(nproc)" --config Release --target llama-completion
    install -Dm755 build/bin/llama-completion ~/.local/bin/llama-completion
    echo "Installed llama-completion to ~/.local/bin/llama-completion"
    cd -

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

# Download default standard models
echo "[3/5] Downloading default standard whisper models..."
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

# Build and install kloyce
echo "[4/5] Building kloyce..."
SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$SCRIPT_DIR"
cargo install --path kloyce
cargo install --path kloyce-ctl

# Install service and config
echo "[5/5] Installing service and config..."
cargo run --release --bin kloyce -- install

echo ""
echo "=== Setup complete! ==="
echo ""
echo "Next steps:"
echo "  1. Reload systemd:    systemctl --user daemon-reload"
echo "  2. Start the service: systemctl --user enable --now kloyce"
echo "  3. Reload hyprland:   hyprctl reload"
echo "  4. Press SUPER+R to start voice input!"
echo ""
echo "Dashboard: http://localhost:9876"
