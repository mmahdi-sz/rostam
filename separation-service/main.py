import asyncio
import base64
import io
import logging
import os
import sys
import tempfile
import time
from contextlib import asynccontextmanager

import uvicorn
from fastapi import FastAPI, File, Form, HTTPException, Request, UploadFile
from fastapi.responses import JSONResponse

os.environ.setdefault("OMP_NUM_THREADS", "16")

logging.basicConfig(
    level=logging.INFO,
    format="[separation %(levelname)s] %(message)s",
    stream=sys.stderr,
)
log = logging.getLogger("separation")

MODEL_DIR = os.path.join(os.path.dirname(__file__), "models")
MODEL_NAME = "Kim_Vocal_2.onnx"
MAX_FILE_BYTES = 50 * 1024 * 1024  # 50MB

_separator_quality = None
_separator_fast = None
_model_loaded = False
_request_lock = asyncio.Lock()
_trace_counter = 0


def next_trace_id() -> int:
    global _trace_counter
    _trace_counter += 1
    return _trace_counter


def load_models():
    global _separator_quality, _separator_fast, _model_loaded
    try:
        from audio_separator.separator import Separator

        log.info(f"[separation event=model_load_start] model={MODEL_NAME} model_dir={MODEL_DIR}")
        os.makedirs(MODEL_DIR, exist_ok=True)

        _separator_quality = Separator(
            model_file_dir=MODEL_DIR,
            output_format="WAV",
            log_level=logging.WARNING,
        )
        _separator_quality.load_model(MODEL_NAME)

        _separator_fast = Separator(
            model_file_dir=MODEL_DIR,
            output_format="WAV",
            log_level=logging.WARNING,
        )
        _separator_fast.load_model(MODEL_NAME)

        _model_loaded = True
        log.info(f"[separation event=model_load_done] model={MODEL_NAME}")
    except Exception as e:
        log.error(f"[separation event=model_load_failed] err={e}")
        _model_loaded = False


@asynccontextmanager
async def lifespan(app: FastAPI):
    log.info("[separation event=startup] pre-loading model in background thread")
    loop = asyncio.get_event_loop()
    await loop.run_in_executor(None, load_models)
    log.info(f"[separation event=startup_done] model_loaded={_model_loaded}")
    yield
    log.info("[separation event=shutdown]")


app = FastAPI(lifespan=lifespan)


@app.get("/health")
async def health():
    return {"status": "ok", "model_loaded": _model_loaded}


@app.post("/separate")
async def separate(file: UploadFile = File(...), mode: str = Form("quality")):
    trace_id = next_trace_id()

    if mode not in ("quality", "fast"):
        raise HTTPException(status_code=400, detail=f"Invalid mode: {mode}. Use 'quality' or 'fast'.")

    content = await file.read()
    file_size = len(content)
    log.info(f"[separation trace={trace_id} event=request_start] mode={mode} file_size={file_size} filename={file.filename}")

    if file_size > MAX_FILE_BYTES:
        log.warning(f"[separation trace={trace_id} event=file_too_large] size={file_size}")
        raise HTTPException(status_code=400, detail=f"File too large: {file_size} bytes (max {MAX_FILE_BYTES})")

    if not _model_loaded:
        log.error(f"[separation trace={trace_id} event=model_not_loaded]")
        raise HTTPException(status_code=503, detail="Model not loaded yet. Try again in a few seconds.")

    async with _request_lock:
        log.info(f"[separation trace={trace_id} event=processing_start] mode={mode}")
        t_start = time.monotonic()

        try:
            result = await asyncio.get_event_loop().run_in_executor(
                None, _run_separation, trace_id, content, file.filename or "audio.mp3", mode
            )
        except ValueError as e:
            log.error(f"[separation trace={trace_id} event=invalid_audio] err={e}")
            raise HTTPException(status_code=400, detail=str(e))
        except Exception as e:
            log.error(f"[separation trace={trace_id} event=processing_failed] err={e}")
            raise HTTPException(status_code=500, detail=str(e))

        elapsed = time.monotonic() - t_start
        log.info(f"[separation trace={trace_id} event=processing_done] elapsed={elapsed:.1f}s vocals_wav={len(result['vocals_wav'])} instrumental_wav={len(result['instrumental_wav'])} ext={result['compressed_ext']}")

    return JSONResponse(content=result)


