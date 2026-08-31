# Changelog

All notable changes to this project will be documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)  
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html)

## [2.5.6] - 2026-08-31

### Added
- **Dedicated YouTube Playlist Caption & Clean Preview Layout (`src/youtube/format.rs`, `src/youtube/handle.rs`, `config/i18n.json`)**:
  - Implemented specialized playlist preview card layout displaying playlist icon (`🗂`), title, channel name, video count (`🔢`), view count, and direct link.
  - Eliminated empty missing fields (`-` for duration, likes, date) for playlist URLs.
  - Added `playlist_label` and `video_count_label` across all 4 supported languages (`fa`, `en`, `it`, `ru`).
  - Streamlined playlist UX by skipping redundant individual description text chunk messages.
  - Added unit test coverage `test_build_caption_playlist` verifying playlist caption structure.

## [2.5.5] - 2026-08-31

### Fixed
- **Playlist Fast Flat-Extraction & Symlink Local Storage Path Resolution (`src/youtube/fetch.rs`, `src/bot/files.rs`)**:
  - Added smart conditional `--flat-playlist` in `fetch_video_info` for playlist URLs (`list=` / `/playlist`), preventing yt-dlp 60-second timeouts on large playlists while maintaining full quality format extraction for single videos.
  - Resolved `file path outside allowed local directory` errors on user media uploads by canonicalizing both `allowed_prefix` and `file_path` across symlinked storage paths (`/mnt/data` -> `/data`).

## [2.5.4] - 2026-08-31

### Fixed
- **YouTube Single Video Full Format Extraction & Deno JS Challenge Solver (`src/youtube/fetch.rs`, systemd/env)**:
  - Removed erroneous `--flat-playlist`, `--ignore-no-formats-error`, and `youtubetab:skip=authcheck` flags from `fetch_video_info`, ensuring yt-dlp extracts complete playable video stream formats instead of only storyboard preview images.
  - Resolved Deno JS runtime challenge solving across production and dev services, preventing false-positive cookie rotation cycling and spurious cookie exhaustion alerts (`cookie_retry_exhausted`).

## [2.5.3] - 2026-08-29

### Fixed
- **Direct YouTube URL Extraction in Text/Caption Dispatcher (`src/app/dispatch/text.rs`, `src/app/dispatch/flow.rs`, `src/youtube/extract.rs`)**:
  - Fixed message dispatcher to extract embedded YouTube links directly from multi-line text and media captions (`extract_youtube_urls`) instead of failing on non-URL text wrapper strings in `detect_social_platform`.
  - Added fallback to `message.caption` for forwarded posts and media items containing YouTube, Spotify, or SoundCloud URLs.
  - Added unit test `test_extract_from_multiline_persian_post` verifying accurate extraction of `youtu.be` links from multi-line Persian posts with query parameters.

## [2.5.2] - 2026-08-29

### Fixed
- **YouTube Members-Only Video Detection & False Alarm Prevention (`src/youtube/fetch.rs`, `src/youtube/handle.rs`, `config/i18n.json`)**:
  - Implemented early detection for subscriber-only and members-only YouTube videos via `"availability": "subscriber_only"` / `"premium_only"` in yt-dlp JSON extraction.
  - Added specialized `FetchError::MembersOnly` error variant and updated `classify_ytdlp_stderr` to detect member-restricted error patterns across yt-dlp versions.
  - Prevented member-restricted videos from exhausting the entire Firefox cookie pool and falsely triggering global admin outage alerts (`cookie_retry_exhausted`).
  - Added localized `youtube.members_only` user notice across all 4 supported languages (`fa`, `en`, `it`, `ru`).
  - Added unit test coverage (`test_classify_members_only`) for members-only error parsing.

## [2.5.1] - 2026-08-23

### Added
- **YouTube Playlist Multi-Cookie Rotation & Resilient Retry (`src/youtube/download/playlist.rs`, `src/youtube/fetch.rs`)**:
  - Implemented automatic per-item cookie rotation and retry loop for YouTube playlist downloads (`download_single_playlist_item_with_retry`).
  - Added centralized yt-dlp error classifier (`classify_ytdlp_stderr` / `YtdlpErrorClassification`) detecting HTTP 429 (`RateLimited`), Age Restrictions (`AgeRestricted`), Expired/Invalid Sessions (`BadCookie`), and Members-Only videos (`MembersOnly`).
  - Videos encountering age restrictions or invalid sessions now automatically cycle through non-cooldown Firefox profiles in `CookiePool` instead of failing the playlist.
  - Successfully validated cookies are preserved across subsequent items to minimize rotation overhead.
  - Integrated asynchronous job cancellation (`_cancel.notified()`) directly inside the playlist download loop.
- **Deno JS Runtime System Integration**:
  - Added global `/usr/local/bin/deno` symlink and execution permissions ensuring `yt-dlp` resolves YouTube signature solving and `n` challenges reliably across all services.

## [2.5.0] - 2026-08-19

### Added
- **Force Join Test Coverage Completed (Phases 1-4, `src/force_join.rs`, domain `fj`)**: Brought the mandatory channel/group membership enforcement subsystem from 0% to comprehensive test coverage across four structured phases:
  - **Phase 1 (Pure Unit Tests)**: Added 9 unit tests covering the membership access matrix across all 6 Frankenstein `ChatMember` status variants (`Creator`, `Administrator`, `Member`, `Restricted` allowed; `Left`, `Kicked` blocked), chat ID & public link parsing (`@username`, `-100...` numeric IDs, `t.me/` URLs, private invite link detection), `Lock` struct predicates (mandatory, expired, display name fallback chain), Persian/Arabic digit normalization (`to_en_digits`), Jalali date formatting (`fmt_jalali_dt`), and admin UI keyboard/view builders.
  - **Phase 2 (Redis & Lua Integration Tests)**: Added 4 integration tests (`test_lock_crud_lifecycle`, `test_cache_status_lua_transitions`, `test_mandatory_locks_filtering`, `test_cache_status_concurrent_calls_no_lost_updates`) validating atomic Redis hash storage, 3-state Lua counter transitions (`pending` ↔ `already` ↔ `linked`), active mandatory filtering, and high-concurrency race protection (20 parallel workers with zero lost updates).
  - **Phase 3 (TestAPI Endpoints)**: Built 5 TestAPI endpoints (`/test/fj/gate`, `/test/fj/admin/menu`, `/test/fj/admin/locks`, `/test/fj/admin/manage`, `/test/fj/admin/toggle_mode`) calling real handlers with simulated Frankenstein mock interception, integrated into the automated `scripts/run_testapi_suite.sh` matrix.
  - **Phase 4 (Trivial Helper Documentation)**: Formally documented test coverage strategies and intentional exclusions of trivial wrappers (`no_preview`, `send_text_np`, `edit_text_np`) and key formatters transitively exercised by Phases 1-3.
