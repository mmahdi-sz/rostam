import asyncio
import base64
import io
import logging
import os
import sys
import tempfile
import threading
import time
from contextlib import asynccontextmanager

import uvicorn
from fastapi import FastAPI, File, Form, HTTPException, Request, UploadFile
from fastapi.responses import JSONResponse

from cpu_monitor import start_monitor, available_cores, pick_cores
from cpu_broker import start_broker, acquire, release, is_overloaded, get_redis, RESERVED_KEY, QUEUE_KEY

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
_trace_counter = 0
_sep_lock = asyncio.Lock()
_all_cpu_cores: set = set()


def next_trace_id() -> int:
    global _trace_counter
    _trace_counter += 1
    return _trace_counter


def load_models():
    global _separator_quality, _separator_fast, _model_loaded, _all_cpu_cores
    try:
        import multiprocessing
        from audio_separator.separator import Separator

        log.info(f"[separation event=model_load_start] model={MODEL_NAME} model_dir={MODEL_DIR}")
        os.makedirs(MODEL_DIR, exist_ok=True)

        # Record all available cores so we can restore affinity after separation.
        _all_cpu_cores = set(range(multiprocessing.cpu_count()))

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
    log.info("[separation event=startup] starting cpu monitor and broker")
    await start_monitor()
    await start_broker()
    log.info("[separation event=startup] pre-loading model in background thread")
    loop = asyncio.get_event_loop()
    await loop.run_in_executor(None, load_models)
    log.info(f"[separation event=startup_done] model_loaded={_model_loaded}")
    yield
    log.info("[separation event=shutdown]")


app = FastAPI(lifespan=lifespan)


@app.get("/health")
async def health():
    cores = await available_cores()
    return {"status": "ok", "model_loaded": _model_loaded, "available_cores": cores}


@app.get("/cpu/status")
async def cpu_status():
    r = await get_redis()
    reserved = len(await r.hgetall(RESERVED_KEY))
    overloaded = await is_overloaded()
    queue_len = await r.zcard(QUEUE_KEY)
    cores = await available_cores()
    return {
        "available_cores": cores,
        "reserved_count": reserved,
        "overloaded": overloaded,
        "queue_length": queue_len,
    }


@app.post("/separate")
async def separate(
    file: UploadFile = File(...),
    mode: str = Form("quality"),
    user_id: int = Form(0),
    is_vip: bool = Form(False),
):
    trace_id = next_trace_id()

    if mode not in ("quality", "fast"):
        raise HTTPException(status_code=400, detail=f"Invalid mode: {mode}. Use 'quality' or 'fast'.")

    content = await file.read()
    file_size = len(content)
    log.info(f"[separation trace={trace_id} event=request_start] mode={mode} file_size={file_size} filename={file.filename} user_id={user_id} is_vip={is_vip}")

    if file_size > MAX_FILE_BYTES:
        log.warning(f"[separation trace={trace_id} event=file_too_large] size={file_size}")
        raise HTTPException(status_code=400, detail=f"File too large: {file_size} bytes (max {MAX_FILE_BYTES})")

    if not _model_loaded:
        log.error(f"[separation trace={trace_id} event=model_not_loaded]")
        raise HTTPException(status_code=503, detail="Model not loaded yet. Try again in a few seconds.")

    # Acquire CPU cores from broker (blocks until available or caller cancels).
    log.info(f"[separation trace={trace_id} event=acquire_start] user_id={user_id} is_vip={is_vip}")
    cores = await acquire(user_id=user_id, is_vip=is_vip)
    log.info(f"[separation trace={trace_id} event=acquire_done] cores={cores}")

    async with _sep_lock:
        try:
            log.info(f"[separation trace={trace_id} event=processing_start] mode={mode} cores={cores}")
            t_start = time.monotonic()

            result = await asyncio.get_event_loop().run_in_executor(
                None, _run_separation, trace_id, content, file.filename or "audio.mp3", mode, cores
            )

            elapsed = time.monotonic() - t_start
            log.info(f"[separation trace={trace_id} event=processing_done] elapsed={elapsed:.1f}s cores={cores}")
        except ValueError as e:
            log.error(f"[separation trace={trace_id} event=invalid_audio] err={e}")
            raise HTTPException(status_code=400, detail=str(e))
        except Exception as e:
            log.error(f"[separation trace={trace_id} event=processing_failed] err={e}")
            raise HTTPException(status_code=500, detail=str(e))
        finally:
            await release(cores)
            log.info(f"[separation trace={trace_id} event=cores_released] cores={cores}")

    return JSONResponse(content=result)


