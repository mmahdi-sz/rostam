# Changelog

All notable changes to this project will be documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)  
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html)

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
