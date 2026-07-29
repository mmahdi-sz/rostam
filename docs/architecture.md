# Architecture & Design Overview

## Architecture Overview

`ros-telegram-bot` is an enterprise-grade Rust Telegram bot engineered for low latency, high throughput, and high observability.

```mermaid
graph TD
    User([Telegram User]) <--> TG[Telegram API / Local Bot API Server]
    TG <--> Dispatch[app::dispatch / Update Dispatcher]

    subgraph Core System
        Dispatch --> State[AppState & FlowManager]
        Dispatch --> YT[youtube Subsystem]
        Dispatch --> Surge[surge_dl Subsystem]
        Dispatch --> AI[AI Subsystems: STT, Denoise, Separation, GWM, Upscale]
        Dispatch --> PDF[pdfcompress Subsystem]
    end

    subgraph Infrastructure
        State <--> DB[(PostgreSQL Database)]
        State <--> Redis[(Redis Cache & Cooldown)]
        Metrics[Prometheus & Health Check :14380] <--- Core System
    end
```

## Subsystem Details
- **YouTube Downloader (`youtube`):** Multi-profile Firefox cookie pool, resolution selection, NLLB translator, ffmpeg sub-flag patching.
- **Surge Downloader (`surge_dl`):** Direct link downloader with SSRF validation, progress bar, chunked RAR archive splitting for large files.
- **AI Suite:** Speech-to-text (Vosk/STT), Denoise (DeepFilterNet), Vocal Separation (Demucs / 6589 broker), Moebius ONNX Gemini watermark remover.
- **PDF Compressor (`pdfcompress`):** Ghostscript presets (`screen`, `ebook`, `printer`, `prepress`).
- **Sync & Concurrency:** RAII guards for metrics (`ActiveDownloadGuard`, `RequestDurationGuard`, `TaskGuard`), `lock_or_recover` for poison-resilient mutex operations.
