# rostam — Telegram media & utility bot

**English** · [فارسی](README_fa.md)

A multilingual Telegram bot (Rust, crate `ros-telegram-bot`) — UI in Persian,
English and Italian (switch with `/language`) — that bundles a media
toolbox — YouTube downloading, vocal separation, speech-to-text, audio denoise,
image upscaling, watermark removal, PDF compression, IP lookup, custom emoji
packs — behind a ranked/paywalled UI. Heavy CPU work runs in Python sidecar
services coordinated by a Redis-backed CPU broker.

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
                          ┌─────────▼─────────┐
                          │  rostam (Rust)    │
                          │  target/release/  │
                          └───┬───────┬───┬───┘
             ┌────────────────┘       │   └─────────────────┐
      ┌──────▼──────┐          ┌──────▼──────┐       ┌──────▼───────┐
      │ PostgreSQL  │          │   Redis     │       │  sidecars    │
      │  :5432      │          │   :6379     │◄──────│  CPU broker  │
      └─────────────┘          └─────────────┘       └──────┬───────┘
                                                    ┌────────┴────────┐
                                            ┌───────▼────┐  ┌─────────▼──┐
                                            │ separation │  │  surge dl  │
                                            │   :6589    │  │   :1700    │
                                            └────────────┘  └────────────┘
```

The bot connects to Postgres, Redis and the sidecars **lazily** — none is
required at startup. If Postgres is down it runs with persistence disabled; if a
sidecar is down, only that feature returns "service unavailable". The only hard
startup requirement is `BOT_TOKEN`.

---

## Features

YouTube download (quality/subtitle/traffic paywalls) · vocal/instrumental
separation · speech-to-text (Vosk) · audio denoise (DeepFilterNet)
· image upscale (Real-ESRGAN) · watermark removal (Moebius ONNX, in-process) ·
PDF compression (Ghostscript) · fast parallel direct-link downloads (Surge) ·
IP lookup · custom emoji packs · ranks & paywall · referrals · admin stats panel.

Commands: `/start`, `/panel`, `/language`, `/rank`, `/emoji`, `/se`.

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
