# rostam — Telegram media & utility bot

**English** · [فارسی](README_fa.md) · [Русский](README_ru.md)

A multilingual Telegram bot (Rust, crate `ros-telegram-bot`, binary `rostam-dev`
in dev / `rostam` in production) — UI in **Persian, English, Italian and Russian**
(switch with `/language`) — that bundles a media toolbox — YouTube downloading,
vocal separation, speech-to-text, audio/video denoise, text-to-speech, image
upscaling, B&W colorization, background removal, watermark removal, PDF & archive
compression, direct-link downloads, IP lookup, custom emoji packs — behind a
ranked/paywalled UI. Most ML runs **locally** (ONNX via `ort`, CTranslate2 via
`ct2rs`, Vosk, Piper); the remaining heavy CPU work runs in sidecar services or
child processes coordinated by a Redis-backed CPU broker.

Current release: **2.1.2** (see [CHANGELOG.md](CHANGELOG.md) — `Cargo.toml` stays
at `0.1.0`, the release version lives in git tags + changelog).

---

## 🚀 Quick Start (one-shot installer)

`install.sh` bootstraps a bare Debian/Ubuntu or Arch host end to end: system
packages, Rust, deno, yt-dlp, ~8 GB of model assets, PostgreSQL + Redis, the
release build, every sidecar, and the `rostam.service` systemd unit.

```bash
sudo bash <(curl -Ls https://raw.githubusercontent.com/mmahdi-sz/rostam/master/install.sh)

# or, from a checkout:
sudo bash install.sh --dir /opt/rostam --branch master
```

Options: `--dir`, `--branch`, `--skip-bot-api`, `--skip-firefox`, `--fresh`.
Env: `ROSTAM_REPO`, `ROSTAM_BRANCH`, `ROSTAM_ASSET_CACHE` (a directory of
already-downloaded model files, looked up by basename — lets you re-install or
provision offline without re-fetching the ~8 GB).

There is no Docker image: the crate links `files/runtime/libvosk.so` through
`build.rs` rpath, embeds `migrations/` at compile time, and drives seven
sidecar/child processes — a container adds work without removing any.

---

## Environments

| | Dev | Prod |
|---|---|---|
| Dir | `dev/` | build `prod/`, runtime `production/` |
| Service | `rostam_dev.service` (alias `abc.service`) | `rostam.service` |
| DB | `ros_telegram_bot` | `ros_telegram_bot_production` |
| Binary | `target/debug/rostam-dev` | `production/rostam` (`--release`) |
| Branch | `dev` | `master` |
| Bot API | `127.0.0.1:8081` | `127.0.0.1:8082` |
| Health | `14381` | `14380` |
| Cookie refresher | off | on |

Never write to the production database from a dev run.

---

## 🧪 Testing & Observability

### Running TestAPI Suite & Unit Tests
```bash
# Run unit test matrix (140 tests across 37 modules).
# Note: this is a bin-only crate — `cargo test --lib` finds nothing, use `cargo test`.
cargo test

# Run TestAPI integration suite (24 endpoints, real handlers + dev DB)
bash scripts/run_testapi_suite.sh
```

Every feature must ship with a `/test/<domain>/<action>` endpoint in
`src/testapi/` (compiled only under `--features testapi`, bound to `127.0.0.1`,
port from `TESTAPI_PORT`, default `14379`, enabled by `TESTAPI_ENABLED=1`) and be
wired into `scripts/run_testapi_suite.sh`. Endpoints that touch the DB (`/test/quota`,
`/test/redeem/apply`, `/test/referral/spend`) call the **real** functions against
the dev database using throwaway ids in the `-999xxx` range.

### Health & Metrics Endpoints
- **Health Check:** `GET http://127.0.0.1:14380/health` (dev uses `HEALTH_PORT=14381`
  to avoid colliding with production)
- **Prometheus Metrics:** `GET http://127.0.0.1:14380/metrics`

---

## 📚 Documentation
- **Architecture & Diagrams:** [docs/architecture.md](docs/architecture.md)
- **Operations & Incident Runbook:** [docs/ops-runbook.md](docs/ops-runbook.md)
- **Architecture Decision Records (ADRs):** [docs/adr/](docs/adr/)


---

## What it installs