- **Fail-Open Resilience Regression Guard (`test_membership_check_fail_open_on_api_error`)**: Added dedicated regression test and architectural documentation verifying the two-layer force-join security design: fail-closed at lock configuration time (`toggle_lock_mode` gating on bot admin rights) and fail-open at check time (`Err(_) => true` on Telegram Bot API errors). Prevents future regression that would cause total bot denial-of-service during Telegram API disruptions or channel administrative changes.
- **Decoupled Telemetry Batch Flusher (`src/stats/`)**: Implemented non-blocking Tokio MPSC telemetry queue for `record_event_user` and `record_error_global` with a background flusher task inserting batches every ~250ms, decoupling high-frequency stats collection from user-facing database connection checkout.
- **Hardsub Subtitle Burner (`src/studio/burn.rs`, domain `studio_burn` / `stb`)**: Added hardsub video burning tool inside Photo & Video Magic Studio (`studio.burn_button` "حک زیرنویس روی ویدیو" 🎬) for embedding subtitle files (`.srt`, `.ass`/`.ssa`, `.vtt`) directly onto video streams (`ffmpeg`).
- **Native Style Preservation & Forced Style Fallbacks**: Preserves native ASS/SSA embedded styling (`-vf ass='sub.ass'`), while converting WebVTT to SRT and applying default forced style parameters (`-vf subtitles='sub.srt':force_style=...`) for SRT/VTT files.
- **Flexible Order Ingest & Background Ingestion**: Supports flexible input sequence (video first or subtitle first) with automatic background video ingestion upon video receipt, and seamlessly replaces subtitle files on-the-fly when updated.
- **Cancellable Live Progress Ticker & Re-Arming**: Integrated ffmpeg `-progress pipe:1` live ticker parser (rendering percent, speed, and ETA), cancellation button (`stb:jobcancel`), RAII workdir cleanup (`TempDirGuard`), and Studio menu re-arming.
- **CPU Broker Gating & Esfandyar Paywall**: Gated hardsub burning at `Rank::Esfandyar` (`block_feature`), protected with CPU Broker (`acquire_cpu`, `release_cpu`, `is_user_cpu_busy`), core pinning (`pin_current_thread`), and memory trimming (`trim_memory()`).
- **Subtitle & Audio Stream Extractor (`src/studio/extract.rs`, domain `studio_extract` / `strex`)**: Added stream extraction tool inside Photo & Video Magic Studio (`studio.extract_button` "جداسازی زیرنویس و صدا" 🎵) for pulling embedded audio tracks and subtitle streams directly out of video containers lossless without re-encoding (`ffmpeg -c copy`).
- **Stream Discovery & Native Format Mapping (`probe_media_streams`)**: Integrated `ffprobe` stream enumeration to detect codecs, languages, and titles, automatically mapping streams to native file formats (`.srt`, `.ass`, `.vtt`, `.sup`, `.sub`, `.m4a`, `.mp3`, `.flac`, `.opus`, `.ogg`, `.ac3`, `.dts`, `.eac3`, `.thd`, `.wav`).
- **Playable Audio & Document Delivery (`AsyncTelegramApiMetered`)**: Delivered extracted subtitle files as Telegram Documents (`send_document_metered`), playable audio formats (MP3, M4A, FLAC, OPUS, OGG) as Telegram Audio (`send_audio_metered`), and raw audio streams as Telegram Documents.
- **Stage-Aware Live Ticker & Real Cancellation**: Implemented live stage status ticker (`Downloading`, `Probing`, `Extracting`, `Uploading file X of Y`), real cancellation button (`strex:jobcancel`) via `ACTIVE_STUDIO_JOBS`, RAII directory cleanup (`TempDirGuard`), and Studio flow re-arming.
- **CPU Broker Gating & Concurrency Control**: Gated stream extraction via CPU Broker (`acquire_cpu`, `release_cpu`, `is_user_cpu_busy`) and automatic memory trimming (`trim_memory()`).
- **Universal Download Ticker & Ticker Keyboard Routing (`src/studio/pipeline.rs`)**: Updated `spawn_download_ticker` with `get_job_cancel_keyboard(domain_prefix)` to dynamically attach domain cancel keyboards and render localized download detail metrics (`{domain_prefix}.status_downloading_detail`).
- **Programming & Tech Cafe Start Menu (`start:dev_cafe`)**: Created dedicated developer & sysadmin sub-menu in the `/start` menu (`start.dev_cafe_button` ☕️) and moved Package Converter (`tools:pkg`) under it across all 4 supported languages (`fa`, `en`, `it`, `ru`).
- **Universal Package Format Converter (`pkgconvert`)**: Introduced multi-format Linux package conversion (`.deb` ↔ `.rpm` ↔ `.pkg.tar.zst`) accessible via the **Programming & Tech Cafe** menu.
- **Pacman Package Post-Processing Module (`src/pkgconvert/pacman_fix.rs`)**: Introduced atomic `.pkg.tar.zst` archive post-processing: splits `pkgver` hyphens into valid Pacman syntax (`pkgver = X` and `pkgrel = Y`), strips redundant inner shebangs (`#!/usr/bin/env sh`), and removes unnecessary `sudo` calls from `.INSTALL` scriptlets.
- **Default Terminal Installation Command Prompts**: Surfaced default terminal installation commands (`sudo dpkg -i`, `sudo rpm -i`, `sudo pacman -U`) formatted inside ` ```bash\n{cmd}\n``` ` code blocks in output captions across all 4 supported languages (`fa`, `en`, `it`, `ru`).
- **Rust-Native Package Pre-Validation (`src/pkgconvert/validate.rs`)**: Implemented pre-extraction security pipeline checking Magic Bytes (`!<arch>\n`, `0xEDABEEDB`, `0x28B52FFD`), enforcing a 500 MB decompressed size cap, 200 MB single file cap, 10,000 max entry count, and validating paths/symlinks against Path Traversal and symlink escape attacks.
- **Bubblewrap Subprocess Sandboxing (`src/pkgconvert/engine.rs`)**: Isolated conversion tools (`alien` for `deb ↔ rpm` and `fpm` for `pacman` conversions) inside a `bwrap` sandbox with `--unshare-all`, `--unshare-user`, `--unshare-pid`, `--uid 65534`, `--gid 65534`, `/var/tmp` tmpfs, read-only mounts (`/usr`, `/bin`, `/lib`, `/lib64`, `/var`, `/etc`), and custom environment variables (`PATH`, `SHELL=/bin/bash`, `HOME=/tmp`, `LANG=C.UTF-8`).
- **One-Shot Installer Dependency Automation (`install.sh`)**: Added system package dependencies (`alien`, `rpm`, `bubblewrap`, `libarchive-tools` for `bsdtar`, `ruby`, `ruby-dev`) and automatic `fpm` gem installation to `install.sh`.
- **Live Status Ticker & Cancellable Pipeline (`src/pkgconvert/handle.rs`)**: Added real-time progress stage ticker (`Downloading`, `Validating`, `Converting`, `Uploading`), 3s status updates, real cancellation button (`pkg:jobcancel`) with quota refunding and temp directory cleanup, and automatic flow state re-arming.
- **CPU Broker Integration & Multi-Rank Quotas (`src/rank/types.rs`, `src/rank/quota.rs`)**: Integrated conversion workers with CPU Broker (`acquire_cpu` / `release_cpu` / `is_user_cpu_busy`). Added daily quota tracking (`QuotaKind::PkgConvertDaily`) gating usage to Sepahbod rank (5/day) and Esfandyar/Rostam (20/day).
- **Multi-Language Localization & Admin Stats (`config/i18n.json`, `src/admin/mod.rs`)**: Added full localized string blocks across all 4 supported languages (`fa`, `en`, `it`, `ru`), and registered `pkgconvert`, `studio_extract`, and `studio_burn` features under `F_FILES` in the Admin Stats Panel.
- **TestAPI Endpoint Matrix (`src/testapi/endpoints/`)**: Added `/test/pkg/validate`, `/test/pkg/convert`, `/test/pkg/ux`, `/test/studio/extract`, and `/test/studio/burn` endpoints, integrated into `scripts/run_testapi_suite.sh` matrix suite.
- **Oversized Hardsub Output Is Split Instead of Rejected (`split_video_into_parts`, `upload_part_count`)**: An output above the `MAX_UPLOAD_BYTES` (2000 MB) Telegram ceiling is now cut into `ceil(bytes / cap).max(2)` roughly equal pieces by duration and sent as consecutive parts, instead of failing with `error.oversized`. The cut is a stream copy (`-c copy -map 0 -f segment -segment_time <total/parts> -reset_timestamps 1`), so there is no second re-encode and quality is unchanged; it runs on `spawn_blocking` off the CPU Broker because a remux is I/O-bound. Part count is derived rather than fixed at two — halving a 5000 MB output would leave each half unsendable. Cancel is re-checked before the split and between part uploads, cuts land on keyframes so every piece is re-checked against the cap (a piece still over it falls back to `error.oversized`), and each part carries its own `ffprobe` metadata and extracted thumbnail. Added `studio.burn.status_splitting` and `studio.burn.job_done_part` (`Part X of Y`) in all 4 languages (`fa`, `en`, `it`, `ru`).

### Changed
- **App Dispatcher Decoupled & Monolith Split (`src/app/dispatch/`)**: Deconstructed the largest 3,406-line "God File" (`src/app/dispatch.rs`) into a clean, 4-file domain structure under `src/app/dispatch/`:
  - `mod.rs` (322 lines): Gating entry hub implementing `handle_update` and `gate_force_join` (DEV_MODE, service message filtering, rate-limiting, referral attribution hook, language gate).
  - `text.rs` (363 lines): `handle_message` containing command dispatch, `/start` deep-linking (`redeem<CODE>`, referral payloads), and direct text/URL link routers.
  - `flow.rs` (809 lines): `handle_flow_message` handling all 17+ active `FlowState` match arms (admin tools, AI lab media inbounds, archives, video studio, emoji workflows).
  - `callback.rs` (1,979 lines): `handle_callback` routing all inline glass button interactions across all 34 subsystems.
  - Pure refactor with zero behavior changes, validated via a 4-step incremental extraction process with full compilation and test suite verification (224 unit tests + TestAPI suite).
- **PostgreSQL Connection Pool Migration (`deadpool-postgres = "0.14"`)**: Migrated global database architecture from a single static `tokio_postgres::Client` (previously wrapped in `Arc` with `unsafe` pointer casts and `Box::leak` in `startup.rs` and `log.rs`) to `deadpool_postgres::Pool`. Implemented a layered adapter pattern preserving all existing `&Client` query signatures while managing a bounded pool of 16 (dev) / 24 (prod) connections against PostgreSQL's `max_connections = 100`.
- **Atomic Multi-Step Database Transactions**: Wrapped 5 critical multi-step database mutations in `client.transaction()`: gift code redemption + rank activation, referral point claims + rank activation, default emoji pack switching, emoji dictionary bulk import/replacement, and the expired redeem code sweeper to eliminate partial-state corruption on process interruptions.
- **Cookie Pool Database Module Decoupling (`src/database/cookie_pool.rs`)**: Isolated cookie-pool-specific persistence queries out of the monolithic database module into a dedicated submodule.
- **Unified Job Cancellation Infrastructure (`src/common/job.rs`)**: Consolidated cancellation registries across all 13 active processing modules into `JobRegistry<K, V>` and `JobGuard<K, V>` (`spotify`, `soundcloud`, `denoise`, `pkgconvert`, `deoldify`, `musicset`, `stt`, `upscale`, `moss_tts`, `pdfcompress`, `filecompress`, `youtube`, and `studio`):
  - **6 Concurrency Pitfalls Resolved**: Replaced raw `.lock()` with panic-recovering `lock_or_recover`, eliminated early-return unregistration leaks via RAII `JobGuard`, enforced sequential memory consistency (`Ordering::SeqCst`), retained cancellation tokens in registry until guard drop for late-arriving callbacks, supported specialized notification types (YouTube `u64 -> Arc<Notify>`), and eliminated premature unregistration caused by tuple discards by separating `register()` and `register_with_guard()`.
  - **Shared Studio Domain Registry**: Coordinated video trimming, hardsub burning, stream extraction, and compression under a single domain-wide cancellation registry (`ACTIVE_STUDIO_JOBS`), enforcing 1 concurrent Studio job per user.
  - **Pre-Migration Test Gate Convention**: Established standing protocol requiring unit or TestAPI regression tests before migrating cancellation behaviors.
