# CLAUDE.md

## Project Summary

This project is a Rust Telegram bot named `ros-telegram-bot`.
It uses the `frankenstein` crate for Telegram Bot API access and runs as a
systemd service named `abc`.

The bot currently supports:

- Telegram long polling with `getUpdates`
- `/start` command with a green inline button
- Reading `BOT_TOKEN` from `.env`, `/etc/default/abc`, or process env
- Firefox Cookie Pool management for user `mahdi`
- Optional PostgreSQL persistence for Cookie Pool state
- systemd deployment through `abc.service`
- Local bare Git server under `git-server/ros-telegram-bot.git`

Secrets are not tracked. `.env`, `target/`, and `git-server/` are ignored.

## Runtime

Debug build is the intended runtime target:

```bash
cargo build
systemctl restart abc
```

The systemd unit runs:

```text
/mnt/data/mahdidev/ros/dev/target/debug/ros-telegram-bot
```

Service file:

```text
systemd/abc.service
```

Installed unit:

```text
/etc/systemd/system/abc.service
```

Current service name:

```text
abc.service
```

Useful service commands:

```bash
systemctl status abc --no-pager
journalctl -u abc -f
systemctl restart abc
```

## Environment

The bot reads config in this order:

1. `.env`
2. `/etc/default/abc`
3. process environment

Required:

```text
BOT_TOKEN=...
```

Optional PostgreSQL:

```text
DATABASE_URL=postgres://postgres:postgres@localhost:5432/ros_telegram_bot
```

If `DATABASE_URL` is missing, Cookie Pool state stays in memory only.

## Telegram Commands

```text
/start
```

Sends a message with an inline green button. Pressing the button replies with
`سلام`.

```text
/cookie_status
```

Shows Cookie Pool state:

- `available_cookies`
- `selectable_cookies`
- `cooldown_list`
- `last_used_cookie`
- `next_available_in`

```text
/cookie_next
```

Selects the next Firefox cookie profile and returns the `yt-dlp` browser spec:

```bash
yt-dlp --cookies-from-browser 'firefox:/path/to/profile'
```

```text
/cookie_429
```

Marks the last selected cookie as rate-limited and moves it to a 20-hour
cooldown.

## Cookie Pool Rules

Implemented in:

```text
src/cookie_pool.rs
```

Rules:

- Initial pool is discovered from Firefox profiles under:

```text
/home/mahdi/.mozilla/firefox
```

- Maximum pool size is 20.
- Selection excludes cookies currently in cooldown.
- Selection excludes `last_used_cookie`.
- Selection is random from the remaining pool.
- If no cookie is selectable, the bot reports that the pool is empty and shows
  the next cooldown expiration time.
- A cookie enters cooldown only after `/cookie_429`.
- Cooldown duration is 20 hours.

Important state:

```text
available_cookies
last_used_cookie
cooldown_list
```

## PostgreSQL Storage

Database code lives under the requested path:

```text
src/database/posfreSQL/postgresql.rs
src/database/posfreSQL/schema.sql
```

Rust module bridge:

```text
src/database/mod.rs
```

Stored tables:

- `cookie_pool_cookies`
- `cookie_pool_state`
- `cookie_pool_cooldowns`

The database layer stores:

- discovered Firefox cookie profiles
- last used cookie id
- cooldown entries and expiration epochs

The schema is created automatically at startup when `DATABASE_URL` is set.

## Git Server

A local bare Git repository was created as a simple Git server:

```text
git-server/ros-telegram-bot.git
```

Remote:

```text
origin -> git-server/ros-telegram-bot.git
```

Current branch:

```text
master
```

Commits were made chunk by chunk with explanatory commit bodies:

```text
dccc92c chore: add project config and deployment docs
a9dd547 feat: add firefox cookie pool manager
fb0f75f feat: add postgresql cookie pool storage
490ace4 feat: wire cookie pool into telegram bot
```

## What Is Still Needed For A Full YouTube Downloader Bot

The project does not yet execute `yt-dlp` downloads. The Cookie Pool and
database support are ready, but the downloader workflow still needs:

- `/dl <youtube-url>` command
- YouTube URL validation
- `yt-dlp` process execution
- output directory management
- file size checks for Telegram sending
- automatic 429 detection from `yt-dlp` output
- retry with the next cookie when 429 happens
- queueing so multiple downloads do not corrupt Cookie Pool state
- cleanup of old downloaded files

## Verification Done

Verified:

```bash
cargo build
systemctl restart abc
systemctl status abc --no-pager
```

The service was active after restart and loaded 15 Firefox profiles into the
Cookie Pool.
