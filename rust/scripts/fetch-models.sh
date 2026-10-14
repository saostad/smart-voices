#!/usr/bin/env bash
# Download and extract the Kokoro TTS model used by smart-voices.
set -euo pipefail

MODEL_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-int8-multi-lang-v1_0.tar.bz2"
MODEL_DIR="kokoro-int8-multi-lang-v1_0"

default_models_dir() {
    case "$(uname -s)" in
        Darwin) printf '%s\n' "$HOME/Library/Application Support/smart-voices/models" ;;
        *) printf '%s\n' "$HOME/.local/share/smart-voices/models" ;;
    esac
}

MODELS_ROOT="${1:-$(default_models_dir)}"
mkdir -p "$MODELS_ROOT"

if [ -d "$MODELS_ROOT/$MODEL_DIR" ]; then
    echo "Model already present at $MODELS_ROOT/$MODEL_DIR"
    echo "$MODELS_ROOT/$MODEL_DIR"
    exit 0
fi

TMP_FILE="$(mktemp "$MODELS_ROOT/.kokoro-download.XXXXXX.tar.bz2")"
trap 'rm -f "$TMP_FILE"' EXIT

curl -fL -o "$TMP_FILE" "$MODEL_URL"
tar -xjf "$TMP_FILE" -C "$MODELS_ROOT"
rm -f "$TMP_FILE"
trap - EXIT

echo "$MODELS_ROOT/$MODEL_DIR"
