# CLAUDE.md

Rust Telegram bot `ros-telegram-bot` (crate `frankenstein`), runs as systemd service `abc`.
Single-language Farsi UI. Uses yt-dlp, local Bot API, optional PostgreSQL + Redis.

## Hard Rules (MUST FOLLOW)

1. **After every change**: `git add <files> && git commit -m "..."`, then restart the relevant
   service: `systemctl restart abc` (Rust bot) and/or `systemctl restart separation` (Python svc).
   Commit first, then restart.

2. **User-facing strings → `i18n.json`** (nested keys), read via `i18n::t("key")` /
   `i18n::tf("key", &[("name", val)])`. Operator/dev logs (`println!`, panics, journalctl) stay
   hardcoded — never put them in `i18n.json`.

3. **Tracing**: every non-trivial flow needs grep-friendly operator logs covering routing →
   handler → external calls → Telegram response. Use a stable trace id and structured lines:
   `[domain trace=N event=name] key=val`. Log routing inputs (user_id, chat_id, branch),
   function boundaries, external work (cmd/args/exit/retry/cookie), and Telegram ops. Never log
   secrets (tokens, raw cookies, DB URLs). Grep: `journalctl -u abc -n 300 | rg "trace|event"`.

## Build & Run

```bash
cargo build            # debug build is the runtime target
systemctl restart abc
journalctl -u abc -f
```

Binary: `target/debug/ros-telegram-bot`. Unit: `systemd/abc.service` → `/etc/systemd/system/abc.service`.

## Config (read order: `.env` → `/etc/default/abc` → process env)

