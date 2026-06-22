# ASR Service

FastAPI microservice for speech-to-text using `nvidia/nemotron-3.5-asr-streaming-0.6b` (ONNX int4, CPU-only).

## Setup

### 1. System dependencies

```bash
sudo apt install ffmpeg python3 python3-pip python3-venv
```

### 2. Create venv and install packages

```bash
mkdir -p /home/mahdi/asr_service
cp asr-service/asr_service.py /home/mahdi/asr_service/
cp asr-service/requirements.txt /home/mahdi/asr_service/
cp asr-service/download_model.py /home/mahdi/asr_service/

cd /home/mahdi/asr_service
python3 -m venv venv
venv/bin/pip install -r requirements.txt
```

### 3. Download model weights

```bash
# ~1.5 GB download to /opt/asr_model
sudo mkdir -p /opt/asr_model
sudo chown mahdi:mahdi /opt/asr_model
cd /home/mahdi/asr_service
venv/bin/python download_model.py
```

### 4. Install and start systemd service

```bash
sudo cp asr-service/asr.service /etc/systemd/system/asr.service
sudo systemctl daemon-reload
sudo systemctl enable asr
sudo systemctl start asr
```

### 5. Verify

```bash
systemctl status asr
journalctl -u asr -f

# Health check
curl http://127.0.0.1:8765/health
# {"status":"ok","model_loaded":true}

# Transcribe a file
curl -X POST http://127.0.0.1:8765/transcribe \
  -F "audio=@/path/to/voice.ogg"
# {"text":"hello world","language":"en-US","duration_seconds":2.1}
```

## API

### `POST /transcribe`

| Field | Type | Description |
|-------|------|-------------|
| `audio` | file (multipart) | Any audio format supported by ffmpeg |

Response:
```json
{
  "text": "transcribed text",
  "language": "en-US",
  "duration_seconds": 3.5
}
```

Returns `503` while model is loading, `422` for invalid audio.

### `GET /health`

```json
{"status": "ok", "model_loaded": true}
```

## Logs

```bash
journalctl -u asr -f
journalctl -u asr -f | grep 'event='
```
