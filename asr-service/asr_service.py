"""
ASR microservice — nemotron-3.5-asr-streaming-0.6b (ONNX int4, CPU)

Architecture: Cache-Aware FastConformer-RNNT (streaming)
  encoder.onnx  — processes 8960-sample chunks, stateful cache
  decoder.onnx  — 2-layer LSTM prediction network
  joint.onnx    — joiner (encoder + decoder → logits)
  silero_vad.onnx — VAD: detects speech segments before ASR

Tensor names from genai_config.json:
  Encoder inputs:  audio_signal, length, cache_last_channel, cache_last_time,
                   cache_last_channel_len, lang_id
  Encoder outputs: outputs, encoded_lengths, cache_last_channel_next,
                   cache_last_time_next, cache_last_channel_len_next
  Decoder inputs:  targets, h_in, c_in
  Decoder outputs: decoder_output, h_out, c_out
  Joiner inputs:   encoder_output, decoder_output
  Joiner outputs:  joint_output
"""

import asyncio
import json
import logging
import multiprocessing
import os
import re
import subprocess
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Optional

import numpy as np
import soundfile as sf
import uvicorn
from fastapi import FastAPI, File, Form, HTTPException, UploadFile
from fastapi.responses import JSONResponse

# CPU Broker lives in the separation-service directory.
_sep_dir = os.getenv(
    "SEPARATION_SERVICE_DIR",
    os.path.join(os.path.dirname(__file__), "..", "separation-service"),
)
sys.path.insert(0, os.path.abspath(_sep_dir))

from cpu_monitor import start_monitor, available_cores, pick_cores  # noqa: E402
from cpu_broker import (  # noqa: E402
    start_broker, acquire, release, is_overloaded,
    get_redis, RESERVED_KEY, QUEUE_KEY,
)

logging.basicConfig(
    level=logging.INFO,
    format="[asr %(levelname)s] %(message)s",
    stream=sys.stderr,
)
log = logging.getLogger("asr")

MODEL_DIR = Path(os.getenv("ASR_MODEL_DIR", "/opt/asr_model"))
# 16kHz × 560ms = 8960 samples per chunk
CHUNK_SAMPLES = 8960
SAMPLE_RATE = 16000
BLANK_ID = 13087
MAX_SYMBOLS_PER_STEP = 10

# Encoder cache dimensions from genai_config:
#   hidden_size=1024, num_hidden_layers=24
#   left_context=56, conv_context=8, pre_encode_cache_size=9
HIDDEN_SIZE = 1024
NUM_LAYERS = 24
LEFT_CONTEXT = 56
CONV_CONTEXT = 8
PRE_ENCODE_CACHE = 9

# Decoder LSTM dimensions: hidden_size=640, num_hidden_layers=2
DEC_HIDDEN = 640
DEC_LAYERS = 2

_executor = ThreadPoolExecutor(max_workers=4)
_sessions: dict = {}
_tokenizer: Optional[list] = None
_model_loaded = False
_all_cpu_cores: set = set()
_trace_counter = 0

# VAD constants
VAD_FRAME_SAMPLES = 512          # 32ms at 16kHz
VAD_CONTEXT_SAMPLES = 64
VAD_THRESHOLD = 0.5
VAD_NEG_THRESHOLD = 0.35
VAD_MIN_SPEECH_MS = 200          # ignore segments shorter than this
VAD_PADDING_MS = 100             # pad each segment on both sides
VAD_MIN_SILENCE_MS = 300         # merge segments with gap shorter than this


def _next_trace_id() -> int:
    global _trace_counter
    _trace_counter += 1
    return _trace_counter