- **Shared Processing Abstractions & Duplication Elimination (`src/common/`)**: Replaced hand-rolled boilerplates across all 16 processing modules (`feynobg`, `youtube`, `surge_dl`, `spotify`, `soundcloud`, `deoldify`, `moss_tts`, `denoise`, `stt`, `upscale`, `pdfcompress`, `pkgconvert`, `filecompress`, `musicset`, `separation`, `studio`) with 4 centralized, deadlock-safe abstractions:
  - **`CpuBrokerGuard` (`src/common/cpu_broker.rs`)**: Standardized CPU Broker slot checkout, thread CPU affinity pinning (`moebius::cpu::pin_current_thread`), and anti-spam concurrency gating (`CpuBrokerGuard::is_user_busy`). Implemented leak-proof RAII `Drop` with fire-and-forget background release fallback, backed by server-side `RESERVE_TTL` (15m) and sweeper safety in `separation.service`.
  - **`ProgressTicker` (`src/common/ticker.rs`)**: Standardized periodic Telegram status ticker loops with text deduplication (preventing `Bad Request: message is not modified` errors), seamless `Arc<AtomicBool>` cancel flag integration, and MarkdownV2 premium emoji formatting.
  - **`ffmpeg.rs` (`src/common/ffmpeg.rs`)**: Standardized duration/metadata probing (`probe_metadata`, `probe_duration`) and 16kHz mono WAV transcoding (`convert_to_wav`) with deadlock-safe `Stdio::null()` and process pipe draining.
  - **`TempDirGuard` & `job_cancel_keyboard` (`src/common/dir.rs`, `src/common/keyboard.rs`, `src/common/format.rs`)**: Enforced RAII filesystem removal across all exit paths and unified cancel inline keyboard builders.
  - **"Don't Force-Fit" Architectural Principle**: Preserved intentional domain-specific variations where standardizing would cause semantic drift (YouTube custom caption time formatting `0:45`, Surge DL external daemon byte-rate polling loop, Moss TTS WAV→Opus voice-note transcoding).
  - **Technical Debt Elimination**: Resolved the largest duplication category from the original ~6,100-line repository audit with zero test regressions verified across 248 unit tests and the full TestAPI endpoint suite.
- **Hardsub Duration Cap Raised to 2 Hours (`MAX_BURN_DURATION_SECS`)**: Raised from 3600 s to 7200 s. The `studio.burn.error.too_long` message renders `{max}` from the constant, so no i18n change was needed.