def _pin_all_threads(cores: set, trace_id: int, log_event: str | None = None) -> int:
    """Pin every thread (TID) currently in this process to `cores`. Returns count pinned."""
    pinned = 0
    for tid_str in os.listdir("/proc/self/task"):
        try:
            tid = int(tid_str)
            os.sched_setaffinity(tid, cores)
            pinned += 1
        except (ValueError, ProcessLookupError, PermissionError):
            continue
    if log_event:
        log.info(f"[separation trace={trace_id} event={log_event}] cores={sorted(cores)} threads_pinned={pinned}")
    return pinned


def _pinner_loop(cores: set, trace_id: int, stop_event: threading.Event, seen: set):
    """
    Continuously re-pin newly spawned threads to `cores`. ONNX/OpenMP/PyTorch
    spawn their internal worker thread pools lazily during the first inference
    call, so a one-shot pin at the start misses them. Poll until separation is done.
    """
    while not stop_event.wait(timeout=0.2):
        try:
            current = set(int(t) for t in os.listdir("/proc/self/task"))
        except OSError:
            continue
        new = current - seen
        if new:
            for tid in new:
                try:
                    os.sched_setaffinity(tid, cores)
                except (ProcessLookupError, PermissionError):
                    continue
            log.info(f"[separation trace={trace_id} event=affinity_repin] cores={sorted(cores)} new_threads={len(new)}")
            seen |= new


def _run_separation(trace_id: int, audio_bytes: bytes, filename: str, mode: str, cores: list) -> dict:
    # Set OMP/BLAS thread counts before any inference runs.
    core_count = max(1, len(cores))
    os.environ["OMP_NUM_THREADS"] = str(core_count)
    os.environ["OPENBLAS_NUM_THREADS"] = str(core_count)
    os.environ["MKL_NUM_THREADS"] = str(core_count)

    pinner_thread = None
    stop_event = threading.Event()
    if cores:
        core_set = set(cores)
        try:
            seen = set(int(t) for t in os.listdir("/proc/self/task"))
        except OSError:
            seen = set()
        _pin_all_threads(core_set, trace_id, "affinity_set")
        pinner_thread = threading.Thread(
            target=_pinner_loop, args=(core_set, trace_id, stop_event, seen), daemon=True
        )
        pinner_thread.start()

    try:
        return _do_separation(trace_id, audio_bytes, filename, mode, core_count)
    finally:
        if pinner_thread:
            stop_event.set()
            pinner_thread.join(timeout=2)
        # Restore affinity to all cores so the service process isn't stuck
        # on broker-assigned cores after the job completes.
        if cores and _all_cpu_cores:
            _pin_all_threads(_all_cpu_cores, trace_id, "affinity_restored")


def _do_separation(trace_id: int, audio_bytes: bytes, filename: str, mode: str, core_count: int) -> dict:
    import subprocess

    with tempfile.TemporaryDirectory(prefix=f"sep_{trace_id}_") as work_dir:
        ext = os.path.splitext(filename)[1] or ".mp3"
        input_path = os.path.join(work_dir, f"input{ext}")
        with open(input_path, "wb") as f:
            f.write(audio_bytes)

        # Validate audio with ffprobe.
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

        log.info(f"[separation trace={trace_id} event=separator_run] mode={mode} overlap={overlap} threads={core_count}")
        output_files = separator.separate(input_path)

        log.info(f"[separation trace={trace_id} event=separator_output] files={output_files}")

        vocals_path = None
        instrumental_path = None
        for path in output_files:
            lower = os.path.basename(path).lower()
            if "(instrumental)" in lower or "no_vocal" in lower or "accompaniment" in lower or "karaoke" in lower:
                instrumental_path = path
            elif "(vocals)" in lower or "(vocal)" in lower:
                vocals_path = path

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


@app.post("/cpu/acquire")
async def cpu_acquire_endpoint(user_id: int = Form(0), is_vip: bool = Form(False)):
    trace_id = next_trace_id()
    log.info(f"[cpu_acquire trace={trace_id} event=request] user_id={user_id} is_vip={is_vip}")
    cores = await acquire(user_id=user_id, is_vip=is_vip)
    log.info(f"[cpu_acquire trace={trace_id} event=acquired] cores={cores}")
    return {"cores": cores}


@app.post("/cpu/release")
async def cpu_release_endpoint(request: Request):
    body = await request.json()
    cores = body.get("cores", [])
    trace_id = next_trace_id()
    log.info(f"[cpu_release trace={trace_id} event=request] cores={cores}")
    await release(cores)
    return {"ok": True}


if __name__ == "__main__":
    uvicorn.run("main:app", host="0.0.0.0", port=6589, log_level="info")
