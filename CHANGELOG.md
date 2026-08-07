# Changelog

All notable changes to this project will be documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)  
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html)

---

## [Unreleased]

## [2.1.2] - 2026-08-07

### Fixed
- **Blank Env Var Read as a Value**: `systemd`'s `EnvironmentFile` exports `KEY=` as an empty string, so `--skip-bot-api` (which writes `BOT_API_BASE_URL=`) made the bot build a Telegram client against an empty base URL and every `get_updates` failed with `HTTP error: builder error`. `config_value` now treats a blank env var as unset, so the bot falls back to the official API as intended.
- **Installer: `rar` Never Installable on Debian**: `rar` ships in Debian's `non-free` component, and the official Debian images enable `main` only, so `apt-get install rar` always fell through to the "rar unavailable" warning and archive splitting stayed degraded. The installer now enables `contrib non-free non-free-firmware` on the existing Debian/Ubuntu suites before `apt-get update`, handling both the deb822 `.sources` and legacy one-line `sources.list` layouts, and is idempotent on re-run.
- **Installer: Missing Model Assets**: `install.sh` never fetched the NLLB-200 translator, the background-removal ONNX or the Persian Piper voice, so a fresh install came up with translation, `/nobg` and Persian TTS dead. All three are downloaded now, and the 223-byte CTranslate2 `config.json` is generated. `ddcolor_modelscope.onnx` has no HTTP mirror — the installer now scrapes Google Drive's confirm token, pulls the 1.9 GB `ddcolorizer_onnx_models.zip`, extracts only that file and verifies its sha256 (`03d13d21…`), falling back to a warning instead of failing the install if Drive rate-limits.
- **Installer: Missing Build and Runtime Packages**: the build died at `espeak-rs-sys` (`Unable to find libclang`) on a clean host; `clang` and `libclang-dev` are now installed, along with `espeak-ng` and `p7zip` (needed at runtime by Persian TTS and archive compression) and `sudo` (the script's own Postgres provisioning uses it, and minimal images do not ship it).
- **Installer: Surge Unit Restart Loop**: the daemon unit was named `surge.service`, and `surge server start` refuses to run when it sees a unit by that name ("system service is already running"), so it restart-looped until systemd gave up and `:1700` was never reachable. Renamed to `rostam-surge.service`; the legacy unit is removed on upgrade.
- **`edge-tts` Not Installed**: `src/moss_tts/engine.rs` expects it in `separation-service/venv/bin`, but it was absent from `requirements.txt`, so English TTS relied on the binary happening to be on `PATH`.

### Added
- **`ROSTAM_ASSET_CACHE`**: point it at a directory of already-downloaded model files and `install.sh` copies them in by basename instead of re-fetching ~8 GB — offline provisioning, and how the end-to-end container test runs.

### Removed
- **`Dockerfile` and `docker-compose.yml`**: neither could produce a working image — `migrations/` (embedded at compile time), `build.rs` and `files/runtime/libvosk.so` were never copied, the base image predated edition 2024, and the compose file published health/metrics on `0.0.0.0` with credentials that did not match `DATABASE_URL`. `install.sh` is the single supported install path; README's Quick Start now documents it.

## [2.1.1] - 2026-08-07

### Security
- **Bot Token Redaction**: The bot token no longer reaches journald or the admin error panel. `frankenstein` errors embed the full request URL, so the token was written verbatim to logs and to `stats_errors`; both paths now run through a redactor. Tests use a synthetic token only.
- **Separation Service Loopback**: `separation-service` now binds `127.0.0.1` by default (`SEP_BIND_HOST`) instead of `0.0.0.0`, and runs as `User=mahdi` with `NoNewPrivileges=yes`. Note: its endpoints still have no authentication — exposure is closed, authentication is not.

### Fixed
- **Quota Race Across 8 Handlers**: Quotas were read, then the work ran for minutes, then usage was debited — two concurrent requests both passed the check. `reserve_usage` now performs the limit check and the increment in a single SQL statement against a row locked by the `(user_id, quota_type)` primary key, and every failure path between reservation and delivery refunds. Two-window handlers reserve the shorter window first and refund it inline if the longer window rejects.
- **Free Work on a Zero Magnitude**: A failed WAV-header or `ffprobe` probe yielded a magnitude of zero, which always fits a quota, so an exhausted user got free work. Denoise, separation and STT now reserve at least one second.
- **STT Unlimited-Window Overflow**: An unlimited weekly limit (`u64::MAX`) became negative when cast to `i64`, rejecting every request. Now clamped to the `i64` ceiling.
- **Redeem and Referral Ledger Atomicity**: Both subsystems decided then recorded across an `await`, allowing double grants. Atomicity now lives in single statements, since `tokio_postgres::Client::transaction()` requires `&mut self` and the shared client is an `Arc<Client>`.
- **Language Loss and Shutdown Drain Across `tokio::spawn`**: `tokio::task_local!` does not cross `tokio::spawn`, so spawned handlers lost the user's language and fell back to Persian, and were not counted in the shutdown drain. Replaced with `spawn_user_task`, which carries the context.
- **Service Restart Time**: Restarting the bot service took 91 seconds; now 0.07 seconds.
- **Dev/Prod Health Port Collision**: Dev now uses `HEALTH_PORT=14381`.
- **Stale Binary Name**: `install.sh`, `Dockerfile`, `systemd/abc.service` and `README.md` referenced the crate name `ros-telegram-bot` where the binary is `rostam-dev`, breaking install and container builds.
- **YouTube Downloads**: Automatically retry with next cookie in pool when yt-dlp returns zero formats due to `database is locked`, `The page needs to be reloaded`, or bot detection challenges, resolving transient "no downloadable quality found" errors.
- **User Panel Rank Display**: Panel now displays live `effective_rank` (`Dalavar`) with status `(منقضی شده)` when a paid rank has expired, replacing misleading active rank labels.
- **Rank Expiry Formatting**: Displays remaining hours/minutes when less than 24 hours remain on active rank subscriptions instead of `(0 روز)`.
- **Multi-Language i18n**: Added `rank.expiry_with_hours` and `rank.expiry_expired` across all 4 supported languages (`fa`, `en`, `it`, `ru`).

### Added
- **Database-Failure Notice**: A quota database error now tells the user to contact `@mmahdi_sz` instead of failing silently — `rank.quota_db_error` in all 4 languages, sent without a parse mode so one string is safe from handlers with differing parse modes. This is deliberately fail-closed.
- **`/test/quota` TestAPI Endpoint**: Calls the real `rank::quota` reserve/refund/get functions against the dev database. Twelve checks in `scripts/run_testapi_suite.sh` cover a counter quota (upscale) and a magnitude quota (denoise): successful reservation, rejection at the limit with usage unchanged, refund and re-reservation, exact fit, and a refund larger than usage not going negative.

### Removed
- **Obsolete ASR Microservice**: `asr.service` (uvicorn on `127.0.0.1:8765`) was stopped and disabled. Nothing in `src/` referenced it — denoising runs in-process through `crate::stt::deepfilter`. The stale `src/denoise/mod.rs` doc comment claiming a port-8765 sidecar was corrected.

## [2.1.0] - 2026-08-06

### Changed
- **Persian Text-to-Speech Engine**: Replaced Persian `edge-tts` branch with local ONNX Piper TTS (`kiarashQ/fa-ir-tts-piper-ar-mantatts-v1`) coupled with HomoFast eSpeak G2P frontend and homograph disambiguation.
- **English Text-to-Speech**: English TTS continues to use `edge-tts` (`en-US-AvaNeural`).

### Removed
- **Voice Cloning**: Removed dead voice cloning mode, voice prompt recording, and associated UI strings/keyboards.


### Added
- **Social Media Platform Detection (`detect_social_platform`)**: Automatic classification of social media and messaging platform URLs (`telegram`, `instagram`, `tiktok`, `twitter`, `pinterest`, `facebook`, `threads`, `soundcloud`, `spotify`, `aparat`, `rubika`, `eitaa`).
- **YouTube Auto-Redirect**: Seamless routing of YouTube URLs received in direct chat or Surge downloader flow directly to `handle_youtube_url`.
- **Unsupported Platform Redirection & Tools Menu**: Automatic notice message (`surge.unsupported_platform`) and standalone Tools menu fallback (`send_tools_menu`) for unsupported social platform URLs.
- **Unified 3-Step Dispatcher Pipeline**: Restructured text message routing into a 3-step pipeline: IP lookup -> Social platform check -> Direct link download check.
- **Multi-Language i18n Support**: Added `surge.unsupported_platform` and `platforms.*` translations across all 4 supported languages (`fa`, `en`, `it`, `ru`).

## [2.0.0] — 2026-08-06

### Added
- **Text-to-Speech (MOSS-TTS-Nano)**: Local TTS engine (MOSS-TTS-Nano 100M) for natural speech synthesis.
- **Voice Cloning**: Support for custom voice cloning by sending a sample voice recording.
- **Live TTS Progress**: Real-time progress bar (percentage, elapsed time, ETA) during TTS processing.
- **Voice / Audio Fallback**: Auto-fallback to audio documents if Telegram privacy restricts voice messages.
- **Weekly TTS Quotas**: Tier-based weekly quotas (Dalavar/Sepahbod/Esfandyar: 30m, Sohrab: 100m, Rostam: 600m).
- **B&W Photo Colorization (DeOldify)**: Neural network image colorization for old black & white photos.
- **DeOldify Quotas**: Tier-based weekly quotas (Dalavar/Sepahbod/Esfandyar: 3, Sohrab: 15, Rostam: 100).
- **Background Removal (FeyNobg)**: Automatic image background removal with transparent PNG output.
- **FeyNobg Quotas**: Tier-based weekly quotas (Dalavar/Sepahbod/Esfandyar: 3, Sohrab: 30, Rostam: 150).
- **File Compression (ZIP / 7Z / RAR)**: Multi-format archive builder with 3 algorithms (LZMA2, PPMd, BZip2).
- **Compression Customization**: Compression levels 0–9 and archive splitting from 5 MB to 2 GB.
- **Multi-Media Compression Intake**: Support for document, video, audio, photo, voice, video_note, and animation.
- **Admin Broadcast System**: Full admin broadcast panel supporting `copy_message`, `forward_message`, and message pinning.
- **Rate-Limited Broadcast**: Throttled broadcast speed to 15 msg/sec (67ms delay) with automatic blocked-user detection (`is_blocked = true`).
- **Subscription Shop & Rank Guide**: Detailed rank pricing page (`rank:shop`, `rank:guide`) with direct prefilled admin contact links and `config.yml` pricing config.
- **Referral Leaderboard**: Top referrers leaderboard with medals (🥇, 🥈, 🥉) and database join queries.
- **YouTube MP3 Audio Download**: `audio_only` MP3 extraction (`--extract-audio --audio-format mp3 -q 0`) with smart filename and metadata caption.
- **Live Denoise Progress & Video Denoise**: Real-time progress bar (ETA) and MP4/MKV/WebM video audio denoise via FFmpeg CPU broker integration.
- **Database Migrations**:
  - `V002`: Added `duration` and `bitrate` columns to `stats_downloads`.
  - `V003`: Added `is_blocked` column to `stats_users`.
  - `V004`: Added `username` column to `stats_users`.
- **i18n Expansion**: Complete multi-language support across 4 languages (`fa`, `en`, `it`, `ru`) with 1,600+ new i18n key lines.

## [1.24.4] — 2026-07-29

### Fixed
- Reduce the YouTube rate-limit safety cooldown from four hours to one hour.
- Tell users the downloader may remain unavailable for up to one hour while cookie refresh can recover it earlier.

## [1.24.3] — 2026-07-29

### Fixed
- Show users a clear temporary-unavailability message when the entire YouTube cookie pool fails.
- Notify the admin with cookie/Gmail diagnostics when all profiles are unavailable, throttled to one alert per 30 minutes.

## [1.24.2] — 2026-07-29

### Fixed
- Add the missing localized `rank.panel_title` key required by the TestAPI and deploy validation.

## [1.24.1] — 2026-07-29

### Fixed
- Escape the English start-menu hyphen for Telegram MarkdownV2.
- Reject malformed, credential-bearing, shell-like, and private direct-download URLs.
- Sanitize downloaded filenames and prevent Telegram file paths/tokens from entering logs.

## [1.20.0] — 2026-07-29 (۱۴۰۴-۰۵-۰۸)

### Added
- Enterprise-grade CI/CD pipeline (GitHub Actions: fmt, clippy, test, security audit, testapi suite)
- Dependabot configuration for automatic Cargo dependency updates
- 105+ unit tests across 33 modules
- 10 new TestAPI endpoints: youtube, stt, separation, gwm, admin, surge, health, rank, referral
- `src/validation.rs`: SSRF prevention with `sanitize_url`, `is_safe_url`, `sanitize_text_input`
- Multi-stage Dockerfile (builder: rust:1.82-bookworm, runtime: debian:bookworm-slim) with `HEALTHCHECK`
- Docker Compose configuration (bot + postgres:16-alpine + redis:7-alpine)
- Architecture documentation (`docs/architecture.md`) with Mermaid component diagram
- Operations runbook (`docs/ops-runbook.md`)
- Architecture Decision Records: structured-logging (ADR-004), prometheus-metrics (ADR-005),
  rate-limiting (ADR-006), graceful-shutdown (ADR-007)
- Module `//!` doc comments for core modules (`rank`, `cookie_pool`, `redeem`, `denoise`, `admin`, `emoji`, etc.)
- Per-feature Prometheus metrics (`youtube_downloads_total`, `stt_requests_total`, `pdf_compress_total`, `separation_requests_total`, `gwm_requests_total`)

### Security
- SSRF prevention: private ranges 127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16,
  169.254.0.0/16 blocked in URL validation & `ip_lookup` HTTP requests
- Added `.env`, `*.pem`, `*.key`, `cookies.sqlite` to `.gitignore`
- Handled error logging in `youtube/handle.rs` via `record_error_global`
- Fixed potential `unwrap()` panics in production handlers (`gemini_watermark`, `youtube/selection/panel`, `moebius/detect`)

---

## [1.19.0] — (previous release)

_See git commit history for earlier versions._
