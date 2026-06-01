# CLAUDE.md

## After Every Change (MUST FOLLOW)

After every code or config change:

1. Stage and commit to the local git repository:

```bash
git add <changed files>
git commit -m "..."
```

2. Restart the service:

```bash
systemctl restart abc
```

Always do both steps — commit first, then restart.

## Strings & i18n (MUST FOLLOW)

All user-facing strings — Telegram messages, captions, button labels,
error messages shown to users — MUST live in `i18n.json` at the repo root,
NOT hardcoded in Rust source.

Rules:

- Add the string to `i18n.json` under a nested key (e.g.
  `youtube.caption.channel_label`).
- In code, read it via `i18n::t("key")` or `i18n::tf("key", &[("name", value)])`
  for templates with `{placeholders}`.
- Operator/dev-facing strings (`println!`, `eprintln!`, panics, journalctl
  logs) stay hardcoded — i18n is for end-user text only.
- The file is currently single-language (Farsi). Structure is nested JSON.

## Premium Emoji System

All UI emoji are premium custom emoji managed via `i18n.json`.

### How it works

- **IDs**: stored in `emoji.panel.icons.*` in `i18n.json` (24 emoji, e.g. `"cancel": "5215204871422093648"`)
- **Inline keyboard buttons**: use `icon_custom_emoji_id` field on `InlineKeyboardButton` — handled automatically by `btn_icon()` in `src/emoji/panel.rs`
- **Reply keyboard buttons**: use `icon_custom_emoji_id` field on `KeyboardButton` struct literal (bon typestate issue prevents builder use)
- **Plain text messages**: `entities_for_text(text)` in `src/i18n.rs` scans the text for known emoji chars, looks up their premium IDs, and returns `Vec<MessageEntity>` — called automatically by `send_text()` in `src/bot.rs`
- **MarkdownV2 messages** (list page, pending emojis): entities are NOT added — they contain `tg://emoji` inline images that would conflict

### Adding a new premium emoji

1. Add `"key": "ID"` to `emoji.panel.icons` in `i18n.json`
2. Add `("🔥", "key")` to `EMOJI_MAP` in `src/i18n.rs` (longer/variation-selector forms first)
3. Use `btn_icon(text, CB_FOO, "key")` for inline buttons, or just put the emoji char in any text message — `send_text()` handles the rest automatically

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
- Full emoji management panel (`/emoji`)

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
/emoji
```

Opens the emoji management panel. Clears any active flow for the user.

```text
/se [id_or_name] [alias]
```

Sets an alias on an emoji item. Example: `/se 5 boss` or `/se sparkle1 star`.
Use `-` as alias to remove it.

```text
/cookie_status
```

Shows Cookie Pool state.

```text
/cookie_next
```

Selects the next Firefox cookie profile and returns the `yt-dlp` browser spec.

```text
/cookie_429
```

Marks the last selected cookie as rate-limited and moves it to a 20-hour
cooldown.

## Emoji Panel

Implemented across:

```text
src/emoji/mod.rs
src/emoji/flow.rs       — FlowManager, FlowState, PendingEmoji
src/emoji/handler.rs    — all callback + message handlers
src/emoji/panel.rs      — keyboard builders, text formatters, CB_* constants
src/emoji/store.rs      — all DB queries
src/emoji/smart_name.rs — unicode → ASCII smart name
src/emoji/import.rs     — SQL parse, analyze, execute import modes
```

### Flow States

| State | Trigger | Exit |
|-------|---------|------|
| `AwaitingEmojis` | CB_ADD | cancel button or pack chosen |
| `AwaitingPackChoice` | emojis collected | pack name typed or inline button |
| `AwaitingPackAlias` | set alias button | any text |
| `AwaitingTestText` | CB_TEST | cancel button or `/emoji` |
| `AwaitingImportFile` | CB_IMPORT | cancel button or document sent |
| `AwaitingImportMode` | file analyzed | import mode button pressed |

### Callback Prefixes

All emoji callbacks start with `emoji:`. Defined as `CB_*` constants in
`src/emoji/panel.rs`.

### Emoji List Format

```
• ![fallback](tg://emoji?id=ID) fallback = numeric_id | smart_name | alias
```

Premium emoji comes first, then static fallback.

### Export / Import

- **Export**: generates `emoji_{jalali-date}_{HH-MM}.sql` with `CREATE TABLE IF NOT EXISTS` + `INSERT` for the current user only. Sent as a Telegram document.
- **Import**: user sends the SQL file. Bot parses and analyzes it, shows a report with counts and duplicates, then offers:
  - **جایگزین** — delete all current data, insert from file
  - **ادغام** — append to existing data, IDs continue
  - **ادغام هوشمند** — append, skip duplicate `custom_emoji_id`s
  - If DB is empty, only a single confirm button is shown.

### ID Sequence Reset

When a user deletes their last pack, both `emoji_packs_id_seq` and
`emoji_items_id_seq` are reset to 1 so the next pack starts from id=1.

## Source Layout

```text
src/main.rs                          — event loop + routing (~160 lines)
src/config.rs                        — BOT_TOKEN / DATABASE_URL reading
src/bot.rs                           — send_text, send_text_md, send_start_button
src/cookie_pool.rs                   — CookiePool + format helpers + save_snapshot
src/youtube.rs                       — yt-dlp fetch + handle_youtube_url
src/i18n.rs                          — t() / tf() / entities_for_text() helpers, reads i18n.json
src/database/mod.rs
src/database/posfreSQL/postgresql.rs — PostgreSQL connection + cookie pool tables
src/database/posfreSQL/schema.sql    — CREATE TABLE statements
src/emoji/mod.rs
src/emoji/flow.rs
src/emoji/handler.rs
src/emoji/panel.rs
src/emoji/store.rs
src/emoji/smart_name.rs
src/emoji/import.rs
```

## PostgreSQL Tables

Cookie pool:
- `cookie_pool_cookies`
- `cookie_pool_state`
- `cookie_pool_cooldowns`

Emoji:
- `emoji_packs` (id SERIAL, owner_user_id, name, alias, is_default, item_count)
- `emoji_items` (id SERIAL, pack_id, owner_user_id, custom_emoji_id, fallback, smart_name, alias, position)

Schema is created automatically at startup when `DATABASE_URL` is set.

## Cookie Pool Rules

Implemented in `src/cookie_pool.rs`.

- Pool discovered from Firefox profiles under `/home/mahdi/.mozilla/firefox`
- Maximum pool size: 20
- Selection excludes cookies in cooldown and `last_used_cookie`
- Selection is random from remaining pool
- Cooldown duration: 20 hours, triggered only by `/cookie_429`

## Git Server

```text
origin -> git-server/ros-telegram-bot.git
branch: master
```