- `BOT_TOKEN` (required)
- `DATABASE_URL` (optional PostgreSQL; without it Cookie Pool is in-memory only)
- `ADMIN_USER_ID` (optional; loads emoji cache from this user's DB, needed for `{key}` expansion)
- `BOT_API_BASE_URL` (optional local Bot API, e.g. `http://127.0.0.1:8081`; built via
  `Bot::new_url("{base}/bot{token}")`. If host is localhost, startup calls `logOut` on official
  API first via `Bot::new(token)` then switches.)

## Commands

`/start`, `/emoji`, `/se [id_or_name] [alias]` (alias `-` removes), `/cookie_status`,
`/cookie_next`, `/cookie_429`. YouTube URLs are auto-detected.

## Features (all implemented)

- **YouTube downloader**: URL → preview → quality/codec/audio/subtitle selection → yt-dlp →
  upload via local Bot API → cancel button. Files >2GB auto-split with ffmpeg `-c copy`.
- **Emoji panel** (`/emoji`): add/test/list/import-export-SQL/pack management + `{key}` premium
  emoji template system.
- **AI Lab**: STT (Vosk + DeepFilterNet3), noise removal (DeepFilterNet3), image upscale
  (Real-ESRGAN NCNN Vulkan), vocal separation (Python FastAPI + Kim_Vocal_2.onnx), Gemini
  watermark removal (gwt-mini binary).
- **Cookie Pool**: Firefox profile rotation for yt-dlp, auto-refresh every 6h, 429 handling.
- **CPU Broker**: Redis-based core reservation for heavy AI tasks.

## Emoji template `{key}` system

`{key}` placeholders in any text sent via `send_text()` expand from the global emoji cache.
Resolution order: exact smart_name → prefix group (`fire`→fire1,fire2…) → alias → item DB id →
raw 19-digit Telegram emoji id. Pack-scoped: `{pack:item}` (pack by name/alias/id). Cache loaded
at startup from `ADMIN_USER_ID`'s rows, refreshed every 5 min. Impl: `src/emoji/cache/`.

## Premium UI emoji

All UI emoji are premium custom emoji via `i18n.json`. IDs in `emoji.panel.icons.*`. Char→key map
in `src/i18n/emoji_map.rs` (variation-selector forms first). `send_text()` auto-converts known
chars to `CustomEmoji` entities (`src/i18n/entities.rs`). Inline buttons: `btn_icon(text, cb, key)`
in `src/emoji/panel/buttons.rs` (uses `icon_custom_emoji_id`). MarkdownV2 needs explicit
`apply_premium_to_md()` (`src/i18n/premium_md.rs`) — does NOT get entities automatically.
Add new: add ID to `emoji.panel.icons`, add `("🔥","key")` to `EMOJI_MAP`.

## Source Layout

```text
src/main.rs                  — mod declarations + app::run()
src/app/                     — mod (event loop), startup, dispatch (routing), state
src/config.rs                — env reading
src/bot.rs                   — send_text, send_text_md, send_start_button
src/cookie_pool/             — CookiePool + format helpers
src/modules/cookie_refresher.rs — Firefox profile refresh cycle
src/i18n/                    — mod (t/tf), emoji_map, entities, premium_md
src/youtube/                 — extract, fetch, format, handle, quality_keyboard, trace, types,
                               lang_names; selection/ (menu); download/ (runner, store, cancel,
                               progress, status, split, upload, helpers, etc.)
src/database/                — mod, posfreSQL/{postgresql.rs, schema.sql}
src/stt/                     — vosk, deepfilter, config, handle, types
src/emoji/                   — cache/, flow, handler/, panel/, store/, smart_name, import/
src/upscale/                 — handle
src/separation/              — client, handle, types, error
src/gemini_watermark/        — remove, handle
```

## Message Routing Order (`src/app/dispatch.rs`)

1. addemoji link detection (`t.me/addemoji/Pack`, not starting with `/`)
2. active flow handling (non-Idle state)
3. STT audio handling (`AwaitingSttAudio` + voice/audio/document)
4. command dispatch (`/emoji`, `/se`, `/start`, `/cookie_*`, YouTube URLs)

Messages starting with `/` skip step 1, so commands always reach dispatch.

## Subsystem notes

### YouTube
- Always pass yt-dlp `--js-runtimes deno:/root/.deno/bin/deno` (systemd PATH lacks it; YouTube
  may return only storyboards otherwise).
- A resolution is selectable only with a recognized video codec at that exact height. Codecs:
  `avc1`→H264, `hvc1`/`dvh1`→H265, `vp9`/`vp09`→Vp9, `av01`→Av1. Never infer lower qualities.
- Request store: `REQUESTS: HashMap<u64, YoutubeRequest>` (`download/store.rs`),
  store/get/take. Selection shared across clones via `Arc<Mutex<Option<Selection>>>`.
- Cancel: `ACTIVE_DOWNLOADS: HashMap<u64, Arc<Notify>>` (`download/cancel.rs`); `UnregisterGuard`
  ensures cleanup on every path. Progress edits attach `yt:cancel:{rid}` keyboard.
- Callback prefixes: quality `yt:q:{rid}:{height}`, cancel `yt:cancel:{rid}`, selection `yt:s:*`
  (codec `c`, audio `a`, subtitle `t`, submenu `sm/sb/sp`, confirm `go`).
- Output: `downloads/yt/{trace_id}/`, format `{format_id}+bestaudio/best` merged to mp4.

### Cookie Pool / refresh
- Firefox profiles discovered from `/home/mahdi/.mozilla/firefox` (max 20), cached in
  `cookie_profiles_cache/`. yt-dlp reads `cookies.sqlite` directly. Random selection excluding
  last-used + cooldown.
- Refresh every 6h, profiles 3-at-a-time parallel. Per profile: kill firefox → check login →
  open firefox (`sudo -u mahdi firefox --profile ...` with `DISPLAY=:10`,
  `XDG_RUNTIME_DIR=/run/user/1002`, X11) → open 3 random links from `files/youtube_links.txt` →
  wait up to 1h → copy cookies to cache.
- 429: `mark_last_rate_limited()` (4h cooldown safety net) → channel to event loop → 30-min task
  → per-profile refresh → re-add to pool.
- Logs: `journalctl -u abc -f | grep cookie_refresh`, format `[cookie_refresh profile=x event=y]`.
- Add profile: create Firefox profile + Google login, ensure `cookies.sqlite`, restart abc.

### Image upscale (Real-ESRGAN)
- Models: `realesrgan-x4plus` (default x4), `realesrgan-x4plus-anime` (x4),
  `realesr-animevideov3-x{2,3,4}`. UI: "عمومی x4" + collapsible "انیمه و کارتون ▼".
- State `AwaitingUpscaleImage { scale_factor, model_name, anime_expanded }` — all required.
- Callbacks: `upscale:model:{name}`, `upscale:anime_toggle`, `upscale:cancel`.

### Vocal separation
- Python FastAPI on port 6589 (`separation-service/`), model `Kim_Vocal_2.onnx`, one request at a
  time via asyncio.Lock, max 50MB. systemd unit `separation.service`.
- Setup: `bash separation-service/install.sh` then enable+start separation.
- Flow: audio → mode keyboard (quality/fast) → download → POST → returns base64 vocals +
  instrumental → two .wav sent. Callbacks: `sep:quality:{id}`, `sep:fast:{id}`, `sep:cancel:{id}`.
- Health: `curl http://127.0.0.1:6589/health`. Logs: `journalctl -u separation -f`.

### Gemini watermark removal
- Binary `files/runtime/gwt-mini` (v0.3.1). Base args:
  `-i {in} -o {out} --denoise telea --radius 25 --quiet --no-banner`.
- Multi-pass (max 3): pass 1 detection gate (threshold 0.25, retry `--legacy` on `[SKIP]`, both
  skip → NoWatermarkDetected); passes 2-3 residual cleanup (threshold 0.05, chained). All passes
  sent to user (trade-off: pass 1 preserves detail, pass 3 cleanest). Impl `src/gemini_watermark/`.
- Callbacks: `ai:gwm`, `gwm:cancel`. Logs: `journalctl -u abc -f | grep '\[gwm'`.

### CPU Broker (use for any multi-second CPU task)
- `separation-service/cpu_broker.py` (`acquire(user_id, is_vip)` → real core list,
  `release(cores)`), `cpu_monitor.py` (sliding-window /proc/stat). Pin with
  `os.sched_setaffinity(0, set(cores))` + `OMP_NUM_THREADS`. Release in `finally`.
- Redis: `cpu:reserved` (Hash), `cpu:queue` (Sorted Set, VIP priority), `cpu:notify` (pub/sub).
  Reservation TTL 15 min. Rust queue UX: 5-min silent wait msg → 30-min "under pressure" → timeout.

## PostgreSQL tables (auto-created when `DATABASE_URL` set)

Cookie pool: `cookie_pool_cookies`, `cookie_pool_state`, `cookie_pool_cooldowns`.
Emoji: `emoji_packs`, `emoji_items`. Schema: `src/database/posfreSQL/schema.sql`.

## Runtime deps (tracked under `files/`)

Vosk (`libvosk.so` + Persian/English models), DeepFilterNet3 (`deep-filter` binary +
`DeepFilterNet3_onnx.tar.gz`, extracted on first run), Real-ESRGAN (`realesrgan-ncnn-vulkan` +
models), gwt-mini binary, separation `Kim_Vocal_2.onnx`. build.rs links libvosk via
`files/runtime`. System pkgs: ffmpeg, libvulkan1 + mesa-vulkan-drivers, Python 3 + pip.

## Git server

`origin` → `git-server/ros-telegram-bot.git`, branch `master`.