| Layer | Details |
|---|---|
| **System packages** | git, curl, unzip, tar, ffmpeg, ghostscript, PostgreSQL, Redis, Python 3 (+venv), build tools, cmake, clang + libclang (bindgen for `espeak-rs-sys`), espeak-ng, p7zip, sudo, Firefox |
| **Rust** | via `rustup` (edition 2024 needs rustc ≥ 1.85) |
| **deno** | to `/opt/deno` — passed to yt-dlp `--js-runtimes` for YouTube JS challenges |
| **yt-dlp** | latest static binary → `/usr/local/bin/yt-dlp` |
| **Models (~8 GB)** | Vosk STT ×4 (fa/en), `libvosk.so`, Moebius ONNX (watermark), DeepFilterNet 3 + `deep-filter` binary, Real-ESRGAN, background removal (`mmahdi-sz/FeyNobg-ONNX`), NLLB-200 600M CTranslate2 int8 (`model.bin` + tokenizer + generated `config.json`), Piper fa_IR TTS (`kiarashQ/fa-ir-tts-piper-ar-mantatts-v1`) |
| **PostgreSQL** | creates DB `ros_telegram_bot` (schema applied by `refinery` from `migrations/`, V001–V005) |
| **The bot** | `cargo build --release` → `rostam.service` (dev: `rostam_dev.service`) |
| **separation-service** | vocal/instrumental split + CPU broker, binds **`127.0.0.1:6589`** (`SEP_BIND_HOST` is a container escape hatch), auto-downloads its model; its venv also provides `edge-tts` for English TTS |
| **surge** | [SurgeDM/Surge](https://github.com/SurgeDM/Surge) parallel download manager daemon (:1700), latest release binary → **`rostam-surge.service`** (not `surge.service`: the CLI refuses to start when it sees a unit by that name) |
| **Local Telegram Bot API** | built from tdlib source (:8081) — raises the upload cap to 2 GB |

`files/models/deoldify/ddcolor_modelscope.onnx` (980 MB) has no HTTP mirror — the
installer scrapes Google Drive's confirm token, downloads the 1.9 GB
`ddcolorizer_onnx_models.zip`, extracts just that file and verifies its sha256. If
Drive rate-limits, it warns and continues; colorization stays offline until the
file is copied in. `rar` lives in Debian's `non-free` component, so the installer
enables `contrib non-free non-free-firmware` on the existing Debian/Ubuntu suites
before `apt-get update` (both the deb822 `.sources` and legacy one-line layouts).

---

## Architecture

```
                       ┌──────────────────────────┐
   Telegram  ─────────►│  local Bot API  :8081     │  (2 GB uploads; prod :8082)
                       └────────────┬─────────────┘
                                    │
                          ┌─────────▼─────────┐        ┌───────────────────┐
                          │  rostam (Rust)    │───────►│  in-process ML     │
                          │  target/release/  │        │  ort (Moebius,     │
                          │     rostam        │        │   DeOldify, nobg)  │
                          └───┬───────┬───┬───┘        │  ct2rs (NLLB)      │
             ┌────────────────┘       │   └───────┐    │  vosk (STT)        │
             │                        │           │    │  piper_rs (fa TTS) │
      ┌──────▼──────┐          ┌──────▼──────┐  ┌─▼────────────┐ └─────────┘
      │ PostgreSQL  │          │   Redis     │  │  Firefox     │
      │  :5432      │          │   :6379     │  │  cookie pool │
      └─────────────┘          └──────┬──────┘  └──────────────┘
                                      │
                          ┌───────────┴───────────┐
                          │      CPU broker        │  (queue + core reservation,
                          │  127.0.0.1:6589        │   Redis-backed)
                          └───────────┬───────────┘
                            ┌─────────┴─────────┐
                    ┌───────▼────┐      ┌───────▼────┐
                    │ separation │      │  surge dl  │
                    │   :6589    │      │   :1700    │
                    └────────────┘      └────────────┘

   child processes (not sidecars): deep-filter (denoise) ·
   realesrgan-ncnn-vulkan (upscale) · ffmpeg · ghostscript · 7z/rar · edge-tts
```

The bot connects to Postgres, Redis and the sidecars **lazily** — none is
required at startup. If Postgres is down it runs with persistence disabled (and a
quota lookup failure tells the user to contact the admin rather than granting free
work — the quota gate is deliberately **fail-closed**); if a sidecar is down, only
that feature returns "service unavailable". The only hard startup requirement is
`BOT_TOKEN`.

The heaviest ML runs **in-process** inside the Rust binary (no Python round-trip):
watermark inpainting, colorization and background removal via the `ort` ONNX
runtime, subtitle translation via `ct2rs` (CTranslate2), speech-to-text via `vosk`,
and Persian TTS via `piper_rs`. Vocal separation and parallel direct downloads live
in external sidecars; denoise, upscale, archive and PDF compression shell out to
child processes. Any multi-second CPU job — in-process, subprocess or sidecar — is
scheduled through the **CPU broker** (`127.0.0.1:6589`), which queues work and
reserves physical cores via Redis so concurrent tasks don't thrash the box.

There is **no connection pool**: `src/database/postgresql/mod.rs` holds a single
`Arc<Client>`, and `tokio_postgres::Client::transaction()` needs `&mut self`, so the
codebase has zero SQL transactions by construction. Every atomic operation (quota
reserve, redeem consume, referral spend) is expressed as **one SQL statement** with
`ON CONFLICT` + `RETURNING` against a primary-key-locked row. Never emit
`BEGIN`/`COMMIT` through the shared client — it would swallow other users' statements.

---

## Features

**Media & AI:** YouTube download (video/playlist/MP3, quality/subtitle/traffic
paywalls) · vocal/instrumental separation · speech-to-text (Vosk) · audio **and
video** denoise (DeepFilterNet 3) · text-to-speech (Piper fa / edge-tts en) ·
image upscale (Real-ESRGAN, photo + anime) · B&W photo colorization (DeOldify) ·
background removal (transparent PNG) · watermark removal (Moebius ONNX,
in-process) · subtitle translation (NLLB).

**Files & links:** PDF compression (Ghostscript) · archive compression (ZIP / 7Z /
RAR, LZMA2 / PPMd / BZip2, levels 0–9, splitting 5 MB – 2 GB, password) · fast
parallel direct-link downloads (Surge) · social-platform link detection
(`detect_social_platform`: telegram, instagram, tiktok, twitter, pinterest,
facebook, threads, soundcloud, spotify, aparat, rubika, eitaa) with YouTube URLs
auto-routed to the YouTube flow.

**Platform:** ranks & paywall · windowed quotas with reserve-before-work ·
referrals + leaderboard · redeem/gift codes · force-join gate · custom emoji packs ·
IP lookup · admin stats/traffic/error panel · admin broadcast (copy/forward + pin,
15 msg/s, auto blocked-user detection).

Commands: `/start`, `/panel`, `/language`, `/rank`, `/ref` (aliases `/re`,
`/referral`), plus admin cookie-pool commands. Everything else is inline
callbacks in `{domain}:{action}:{args}` form.

---

## AI & Machine-Learning pipeline

Every model runs **locally** — no external AI API, no per-call cost, no data
leaving the box. (The one exception is English TTS, which uses Microsoft's
`edge-tts` CLI.)

| Capability | Model / engine | Runs where |
|---|---|---|
| **Subtitle translation** | Meta **NLLB** via `ct2rs` (CTranslate2 native binding) | in-process |
| **Speech-to-text** | **Vosk** (Persian + English, small & large models) | in-process (`libvosk.so`) |
| **Audio/video denoise** | **DeepFilterNet 3** (`files/runtime/deep-filter`) | subprocess |
| **Image upscale** | **Real-ESRGAN** (ncnn-vulkan): photo + anime models (x2/x3/x4) | subprocess |
| **Watermark removal** | **Moebius** ONNX inpainting (`ort` crate) | in-process |
| **Colorization** | **DeOldify** ONNX | in-process (`ort`) |
| **Background removal** | ONNX segmentation → transparent PNG | in-process (`ort`) |
| **Text-to-speech (fa)** | **Piper** `fa_IR-mantatts-par.onnx` via `piper_rs`, HomoFast eSpeak G2P + homograph disambiguation | in-process |
| **Text-to-speech (en)** | `edge-tts` (`en-US-AvaNeural`) | subprocess |

- **Subtitle translate** — when a video has no Persian subtitle, the bot pulls the
  English SRT, feeds it through the local NLLB translator (`src/youtube/translator/`),
  and emits `translated_fa.srt` with the original timing preserved.
- **STT** — pick language + model size from an inline keyboard. Large ("accurate")
  models are paywalled; the small ones are free. An optional DeepFilterNet denoise
  pass can be toggled before transcription.
- **Upscale** — a photo mode plus a dedicated **Anime** toggle exposing
  `realesrgan-x4plus-anime` and `realesr-animevideov3` at x2/x3/x4.
- **TTS** — live progress bar (percent / elapsed / ETA); falls back to an audio
  document if the user's privacy settings block voice messages. Voice cloning was
  removed in 2.1.0.
- **Denoise** — works on voice/audio **and** MP4/MKV/WebM (audio track extracted,
  denoised, remuxed through ffmpeg), with a live ETA progress bar.

---

## YouTube — advanced features

- **Playlist auto-detect** — a playlist URL is expanded (`--flat-playlist`), the
  user picks quality **once**, then every item downloads in a loop with a pinned
  live status message; per-item failures are reported without aborting the batch.
- **Quality & codec selection** — resolution, video codec, audio language and
  audio-only quality (best / 128k / 64k) are all chosen from one inline panel.
- **Soft subtitles** — embedded subtitles are written as `mov_text` with
  `displayFlags=0` so players *list* the track without force-showing it. After an
  embedded download the bot patches 4 bytes in place (first enabled tx3g →
  `0xC0000000`) so VLC & co. behave, with no remux — it survives `-c copy`.
- **Two subtitle modes** — download as a separate `.srt` **file**, or **embed**
  into the container.
- **Audio-only MP3** — `--extract-audio --audio-format mp3 -q 0`, with a smart
  filename and a metadata caption.
- **JS challenge solving** — yt-dlp is handed a `deno` runtime (`--js-runtimes`)
  so YouTube's JS player challenges resolve (without it you only get storyboards).

---

## Cookie pool & anti-ban system

YouTube rate-limits and IP-bans aggressively; the cookie pool spreads load across
many Google logins so no single account gets burned.

- **Rotation** — each request draws the next available cookie from a pool of
  Firefox profiles (`cookies.sqlite`), round-robin, with the last-used cookie
  persisted in Postgres.
- **Cooldown** — a cookie that hits a `429` is parked in a cooldown table
  (`cookie_pool_cooldowns`, Postgres + in-memory) and skipped until its window
  expires; the main loop re-adds it automatically.
- **Auto-refresh** — a background task (`spawn_cookie_refresher`, default every
  600 s) drives a real Firefox profile to keep the Google session — and therefore
  the cookies — fresh.
- **Manual ops** — `/cookie_status`, `/cookie_next`, `/cookie_429` inspect and
  drive the pool by hand.
- **Retry on a poisoned cookie** — when yt-dlp returns zero formats because of
  `database is locked`, `The page needs to be reloaded` or a bot-detection
  challenge, the next cookie in the pool is tried automatically instead of
  reporting "no downloadable quality found".
- **Pool exhaustion** — if every profile is unavailable the user gets a clear
  temporary-unavailability message (safety cooldown **1 hour**, refresh can recover
  it sooner) and the admin gets a cookie/Gmail diagnostic alert, throttled to one
  per 30 minutes.

State survives restarts via three tables: `cookie_pool_cookies`,
`cookie_pool_state`, `cookie_pool_cooldowns`.

---

## CPU broker & job queue

Any multi-second CPU task goes through a **Redis-backed broker** hosted by the
separation service on **`127.0.0.1:6589`**. Callers today: upscale, watermark
(Moebius), colorization, background removal, TTS, PDF compression, archive
compression, YouTube post-processing and Surge downloads.

- `POST /cpu/acquire` reserves a set of physical cores for the caller; the task
  runs pinned to them and releases on completion.
- `GET /cpu/status` reports available cores, queue length and an `overloaded`
  flag, so the bot can tell the user "system busy, queued" instead of thrashing.
- Work is **queued**, not dropped — callers wait their turn (the separation
  timeout is 40 min to cover queue + processing).

This keeps a handful of concurrent heavy jobs from oversubscribing the CPU and
starving the Telegram event loop.

---

## Ranks, paywall & quotas

Five ranks, ascending: **Dalavar** (free) → **Sepahbod** → **Esfandyar** →
**Sohrab** → **Rostam**.

- The effective rank is read **live** on every gated click
  (`rank::effective_rank`) — never cached — and expires via `user_ranks.expires_at`.
  The panel shows the live effective rank, marking an expired paid rank as
  «منقضی شده» and counting down in hours/minutes under one day.
- **Buttons are always visible**; the paywall fires on click, so lower ranks see
  what they're missing.
- Gated: YouTube quality, YouTube subtitles, YouTube traffic (daily + monthly,
  estimated from bitrate × duration), denoise (daily + weekly), STT (fast/accurate
  × daily/weekly), separation (daily + weekly), upscale x2/x3/x4 weekly, TTS
  weekly, DeOldify weekly, background-removal weekly, archive-compression CPU
  seconds (daily + monthly).
- Quotas live in `user_quotas` (PK `(user_id, quota_type)`, rolling window from
  `window_start`).

### Reserve before the work, refund on failure

Quotas are **reserved up front**, not debited afterwards. `rank::quota::reserve_usage`
performs the limit check *and* the increment in **one SQL statement** against the
row locked by `user_quotas_pkey`, so two concurrent requests cannot both pass the
check. Every failure path between reservation and delivery calls `refund_usage`.

- Two-window handlers reserve the **shorter** window first and refund it inline if
  the longer window rejects.
- A magnitude of zero (failed WAV-header/`ffprobe` probe) is clamped to **at least
  one second**, so a broken probe can't buy free work.
- A DB error on the gate **fails closed**: the user gets `rank.quota_db_error`
  (all 4 languages) telling them to contact `@mmahdi_sz`.
- Archive compression is the one exception that still settles afterwards — its unit
  is CPU seconds, unknowable before the job ends, so it reserves 1 second and adds
  the remainder on completion.
- `TrafficDaily`/`TrafficMonthly` still meter post-hoc.

`/test/quota` exercises the real reserve/refund/get functions against the dev DB
(counter and magnitude kinds, over-limit rejection, refund, exact fit, refund
larger than usage).

---

## Referral system (anti-fraud)

Invite link: `https://t.me/{bot}?start={referrer_id}`.

- A `/start` with a numeric payload records a **pending** referral
  (`referral_pending`) — it does **not** score immediately.
- An hourly sweeper (`sweep_confirm`) re-checks pending rows older than
  **`PENDING_DAYS` = 2**: if the invitee is still a force-join member they're
  promoted to `referrals` (1 point); if they left, the row is discarded. This kills
  the "join → get counted → leave" fraud loop.
- Points are **spent** to activate a rank for **31 days** (`ACTIVATION_DAYS`) via a
  referral-specific ladder (`TIERS`): Sohrab = 10, Esfandyar = 20, Rostam = 50.
  Balance = `COUNT(referrals) − SUM(points_spent)`. Downgrades are rejected. The
  spend is a single idempotent statement (migration `V005`), so a double-tap can't
  grant twice or drive the balance negative.
- **Leaderboard** — top referrers with 🥇/🥈/🥉, joined against `stats_users` for
  usernames.
- **Gotcha**: referral requires a mandatory force-join lock — without it
  `is_joined()` is always true and every pending auto-confirms.

---

## Redeem / gift codes

- Admins generate codes from the panel with flags in any order: `30d es 1u` → 30
  days, Esfandyar rank, 1 use (`<n>d` = days, `<n>u` = uses, rank abbrev
  `da/se/es/so/ro`).
- Codes are 8 chars from an unambiguous alphabet (no `0/O/1/I`) so they ride
  Telegram `?start=` deep-links cleanly.
- **Sliding 7-day expiry**: a code is born with `now + 7d`; each redemption resets
  the window; after expiry it's swept away (`spawn_redeem_sweeper`).
- `max_uses` caps redemptions; `redeem_redemptions` (PK `code,user_id`) enforces
  one-redeem-per-user.

---

## Force-join system

A mandatory channel-membership gate that runs after language selection and before
any feature:

- Membership is **cached in Redis** so the gate doesn't hammer Telegram's API on
  every action.
- The cache is kept warm by **listening to `chat_member` updates**
  (`on_chat_member_update`) — the moment a user joins or leaves, Redis is updated
  with zero extra API calls.
- The green "I've joined" button forces a **live** re-check, bypassing the cache.

Force-join is also the trust anchor for referral confirmation (see above).

---

## Admin panel & monitoring

Admin-only callbacks (`admin:panel`), gated by `ADMIN_USER_ID`:

- **Stats** — per-feature event counts (`stats_events`) and user counts
  (`stats_users`).
- **Traffic** — per-user download/upload bytes and upload success
  (`stats_downloads`); the first-upload timestamp is stamped on `stats_users`.
- **Errors (last 24h)** — every feature error is recorded (`stats_errors`,
  500-char cap) and rendered on demand, so you can triage failures from inside
  Telegram.
- **Ops** — manage the force-join lock and generate redeem codes without leaving
  the chat.
- **Broadcast** — send to every known user via `copy_message` or `forward_message`,
  optionally pinning, throttled to 15 msg/s (67 ms delay); users who blocked the bot
  are marked `is_blocked = true` automatically.

---

## Trace logging

Every user action carries one **trace id** threaded through every helper, so
`rg trace=N` replays the whole action in order. Two line kinds:

```
[<domain> trace=N actor] user=@<u> id=<id> rank=<R> clicked=<cb>    # once, at entry
[<domain> trace=N event=<step>] k=v => ok|fail err=..|pass|blocked  # per step, BEFORE it runs
```

A missing event line means that step was never reached — the absence *is* the
signal. Secrets never appear: the bot token is redacted before any error string
reaches journald, `stats_errors` or the admin error panel (`frankenstein` errors
embed the full request URL, which contains the token). IDs are truncated to their
first 6 characters.

Example (fails at CPU acquire — `cores=[]` is the smoking gun):

```
[upscale trace=43 actor] user=@parsa id=671234… rank=Dalavar clicked=upscale:model:realesrgan-x4plus
[upscale trace=43 event=quota_check] used=2 limit=3 => pass
[upscale trace=43 event=cpu_acquire] => cores=[]
```

Tail with `journalctl -u rostam -n 300 | rg trace=` (dev: `-u rostam_dev`).
Macros live in `src/log.rs` (`log_actor!`, `log_actor_id!`, `log_ev!`) — never raw
`eprintln!`.

Handlers are spawned with **`spawn_user_task`**, not bare `tokio::spawn`:
`tokio::task_local!` does not cross a spawn boundary, so a raw spawn would lose the
user's language (silently serving Persian) and drop the shutdown-drain guard. Only
the long-lived daemon spawns in `src/app/startup.rs` stay raw — converting those
would keep the drain from ever reaching zero.

---

## Jalali (Persian) dates

All user-facing dates are Jalali. The conversion is timezone-correct — never
hand-rolled: `DateTime::from_timestamp` → `.with_timezone(Asia::Tehran)` →
`youtube::jalali::gregorian_to_jalali` (see `rank/panel.rs::fmt_jalali`).

---

## Configuration (`.env`)

The installer creates `.env` from `.env.example`. Key values:

| Variable | Purpose |
|---|---|
| `BOT_TOKEN` | **required** — from @BotFather |
| `ADMIN_USER_ID` | Telegram id with admin panel + emoji-cache access |
| `DATABASE_URL` | `postgres://postgres:postgres@localhost:5432/ros_telegram_bot` (prod: `..._production`) |
| `REDIS_URL` | default `redis://127.0.0.1:6379` |
| `BOT_API_BASE_URL` | `http://127.0.0.1:8081` (prod `:8082`; unset → official API, 20 MB cap) |
| `HEALTH_PORT` | health + metrics port, default `14380`; **dev must use `14381`** |
| `TESTAPI_ENABLED` / `TESTAPI_PORT` | enable the local test API (`1`) and its port (default `14379`) — requires `--features testapi` |
| `COOKIE_REFRESH_ENABLED` | `false` in dev, `true` in prod |
| `SEP_BIND_HOST` | separation-service bind address; defaults to `127.0.0.1`, only override inside a container |
| `DENO_PATH` | path to the deno binary (default `/opt/deno/bin/deno`) |
| `IPINFO_TOKEN`, `ABUSEIPDB_KEY` | optional IP-lookup enrichment |

Rank pricing and the (currently inert) `buy_url` live in `config/config.yml`; user
strings live in `config/i18n.json` for all four languages and can be reloaded at
runtime with `i18n::reload()`.

All feature paths (`files/models/*`, `models/piper/*`, `config/i18n.json`) are
resolved **relative to the bot's working directory**, so the systemd unit must set
`WorkingDirectory` accordingly.

---

## Services & ports

| Service | Unit | Port |
|---|---|---|
| Bot (prod) | `rostam.service` | health 14380 |
| Bot (dev) | `rostam_dev.service` (alias `abc.service`) | health 14381 |
| Separation + CPU broker | `separation.service` | 6589 (loopback) |
| Surge (downloads) | `rostam-surge.service` | 1700 (**all interfaces** — the daemon has no bind-address option; bearer-token authenticated, block the port at the firewall) |
| Local Bot API | `telegram-bot-api.service` | 8081 dev / 8082 prod (loopback) |
| PostgreSQL | `postgresql.service` | 5432 (loopback) |
| Redis | `redis-server` / `redis` | 6379 (loopback) |

No bot port is exposed to the outside world; keep them out of the `ufw` allow list.
`rostam_dev.service` and `separation.service` run as **`mahdi`** with
`NoNewPrivileges=yes` (systemd drop-ins under
`/etc/systemd/system/<unit>.d/`); `rostam.service` still runs as root — see
[plans/README.md](plans/README.md) for the hardening recipe.

```bash
journalctl -u rostam_dev -f              # dev bot logs
curl http://127.0.0.1:6589/health        # separation + CPU broker
curl http://127.0.0.1:14381/health       # dev bot health
```

---

## Updating

```bash
sudo bash /mnt/data/mahdidev/ros/deploy.sh
```

`deploy.sh` runs the whole pipeline (i18n check → push → prod sync → `--release`
build → `patchelf` → restart). Read the script for the exact steps; don't hand-roll
a deploy.

Dev loop:

```bash
cargo build && sudo systemctl restart rostam_dev && journalctl -u rostam_dev -f
```

---

## Manual / development setup

```bash
git clone https://github.com/mmahdi-sz/rostam.git && cd rostam
cp .env.example .env          # fill in BOT_TOKEN
cargo build                   # debug build (needs files/runtime/libvosk.so)
./target/debug/rostam-dev
```

`cargo build` links against `files/runtime/libvosk.so` (see `build.rs`), so the
model/runtime assets must be present — run `install.sh` once to fetch them, or
place them manually.

---

## Known gaps

- **Rank purchase is fully manual.** `buy_url` in `config/config.yml` is inert and
  there is no payment code; the shop page links to the admin. Deliberate.
- **`separation-service` has no endpoint authentication.** It binds to loopback
  (`127.0.0.1:6589`), which closes exposure but not authentication — 9 Rust request
  builders and 5 duplicated `SEP_BASE` constants would have to change.
- **`rostam.service` (production) still runs as root**, along with root-owned files
  under `production/`. The dev unit is hardened; production was deliberately left
  alone. Recipe in [plans/README.md](plans/README.md).
- **`surge` daemon (:1700)** authenticates the bot via a root-owned token file
  (`/root/.local/state/surge/token`); if the bot runs as a non-root user without
  access to it, `tools:surge` returns 401.
- **The 30 s graceful-shutdown drain** (`src/app/mod.rs`) is shorter than real jobs —
  a 10-minute STT run still gets cut off.
- **Cancel-flag maps overwrite instead of rejecting** (`ACTIVE_STT_JOBS`,
  `ACTIVE_DENOISE_JOBS`, `register_upscale`), so a user's second concurrent job
  orphans the first's cancel flag and "cancel" only cancels the newest job.
- **The AI testapi endpoints are mocks** (`/test/stt/recognize`,
  `/test/separation/submit`, `/test/gwm/detect`, `/test/denoise/process`,
  `/test/tts/generate`, `/test/deoldify/colorized`, `/test/nobg/process`) — they build
  a canned response instead of driving the handler, which needs media fixtures + ONNX.
- **Firefox cookie-pool** needs an X display; on a headless server the YouTube
  cookie refresher won't run (YouTube still works without cookies). Set
  `COOKIE_REFRESH_DISPLAY` / run under Xvfb to enable it.
- **`deep-filter` binary** is fetched from DeepFilterNet releases; if that asset
  is unavailable, denoise and STT denoise degrade (base STT still works).
- **Local Bot API** requires Telegram `api_id`/`api_hash` from
  <https://my.telegram.org>; without it, use `--skip-bot-api` (official API, 20 MB
  upload cap).
