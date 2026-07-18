# rostam — Telegram media & utility bot

**English** · [فارسی](README_fa.md)

A multilingual Telegram bot (Rust, crate `ros-telegram-bot`) — UI in Persian,
English and Italian (switch with `/language`) — that bundles a media
toolbox — YouTube downloading, vocal separation, speech-to-text, audio denoise,
image upscaling, watermark removal, PDF compression, IP lookup, custom emoji
packs — behind a ranked/paywalled UI. Most ML runs **locally and in-process**
(ONNX, CTranslate2, Vosk); the remaining heavy CPU work runs in sidecar services
coordinated by a Redis-backed CPU broker.

---

## Quick install (bare server)

On a fresh **Debian/Ubuntu** or **Arch** server, one command sets up everything:

```bash
bash <(curl -Ls https://raw.githubusercontent.com/mmahdi-sz/rostam/master/install.sh)
```

The installer auto-elevates with `sudo`, clones the repo to `/opt/rostam`, and
provisions the whole stack. It is **idempotent** — safe to re-run. It will prompt
for your `BOT_TOKEN` (and, for the local Bot API, Telegram `api_id`/`api_hash`).

> The installer downloads ~4.2 GB of models, and builds
> the Rust bot (and optionally the Telegram Bot API server) from source — the
> first run takes a while and needs ~12 GB free disk.

### Installer options

```
--dir <path>      install location (default /opt/rostam)
--branch <name>   git branch (default master)
--skip-bot-api    skip building the local Telegram Bot API server
--skip-firefox    skip Firefox (cookie-pool refresher)
--fresh           re-clone / rebuild from scratch
```

---

## What it installs