def _run_separation(trace_id: int, audio_bytes: bytes, filename: str, mode: str) -> dict:
    import subprocess

    with tempfile.TemporaryDirectory(prefix=f"sep_{trace_id}_") as work_dir:
        ext = os.path.splitext(filename)[1] or ".mp3"
        input_path = os.path.join(work_dir, f"input{ext}")
        with open(input_path, "wb") as f:
            f.write(audio_bytes)

        # Validate audio with ffprobe
        probe = subprocess.run(
            ["ffprobe", "-v", "error", "-show_entries", "format=duration",
             "-of", "csv=p=0", input_path],
            capture_output=True, text=True
        )
        if probe.returncode != 0:
            raise ValueError(f"Not a valid audio file: {probe.stderr.strip()}")
        try:
            duration = float(probe.stdout.strip())
        except ValueError:
            raise ValueError("Could not determine audio duration")

        log.info(f"[separation trace={trace_id} event=audio_validated] duration={duration:.1f}s")

        separator = _separator_quality if mode == "quality" else _separator_fast

        overlap = 0.50 if mode == "quality" else 0.25
        separator.arch_specific_params = {"overlap": overlap}

        log.info(f"[separation trace={trace_id} event=separator_run] mode={mode} overlap={overlap}")
        output_files = separator.separate(input_path)

        log.info(f"[separation trace={trace_id} event=separator_output] files={output_files}")

        # Find vocals and instrumental output files.
        # Check instrumental FIRST — output filenames contain model name (Kim_Vocal_2)
        # so both files have "vocal" in their path. We identify instrumental by its
        # stem tag, then treat the remaining file as vocals.
        vocals_path = None
        instrumental_path = None
        for path in output_files:
            lower = os.path.basename(path).lower()
            if "(instrumental)" in lower or "no_vocal" in lower or "accompaniment" in lower or "karaoke" in lower:
                instrumental_path = path
            elif "(vocals)" in lower or "(vocal)" in lower:
                vocals_path = path

        # Fallback: if stem tags not found, use file index order from audio-separator
        # (index 0 = vocals stem, index 1 = instrumental stem for Kim_Vocal_2)
        if not vocals_path or not instrumental_path:
            log.warning(f"[separation trace={trace_id} event=stem_tag_fallback] files={output_files}")
            if len(output_files) >= 2:
                vocals_path = output_files[0]
                instrumental_path = output_files[1]
            else:
                raise RuntimeError(f"Separator did not produce 2 output files: {output_files}")

        with open(vocals_path, "rb") as f:
            vocals_wav_b64 = base64.b64encode(f.read()).decode()
        with open(instrumental_path, "rb") as f:
            instrumental_wav_b64 = base64.b64encode(f.read()).decode()

        # Convert WAV → compressed format matching original input extension.
        # Supported: mp3, ogg, m4a, aac, flac. Fallback: mp3 320k.
        compressed_ext = ext.lstrip(".").lower()
        if compressed_ext not in ("mp3", "ogg", "m4a", "aac", "flac"):
            compressed_ext = "mp3"

        ffmpeg_codec = {
            "mp3": ["-codec:a", "libmp3lame", "-qscale:a", "0"],
            "ogg": ["-codec:a", "libvorbis", "-qscale:a", "6"],
            "m4a": ["-codec:a", "aac", "-b:a", "256k"],
            "aac": ["-codec:a", "aac", "-b:a", "256k"],
            "flac": ["-codec:a", "flac"],
        }[compressed_ext]

        vocals_compressed_path = os.path.join(work_dir, f"vocals.{compressed_ext}")
        instrumental_compressed_path = os.path.join(work_dir, f"instrumental.{compressed_ext}")

        for src, dst in [(vocals_path, vocals_compressed_path), (instrumental_path, instrumental_compressed_path)]:
            r = subprocess.run(
                ["ffmpeg", "-y", "-i", src] + ffmpeg_codec + [dst],
                capture_output=True
            )
            log.info(f"[separation trace={trace_id} event=ffmpeg_convert] src={os.path.basename(src)} dst={os.path.basename(dst)} ok={r.returncode == 0}")
            if r.returncode != 0:
                log.warning(f"[separation trace={trace_id} event=ffmpeg_failed] stderr={r.stderr[-200:].decode(errors='replace')}")

        with open(vocals_compressed_path, "rb") as f:
            vocals_compressed_b64 = base64.b64encode(f.read()).decode()
        with open(instrumental_compressed_path, "rb") as f:
            instrumental_compressed_b64 = base64.b64encode(f.read()).decode()

        log.info(
            f"[separation trace={trace_id} event=encode_done] "
            f"vocals_wav={len(vocals_wav_b64)} instrumental_wav={len(instrumental_wav_b64)} "
            f"vocals_compressed={len(vocals_compressed_b64)} instrumental_compressed={len(instrumental_compressed_b64)} "
            f"compressed_ext={compressed_ext} duration={duration:.1f}s"
        )

        return {
            "vocals_wav": vocals_wav_b64,
            "instrumental_wav": instrumental_wav_b64,
            "vocals_compressed": vocals_compressed_b64,
            "instrumental_compressed": instrumental_compressed_b64,
            "compressed_ext": compressed_ext,
            "duration_seconds": duration,
        }


if __name__ == "__main__":
    uvicorn.run("main:app", host="0.0.0.0", port=6589, log_level="info")