### Fixed
- **Redis Connection Lifecycle Bug in Force Join (`src/force_join.rs`)**: Resolved a critical production failure where `force_join.rs` cached a live `MultiplexedConnection` in a static `OnceCell` instead of a reconnectable `redis::Client`. If the connection dropped or Redis restarted, the static handle became permanently dead, silently breaking all subsequent force-join operations (`add_lock`, `cache_status`, `check_lock_membership`) until a manual service restart. This belongs to the exact same failure class as the PostgreSQL static-client bug fixed earlier in this cycle. Uncovered directly because real integration tests were written for a previously-untested (0% coverage) module, and fixed by caching only the `redis::Client` and opening a fresh connection per call (matching the existing correct pattern in `src/bot/transfer.rs`). Verified by running all 4 integration tests together in a single process, both sequentially (`--test-threads=1`) and concurrently.
- **Subprocess Stdio Pipe Deadlocks (5 locations)**: Resolved potential Linux 64KB OS pipe buffer exhaustion deadlocks across subprocess invocations by redirecting unused output streams to `Stdio::null()` in Spotify (`handle.rs`), SoundCloud download (`handle.rs`), PDFCompress (`handle.rs`), and Surge DL rar/add operations (`handle.rs`), and implementing active concurrent stderr draining via background thread in Package Converter (`pkgconvert/engine.rs`).
- **Gift/Redeem Code Double-Spending Concurrency Verification**: Audited and confirmed atomic CTE row-level locking and `PRIMARY KEY (code, user_id)` constraints with 12-task parallel concurrency stress tests, proving resilience against race conditions during simultaneous code redemptions.
- **Hardsub Re-Encoded Every Source With libx264, Inflating AV1/HEVC Output Past The Upload Cap (`video_encoder_args`)**: The burn always ran `-c:v libx264 -preset medium -crf 22`, ignoring the probed source codec. Because AV1 is roughly 2-3x more efficient than H.264 at equal quality, a 900 MB / 2402 kbps AV1 input came back as an H.264 file over the 2000 MB ceiling and was rejected — while the failure message still showed the *input's* `AV1 | 2402 kbps` metadata, hiding the codec swap. The encoder is now matched to the source: `av1` → `libsvtav1 -preset 9 -crf 32` (the same preset `studio/compress.rs` already uses for AV1), `hevc`/`h265` → `libx265 -preset medium -crf 26`, `vp9` → `libvpx-vp9 -crf 32 -b:v 0 -row-mt 1`, anything else → the previous x264 settings. Each CRF is on its own encoder's scale, roughly equivalent to x264 CRF 22. `-pix_fmt yuv420p` is now forced only on the x264 path, so a 10-bit AV1/HEVC source is no longer silently downconverted to 8-bit.
- **Hardsub Burner Audit — 21 Bugs (`src/studio/burn.rs`)**: Full correctness pass over the hardsub burning flow.
  - **Subtitle files ingested as video**: `is_video_message_metadata` returns `true` for any unknown document, so a `.srt`/`.ass` upload was downloaded as the background video. `handle_input_message` now checks `detect_subtitle_format` **before** the video guess.
  - **English localization missing**: the entire `en.studio.burn` subtree and `en.studio.*_button` keys had been pasted inside `en.admin.tts`, so English users saw Persian fallbacks. Moved to `en.studio`, and added `error.download_failed`, `error.too_long`, `error.oversized` to all 4 languages (`fa`, `en`, `it`, `ru`).
  - **ffmpeg filtergraph escaping**: paths and `force_style` were shell-escaped (`'\''`), which ffmpeg's own filtergraph parser does not understand — a path containing `:` or `,` split the filter. Replaced by `escape_filter_value` (single backslash for `\ ' : , ; = [ ]`) and the shared `build_filter_arg` builder.
  - **User filename in paths and filters**: inputs are now copied to fixed work-dir names (`input.<ext>`, `sub.<ext>`); the raw name survives only as an `md_escape`d, `sanitize_filename`d caption.
  - **Abandoned session and cancel leaks**: `stb:cancel` now calls `burn::abort_session` (stop ticker, set cancel flag, drop the `ACTIVE_STUDIO_JOBS` entry, remove the work dir), and entering the prompt aborts any previous burn session instead of orphaning its temp directory.
  - **Duplicate video ingest**: a second video mid-download left two tickers editing one message. First video wins; duplicates are logged and ignored.
  - **Silent download failures**: a failed video fetch left the user staring at a dead ticker. Errors now stop the ticker, call `record_error_global`, render `studio.burn.error.download_failed`, and re-arm the prompt.
  - **Ticker never completing / never shown**: the download ticker's stop flag is now set on every exit path, and the job starts through a single `try_claim_job` claim so it can never run twice regardless of input order.
  - **Cancel blocked and zombie processes**: the `-progress pipe:1` reader moved to its own `std::thread` (a stalled ffmpeg no longer blocks the cancel check), the wait loop is `try_wait` + 300 ms poll, and the child is always `wait()`ed with the reader joined.
  - **ffmpeg stderr discarded**: stderr is written to `work_dir/ffmpeg.log` and its tail is logged on failure.
  - **Out-of-order ticker edits**: edits are coalesced through a `tokio::sync::watch` editor task, throttled by percent change or a 3 s minimum interval.
  - **CPU Broker gating in the wrong place**: `is_user_cpu_busy` is checked at execute time (minutes of uploading pass after the prompt), and the cancel flag is re-checked right after `acquire_cpu` returns so a queued cancel does not run the job anyway.
  - **Paywall skipped when the database is unavailable**: `enter_burn_prompt` now fails closed with `record_error_global` instead of granting the feature.
  - **Broken re-arm**: the re-arm path built a `FlowManager::new()` (a fresh empty map, so setting flow state did nothing) and error paths did not re-arm at all. All exit paths now re-arm the burn prompt with a fresh session through the real `FlowManager`.
  - **Missing output metadata, caps, and container mismatch**: output is always MP4 + AAC (`-movflags +faststart`), a thumbnail is extracted with `ffmpeg -vframes 1` and sent with `width`/`height`/`duration`, inputs longer than `MAX_BURN_DURATION_SECS` are rejected with `error.too_long`, and oversized outputs are handled by the split path below.
  - **i18n and stats correctness**: `sub_replaced` used `tf(key, &[])` for a template with no parameters; `record_error_global` was never called on handler failures; success/failure/cancel stats events were incomplete.
- **TestAPI `/test/studio/burn` no longer reimplements the feature**: the endpoint now calls the real `build_filter_arg`, `detect_subtitle_format`, `is_video_message_metadata`, `sanitize_filename`, `video_encoder_args`, `upload_part_count`, and `split_segment_secs`, and returns the routing decision, fixed work-dir names, both keyboards, all four error texts, the download/burn/upload ticker texts, the resolved encoder and its full argument list, the split plan (`split_needed`, `split_parts_planned`, `split_part_bytes_max`, `split_segment_secs`), `stats_events`, and the `trace` id. Added suite cases for `.srt` routing, filename sanitization plus non-shell-quoted filter args, the duration cap, source-codec-to-encoder mapping (including that AV1 never forces `pix_fmt`), and the split path (2402 MB → 2 parts each under the cap, 5000 MB → 3 parts, small output not split).
- **MarkdownV2 Package Extension & Codeblock Escaping (`pkgconvert`)**: Wrapped interpolated file extension strings (`.deb`, `.rpm`, `.pkg.tar.zst`) with `md_escape` across all Telegram message bodies and document upload captions, and added explicit newlines to ` ```bash\n{cmd}\n``` ` codeblocks to prevent MarkdownV2 syntax parse errors and titlebar leakage.
- **Callback Action Prefix Parsing (`handle_pkg_callback`)**: Updated callback handler to parse action strings directly (`deb:pacman`, `deb:rpm`) after prefix stripping by `dispatch.rs`, resolving silent callback drop on option clicks.

---

## [2.4.3] - 2026-08-12

### Fixed
- **Telegram MethodResponse Deserialization in Transfer Metering (`transfer`)**: Updated `send_params_metered` in `src/bot/transfer.rs` to fallback to deserializing `frankenstein::response::MethodResponse<R>` and returning `.result` when callers pass inner types directly (e.g., `Message` in `send_file_with_upload_ticker`). Fixes `err=missing field message_id` failures across 15+ processing modules (`separation`, `denoise`, `studio_trim`, `studio_compress`, `feynobg`, `deoldify`, `upscale`, `moss_tts`, `emoji`, `pdfcompress`, `filecompress`, `gemini_watermark`, `surge_dl`, `spotify`, `soundcloud`, `musicset`).

---

## [2.4.2] - 2026-08-12

### Added
- **Universal Live Upload Ticker Engine (`send_file_with_upload_ticker`)**: Added real-time upload progress bars (`[●●●●●○○○○○]`), percentage (`%`), upload speed (`MB/s` / `KB/s`), and remaining time (`ETA`) status tickers across all 14 bot modules (`pdfcompress`, `spotify`, `soundcloud`, `musicset`, `studio_trim`, `studio_compress`, `denoise`, `separation`, `moss_tts`, `deoldify`, `upscale`, `feynobg`, `gemini_watermark`, `filecompress`, `surge_dl`, `emoji`).
- **Byte-Accurate Telegram Transfer Metering**: Introduced `AsyncTelegramApiMetered` trait (`src/bot/transfer.rs`) providing real-time upload progress tracking (`send_document_metered`, `send_video_metered`, `send_audio_metered`, `send_voice_metered`, `send_photo_metered`) across all 25 upload locations in the codebase.
- **Adaptive Speed & ETA Calibration**: Implemented EMA-backed speed caching in Redis (`load_ema_bps` / `update_ema_bps`) to self-calibrate transfer stage ETAs across Telegram server environments.
- **Multi-Language Upload Stage Titles**: Added `transfer.stage.sending_audio`, `transfer.stage.sending_video`, `transfer.stage.sending_photo`, and `transfer.stage.sending_document` localized stage titles across all 4 supported languages (`fa`, `en`, `it`, `ru`).
- **PDF Compression Live Ticker & Flow Re-Arm (`pdfcompress`)**: Added live progress ticker and cancellation button (`pdf:jobcancel`), automatic re-arming of `FlowState::AwaitingPdfCompressFile` and prompt re-sending on completion to maintain an uninterrupted user experience.
- **File Compression Glass Ingest Keyboard (`filecompress`)**: Attached inline glass buttons ("Finish upload" `fc:done` and "Cancel" `fc:cancel`) directly under file receipt messages (`📥 دریافت شد: ...`) instead of reply keyboards.
- **Studio Trim Smart Timestamp Extractor (`studio_trim`)**: Implemented Regex scanner (`TIMESTAMP_RANGE_RE`) in `src/studio/trim.rs` to extract timestamp ranges (e.g., `00:00:00 - 02:00:00`, `01:15 ~ 05:30`) embedded anywhere in arbitrary long text, descriptions, or captions.

### Fixed
- **PDF Compression Output Filename (`pdfcompress`)**: Updated Ghostscript output path in `src/pdfcompress/handle.rs` to append `_lite` before `.pdf` extension (e.g., `document_lite.pdf`).
- **File Compression Output Send Error (`filecompress`)**: Fixed `خطا در ارسال فایل خروجی` by checking file size limits (<2GB) before upload and formatting raw caption templates prior to `apply_premium_to_md`.
- **Transfer Progress i18n Placeholders (`transfer`)**: Restructured flat `transfer` keys into nested JSON objects in `config/i18n.json` across all 4 languages (`fa`, `en`, `it`, `ru`), resolving raw unparsed placeholders `!transfer.stage.sending_video!` and `!youtube.progress_eta_label!`.
- **YouTube Status Ticker Speed & ETA Fallback**: Enhanced YouTube download status ticker (`src/youtube/download/progress.rs`) with `parse_bytes_str` fallback to compute speed and ETA when yt-dlp stdout reports `"Unknown"` / `"N/A"`.
- **Upload Cancellation Stream Interruption**: Fixed chunk stream cancellation in `send_params_metered` using `and_then` stream mapping to immediately abort local file reads when `cancel` flag is signaled.
- **Transfer Snapshot ETA & Speed for Completed Transfers**: Fixed ETA display (`"—"` when total is 0) and average speed reporting (`bytes_done / total_elapsed`) when stage reaches `Stage::Done`.

---

## [2.4.1] - 2026-08-11

### Added
- **Admin Stats Panel Coverage (`studio_trim` & `studio_compress`)**: Registered Video Trim (`studio_trim`) and Video Compression (`studio_compress`) features in admin stats panel `F_FILES` array (`src/admin/mod.rs`) and added `admin.stats.names.studio_compress` i18n keys across all 4 supported languages (`fa`, `en`, `it`, `ru`), surfacing full usage metrics under the **`فایل‌ها` (Files)** section.
- **End-to-End Transfer Speed & Byte Tracking**: Added real-time transfer tracking (`TransferResult` in `src/bot/files.rs` and `UploadResult` / `timed_send` in `src/bot/upload.rs`) measuring download and upload byte throughput, elapsed duration, and speed across all 13 processing modules (`stt`, `feynobg`, `deoldify`, `upscale`, `gemini_watermark`, `pdfcompress`, `denoise`, `spotify`, `soundcloud`, `moss_tts`, `studio`, `filecompress`, `separation`).
- **Transfer Database Analytics & Metrics**: Added DB migration `V006__add_transfer_speed_and_feature_to_stats.sql` adding `feature`, `download_speed_bps`, `upload_speed_bps`, and `file_count` columns to `stats_downloads`, and registered Prometheus counters/histograms (`bot_transfer_bytes_total`, `bot_transfer_speed_bytes_per_second`, `bot_transfer_files_total`).
- **Live Upload Speed Display**: Added real-time upload speed estimation (`MB/s` / `KB/s`) to YouTube download status ticker (`format_upload_body`) and multi-language i18n keys (`transfer.*`) across all 4 supported languages (`fa`, `en`, `it`, `ru`).
- **Pre-Download Video Metadata Validation (`studio`)**: Added zero-latency pre-download video metadata validation (`is_video_message_metadata`) in `src/studio/mod.rs` shared by Video Compress (`studio_compress`) and Video Trim (`studio_trim`). Inspects `mime_type` (`video/`) and file extensions (`mp4`, `mkv`, `avi`, `mov`, `webm`, `flv`, `wmv`, `m4v`, `3gp`, `ts`, `mts`) directly from Frankenstein's `Message` update **before** executing `getFile` or creating temporary directories. Non-video file uploads return early localized errors (`studio.compress.error.not_a_video` and `studio.trim.error.not_a_video`) across all 4 languages (`fa`, `en`, `it`, `ru`) without network download overhead.
- **Uncompressed Document Support for Video Compression**: Updated `src/app/dispatch.rs` and `src/studio/compress.rs` to accept video files sent as Telegram `Document` (file mode) in addition to standard `Video` messages.
- **Studio Live Download Progress Ticker**: Added real-time download status ticker (`spawn_download_ticker` in `src/studio/pipeline.rs`) for Video Trim (`studio_trim`) and Video Compression (`studio_compress`) updating status messages every 2 seconds with elapsed time, downloaded volume (`MB`), total volume (`MB`), progress percentage (`%`), live download speed (`MB/s` / `KB/s`), and estimated remaining time (`ETA`) across all 4 supported languages (`fa`, `en`, `it`, `ru`).
- **Adobe Premiere Animation Custom Emoji (`adobe_pr_animasion`)**: Added custom emoji ID `5352591263084330931` (`adobe_pr_animasion`) and `🎥` unicode mapping to `EMOJI_MAP` (`src/i18n/emoji_map.rs`), replacing standard movie/clapper emojis across Studio start menu buttons, header titles, section sub-prompts, and live status tickers in all 4 supported languages (`fa`, `en`, `it`, `ru`).
- **Video Trim Custom Scissors Emoji (`scissors`)**: Added missing `scissors` custom emoji ID (`5237808360882977239`) to `icons` in `config/i18n.json` across all 4 languages (`fa`, `en`, `it`, `ru`), resolving missing custom emoji icons on the Video Trim inline button.


### Fixed
- **YouTube 0 Video Formats Bad Cookie Fallback**: Added explicit 0 video formats detection check (`video_formats.is_empty()`) when cookies are enabled in `src/youtube/fetch.rs`. Automatically flags degraded or challenged cookies as `FetchError::BadCookie`, causing the handler loop in `src/youtube/handle.rs` to immediately rotate to the next active cookie profile in the Cookie Pool instead of returning 0 downloadable qualities to the user.
- **Video Compression Initial Codec Matching**: Updated `src/studio/compress.rs` to automatically match the initial selected target compression codec to the original video codec (`AV1`, `H.265`/`HEVC`, `VP9`, `H.264`) detected by `ffprobe` upon upload, ensuring uploaded AV1, HEVC, and VP9 videos default to their native target compression codec.
- **FFmpeg Trimming & Progressive Streaming (`faststart` & `-avoid_negative_ts`)**: Added `-movflags +faststart` to all MP4 outputs in `src/studio/trim.rs` and `src/studio/compress.rs` (moving the `moov` atom to the front of the file for progressive streaming in Telegram), added `-avoid_negative_ts make_zero` seeking alignment, and added explicit `copy_success` vs `copy_failed_fallback_encode` trace logging.
- **Document Mode Video Validation (`is_video_message_metadata`)**: Expanded `is_video_message_metadata` in `src/studio/mod.rs` to support video files sent in Telegram Document (file mode). Accepts `application/octet-stream`, `binary/octet-stream`, `application/x-matroska`, all video file extensions (`.mp4`, `.mkv`, `.avi`, `.mov`, `.webm`, `.flv`, `.wmv`, `.m4v`, `.3gp`, `.3g2`, `.ts`, `.mts`, `.m2ts`, `.vob`, `.ogv`, `.qt`, `.f4v`, `.asf`, `.rm`, `.rmvb`, `.mpg`, `.mpeg`, `.mpe`, `.mpv`, `.divx`, `.xvid`, `.m2v`, `.264`, `.h264`, `.265`, `.h265`, `.hevc`, `.av1`), as well as generic Document uploads, preventing false `not_a_video` rejection errors.
- **Button Label Text Sanitization**: Removed static unicode emojis (`✂️`, `🎥`) from inline glass button label texts (`trim_button`, `compress_button`) across all 4 languages in `config/i18n.json` to prevent duplicated emoji rendering alongside Telegram premium button icons.
- **YouTube Quality Paywall Guard**: Added resolution rank limit checks (`user_rank.max_yt_quality()`) upfront in `handle_go` (selection confirmation) and `run_download` (background downloader task), with DB client fallback (`stats::get_db_client()`) in `handle_resolution_callback`. Ensures unauthorized resolutions immediately trigger `block_limit` paywall without sending false "starting download" status messages or spawning background `yt-dlp` jobs.
- **User-Facing Error Sanitization & Admin Guidance**: Eliminated all raw English/system error leaks (`{error}`, `e.to_string()`, ffmpeg traces, yt-dlp stderr, stack traces) across `youtube`, `redeem`, `feynobg`, and `i18n.json`. Standardized all critical failure templates across all 4 languages (`fa`, `en`, `it`, `ru`) to deliver helpful user-facing guidance and direct support contact info (`@mmahdi_sz`).
- **Live Upload Speed i18n Placeholder**: Fixed `upload_body` template in `i18n.json` across all 4 languages (`fa`, `en`, `it`, `ru`) by inserting missing `{speed}` placeholder to surface real-time upload speed (`MB/s` / `KB/s`) in status tickers.
- **Russian Upscale i18n Parameter Alignment**: Resolved parameter mismatches in `upscale` module for Russian (`ru`) translation: added missing `{scale}` placeholder to `quota_weekly_limit` and cleaned stray placeholders from `preparing` to align with Rust `t()` / `tf()` signatures.
- **CPU Broker Retry Logic (`acquire_cpu`)**: Implemented a 3-attempt retry loop with 3-second delay across `moebius/cpu.rs` and `filecompress/handle.rs` to handle transient connection failures during `separation.service` restarts.
- **FileCompress Flow Re-Arm**: Automatically re-arms flow state (`FlowState::AwaitingCompressFiles`) and re-sends prompt message on all error exit paths (`mkdir_failed`, `download_failed`, `timeout`, `compress_failed`) to prevent stranding the user.
- **FileCompress Menu Telegram Error Filter**: Suppressed Telegram API 400 `message is not modified` error logs in `show_options_menu` when user interactions result in unchanged menu configurations.
- **Spotify Log Noise Suppression**: Wrapped `api_fetch_failed_trying_public_fallback` error log in a `SPOTIFY_CLIENT_ID` presence check in `spotify/client.rs` to eliminate false-positive error logs when using zero-auth embed fallback by design.
- **Cookie Refresh Sidecar Log Clean-up**: Restricted `src_not_found` warning logs in `cookie_refresher.rs` strictly to the primary `cookies.sqlite` database, skipping normal absence of ephemeral `-wal` and `-shm` sidecar files after Firefox process exit.


## [2.4.0] - 2026-08-10

### Added
- **Advanced Video Compression (`studio_compress`)**: Added new inline button "فشرده‌سازی پیشرفته ویدیو" to the Photo & Video Magic Studio (`studio`) menu.
- **Multi-Codec & Auto-Container Selection**: Interactive UI for selecting video codecs (`H.264`, `H.265`, `VP9`, `AV1`). Automatically outputs `.mp4` for H.264 and `.mkv` for H.265, VP9, and AV1.
- **Dynamic Resolution & FPS Filtering**: Hides resolutions and FPS options higher than the source video's original properties.
- **6-Tier Bitrate Controls**: Bitrate ratio selection across 2 rows (`1`, `3/4`, `2/4`, `1/4`, `1/6`, `1/8`) displaying calculated numeric bitrates in English (`XXXX kbps`).
- **SVT-AV1 v4.1+ Integration**: SVT-AV1 preset set to `9` (`-preset 9`) for AV1 compression, using standalone BtbN static FFmpeg build with `libvmaf` support.
- **VMAF Quality Scoring (`libvmaf`)**: Calculates VMAF score after compression via `compute_vmaf_score` using FFmpeg's `libvmaf` filter, appended to job completion captions with RTL formatting (`\u200e`).
- **Localized Time Formatting (`format_eta_hms`)**: Formats elapsed time and ETA in human-readable localized units (`X ثانیه و Y دقیقه و Z ساعت`) across all 4 languages (`fa`, `en`, `it`, `ru`).
- **Telegram Document (File) Delivery**: Compressed output is delivered as a Telegram Document via `send_document` to prevent server-side re-compression. Output filename formatted as `<original_stem>_<CODEC>.<ext>` (e.g., `video_AV1.mkv`).
- **Redis Session Storage**: Compression session state stored in Redis under `studio_comp_session:{user_id}` with 1h TTL.
- **Automatic Re-Arm & Flow Continuation**: Automatically sends a new prompt message and restores `FlowState::AwaitingStudioCompressVideo` upon job completion.
- **TestAPI Endpoint `/test/studio/compress`**: Fully tested via `scripts/run_testapi_suite.sh` covering container selection, filtering, AV1 preset 9, VMAF scoring, and localized duration strings.
- **CPU Concurrency Limit Enforcement**: Added `is_user_cpu_busy` guard checks across all AI and processing features to prevent users from queuing multiple concurrent CPU-intensive jobs, immediately returning an error feedback.
- **Photo & Video Magic Studio (`studio_trim`)**: Top-level inline button "استودیو جادوی عکس و ویدئو" in `/start` menu leading to the media editing studio subsystem (`src/studio/`).
- **Multi-Range Video Trimming & Editing**: Supports multi-segment timestamp cut inputs (`HH:MM:SS` & `MM:SS`) on single or multiple lines, Persian/Arabic-Indic digit normalization (`۰-۹` / `٠-٩` → `0-9`), and whitespace-tolerant dash separator.
- **Video Trimming Enhancements**: Single outer live ticker with ETA display, clamped caption timestamps, cover art thumbnails via `ffmpeg -vframes 1`, and job completion summary message.
- **«رنک رایگان» Button in the `/rank` Shop**: A blue glass (`ButtonStyle::Primary`) row sitting directly below «خرید از ادمین» with the `fire1` premium icon and callback `user:panel:referral`.
- **`referral.banner` Restyled Like `/start`**: Three plain section titles (`🌐` downloader, `🧪` AI Lab, `🧰` toolbox) each followed by its own `<blockquote expandable>` body.
- **Admin Stats Split into 9 Navigable Section Pages**: Overview, Users, YouTube, AI, Music, Files, Money & Plans, System, Errors — all rendered into the same message via `admin::render_section` + `admin::stats_keyboard`.

### Changed
- **Inline Glass Button Cleanliness**: Removed literal static emojis (`❌`, `🚀`, `✂️`) from button labels in `config/i18n.json` across all 4 languages to prevent duplicate emoji rendering alongside custom premium icons (`btn_icon_danger` / `btn_icon_success`).
- **Admin Tree Navigates in One Message**: Errors page and gift-code panel edit in place (`bot::edit_text_html`, `redeem::handle::open_panel_edit`).
- **Stats Queries Batched**: `get_feature_stats_multi` and `get_action_breakdown_multi` fetch every feature of a section in one round-trip.
- **Rank Shop & i18n Translations**: Updated per-rank feature blurbs across all 4 languages to include CPU compression limits, Spotify/SoundCloud capabilities, and exact numerical caps.
- **Referral Rule Simplified**: User point credited immediately upon force-join channel membership confirmation (`referral::confirm_on_join`).
- **Unified HTTP Client Module**: Consolidated 5 separate redundant `reqwest::Client` instances across different subsystems into a shared, lazy-loaded `crate::http::client()` module to optimize resource usage.

### Fixed
- **Instant Subprocess Kill on Job Cancellation**: `stc:jobcancel` handler issues `child.kill()` and `child.wait()` on non-blocking `ffmpeg` subprocesses, releasing CPU broker affinity, freeing memory via `trim_memory()`, and re-arming the Studio menu (`send_studio_menu_new_msg`).
- **CPU Broker Bypasses**: Resolved cases where tasks like `denoise` bypassed the CPU broker, ensuring proper CPU core reservation, thread pinning, and memory trimming via `release_cpu` instead of raw blocking spawns.
- **`/start` Expandable Blockquotes**: Syntax fixed per `docs/markdownv2.md` (`**>` on first line, `>` on following lines, `||` on last line).
- **`/start` Emojis & i18n Completeness**: Re-mapped missing emojis (`🎥`→`🎞️`, `🗣️`→`🎤`), completed Italian and Russian downloader sections, and added missing guide i18n keys across all 4 languages.
- **File Compression Showed "Compressing 0%"**: Fixed status ticker stage rendering, parsed real percent from 7z/rar stdout, and calculated accurate ETA.
- **`getFile` Hang Timeout**: Capped Telegram file downloads at 600s (`GET_FILE_TIMEOUT_SECS`) to prevent unbounded hangs.
- **Memory & CPU Broker Improvements**: Added `MALLOC_ARENA_MAX=2` systemd drop-in and `moebius::cpu::trim_memory()` calls to lower idle RSS. Brokered Vosk STT and DeepFilterNet denoise under CPU Broker.
- **Referral Attribution**: Credited referrals immediately upon channel join confirmation, preventing lost attributions for new users.

### Removed
- **Rank Guide Removed**: Removed unused and oversized `rank.guide` screen to stay within Telegram's UTF-16 message limits.

---

## [2.4.0] - 2026-08-09

### Added
- **Admin Stats Split into 9 Navigable Section Pages**: the single wall of text (and the separate "📊 آمار بیشتر" page) is replaced by an inline-button hub — Overview, Users, YouTube, AI, Music, Files, Money & Plans, System, Errors — all rendered into the *same* message via `admin::render_section` + `admin::stats_keyboard(current)`, with the current section's button styled as active. Callback data follows the project format: `admin:s:{section}` (`CB_ADMIN_SECTION`), replacing `admin:stats_more` and `admin:errors_1d`.
- **Missing Features Now Tracked in the Panel**: Spotify, SoundCloud, Album/Playlist (`musicset`), PDF compression, direct-link download (`surge_dl`), background removal, photo colorization, TTS, IP lookup, ranks/purchases, referrals, cookie pool and broadcast all have stats blocks — previously only stt/denoise/upscale/separation/gwm had one. Each block shows 1d/7d/30d success, 30d failures, a success rate, processing time for timed features, and its `(action, status)` breakdown (top 8 rows).
- **Overview Alerts**: the hub flags every feature whose 30-day success rate is under 80% (with ≥5 samples) and the 24-hour error count, so a broken feature is visible without opening its page.
- **`/test/admin/stats_section` TestAPI Endpoint**: calls the real `render_section` + `stats_keyboard`, returns the rendered text, `html` flag, nav labels/callbacks and whether the current section is marked. Suite covers all 9 sections plus an unknown-key failure path.
- **`/test/rank/guide` TestAPI Endpoint**: renders the real `rank.guide` through `apply_premium_to_html` (the exact path `rank::menu` uses) and cross-checks every number in it against the `Rank` methods that enforce it — `compress_cpu_daily_secs`, `compress_cpu_monthly_secs`, `music_set_limit`. The suite fails unless `mismatches` is empty, so a guide line promising a limit the code doesn't grant can no longer ship.

### Changed
- **Admin Tree Navigates in One Message**: the 24-hour errors page and the gift-code panel used to send *new* messages, orphaning the panel behind them. Both now edit in place — errors through the new `bot::edit_text_html` helper (`apply_premium_to_html` + `ParseMode::Html`, keeping the expandable blockquote), gift codes through `redeem::handle::open_panel_edit`.
- **Stats Queries Batched**: `get_feature_stats_multi` and `get_action_breakdown_multi` fetch every feature of a section in one round-trip instead of one query per feature (was 8 + 5 round-trips for the old two pages). The single-feature `get_feature_stats` / `get_action_breakdown` are deleted.
- **"Top Feature" No Longer Reports `paywall`**: system events (`paywall`, `cpu`, `cookie`, `broadcast`, `referral`) are excluded from the top-feature query via the new `NON_FEATURE_EVENTS` list — a paywall block is not a feature use.
- **Broadcast Pin Toggle Survives Menu Entry**: the pin state rides in the callback data (`broadcast:toggle_pin:{0|1}`, `broadcast:mode:{copy|forward}:{0|1}`) instead of the flow state, which `admin:broadcast` cleared on every entry, silently resetting the toggle to off.
- **Rank Shop Now Lists Every Paid Capability**: the per-rank blurbs named four AI tools and stopped there — the file-compression CPU quota (`compress_cpu_daily_secs` / `compress_cpu_monthly_secs`, 10 min/day for Dalavar up to 200 min/day for Rostam) was never mentioned anywhere, and neither were the Spotify/SoundCloud downloaders: single-track download (free on every rank), album/playlist caps (`music_set_limit` — off for Dalavar/Sohrab, 20 tracks for Sepahbod, unlimited for Esfandyar/Rostam) and the 7z archive delivery (`can_music_set_archive`, Esfandyar/Rostam only). Four lines added per rank to `rank.features.*` in all four languages, plus the background-removal, colorization and text-to-speech lines that were missing from some of them.
- **Rank Numbers Are Digits, Not Words**: Esfandyar/Sohrab/Rostam spelled their upscale quotas out ("پانصد", "پنجاه") while Dalavar/Sepahbod used digits in the same list.
- **Russian and Italian Rank Text Rewritten from Scratch**: the Italian and Russian per-rank blurbs listed 4–5 lines where Persian listed 13–18, silently hiding traffic, YouTube quality, subtitles, transcription and separation limits from those users; the Russian text was partly still English. All four languages are now generated from one table of the `Rank` values, so every rank carries identical line counts across languages. Italian also spelled numbers out ("cinquecento") and was missing the playlist cap and the burned-in-subtitles line for Esfandyar.
- **Referral Rule Simplified to "start the bot + join the channel = one point"**: the invited user's point is now credited the moment force-join membership is confirmed, by `referral::confirm_on_join` — a single atomic statement that moves the `referral_pending` row into `referrals` (`DELETE … RETURNING` feeding an `INSERT … ON CONFLICT (referred_id) DO NOTHING`). Leaving the channel afterwards keeps the point; re-joining never grants a second one; one point per user, enforced by the `referred_id` primary key rather than by application logic.
- **Removed the 2-Day Referral Confirmation Wait**: `referral::sweep_confirm`, the `PENDING_DAYS` constant, and the hourly `spawn_referral_confirm_sweeper` job are gone. `referral_pending` is now only a short-lived stash for the link payload of users who have not joined yet.
- **Translated All Persian Code Comments to English**: all Persian comments and doc-comments across 60 Rust files in `dev/src/` (800+ lines) were translated into terse, clean English comments adhering to `CLAUDE.md` Hard Rule 6. Zero logic or string literal changes were made.

### Fixed
- **File Compression Showed "Compressing 0%" While It Was Still Downloading, and Never Moved**: the status message rendered `fc.processing` from the moment the job started, with a hardcoded `░░░░░░░░░░ 0%` — the only live value was the elapsed clock. A user on production sat at "compressing 0%" for minutes while the local Bot API was still fetching their video: `/tmp/filecompress_65/` was empty, no `7z` process existed, and `acquire_cpu` had not been reached, so nothing was compressing at all. Three separate causes, all fixed: (1) the new `filecompress::progress::JobProgress` carries the stage, so the download phase renders its own `fc.downloading` text with "file 2 of 3" instead of pretending to compress; (2) the engine now pipes the archiver's stdout and parses its percent (`-bsp1 -bso0` for 7z/ZIP, rar's own ticker — both verified against real output to emit `NN%`), so the bar actually fills; (3) with a real percent, `progress::eta_secs` extrapolates the remaining time and `fc.processing_eta` prints it — measured from when compression started, not from when the job was queued, so the download and broker wait do not skew it. tar/zstd report no percent, so they keep the elapsed-only text rather than a fabricated ETA. The ticker also skips edits that would not change the text, which Telegram rejects anyway.
- **`getFile` Could Hang Forever**: in `--local` mode the Bot API server downloads the whole file from Telegram before answering, and `download_telegram_file` awaited that with no timeout — the unbounded wait behind the frozen status message. Now capped at 600 s (`GET_FILE_TIMEOUT_SECS`), after which the job fails through the normal path: quota refunded, work dir removed, `fc.error.download_failed` sent. The download loop also logs `download_done` with the byte count, so the trace no longer goes silent between files.
- **Idle RSS Stayed at the High-Water Mark of the Heaviest Job**: production sat at 700 MB RSS (653 MB anonymous, 5.4 GB VSZ) after 17 hours, spread over ~30 mostly-empty 64 MB glibc arenas — one per thread of the 35-thread runtime. Nothing was leaking: no ONNX session or Vosk model is cached in a static, `stt::vosk::transcribe` loads and drops its model (97–205 MB) per call. The memory was freed-but-not-returned, parked in the allocator against a next run that comes once or twice a day (STT 22 events/30d, denoise 9, separation 58, watermark 42), so the cache bought nothing. Two changes: `MALLOC_ARENA_MAX=2` via a `20-memory.conf` systemd drop-in on both `rostam` and `rostam_dev`, and a new `moebius::cpu::trim_memory()` (`malloc_trim(0)`, `target_env = "gnu"`) called from `release_cpu` — so every CPU-broker job trims on exit — plus two explicit calls on the paths that bypass the broker: after the Vosk `spawn_blocking` in `stt::handle` and after the remote separation call in `separation::handle`, where the track is held in RAM twice (read buffer plus multipart copy). Peak usage during a job is unchanged; only the idle floor drops.
- **STT Ran Its Two Heaviest Stages Outside the CPU Broker**: both Vosk transcription and DeepFilterNet denoise were raw `tokio::task::spawn_blocking` with no core reservation, so a transcription competed for CPU against every brokered job (colorize, upscale, watermark, compression) instead of queueing behind them — a violation of the >500 ms rule in `CLAUDE.md`. Both stages now `acquire_cpu` before the blocking task, `pin_current_thread` from inside it (threads Vosk and DeepFilterNet spawn inherit the affinity), and `release_cpu` after — which also performs the trim, replacing the standalone `trim_memory()` call. Since the broker can block up to 120 s, the cancel flag is re-checked after acquiring: a user who cancels while queued gets the cores released, the quota refunded (`cancelled_before_transcribe`) and the work dir cleaned. The ffmpeg WAV conversion stays unbrokered — it is a short subprocess, not in-process inference.
- **`fmt_secs` Unit Test Asserted a Persian Zero** that the function never returns; the stats panel deliberately keeps English digits.
- **Album/Playlist Paywall Named the Wrong Rank**: blocking a Dalavar or Sohrab user from a Spotify/SoundCloud set told them Esfandyar was required, while `music_set_limit` grants Sepahbod 20 tracks — the cheapest rank that unlocks the feature. The paywall now names Sepahbod, matching the guide.
- **Referral Attribution Was Silently Dropped for Every New User**: production had 1228 users, 0 rows in `referrals`, and 0 in `referral_pending` — no invite had ever been credited. A brand-new user opening `https://t.me/{bot}?start={referrer_id}` hit the language gate in `dispatch_update`, which sends the language picker and returns *before* `handle_message`, where attribution lived; the force-join lock returned early too. By the time the user reached `handle_message` the `/start` payload was gone and `is_new_user` was already false, so the deep link could never be credited. Attribution now runs at the top of `dispatch_update`, ahead of both gates, and the "brand-new user" check is `stats::user_seen(uid)` (direct `stats_users` lookup) instead of the `record_user` return value, which has not run yet at that point. Past invites cannot be back-filled — their payloads were never stored.