| Layer | Details |
|---|---|
| **System packages** | git, curl, unzip, tar, ffmpeg, ghostscript, PostgreSQL, Redis, Python 3 (+venv), build tools, cmake, Firefox |
| **Rust** | via `rustup` (edition 2024 needs rustc ≥ 1.85) |
| **deno** | to `/opt/deno` — passed to yt-dlp `--js-runtimes` for YouTube JS challenges |
| **yt-dlp** | latest static binary → `/usr/local/bin/yt-dlp` |
| **Models (~4.2 GB)** | Vosk STT (fa/en), Moebius ONNX (watermark), DeepFilterNet, Real-ESRGAN, `libvosk.so` → `files/` |
| **PostgreSQL** | creates DB `ros_telegram_bot` (bot creates its own tables on first start) |
| **The bot** | `cargo build --release` → `rostam.service` |
| **separation-service** | vocal/instrumental split (:6589), auto-downloads its model |
| **surge** | [SurgeDM/Surge](https://github.com/SurgeDM/Surge) parallel download manager daemon (:1700), latest release binary → `surge.service` |
| **Local Telegram Bot API** | built from tdlib source (:8081) — raises the upload cap to 2 GB |

---

## Architecture

```
                       ┌──────────────────────────┐
   Telegram  ─────────►│  local Bot API  :8081     │  (2 GB uploads)
                       └────────────┬─────────────┘
                                    │
                          ┌─────────▼─────────┐        ┌───────────────────┐
                          │  rostam (Rust)    │───────►│  in-process ML     │
                          │  target/release/  │        │  ort  (Moebius)    │
                          └───┬───────┬───┬───┘        │  ct2rs (NLLB)      │
             ┌────────────────┘       │   └───────┐    │  vosk (STT)        │
             │                        │           │    └───────────────────┘
      ┌──────▼──────┐          ┌──────▼──────┐  ┌─▼────────────┐
      │ PostgreSQL  │          │   Redis     │  │  Firefox     │
      │  :5432      │          │   :6379     │  │  cookie pool │
      └─────────────┘          └──────┬──────┘  └──────────────┘
                                      │
                          ┌───────────┴───────────┐
                          │      CPU broker        │  (queue + core reservation,
                          │   hosted on :6589      │   Redis-backed)
                          └───────────┬───────────┘
                            ┌─────────┴─────────┐
                    ┌───────▼────┐      ┌───────▼────┐
                    │ separation │      │  surge dl  │
                    │   :6589    │      │   :1700    │
                    └────────────┘      └────────────┘
```

The bot connects to Postgres, Redis and the sidecars **lazily** — none is
required at startup. If Postgres is down it runs with persistence disabled; if a
sidecar is down, only that feature returns "service unavailable". The only hard
startup requirement is `BOT_TOKEN`.

The heaviest ML runs **in-process** inside the Rust binary (no Python round-trip):
watermark inpainting via the `ort` ONNX runtime, subtitle translation via `ct2rs`
(CTranslate2), and speech-to-text via `vosk`. Only vocal separation and parallel
direct downloads live in external sidecars. Any multi-second CPU job — in-process
or sidecar — is scheduled through the **CPU broker** (`:6589`), which queues work
and reserves physical cores via Redis so concurrent tasks don't thrash the box.

---

## Features

YouTube download (quality/subtitle/traffic paywalls) · vocal/instrumental
separation · speech-to-text (Vosk) · audio denoise (DeepFilterNet)
· image upscale (Real-ESRGAN) · watermark removal (Moebius ONNX, in-process) ·
PDF compression (Ghostscript) · fast parallel direct-link downloads (Surge) ·
IP lookup · custom emoji packs · ranks & paywall · referrals · admin stats panel.

Commands: `/start`, `/panel`, `/language`, `/rank`, `/emoji`, `/se`.

---

## AI & Machine-Learning pipeline

Every model runs **locally** — no external AI API, no per-call cost, no data
leaving the box.

| Capability | Model / engine | Runs where |
|---|---|---|
| **Subtitle translation** | Meta **NLLB** via `ct2rs` (CTranslate2 native binding) | in-process |
| **Speech-to-text** | **Vosk** (Persian + English, small & large models) | in-process (`libvosk.so`) |
| **Audio denoise** | **DeepFilterNet 3** (`deep-filter`, ONNX) | subprocess |
| **Image upscale** | **Real-ESRGAN** (ncnn-vulkan): photo + anime models (x2/x3/x4) | subprocess |
| **Watermark removal** | **Moebius** ONNX inpainting (`ort` crate) | in-process |

- **Subtitle translate** — when a video has no Persian subtitle, the bot pulls the
  English SRT, feeds it through the local NLLB translator (`src/youtube/translator/`),
  and emits `translated_fa.srt` with the original timing preserved.
- **STT** — pick language + model size from an inline keyboard. Large ("accurate")
  models are paywalled; the small ones are free. An optional DeepFilterNet denoise
  pass can be toggled before transcription.
- **Upscale** — a photo mode plus a dedicated **Anime** toggle exposing
  `realesrgan-x4plus-anime` and `realesr-animevideov3` at x2/x3/x4.

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

State survives restarts via three tables: `cookie_pool_cookies`,
`cookie_pool_state`, `cookie_pool_cooldowns`.

---

## CPU broker & job queue

Any multi-second CPU task (separation, upscale, watermark, translate, denoise)
goes through a **Redis-backed broker** hosted by the separation service on
`:6589`:

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
- **Buttons are always visible**; the paywall fires on click, so lower ranks see
  what they're missing.
- Gated: YouTube quality, YouTube subtitles, YouTube traffic (daily + monthly,
  estimated from bitrate × duration), denoise, STT accurate models, STT quota,
  separation quota, image-upscale quota.
- Quotas live in `user_quotas` (per user, per type, windowed).

---

## Referral system (anti-fraud)

Invite link: `https://t.me/{bot}?start={referrer_id}`.

- A `/start` with a numeric payload records a **pending** referral
  (`referral_pending`) — it does **not** score immediately.
- An hourly sweeper (`sweep_confirm`) re-checks pending rows older than
  **`PENDING_DAYS` = 2**: if the invitee is still a force-join member they're
  promoted to `referrals` (1 point); if they left, the row is discarded. This kills
  the "join → get counted → leave" fraud loop.
- Points are **spent** to activate a rank for **31 days** via a referral-specific
  ladder (`TIERS`): Sohrab = 10, Esfandyar = 20, Rostam = 50. Balance =
  `COUNT(referrals) − SUM(points_spent)`. Downgrades are rejected.
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

---

## Trace logging

Every user action carries one **trace id** threaded through every helper, so
`rg trace=N` replays the whole action in order. Two line kinds:

```
[<domain> trace=N actor] user=@<u> id=<id> rank=<R> clicked=<cb>    # once, at entry
[<domain> trace=N event=<step>] k=v => ok|fail err=..|pass|blocked  # per step, BEFORE it runs
```

A missing event line means that step was never reached — the absence *is* the
signal. Example (fails at CPU acquire — `cores=[]` is the smoking gun):

```
[upscale trace=43 actor] user=@parsa id=671234… rank=Dalavar clicked=upscale:model:realesrgan-x4plus
[upscale trace=43 event=quota_check] used=2 limit=3 => pass
[upscale trace=43 event=cpu_acquire] => cores=[]
```

Tail with `journalctl -u rostam -n 300 | rg trace=`.

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
| `DATABASE_URL` | `postgres://postgres:postgres@localhost:5432/ros_telegram_bot` |
| `REDIS_URL` | default `redis://127.0.0.1:6379` |
| `BOT_API_BASE_URL` | `http://127.0.0.1:8081` (local Bot API; unset → official API, 20 MB cap) |
| `DENO_PATH` | path to the deno binary (default `/opt/deno/bin/deno`) |
| `IPINFO_TOKEN`, `ABUSEIPDB_KEY` | optional IP-lookup enrichment |

All feature paths (`files/models/*`, `config/i18n.json`) are resolved **relative
to the bot's working directory** (`/opt/rostam`), so the systemd unit sets
`WorkingDirectory` accordingly.

---

## Services & ports

| Service | Unit | Port |
|---|---|---|
| Bot | `rostam.service` | — |
| Separation | `separation.service` | 6589 |
| Surge (downloads) | `surge.service` | 1700 |
| Local Bot API | `telegram-bot-api.service` | 8081 |
| PostgreSQL | `postgresql.service` | 5432 |
| Redis | `redis-server` / `redis` | 6379 |

```bash
journalctl -u rostam -f                 # bot logs
curl http://127.0.0.1:6589/health       # separation
```

---

## Updating

```bash
cd /opt/rostam
git pull
cargo build --release
sudo systemctl restart rostam
```

Or re-run the installer (idempotent) to also refresh assets and sidecars.

---

## Manual / development setup

```bash
git clone https://github.com/mmahdi-sz/rostam.git && cd rostam
cp .env.example .env          # fill in BOT_TOKEN
cargo build                   # debug build (needs files/runtime/libvosk.so)
./target/debug/ros-telegram-bot
```

`cargo build` links against `files/runtime/libvosk.so` (see `build.rs`), so the
model/runtime assets must be present — run `install.sh` once to fetch them, or
place them manually.

---

## Known gaps

- **`surge` daemon (:1700)** is installed from [SurgeDM/Surge](https://github.com/SurgeDM/Surge)
  and authenticates the bot via a root-owned token file
  (`/root/.local/state/surge/token`). Both the daemon and the bot therefore run
  as **root** so they share that token — running the bot as a non-root user would
  make `tools:surge` return 401.
- **Firefox cookie-pool** needs an X display; on a headless server the YouTube
  cookie refresher won't run (YouTube still works without cookies). Set
  `COOKIE_REFRESH_DISPLAY` / run under Xvfb to enable it.
- **`deep-filter` binary** is fetched from DeepFilterNet releases; if that asset
  is unavailable, STT denoise degrades (base STT still works).
- **Local Bot API** requires Telegram `api_id`/`api_hash` from
  <https://my.telegram.org>; without it, use `--skip-bot-api` (official API, 20 MB
  upload cap).
