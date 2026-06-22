# CLAUDE.md

> ## ⚡ Code analysis: use CodeGraph FIRST (saves tokens)
>
> This repo is indexed by the **codegraph** MCP server (SQLite knowledge graph of every
> symbol/edge/file). For understanding code, finding callers/callees, tracing impact, or
> locating where to edit — **prefer codegraph over reading/grepping whole files**. One call
> returns the verbatim source PLUS who calls it and what it affects → far fewer tokens and
> round-trips than reading files yourself.
>
> - **Before relying on it, sync the index** so it reflects the latest code:
>   `codegraph sync` (or `codegraph init` for the first build — `-i` flag is deprecated/no-op).
>   Run sync after you make edits, then query.
> - **Tools:** `codegraph_explore` (PRIMARY — natural-language question or a bag of symbol/file
>   names → verbatim source grouped by file; usually the only call you need),
>   `codegraph_search`, `codegraph_node` (one symbol's source + caller/callee trail),
>   `codegraph_callers`.
> - Fall back to raw Read/Grep only to confirm a specific detail codegraph didn't cover.
> - Index lives at `/mnt/data/mahdidev/ros/.codegraph`. If MCP shows "no tools", reconnect via
>   `/mcp` (startup-timing issue; the server is fine). Project is on `/mnt` so the file watcher
>   can be flaky — `--no-watch` in the MCP args + manual `codegraph sync` is the reliable combo.

Rust Telegram bot `ros-telegram-bot` (crate `frankenstein`), runs as systemd service `abc` (dev/test).
Single-language Farsi UI. Uses yt-dlp, local Bot API, optional PostgreSQL + Redis.

## Environments

| | Dev | Production |
|---|---|---|
| Dir | `/mnt/data/mahdidev/ros/dev` | `/mnt/data/mahdidev/ros/production` |
| Service | `abc.service` | `rostam.service` |
| Database | `ros_telegram_bot` | `ros_telegram_bot_production` |
| Binary | `target/debug/ros-telegram-bot` | `ros-telegram-bot` (copied from prod build) |
| Git branch | `dev` | — (receives deployed binary) |

Build repo for production: `/mnt/data/mahdidev/ros/prod` (tracks `github/master`)

## Deploy

```bash
sudo bash /mnt/data/mahdidev/ros/deploy.sh
```

مراحل داخل deploy.sh:
1. commit تغییرات uncommit در `dev`
2. merge `dev` → `master` در dev repo
3. push `master` به GitHub (`github` remote)
4. در `prod` repo: `git fetch github master && git reset --hard`
5. `cargo build --release` در `prod`
6. کپی binary به `/mnt/data/mahdidev/ros/production/ros-telegram-bot`
7. کپی `i18n.json` به `/mnt/data/mahdidev/ros/production/i18n.json`
8. `systemctl restart rostam`

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
- `REDIS_URL` (default `redis://127.0.0.1:6379`; used by the cookie freshness worker. **Shared
  between dev and prod** so cookies aren't refreshed redundantly.)
- `ENV_LABEL` (cookie refresh-lock owner; falls back to `dev`/`prod` via `DEV_MODE`).
- `COOKIE_REFRESH_ENABLED` (default true; `false` disables the cookie worker).
- `COOKIE_FRESH_TTL_SECS` (default 36h), `COOKIE_WORKER_INTERVAL_SECS` (default 10 min),
  `COOKIE_REFRESH_LOCK_TTL_SECS` (default 30 min) — see Cookie Pool / refresh.

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
  watermark removal (gwt-mini binary), ASR (Nemotron RNNT ONNX int4).
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
`apply_premium_to_md()`; HTML (`ParseMode::Html`, e.g. `rank.guide`) needs explicit
`apply_premium_to_html()` (wraps emojis in `<tg-emoji emoji-id=...>` tags, skips tag contents,
randomizes 🔥 across `emoji.panel.icons.fire1..fire4`). Both in `src/i18n/premium_md.rs` — neither
gets entities automatically. Add new: add ID to `emoji.panel.icons`, add `("🔥","key")` to `EMOJI_MAP`.

## Source Layout

```text
src/main.rs                  — mod declarations + app::run()
src/app/                     — mod (event loop), startup, dispatch (routing), state
src/config.rs                — env reading
src/bot.rs                   — send_text, send_text_md, send_start_button
src/cookie_pool/             — CookiePool + format helpers; fresh.rs (Redis freshness/lock store)
src/modules/                 — mod (notify_admin), cookie_refresher (Firefox profile refresh cycle)
src/i18n/                    — mod (t/tf), emoji_map, entities, premium_md
src/youtube/                 — extract, fetch, format, handle, quality_keyboard, trace, types,
                               lang_names; selection/ (menu); download/ (runner, store, cancel,
                               progress, status, split, upload, helpers, etc.)
src/database/                — mod, posfreSQL/{postgresql.rs, schema.sql}
src/admin/                   — mod (render_stats / render_stats_more / render_errors_1d)
src/stats/                   — mod (record_* functions), query (data model + queries)
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
- Firefox profiles discovered from `/home/mahdi/.mozilla/firefox` (**no cap** — all profiles
  used), cached in `cookie_profiles_cache/`. yt-dlp reads `cookies.sqlite` directly. Random
  selection excluding last-used + cooldown.
- **Refresh model (Redis-coordinated, shared dev+prod):** `spawn_cookie_refresher`
  (`src/app/startup.rs`) runs a worker every `COOKIE_WORKER_INTERVAL_SECS` (default 10 min).
  Sequential (one profile at a time). Per profile:
  1. `cookie:fresh:{profile}` key exists in Redis? → skip (still fresh).
  2. else take lock `cookie:refreshing:{profile}` via `SET NX EX` (`COOKIE_REFRESH_LOCK_TTL_SECS`,
     default 30 min). Lost lock (other env refreshing) → skip.
  3. won lock → `cookie_refresher::run` (kill firefox → check login → open firefox
     `sudo -u mahdi firefox --profile ...` `DISPLAY=:10` `XDG_RUNTIME_DIR=/run/user/1002` → 1 link
     from `files/youtube_links.txt` → wait 10 min (`duration_secs=600`) → copy cookies to cache).
  4. success → write `cookie:fresh:{profile}` TTL=`COOKIE_FRESH_TTL_SECS` (default 36h) + release
     lock. Failure → leave lock to expire (back-off, no retry until lock TTL).
- **Redis keys** (impl `src/cookie_pool/fresh.rs`, NOT namespaced — dev+prod share one Redis so a
  profile refreshed by one env is skipped by the other): `cookie:fresh:{profile}` (String, TTL 36h),
  `cookie:refreshing:{profile}` (String, NX lock).
- **Redis down** → cycle skipped entirely (Mode A, never refresh blindly), admin notified once in
  PV (`ADMIN_USER_ID`) until recovery. Config: `REDIS_URL` (default `redis://127.0.0.1:6379`),
  `ENV_LABEL` (lock owner, falls back to dev/prod via `DEV_MODE`).
- **Startup lock cleanup** (`fresh::clear_own_locks`): on worker start, all `cookie:refreshing:*`
  locks owned by this `ENV_LABEL` are deleted (a fresh process can't be mid-refresh — they're
  leaked from a prior run that died before unlocking). Other env's locks untouched. Without this,
  a restart would leave profiles `skip_locked` for up to lock_ttl.
- 429: `mark_last_rate_limited()` (4h cooldown safety net) → channel to event loop → 30-min task
  → per-profile refresh → re-add to pool. (cooldown list bound = number of available cookies.)
- Logs: `journalctl -u abc -f | grep cookie_worker`, format `[cookie_worker profile=x event=y]`.
  Per-profile Firefox logs still `[cookie_refresh profile=x event=y]`.
- Add profile: create Firefox profile + Google login, ensure `cookies.sqlite`, restart abc.

### Image upscale (Real-ESRGAN)
- Models: `realesrgan-x4plus` (default x4), `realesrgan-x4plus-anime` (x4),
  `realesr-animevideov3-x{2,3,4}`. UI: "عمومی x4" + collapsible "انیمه و کارتون ▼".
- State `AwaitingUpscaleImage { scale_factor, model_name, anime_expanded }` — all required.
- Callbacks: `upscale:model:{name}`, `upscale:anime_toggle`, `upscale:cancel`.

### Vocal separation
- Python FastAPI on port 6589 (`separation-service/`), model `Kim_Vocal_2.onnx`, one request at a
  time via `asyncio.Lock` (`_sep_lock`), max 50MB. systemd unit `separation.service`.
- Setup: `bash separation-service/install.sh` then enable+start separation.
- Flow: audio → mode keyboard (quality/fast) → download → POST → returns base64 vocals +
  instrumental → two .wav sent. Callbacks: `sep:quality:{id}`, `sep:fast:{id}`, `sep:cancel:{id}`.
- Health: `curl http://127.0.0.1:6589/health`. Status: `curl http://127.0.0.1:6589/cpu/status`.
  Logs: `journalctl -u separation -f`.

### Gemini watermark removal
- Binary `files/runtime/gwt-mini` (v0.3.1). Base args:
  `-i {in} -o {out} --denoise telea --radius 25 --quiet --no-banner`.
- Multi-pass (max 3): pass 1 detection gate (threshold 0.25, retry `--legacy` on `[SKIP]`, both
  skip → NoWatermarkDetected); passes 2-3 residual cleanup (threshold 0.05, chained). All passes
  sent to user (trade-off: pass 1 preserves detail, pass 3 cleanest). Impl `src/gemini_watermark/`.
- Callbacks: `ai:gwm`, `gwm:cancel`. Logs: `journalctl -u abc -f | grep '\[gwm'`.

### ASR — Nemotron streaming RNNT + Silero VAD
- Python FastAPI on port 8765 (`asr-service/`). Model: `nvidia/nemotron-3.5-asr-streaming-0.6b`
  ONNX int4, CPU-only, **8 هسته** (`OMP_NUM_THREADS=8`). systemd unit `asr.service`.
- Setup: create venv, `pip install -r requirements.txt`, `python download_model.py`
  (~1.5GB to `/opt/asr_model`), دانلود VAD:
  `wget https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx -O /opt/asr_model/silero_vad.onnx`
  سپس `asr.service` نصب شود.
- **Pipeline**:
  1. ffmpeg: تبدیل هر فرمت صوتی به WAV 16kHz mono
  2. Silero VAD (`silero_vad.onnx`، ~2.3MB، MIT): اسکن کامل صدا → لیست segment‌های گفتاری
     با timestamp دقیق. فریم ۳۲ms، threshold=0.5، padding=100ms، merge gap<300ms.
  3. ASR: هر segment جداگانه decode می‌شه. encoder cache برای هر segment reset می‌شه.
     timestamp هر token = زمان شروع segment + offset داخلی (دقت ±80ms).
  4. SRT: token‌ها با timestamp مطلق جمع‌بندی می‌شن → SRT با تقسیم روی punctuation.
- **سرعت**: ~7.5x real-time روی CPU (30 دقیقه در ~4 دقیقه، 8 هسته).
  VAD سکوت‌ها را حذف می‌کند → ~2.4x سریع‌تر از بدون VAD (قبلاً ~9.3 دقیقه).
- ONNX models: `encoder.onnx`, `decoder.onnx`, `joint.onnx`, `silero_vad.onnx`.
  Cache shapes: `cache_last_channel (1,24,56,1024)`, `cache_last_time (1,24,1024,8)`.
  mel input: `(1, T, 128)` — (batch, time, n_mels).
- `POST /transcribe` → `{text, language, duration_seconds, srt}`.
  `POST /transcribe/srt` → فایل SRT مستقیم (Content-Disposition attachment).
  `GET /health` → `{status, model_loaded}`.
- Rust client: `asr-service/asr_client.rs` — `transcribe_voice(path) → Result<AsrResult, AsrError>`.
  60s timeout.
- Logs: `journalctl -u asr -f | grep 'event='`.

### CPU Broker (use for any multi-second CPU task)
- `separation-service/cpu_broker.py` (`acquire(user_id, is_vip)` → real core list,
  `release(cores)`), `cpu_monitor.py` (sliding-window /proc/stat). Pin with process-level
  `sched_setaffinity(pid, cores)` + background pinner thread (polls `/proc/self/task` every 200ms
  to re-pin lazily-spawned ONNX/OMP threads) + `OMP_NUM_THREADS`/`OPENBLAS_NUM_THREADS`. Release
  in `finally`; pinner stopped + affinity restored to all cores after job.
- Redis keys: `cpu:reserved` (Hash, RESERVE_TTL=15min/core), `cpu:queue` (Sorted Set, VIP
  priority), `cpu:notify` (pub/sub), `cpu:overloaded` (String, TTL=300s — set when 3-min avg
  >50% AND 1-min avg >80%).
- **Self-healing sweeper** (`_purge_stale` + 60s `_sweeper`, also run once at `start_broker`):
  purges expired reservations (past RESERVE_TTL) and orphaned queue tickets (past TICKET_TTL=40min,
  >Rust 35-min hard timeout). Ticket pruning is **age-only** (cross-process safe: separation/asr
  each have a private `_waiters`, so "no local waiter" ≠ dead). Cleans leaked state after a crash.
- Core allocation by current CPU usage%: `<50%→4`, `<75%→2`, `<94%→1`, `>=94%→0 (queue)`.
  If `cpu:overloaded` key exists → always 0 (queue).
- Rust queue UX (`handle.rs`): calls `GET /cpu/status` first; if server free → shows
  "در حال پردازش", switches to queue msg after 8s if still running; if busy → shows queue msg
  immediately. After 5 min still running → "سرور تحت فشار" msg. 35-min hard timeout.

## Rank & Paywall system

### Ranks (`src/rank/types.rs`)
پنج مقام: `Dalavar` (رایگان/پیش‌فرض) → `Sepahbod` → `Esfandyar` → `Sohrab` → `Rostam`.
مقام کاربر از جدول `user_ranks` خونده میشه؛ اگه منقضی شده یا نبود → `Dalavar`.
`rank::effective_rank(db.client(), user_id).await` — همیشه live از db بخون.

### Quota (`src/rank/quota.rs`)
جدول `user_quotas (user_id, quota_type, used, window_start)`.
`get_usage(client, uid, kind, window_secs)` / `add_usage(...)` — window sliding بر اساس now.
انواع: `traffic_daily`, `traffic_monthly`, `denoise_daily`, `denoise_weekly`, `stt_weekly`,
`upscale_2x_weekly`, `upscale_4x_weekly`, `ai_chat_monthly`.

### Paywall (`src/rank/paywall.rs`)
دو تابع برای بلاک کردن + نمایش منوی ارتقا:
- `block_feature(api, chat_id, feature, min_rank)` — قابلیت اصلاً مجاز نیست
- `block_limit(api, chat_id, limit_label, min_rank)` — محدودیت عددی (مدت، حجم)

هر دو پیام HTML + دکمه «مشاهده پلن‌ها ↗» (`rank:menu` callback) می‌فرستن.
متن‌ها از i18n: `rank.paywall_feature`, `rank.paywall_limit`, `rank.paywall_button`.

### محدودیت‌های پیاده‌شده
| قابلیت | نقطه چک | نوع |
|---|---|---|
| کیفیت YouTube | `handle_resolution_callback` — کلیک روی دکمه کیفیت | `block_limit` (Xp) |
| زیرنویس YouTube | `handle_sub_toggle` — کلیک روی زبان | `block_feature` |
| فایل جداگانه زیرنویس | `handle_sub_mode_toggle` — سوئیچ به File | `block_feature` |
| حذف نویز روزانه | `handle_denoise_audio` — بعد از گرفتن duration | `block_limit` |
| حذف نویز هفتگی | `handle_denoise_audio` — بعد از گرفتن duration | `block_limit` |
| بهبود خودکار صدا (STT) | `CB_STT_TOGGLE_DENOISE` — کلیک روی دکمه فعال‌سازی | `block_feature` |
| رونویسی دقیق (Large) | `CB_STT_FA_BIG` / `CB_STT_EN_BIG` — کلیک روی دکمه مدل | `block_feature` |
| سقف روزانه/هفتگی رونویسی | `handle_stt_audio` — بعد از گرفتن duration | `block_limit` |
| جداسازی موزیک روزانه/هفتگی | `handle_separation_callback` — بعد از ffprobe duration | `block_limit` |
| ترافیک دانلود روزانه/ماهانه | `handle_go` — قبل از `spawn_download`، حجم تخمینی از bitrate×duration | `block_limit` |
| افزایش کیفیت تصویر هفتگی | `handle_upscale_image` — اول تابع، بر اساس scale factor | `block_limit` |

**قانون:** دکمه‌ها همیشه نشون داده میشن — paywall فقط موقع کلیک میاد.

### عمداً بدون paywall
- **ASR / جادو (Nemotron):** مرحله‌ی تست، عمومی نیست؛ ضمناً از CPU Broker (صف + رزرو هسته) استفاده
  می‌کنه پس منابعش خودکنترله. وقتی عمومی شد سقفش تعریف می‌شه.
- **حذف واترمارک Gemini:** سریع و سبک — محدودیت توجیه نداره.

### rank methodهای تعریف‌شده ولی هنوز wire نشده (فیچرش وجود نداره)
`can_subtitle_hardcode` (در حال ساخت)، `ai_chat_monthly_toomar`، `playlist_limit`.
(حذف‌شده: `yt_search_per_12h`، `gemini_pro_access`، و `stt_per_week` منسوخ.)

### سقف‌های حذف نویز
| مقام | روزانه | هفتگی |
|---|---|---|
| دلاور / سپهبد / اسفندیار | ۳۰ دقیقه | ۲ ساعت |
| سهراب | ۲ ساعت | ۱۰ ساعت |
| رستم | ۱۰ ساعت | ۹۹ ساعت |

### راهنمای پلن‌ها
متن کامل در `i18n.json` کلید `rank.guide` — ارسال با `ParseMode::Html`.
`/rank` و دکمه «مشاهده پلن‌ها» هر دو `rank::menu::send_rank_menu()` رو صدا می‌زنن.

### STT denoise (`src/stt/handle.rs`)
- سهراب و رستم: دیفالت فعال (`stt_denoise_default() -> bool`)
- بقیه: دیفالت غیرفعال
- paywall در `CB_STT_TOGGLE_DENOISE`: وقتی کاربر می‌خواد روشن کنه (`!config.denoise`) rank چک می‌شه؛
  اگه `can_stt_denoise()` برنگشت → `block_feature(api, chat_id, t("stt.denoise_feature_name"), Rank::Sohrab)`
- `enter_stt_config` و `handle_stt_callback` هر دو `database: &Option<PostgresDatabase>` می‌گیرن

### افزایش کیفیت تصویر (`src/upscale/handle.rs` → `handle_upscale_image`)
- چک **اول تابع** (قبل از ارسال status و گرفتن CPU) — quota شمارشی per-image، نیاز به انتظار نداره.
- سقف بر اساس scale factor: `upscale_weekly_quota(scale)`. مدل‌ها: عمومی x4، انیمه pro x4،
  انیمه‌ویدیو x4/x3/x2. سه نوع quota: `Upscale2x/3x/4xWeekly` (هفتگی، تعداد عکس).
- `handle_upscale_image` الان `database: Option<PostgresDatabase>` می‌گیره (Clone از طریق Arc).
- مصرف بعد از ارسال موفق: `add_usage(..., 1, 7*86400)`. next_rank: `upscale_next_rank()`.

| مقام | ×۲ | ×۳ | ×۴ |
|---|---|---|---|
| دلاور / سپهبد / اسفندیار | ۵ | ۳ | ۲ |
| سهراب | ۵۰ | ۳۰ | ۲۰ |
| رستم | ۵۰۰ | ۳۰۰ | ۲۰۰ |

(رستم = ۱۰ برابر سهراب. شمارش هفتگی per-image.)

### ترافیک دانلود یوتیوب (`src/youtube/selection/handlers.rs` → `handle_go`)
- چک قبل از `spawn_download` (تنها نقطه‌ی شروع دانلود).
- حجم تخمینی = `bitrate(kbps) × 1000/8 × duration` (فقط ویدیو، همسان با عددی که در پنل نشون داده می‌شه).
  اگه bitrate یا duration نبود → estimate=0 و چک «فایل بزرگ» اعمال نمی‌شه (ولی سقف صفر بازم بلاک می‌کنه).
- سه حالت: سقف روزانه تموم (`daily_traffic_bytes`)، سقف ماهانه تموم (`monthly_traffic_bytes`)،
  یا حجم فایل > باقی‌مانده. هر سه → `block_limit`.
- پنجره: روزانه ۰۰:۰۰ تهران، ماهانه هر ۳۰ روز از `first_upload_at` (از `stats_users`).
  مصرف بعد از آپلود موفق در `stats::record_upload_done` → `add_traffic` ثبت می‌شه (از قبل بود).
- next_rank: `traffic_daily_next_rank()` (۵گ→اسفندیار ۴۰گ)، `traffic_monthly_next_rank()` (۱۵گ→سپهبد، ۶۰گ→اسفندیار).
- helperها در همون فایل: `estimate_bytes`, `fmt_traffic_fa` (رقم فارسی), `to_fa_digits`.

| مقام | ترافیک روزانه | ترافیک ماهانه |
|---|---|---|
| دلاور | ۵ گیگ | ۱۵ گیگ |
| سپهبد | ۵ گیگ | ۶۰ گیگ |
| اسفندیار | ۴۰ گیگ | ۴۰۰ گیگ |
| سهراب | ۵ گیگ (ارث از دلاور) | ۱۵ گیگ (ارث از دلاور) |
| رستم | ۴۰ گیگ | ۴۰۰ گیگ |

**نکته:** سهراب برای ترافیک از دلاور ارث‌بری می‌کنه (`Self::Dalavar.{daily,monthly}_traffic_bytes()`).

### سقف کیفیت YouTube (`max_yt_quality`)
| مقام | سقف کیفیت |
|---|---|
| دلاور | ۵۰۰p |
| سپهبد | ۱۱۵۰p |
| اسفندیار | بدون محدودیت |
| سهراب | ۵۰۰p (ارث از دلاور) |
| رستم | بدون محدودیت |

`min_for_quality` هماهنگ: ≤۵۰۰→دلاور، ≤۱۱۵۰→سپهبد، بالاتر→اسفندیار. سهراب برای یوتیوب از دلاور
ارث‌بری می‌کنه (`Self::Dalavar.max_yt_quality()`).

### سقف‌های رونویسی صدا به متن (STT)
| مقام | سریع — روزانه | سریع — هفتگی | دقیق — روزانه | دقیق — هفتگی |
|---|---|---|---|---|
| دلاور / سپهبد / اسفندیار | ۳۰ دقیقه | ۲ ساعت | ❌ | ❌ |
| سهراب | ۳ ساعت | ۱۵ ساعت | ۱ ساعت | ۵ ساعت |
| رستم | ۳۰ ساعت | ۱۵۰ ساعت | ۱۰ ساعت | ۵۰ ساعت |

- مدل دقیق (Large): paywall روی کلیک دکمه انتخاب مدل (`CB_STT_FA_BIG`, `CB_STT_EN_BIG`) → `block_feature(..., Rank::Sohrab)`
- quota چک بعد از دانلود + convert (duration مشخص می‌شه)، ثبت usage بعد از موفقیت
- `handle_stt_audio` الان `database: Option<PostgresDatabase>` می‌گیره (Clone‌پذیره چون `Arc<Client>` داخلشه)
- `PostgresDatabase` به `Arc<Client>` تغییر یافت تا Clone‌پذیر بشه (`src/database/posfreSQL/postgresql.rs`)

## آمار و پنل ادمین (`src/stats/` + `src/admin/`)

دسترسی: `/start` → دکمه «پنل ادمین» (فقط برای `ADMIN_USER_ID`) → «آمار». ثبت آمار فقط وقتی
`DATABASE_URL` ست باشه کار می‌کنه.

### مدل ثبت
- جدول عمومی رویداد `stats_events(user_id, feature, action, status, amount, created_at)` —
  هر فیچر بدون تغییر امضای هندلرش باهاش ثبت می‌کنه. `amount` = ثانیه‌ی صدا یا تعداد، بسته به فیچر.
- جدول خطا `stats_errors(feature, message, created_at)` — برای دکمه «خطاهای ۱ روز گذشته».
- توابع ثبت در `src/stats/mod.rs` (روی `OnceLock` کلاینت سراسری، از هر جای کد قابل صدا زدن):
  - `record_event_user(user_id, feature, action, status, amount)` / `record_event_global(...)` (بدون user_id)
  - `record_error_global(feature, message)` (پیام به ۵۰۰ کاراکتر کوتاه می‌شه)
  - یوتیوب دانلود از قبل `stats_downloads` + `record_upload_done` رو داره (جدا از stats_events).
- `feature` ها: `stt`, `denoise`, `upscale`, `separation`, `gwm`, `asr` (آمار اصلی) و
  `youtube`, `emoji`, `cookie`, `paywall`, `cpu` (آمار بیشتر).

### نقاط ثبت (success/fail در هر هندلر)
STT `src/stt/handle.rs` (action=big/fast_fa/en[_dn])، Denoise `src/denoise/handle.rs`،
Upscale `src/upscale/handle.rs` (action=x2/x3/x4)، Separation `src/separation/handle.rs`
(action=quality/fast)، GWM `src/gemini_watermark/handle.rs` (status ok/no_watermark/fail،
amount=pass)، ASR `src/asr/handle.rs`. یوتیوب: `selection/handlers.rs` handle_go (action=q{h}_{codec})
+ `quality_keyboard.rs` cancel. ایموجی: `emoji/handler/{flow_pack_choice,flow_misc,callback}.rs`
(add/test/import/pack_create). کوکی: `youtube/handle.rs` 429 + `modules/cookie_refresher.rs`
refresh ok/fail. پی‌وال: `rank/paywall.rs` (block_feature/block_limit، status=رتبه‌ی لازم) +
`rank/menu.rs` (menu). CPU: separation/asr موقع `cores==0`/`!server_free` (queue) و timeout.

### نمایش (`src/admin/mod.rs`)
- `render_stats(client)` → پنل اصلی: کاربران → کاربران فعال (DAU/WAU/بازگشتی/پرمصرف‌ترین) →
  یوتیوب → ۶ بلوک AI. هر بلوک ✅ موفق + ❌ ناموفق (و ⏱ مدت برای فیچرهای صوتی). همه با
  بازه‌های `1d|3d|7d|30d`. **کل خروجی با `to_fa_digits` رقم‌فارسی می‌شه.**
- `render_stats_more(client)` → دکمه «آمار بیشتر»: تفکیک action/status هر فیچر (یوتیوب/ایموجی/
  کوکی/پی‌وال/CPU) با سقف ۱۴ ردیف.
- `render_errors_1d(client)` → دکمه «خطاهای ۱ روز گذشته»: HTML با `<blockquote expandable>`
  (نقل‌قول جمع‌شو)، آخرین ۴۰ خطای ۲۴ ساعت اخیر. **پیام جدید با `ParseMode::Html`** می‌فرسته
  (پنل دست‌نخورده می‌مونه).
- کوئری‌ها در `src/stats/query.rs`: `get_feature_stats`, `get_active_users`,
  `get_action_breakdown`, `get_recent_errors`, `count_recent_errors`. helperها: `fmt_secs`, `fmt_bytes`.
- Callbackها (`src/bot.rs`): `admin:panel`, `admin:stats`, `admin:stats_more`, `admin:errors_1d`.
  Wiring در `src/app/dispatch.rs`. دکمه‌ها با `btn_icon` (آیکون‌های `stats`/`warning`/`back`).
- **داده تاریخی نداریم** — آمار از زمان افزوده‌شدن ثبت به بعد جمع می‌شه.

## PostgreSQL tables (auto-created when `DATABASE_URL` set)

Cookie pool: `cookie_pool_cookies`, `cookie_pool_state`, `cookie_pool_cooldowns`.
Emoji: `emoji_packs`, `emoji_items`.
Stats: `stats_users`, `stats_downloads` (یوتیوب)، `stats_events` (عمومی)، `stats_errors`.
Schema: `src/database/posfreSQL/schema.sql`.

## Runtime deps (tracked under `files/`)

Vosk (`libvosk.so` + Persian/English models), DeepFilterNet3 (`deep-filter` binary +
`DeepFilterNet3_onnx.tar.gz`, extracted on first run), Real-ESRGAN (`realesrgan-ncnn-vulkan` +
models), gwt-mini binary, separation `Kim_Vocal_2.onnx`. build.rs links libvosk via
`files/runtime`. System pkgs: ffmpeg, libvulkan1 + mesa-vulkan-drivers, Python 3 + pip.

## Git server

`origin` → `git-server/ros-telegram-bot.git`, branch `master`.