### Removed
- **Rank Guide Removed**: `rank.guide` had grown past Telegram's 4 096-character limit (5 277 in Persian, 5 822 in Italian), so every tap on «دیدن قابلیت رتبه‌ها» came back `Bad Request: message is too long` and the user saw nothing change. Rather than paginate a screen that duplicated the shop, the whole guide is gone: its per-rank capability lists were the same `rank.features.{rank}` strings the shop detail screen already renders — and the shop shows live prices from `prices.json` on top. Deleted: `CB_RANK_GUIDE`, `send_rank_guide`, its callback branch, the «دیدن راهنما» button, the `/test/rank/guide` endpoint with its suite block, and the `rank.guide` / `rank.view_guide` keys in all four languages. The quota-reset note that only lived there is gone with it.

## [2.3.0] - 2026-08-08

### Added
- **Spotify Single-Track Downloader (`sp`)**: Automatically detects Spotify track links (`open.spotify.com/track/{id}`, `open.spotify.com/intl-{lang}/track/{id}`, `spotify:track:{id}`). Uses `rspotify` API with zero-auth public embed fallback for metadata extraction. Downloads and transcodes to 320kbps MP3 with high-res cover art and ID3v2.4 tags (`TIT2`, `TPE1`, `TALB`, `APIC`). Includes live status ticker, cancellation (`sp:cancel`), CPU Broker integration, flow re-arming, and TestAPI endpoints (`/test/sp/download_track`, `/test/sp/cancel`).
- **SoundCloud Single-Track Downloader (`sc`)**: Automatically detects SoundCloud track links (`soundcloud.com/{artist}/{track}` and `on.soundcloud.com/{short_code}`). Extracts metadata, cover artwork, and audio via `yt-dlp` native SoundCloud extractor. Transcodes to 320kbps MP3 with ID3 tags, live progress ticker, cancellation (`sc:cancel`), CPU Broker integration, flow re-arming, and TestAPI endpoints (`/test/sc/download_track`, `/test/sc/cancel`).
- **Music Set — Shared Album/Playlist Queue (`ms`)**: one queue behind both downloaders for Spotify albums/playlists and SoundCloud sets. `try_route_set` runs before single-track detection (otherwise yt-dlp collapses a whole playlist into one `track.mp3`), each track goes through its own platform's single-track path, and pending offers are keyed `(user_id, offer_message_id)` so a second link cannot hijack the first. Per-rank caps live in `Rank::music_set_limit()` (Dalavar/Sohrab 0, Sepahbod 20, Esfandyar/Rostam unlimited) and `Rank::can_music_set_archive()`. Failure paths distinguish private/deleted from invalid link and always re-send the start menu. TestAPI: `/test/musicset/*`.
- **ZSTD (`tar.zst`) as a Fourth File Compress Format**: new `ZSTD` button next to ZIP/7Z/RAR, levels 1–19 (`--ultra` deliberately excluded). zstd is a stream compressor, not an archiver, so the engine runs `tar -I 'zstd -{level} -T{threads}' -cf archive.tar.zst …` — a single child process, so the existing cancel loop, `taskset` core pinning and `getrusage` CPU accounting keep working unchanged. Because zstd has no encryption and no multi-volume support, `CompressFmt::{max_level, supports_password, supports_solid, supports_split}` now drive the keyboard: the password, split and solid rows are *removed* for zstd rather than shown disabled, stale callbacks from an older message are rejected, and switching format clears any incompatible setting — a leftover password would otherwise silently produce an unencrypted archive. `fc.welcome_zstd` says so explicitly in all four languages. TestAPI: `/test/compress/ux` reports `has_password_button` / `has_split_button` / `has_solid_button` and `welcome_text`; the suite asserts all three are false for zstd with `max_level` 19, and still true for 7z.
- **Live Elapsed Timer + Cancel Button on Colorize and File Compression**: both status messages were static — `deoldify.preparing` and `fc.processing` printed `00:00` / `0s` once and never updated, and neither carried a cancel button. Each job now registers an `AtomicBool` in a per-user registry (the STT pattern) and a 2 s / 3 s ticker re-renders the elapsed clock as `mm:ss`. Pressing انصراف flips the flag: the result is discarded and the reserved quota refunded (`cancelled_mid_job`). Compression also checks the flag between file downloads and before handing work to the CPU broker, so a cancel before the engine starts costs nothing.
- **Separation Progress Shows Elapsed and Estimated Remaining**: `separation.progress` replaces the old fire-and-forget "queued" / "still busy" edits with one 5 s ticker showing elapsed time plus an ETA of 3× track length (fast) or 5× (quality). The completion message (`separation.done_report`) reports track duration and total operation time.
- **Max-Compression-Level Toast**: pressing «+» at the format's ceiling now answers the callback with a transient toast (`fc.max_level_notice` — "حداکثر فشرده سازی فرمت {fmt} مقدار {max} است"). RAR caps at 5, ZIP/7Z at 9, ZSTD at 19.
- **Cancel Button and Text Guard on the Archive-Password Step**: the ask-password screen had no way out, and sending a file there fell through to the file-intake path. It now carries an انصراف button, and non-text input answers with `fc.password_need_text` instead of being ingested.
- **`/test/compress/ux` TestAPI Endpoint**: dumps the real `options_keyboard` (button text, callback data, custom emoji id, `ButtonStyle` colour), the max-level toast, the ask-password and progress keyboards, the rendered progress text, and the re-entry prompt. `scripts/run_testapi_suite.sh` asserts the solid-mode colours both ways, the `fc:jobcancel` / `fc:cancel` callbacks, `mm:ss` formatting, and — as the failure path — that bumping RAR past level 5 clamps and fires the toast.
- **Cancel Button on the TTS Progress Message**: the synthesis progress message (bar + elapsed + ETA) had no `reply_markup` at all, so a job could not be stopped. It now carries a red انصراف button (`tts:jobcancel`) backed by a per-user `AtomicBool` registry; the flag is passed into `run_tts_engine`, which checks it inside the synthesis loop, releases the CPU broker slot, and returns without writing a file. The handler refunds the reserved weekly quota, deletes any partial output, and re-arms `AwaitingTtsText`.
- **`/test/tts/ux` and `/test/stt/ready` TestAPI Endpoints**: the first reports `TTS_MAX_CHARS`, whether a given length is rejected, the rendered `tts.text_too_long`, and the real progress / ask-text keyboards; the second renders `stt.ready_title` / `stt.ready_again` through the production path and reports the resolved model label plus the premium-emoji span count. The suite asserts `tts:jobcancel` with `danger` styling, that 501 characters are rejected with a resolved (non-`!key!`) message, and that the STT ready text contains premium emoji and never the raw `stt.language.*` key.

