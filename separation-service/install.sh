#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "[install] Creating Python venv..."
python3 -m venv venv

echo "[install] Installing requirements..."
venv/bin/pip install --upgrade pip
venv/bin/pip install -r requirements.txt

echo "[install] Pre-downloading Kim_Vocal_2.onnx model..."
mkdir -p models
venv/bin/python3 -c "
from audio_separator.separator import Separator
import os
s = Separator(model_file_dir='$(pwd)/models', log_level=10)
s.load_model('Kim_Vocal_2.onnx')
print('[install] Model downloaded successfully.')
"

echo "[install] Done."
