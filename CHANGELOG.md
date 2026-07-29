# Changelog

All notable changes to this project will be documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)  
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html)

---

## [Unreleased]

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