class SileroVAD:
    """Stateful Silero VAD wrapper using ONNX Runtime."""

    def __init__(self, session):
        self._session = session
        self._state = np.zeros((2, 1, 128), dtype=np.float32)
        self._context = np.zeros(VAD_CONTEXT_SAMPLES, dtype=np.float32)

    def reset(self):
        self._state = np.zeros((2, 1, 128), dtype=np.float32)
        self._context = np.zeros(VAD_CONTEXT_SAMPLES, dtype=np.float32)

    def process_frame(self, frame: np.ndarray) -> float:
        """Process one 512-sample frame. Returns speech probability."""
        inp = np.concatenate([self._context, frame]).astype(np.float32)
        prob, new_state = self._session.run(
            None,
            {
                "input": inp.reshape(1, -1),
                "state": self._state,
                "sr": np.array(SAMPLE_RATE, dtype=np.int64),
            },
        )
        self._state = new_state
        self._context = frame[-VAD_CONTEXT_SAMPLES:]
        return float(prob[0][0])

    def get_speech_segments(self, audio: np.ndarray) -> list[dict]:
        """
        Run VAD over full audio. Returns list of {start_sec, end_sec} dicts
        covering speech regions, with padding applied and short gaps merged.
        """
        self.reset()
        probs = []
        n = len(audio)
        for start in range(0, n, VAD_FRAME_SAMPLES):
            frame = audio[start: start + VAD_FRAME_SAMPLES]
            if len(frame) < VAD_FRAME_SAMPLES:
                frame = np.pad(frame, (0, VAD_FRAME_SAMPLES - len(frame)))
            probs.append(self.process_frame(frame))

        frame_dur = VAD_FRAME_SAMPLES / SAMPLE_RATE   # 0.032s

        # Build raw speech segments using hysteresis
        segments = []
        in_speech = False
        seg_start = 0.0
        for i, p in enumerate(probs):
            t = i * frame_dur
            if not in_speech and p >= VAD_THRESHOLD:
                in_speech = True
                seg_start = t
            elif in_speech and p < VAD_NEG_THRESHOLD:
                in_speech = False
                segments.append({"start": seg_start, "end": t})
        if in_speech:
            segments.append({"start": seg_start, "end": n / SAMPLE_RATE})

        if not segments:
            return []

        # Apply padding
        pad = VAD_PADDING_MS / 1000
        total_dur = n / SAMPLE_RATE
        segments = [
            {"start": max(0.0, s["start"] - pad), "end": min(total_dur, s["end"] + pad)}
            for s in segments
        ]

        # Merge segments with short silence between them
        min_gap = VAD_MIN_SILENCE_MS / 1000
        merged = [segments[0]]
        for s in segments[1:]:
            if s["start"] - merged[-1]["end"] < min_gap:
                merged[-1]["end"] = s["end"]
            else:
                merged.append(s)

        # Drop very short segments
        min_dur = VAD_MIN_SPEECH_MS / 1000
        merged = [s for s in merged if s["end"] - s["start"] >= min_dur]

        return merged


# ── model loading ──────────────────────────────────────────────────────────────

def _load_onnx_session(path: Path, name: str, num_threads: int):
    import onnxruntime as ort
    opts = ort.SessionOptions()
    opts.inter_op_num_threads = num_threads
    opts.intra_op_num_threads = num_threads
    opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    session = ort.InferenceSession(str(path), sess_options=opts, providers=["CPUExecutionProvider"])
    log.info(f"[asr event=session_loaded] name={name}")
    return session


def load_models():
    global _sessions, _tokenizer, _model_loaded, _all_cpu_cores
    try:
        log.info(f"[asr event=load_start] model_dir={MODEL_DIR}")
        _all_cpu_cores = set(range(multiprocessing.cpu_count()))
        # Load models with default thread count; threads will be re-pinned per-request.
        default_threads = int(os.getenv("OMP_NUM_THREADS", str(multiprocessing.cpu_count())))
        _sessions["encoder"] = _load_onnx_session(MODEL_DIR / "encoder.onnx", "encoder", default_threads)
        _sessions["decoder"] = _load_onnx_session(MODEL_DIR / "decoder.onnx", "decoder", default_threads)
        _sessions["joint"]   = _load_onnx_session(MODEL_DIR / "joint.onnx",   "joint",   default_threads)
        _sessions["vad"]     = SileroVAD(_load_onnx_session(MODEL_DIR / "silero_vad.onnx", "vad", default_threads))

        vocab_path = MODEL_DIR / "vocab.txt"
        with open(vocab_path) as f:
            _tokenizer = [line.rstrip("\n") for line in f]
        log.info(f"[asr event=vocab_loaded] size={len(_tokenizer)}")

        _model_loaded = True
        log.info("[asr event=load_done]")
    except Exception as e:
        log.error(f"[asr event=load_failed] err={e}")
        _model_loaded = False