### Changed
- **Flows Re-Arm Instead of Dumping the User in a Menu**: after a transcription, separation, colorization, or compression finishes, the bot re-sends that feature's own prompt with the same settings (model, separation mode, compression config) rather than the AI-lab / tools menu, so the next file can go straight out. `send_ai_lab` is consequently unused and marked `#[allow(dead_code)]`.
- **Solid-Mode Button Colours**: «کل پوشه : سریع تر» is green (`btn_icon_success`) and «تک تک: مقاوم تر» is blue (`btn_icon_primary`); both were plain before.
- **Separation Mode Keyboard Is One Column**: high quality (green) → fast (blue) → back (red), instead of a two-button row.
- **Faster Ingest of Rapidly Forwarded Files**: `handle_fc_file` awaited two Telegram round-trips (per-file acknowledgement + prompt counter edit) inside the serialized update loop, so a burst of forwards was accepted one at a time. State is committed first and both messages are dispatched off the loop.

### Fixed
- **STT Announced the i18n Key as the Model Name**: the ready message read `مدل stt.language.en_big — آماده است.` — `SttConfig::label_key()` returns a key and it was interpolated without `t()`. It now renders the human label of the button the user actually pressed (e.g. «فارسی (سریع)»), `md_escape`d before interpolation.
- **STT Ready Message Had No Premium Emoji**: neither `stt.ready_title` nor `stt.ready_again` went through `apply_premium_to_md`, and neither edit set `parse_mode(MarkdownV2)`, so the emoji rendered plain. Both now do; the four language templates were escaped accordingly.
- **The 500-Character TTS Limit Was Never Enforced**: `tts.enter_text_default` promised «حداکثر ۵۰۰ کاراکتر» but nothing checked it — `chars().count()` was only used to estimate the duration for the quota reservation. A `TTS_MAX_CHARS = 500` guard now rejects longer input with `tts.text_too_long` (counted in characters, not bytes) and keeps the flow armed so a shorter text can be sent straight away.
- **Compression Cancel Did Not Stop the Work**: انصراف only discarded the finished archive — the `7z`/`rar` process ran to completion and burnt the CPU anyway. The cancel flag now reaches `run_compress`, which polls every 500 ms and `start_kill()`s the child. Replacing the single wrapping `tokio::time::timeout` also fixes the 30-minute timeout path, which used to drop the future and orphan the process; piped stderr is drained by a concurrent task so `wait()` cannot deadlock on a full pipe.
- **Header-Obfuscation Warning Never Reached the User**: the "needs a password first" alert passed the *action* string as `callback_query_id`, so Telegram rejected every one. The real query id is now threaded into `handle_fc_callback`, which also means dispatch no longer pre-answers `fc:` callbacks (Telegram accepts only one answer per query).
- **Flow Re-Arm Was Silently Wiped by the Clear Channel**: `flow_clear_tx` was drained at the *top* of the update loop, so a flow armed later inside a spawned task was erased. Spawned tasks now hold a cloned `FlowManager` and set state directly; the channel, its receiver, and the drain loop are deleted.

