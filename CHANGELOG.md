# Changelog

All notable changes to this project will be documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)  
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html)

---

## [Unreleased]

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