@asynccontextmanager
async def lifespan(app: FastAPI):
    log.info("[asr event=startup] starting cpu monitor and broker")
    await start_monitor()
    await start_broker()
    log.info("[asr event=startup] pre-loading model in background thread")
    loop = asyncio.get_event_loop()
    await loop.run_in_executor(None, load_models)
    log.info(f"[asr event=startup_done] model_loaded={_model_loaded}")
    yield
    log.info("[asr event=shutdown]")


app = FastAPI(lifespan=lifespan)


# ── thread affinity helpers ────────────────────────────────────────────────────

def _pin_all_threads(cores: set, trace_id: int, log_event: str | None = None) -> int:
    pinned = 0
    for tid_str in os.listdir("/proc/self/task"):
        try:
            tid = int(tid_str)
            os.sched_setaffinity(tid, cores)
            pinned += 1
        except (ValueError, ProcessLookupError, PermissionError):
            continue
    if log_event:
        log.info(f"[asr trace={trace_id} event={log_event}] cores={sorted(cores)} threads_pinned={pinned}")
    return pinned


def _pinner_loop(cores: set, trace_id: int, stop_event: threading.Event, seen: set):
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
            log.info(f"[asr trace={trace_id} event=affinity_repin] cores={sorted(cores)} new_threads={len(new)}")
            seen |= new


# ── audio helpers ──────────────────────────────────────────────────────────────

def _to_wav16k(src: str) -> str:
    """Convert any audio file to 16kHz mono WAV via ffmpeg. Returns path to WAV."""
    dst = src + "_16k.wav"
    result = subprocess.run(
        ["ffmpeg", "-y", "-i", src, "-ar", "16000", "-ac", "1", "-f", "wav", dst],
        capture_output=True,
    )
    if result.returncode != 0:
        raise ValueError(f"ffmpeg failed: {result.stderr[-300:].decode(errors='replace')}")
    return dst


def _read_audio(path: str) -> np.ndarray:
    """Read WAV, return float32 array normalised to [-1, 1]."""
    audio, sr = sf.read(path, dtype="float32")
    if audio.ndim > 1:
        audio = audio.mean(axis=1)
    if sr != SAMPLE_RATE:
        raise ValueError(f"Expected {SAMPLE_RATE}Hz, got {sr}Hz after conversion")
    return audio


def _mel_spectrogram(audio: np.ndarray) -> np.ndarray:
    """
    Compute log-mel spectrogram matching audio_processor_config.json:
      n_fft=512, hop_length=160, n_mels=128, fmin=0, fmax=8000,
      window=hann, preemphasis=0.97, center=True, mag_power=2.0
    Returns shape (1, T, 128)  — (batch, time, n_mels) as encoder expects.
    """
    import librosa

    # Preemphasis
    audio_pe = np.append(audio[0], audio[1:] - 0.97 * audio[:-1])

    mel = librosa.feature.melspectrogram(
        y=audio_pe,
        sr=SAMPLE_RATE,
        n_fft=512,
        hop_length=160,
        win_length=400,
        window="hann",
        center=True,
        n_mels=128,
        fmin=0,
        fmax=8000,
        power=2.0,
    )
    # Log with epsilon guard; transpose to (T, n_mels) then add batch dim → (1, T, 128)
    log_mel = np.log(mel + 1e-10)
    return log_mel.T[np.newaxis, :, :].astype(np.float32)   # (1, T, 128)


def _get_lang_id(lang: str) -> int:
    """
    Map language string to lang_id integer.
    'auto' → 0 (model infers language from context).
    """
    LANG_MAP = {
        "auto": 0,
        "en-US": 1, "en-GB": 2, "de-DE": 3, "fr-FR": 4, "es-ES": 5,
        "it-IT": 6, "pt-BR": 7, "ru-RU": 8, "zh-CN": 9, "ja-JP": 10,
        "ko-KR": 11, "ar-SA": 12, "hi-IN": 13, "nl-NL": 14, "pl-PL": 15,
        "tr-TR": 16, "uk-UA": 17, "cs-CZ": 18, "ro-RO": 19, "sv-SE": 20,
    }
    return LANG_MAP.get(lang, 0)


# ── RNNT inference ─────────────────────────────────────────────────────────────