## [2.1.4] - 2026-08-07

### Fixed
- **File Compression Menu Never Opened (`ENTITY_TEXT_INVALID`)**: every click on the compression tool logged `show_options_menu_err … Bad Request: ENTITY_TEXT_INVALID` and the menu never rendered. `fc.welcome` / `fc.welcome_7z` ship pre-written custom-emoji spans (`![📂](tg://emoji?id=…)`), and `apply_premium_to_md` re-wrapped the emoji *inside* them, producing a nested `![![📂](tg://emoji?id=A)](tg://emoji?id=B)` that Telegram rejects. The function now copies an existing span through untouched.
- **Premium Emoji Rendered as Links, Not Emoji**: `apply_premium_to_md` emitted `\![…](tg://emoji?id=…)`. Verified against the Bot API: the escaped `!` makes Telegram return a `text_link` entity pointing at `tg://emoji?id=…` instead of a `custom_emoji` entity, so every emoji it wrapped showed up as a dead link. Now emitted unescaped, which the API confirms as `custom_emoji`.
- **Persian TTS Always Failed with "پردازش هوش مصنوعی با خطا مواجه شد"**: Piper's WAV is written by `hound` as 32-bit float mono, which ffmpeg reads back as `1 channels (FL)` — a layout `libopus` rejects (`Invalid channel layout 1 channels (FL) for specified mapping family -1`), so the Ogg conversion produced nothing (`Nothing was written into output file`) and every Persian request ended in `tts_failed`. The conversion now passes `-ac 1 -ar 48000`, making the layout unambiguous and matching Opus's native rate. English (edge-tts MP3) was unaffected and still works.
- **Missing eSpeak Data After a Partial `cargo clean`**: `espeak-rs-sys` ships its `espeak-ng-data` into `OUT_DIR`, and the path is baked into the binary. With that build directory pruned, startup logged `Error processing file '…/espeak-ng-data/phontab': No such file or directory` and Persian phonemization died before Piper ran. Rebuilt; no `espeak-ng-data` exists system-wide on this host, so the crate's copy is the only one.