def _init_encoder_cache(batch: int = 1):
    """Initialise stateful encoder cache tensors to zeros."""
    # cache_last_channel: (batch, num_layers, left_context, hidden_size)
    cache_last_channel = np.zeros(
        (batch, NUM_LAYERS, LEFT_CONTEXT, HIDDEN_SIZE), dtype=np.float32
    )
    # cache_last_time: (batch, num_layers, hidden_size, conv_context)
    cache_last_time = np.zeros(
        (batch, NUM_LAYERS, HIDDEN_SIZE, CONV_CONTEXT), dtype=np.float32
    )
    # cache_last_channel_len: (batch,) — number of valid frames in channel cache
    cache_last_channel_len = np.zeros((batch,), dtype=np.int64)
    return cache_last_channel, cache_last_time, cache_last_channel_len


def _encode_chunk(
    enc_session,
    audio_chunk: np.ndarray,       # (1, T_chunk, 128)
    cache_last_channel: np.ndarray,
    cache_last_time: np.ndarray,
    cache_last_channel_len: np.ndarray,
    lang_id: int,
) -> tuple:
    """Run one encoder chunk. Returns (encoder_out, encoded_lengths, updated caches)."""
    T = audio_chunk.shape[1]   # axis 1 is time
    length = np.array([T], dtype=np.int64)
    lang_id_arr = np.array([lang_id], dtype=np.int64)

    feed = {
        "audio_signal":          audio_chunk,
        "length":                length,
        "cache_last_channel":    cache_last_channel,
        "cache_last_time":       cache_last_time,
        "cache_last_channel_len": cache_last_channel_len,
        "lang_id":               lang_id_arr,
    }
    outs = enc_session.run(None, feed)
    # outputs: [encoder_out, encoded_lengths, cache_ch_next, cache_t_next, cache_ch_len_next]
    return outs[0], outs[1], outs[2], outs[3], outs[4]


def _decode_step(dec_session, target: int, h: np.ndarray, c: np.ndarray) -> tuple:
    """Single decoder step. Returns (decoder_output, h_out, c_out).
    decoder_output shape from model: (batch, 640, target_len) → transpose to (batch, target_len, 640).
    """
    targets = np.array([[target]], dtype=np.int64)
    outs = dec_session.run(None, {"targets": targets, "h_in": h, "c_in": c})
    # outs[0]: (batch, 640, target_len) → (batch, target_len, 640)
    dec_out = np.transpose(outs[0], (0, 2, 1))
    return dec_out, outs[1], outs[2]


def _joint_step(joint_session, enc_out: np.ndarray, dec_out: np.ndarray) -> np.ndarray:
    """Joint network. Returns logits shape (vocab_size,).
    joint_output shape: (batch, time, target_len, vocab) → take [0, 0, 0, :]
    """
    outs = joint_session.run(
        None,
        {"encoder_output": enc_out, "decoder_output": dec_out},
    )
    return outs[0][0, 0, 0, :]   # (vocab_size,)


def _greedy_rnnt_decode(
    enc_session,
    dec_session,
    joint_session,
    mel: np.ndarray,
    lang_id: int,
) -> list[tuple[int, float]]:
    """
    Greedy RNNT decode over full audio (chunk-by-chunk).
    mel shape: (1, T, 128)
    Returns list of (token_id, timestamp_seconds).
    Each encoder output frame = 80ms.
    """
    T_total = mel.shape[1]
    chunk_mel_frames = LEFT_CONTEXT   # 56 new frames per chunk
    n_chunks = max(1, (T_total + chunk_mel_frames - 1) // chunk_mel_frames)

    padded_len = n_chunks * chunk_mel_frames
    if padded_len > T_total:
        pad = np.zeros((1, padded_len - T_total, 128), dtype=np.float32)
        mel = np.concatenate([mel, pad], axis=1)

    enc = enc_session
    dec = dec_session
    jnt = joint_session

    cache_ch, cache_t, cache_ch_len = _init_encoder_cache()

    h = np.zeros((DEC_LAYERS, 1, DEC_HIDDEN), dtype=np.float32)
    c = np.zeros((DEC_LAYERS, 1, DEC_HIDDEN), dtype=np.float32)
    last_token = BLANK_ID
    dec_out, h, c = _decode_step(dec, last_token, h, c)

    tokens: list[tuple[int, float]] = []

    pre_cache = np.zeros((1, PRE_ENCODE_CACHE, 128), dtype=np.float32)

    # Each encoder output frame = chunk_duration / enc_frames_per_chunk
    # chunk = 56 mel frames × (160 samples/frame) / 16000 Hz = 0.56s
    # encoder outputs 7 frames per chunk → each frame = 0.56/7 = 0.08s
    chunk_duration = chunk_mel_frames * 160 / SAMPLE_RATE   # 0.56s
    global_enc_frame = 0

    for i in range(n_chunks):
        new_frames = mel[:, i * chunk_mel_frames: (i + 1) * chunk_mel_frames, :]
        chunk = np.concatenate([pre_cache, new_frames], axis=1)
        pre_cache = new_frames[:, -PRE_ENCODE_CACHE:, :]

        enc_out, enc_len, cache_ch, cache_t, cache_ch_len = _encode_chunk(
            enc, chunk, cache_ch, cache_t, cache_ch_len, lang_id
        )

        T_enc = enc_out.shape[1]   # 7 frames per chunk
        frame_duration = chunk_duration / T_enc   # 0.08s per frame

        for t in range(T_enc):
            timestamp = global_enc_frame * frame_duration
            enc_frame = enc_out[:, t: t + 1, :]
            sym_count = 0
            while sym_count < MAX_SYMBOLS_PER_STEP:
                logits = _joint_step(jnt, enc_frame, dec_out)
                pred = int(np.argmax(logits))
                if pred == BLANK_ID:
                    break
                tokens.append((pred, timestamp))
                dec_out, h, c = _decode_step(dec, pred, h, c)
                sym_count += 1
            global_enc_frame += 1

    return tokens


def _tokens_to_text(timed_tokens: list[tuple[int, float]]) -> tuple[str, str]:
    """
    Detokenise timed tokens. Returns (clean_text, detected_language).
    """
    pieces = [_tokenizer[tid] for tid, _ in timed_tokens if 0 <= tid < len(_tokenizer)]
    raw = "".join(pieces).replace("▁", " ").strip()
    lang_match = re.search(r"<([a-z]{2}-[A-Z]{2})>", raw)
    language = lang_match.group(1) if lang_match else "unknown"
    text = re.sub(r"<[^>]+>", "", raw).strip()
    return text, language


def _fmt_srt_time(seconds: float) -> str:
    h = int(seconds // 3600)
    m = int((seconds % 3600) // 60)
    s = int(seconds % 60)
    ms = int((seconds % 1) * 1000)
    return f"{h:02d}:{m:02d}:{s:02d},{ms:03d}"


def _tokens_to_srt(timed_tokens: list[tuple[int, float]]) -> str:
    """
    Build SRT from timed tokens.
    Split on sentence-ending punctuation (. ? ! and ,).
    Each segment max ~10 words or sentence boundary.
    """
    if not timed_tokens:
        return ""

    # Detokenise pieces with timestamps preserved per piece
    timed_pieces: list[tuple[str, float]] = []
    for tid, ts in timed_tokens:
        if 0 <= tid < len(_tokenizer):
            piece = _tokenizer[tid]
            piece = re.sub(r"<[^>]+>", "", piece)  # strip lang tags
            if piece:
                timed_pieces.append((piece, ts))

    if not timed_pieces:
        return ""

    # Group into words with start timestamp
    words: list[tuple[str, float]] = []
    current_word = ""
    current_ts = timed_pieces[0][1]
    for piece, ts in timed_pieces:
        if piece.startswith("▁"):
            if current_word:
                words.append((current_word, current_ts))
            current_word = piece[1:]  # strip ▁
            current_ts = ts
        else:
            current_word += piece
    if current_word:
        words.append((current_word, current_ts))

    # Group words into SRT segments
    SPLIT_PUNCT = {'.', '?', '!', ','}
    MAX_WORDS = 10

    segments: list[tuple[float, float, str]] = []
    seg_words: list[str] = []
    seg_start = words[0][1] if words else 0.0
    seg_end = seg_start

    for i, (word, ts) in enumerate(words):
        seg_words.append(word)
        seg_end = ts + 0.08  # add one frame duration as end padding

        ends_sentence = word and word[-1] in SPLIT_PUNCT
        is_last = (i == len(words) - 1)

        if ends_sentence or len(seg_words) >= MAX_WORDS or is_last:
            text = " ".join(seg_words).strip()
            if text:
                segments.append((seg_start, seg_end, text))
            seg_words = []
            seg_start = words[i + 1][1] if i + 1 < len(words) else seg_end

    # Build SRT string
    lines = []
    for idx, (start, end, text) in enumerate(segments, 1):
        lines.append(str(idx))
        lines.append(f"{_fmt_srt_time(start)} --> {_fmt_srt_time(end)}")
        lines.append(text)
        lines.append("")

    return "\n".join(lines)


# ── main inference entry point ─────────────────────────────────────────────────

def _decode_segment(audio_segment: np.ndarray, time_offset: float, lang_id: int) -> list[tuple[int, float]]:
    """Decode one speech segment. Returns timed tokens with absolute timestamps."""
    mel = _mel_spectrogram(audio_segment)
    relative_tokens = _greedy_rnnt_decode(
        _sessions["encoder"],
        _sessions["decoder"],
        _sessions["joint"],
        mel,
        lang_id=lang_id,
    )
    # Shift timestamps by segment start
    return [(tid, ts + time_offset) for tid, ts in relative_tokens]


def _run_transcription(audio_bytes: bytes, filename: str, cores: list, trace_id: int) -> dict:
    """Blocking transcription with VAD-based segmentation, pinned to broker-allocated cores."""
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
        return _do_transcription(audio_bytes, filename, trace_id)
    finally:
        if pinner_thread:
            stop_event.set()
            pinner_thread.join(timeout=2)
        if cores and _all_cpu_cores:
            _pin_all_threads(_all_cpu_cores, trace_id, "affinity_restored")


def _decode_one(
    idx: int,
    seg: dict,
    audio: np.ndarray,
    lang_id: int,
    threads_per_worker: int,
    trace_id: int,
    total_segs: int,
) -> list[tuple[int, float]]:
    """Decode a single segment. Picked from shared queue by the thread pool."""
    os.environ["OMP_NUM_THREADS"] = str(threads_per_worker)
    os.environ["OPENBLAS_NUM_THREADS"] = str(threads_per_worker)
    os.environ["MKL_NUM_THREADS"] = str(threads_per_worker)
    start_s = seg["start"]
    end_s = seg["end"]
    start_sample = int(start_s * SAMPLE_RATE)
    end_sample = int(end_s * SAMPLE_RATE)
    audio_seg = audio[start_sample:end_sample]
    if len(audio_seg) < VAD_FRAME_SAMPLES:
        return []
    tokens = _decode_segment(audio_seg, start_s, lang_id)
    log.info(
        f"[asr trace={trace_id} event=segment_done] "
        f"seg={idx+1}/{total_segs} start={start_s:.2f}s end={end_s:.2f}s tokens={len(tokens)}"
    )
    return tokens


def _do_transcription(audio_bytes: bytes, filename: str, trace_id: int) -> dict:
    with tempfile.TemporaryDirectory(prefix="asr_") as tmp:
        ext = os.path.splitext(filename)[1] or ".ogg"
        src = os.path.join(tmp, f"input{ext}")
        with open(src, "wb") as f:
            f.write(audio_bytes)

        wav_path = _to_wav16k(src)
        audio = _read_audio(wav_path)
        duration = len(audio) / SAMPLE_RATE

        log.info(f"[asr trace={trace_id} event=inference_start] duration={duration:.1f}s samples={len(audio)}")

        vad: SileroVAD = _sessions["vad"]
        segments = vad.get_speech_segments(audio)
        log.info(f"[asr trace={trace_id} event=vad_done] segments={len(segments)}")

        if not segments:
            log.info(f"[asr trace={trace_id} event=no_speech]")
            return {"text": "", "language": "unknown", "duration_seconds": round(duration, 2), "srt": ""}

        lang_id = _get_lang_id("auto")
        total = len(segments)

        # Determine parallelism based on allocated core count.
        core_count = int(os.environ.get("OMP_NUM_THREADS", "1"))
        parallel = max(1, core_count)
        threads_per_worker = 1

        log.info(
            f"[asr trace={trace_id} event=parallel_config] "
            f"cores={core_count} parallel={parallel} threads_per_worker={threads_per_worker}"
        )

        # Dynamic queue: one task per segment, thread pool picks freely.
        # No idle workers — whoever finishes first grabs the next segment.
        with ThreadPoolExecutor(max_workers=parallel) as pool:
            futures = [
                pool.submit(_decode_one, i, seg, audio, lang_id, threads_per_worker, trace_id, total)
                for i, seg in enumerate(segments)
            ]
            all_tokens_nested = [f.result() for f in futures]

        merged: list[tuple[int, float]] = []
        for tokens in all_tokens_nested:
            merged.extend(tokens)
        all_tokens = sorted(merged, key=lambda x: x[1])

        text, language = _tokens_to_text(all_tokens)
        srt = _tokens_to_srt(all_tokens)

        log.info(f"[asr trace={trace_id} event=inference_done] lang={language} total_tokens={len(all_tokens)} text_len={len(text)}")
        return {"text": text, "language": language, "duration_seconds": round(duration, 2), "srt": srt}


# ── endpoints ──────────────────────────────────────────────────────────────────

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


@app.post("/transcribe")
async def transcribe(
    audio: UploadFile = File(...),
    user_id: int = Form(0),
    is_vip: bool = Form(False),
):
    trace_id = _next_trace_id()
    if not _model_loaded:
        raise HTTPException(status_code=503, detail="Model not loaded yet")

    content = await audio.read()
    if len(content) == 0:
        raise HTTPException(status_code=422, detail="Empty audio file")

    filename = audio.filename or "audio.ogg"
    log.info(f"[asr trace={trace_id} event=request] filename={filename} size={len(content)} user_id={user_id} is_vip={is_vip}")

    log.info(f"[asr trace={trace_id} event=acquire_start] user_id={user_id}")
    cores = await acquire(user_id=user_id, is_vip=is_vip)
    log.info(f"[asr trace={trace_id} event=acquire_done] cores={cores}")

    try:
        loop = asyncio.get_event_loop()
        result = await loop.run_in_executor(
            _executor, _run_transcription, content, filename, cores, trace_id
        )
    except ValueError as e:
        log.warning(f"[asr trace={trace_id} event=invalid_audio] err={e}")
        raise HTTPException(status_code=422, detail=str(e))
    except Exception as e:
        log.error(f"[asr trace={trace_id} event=inference_error] err={e}")
        raise HTTPException(status_code=500, detail=str(e))
    finally:
        await release(cores)
        log.info(f"[asr trace={trace_id} event=cores_released] cores={cores}")

    return JSONResponse(content=result)


@app.post("/transcribe/srt")
async def transcribe_srt(
    audio: UploadFile = File(...),
    user_id: int = Form(0),
    is_vip: bool = Form(False),
):
    trace_id = _next_trace_id()
    if not _model_loaded:
        raise HTTPException(status_code=503, detail="Model not loaded yet")

    content = await audio.read()
    if len(content) == 0:
        raise HTTPException(status_code=422, detail="Empty audio file")

    filename = audio.filename or "audio.ogg"
    log.info(f"[asr trace={trace_id} event=request_srt] filename={filename} size={len(content)} user_id={user_id} is_vip={is_vip}")

    log.info(f"[asr trace={trace_id} event=acquire_start] user_id={user_id}")
    cores = await acquire(user_id=user_id, is_vip=is_vip)
    log.info(f"[asr trace={trace_id} event=acquire_done] cores={cores}")

    try:
        loop = asyncio.get_event_loop()
        result = await loop.run_in_executor(
            _executor, _run_transcription, content, filename, cores, trace_id
        )
    except ValueError as e:
        log.warning(f"[asr trace={trace_id} event=invalid_audio] err={e}")
        raise HTTPException(status_code=422, detail=str(e))
    except Exception as e:
        log.error(f"[asr trace={trace_id} event=inference_error] err={e}")
        raise HTTPException(status_code=500, detail=str(e))
    finally:
        await release(cores)
        log.info(f"[asr trace={trace_id} event=cores_released] cores={cores}")

    from fastapi.responses import Response
    srt_filename = filename.rsplit(".", 1)[0] + ".srt"
    return Response(
        content=result["srt"],
        media_type="text/plain; charset=utf-8",
        headers={"Content-Disposition": f'attachment; filename="{srt_filename}"'},
    )


if __name__ == "__main__":
    uvicorn.run("asr_service:app", host="127.0.0.1", port=8765, log_level="info")