### Changed
- **`/test/tts/generate` Calls the Real Engine**: it returned a hardcoded `ok: true` with an i18n caption and never touched `run_tts_engine`, which is why a completely dead Persian TTS passed the suite. It now runs the real synthesis + ffmpeg conversion and reports `output_ext` / `output_bytes` / `err`. `scripts/run_testapi_suite.sh` asserts an actual `ogg` for both Persian and English and that empty text fails.

## [2.1.3] - 2026-08-07

### Added
- **Google Play in the "Under Development" Platform List**: a Play Store link used to be treated as a direct download — production probed one and got `name=details size=0` (an HTML page), then offered it as a file. `play.google.com` / `play.app.goo.gl` now classify as the `playstore` platform, so the dispatcher answers with the `surge.unsupported_platform` notice plus the Tools menu. `platforms.playstore` added for `fa`/`en`/`it`/`ru`.

### Fixed
- **SIGSEGV Mid-Watermark-Removal**: production was killed twice on 2026-08-07 (`status=11/SEGV`, restart counter 3) inside the Moebius DDIM loop. `moebius::model::sessions` returned a `&'static Sessions` fabricated from a raw pointer into the holder's `Option<Sessions>`; the idle reaper freed it after `SESSION_IDLE_TIMEOUT` (120 s) counted from the *start* of the job, so any run longer than that had its ONNX sessions dropped mid-inference and dereferenced freed memory. The sessions are now an `Arc<Sessions>` the running job owns, so an unload only drops the holder's handle — no `unsafe` left in that path. The other ONNX engines (`feynobg`, `deoldify`) hold the holder lock across inference and were never affected.
- **Unbrokered Moebius Job Ran Single-Threaded**: when `/cpu/acquire` timed out (three times in the same window; the broker holds all cores for up to 120 s under load), `threads` fell to `cores.len().max(1)` = 1, stretching a ~25 s run to ~115 s — which is what pushed it past the session idle timeout into the crash above. The fallback is now 2–4 threads.
- **Two Glued Links Broke Playlist Downloads**: two URLs pasted without a space arrive as a single whitespace token, so the second one's scheme was appended to the first one's last query value and yt-dlp got `list=PL…gdhttps:` (`HTTP Error 400: Bad Request`, 4 recorded errors). `extract_youtube_urls` now cuts a token at an embedded `http(s)://` and keeps the first URL.
- **`logOut` Sent on Every Restart**: `build_bot_api` called the official Bot API's `logOut` on every single start when `BOT_API_BASE_URL` pointed at a local server. `logOut` is a one-time migration step and Telegram rate-limits it, so a restart loop burned the limit; worse, any response that wasn't `Logged out`/`Unauthorized` (a `429`, for instance) returned `Err` and failed startup, and with `Restart=always` each restart sent another `logOut`. It now runs once, records `files/.official_logout_done`, skips on subsequent starts, and never fails startup on a `logOut` error — a token still bound to the official API surfaces through `get_updates` against the local server instead.

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
