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

## Emoji Template System

`{key}` placeholders in any text are expanded at send time using the global
emoji cache (loaded from `ADMIN_USER_ID`'s DB). Each `{key}` is replaced with
a randomly chosen emoji from the matching group.

### Key matching rules (checked in order)

1. **Exact smart_name** — `{fire1}` matches only the item named `fire1`
2. **Prefix group** — `{fire}` matches all items whose smart_name starts with
   `fire` followed by digits (e.g. `fire1`, `fire2`, `fire3`)
3. **Alias group** — `{boss}` matches all items with alias `boss`

One entry is picked at random from the group on every render.

### Where expansion happens

- **Test flow (MarkdownV2 — `/emoji` → Test)**: `{key}` → `![fb](tg://emoji?id=ID)`
- **All plain-text `send_text()` calls** (including i18n strings): `{key}` →
  fallback char + `CustomEmoji` `MessageEntity` at the correct UTF-16 offset,
  merged with the existing UI-emoji entities
- **i18n.json strings** can contain `{key}` — expansion is automatic when
  the string is sent via `send_text()`

### Cache lifecycle

- Loaded at startup from `ADMIN_USER_ID`'s `emoji_items` rows
- Refreshed in background every 5 minutes (opens its own DB connection)
- If `ADMIN_USER_ID` is not set, cache stays empty and `{key}` is left as-is
- Implementation: `src/emoji/cache.rs`, global `CACHE: OnceLock<Arc<RwLock<EmojiCache>>>`

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

Optional emoji cache (requires `DATABASE_URL`):

```text
ADMIN_USER_ID=123456789
```

If set, the emoji cache loads from this user's DB at startup and refreshes
every 5 minutes. Required for `{key}` template expansion (see below).

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
src/emoji/cache.rs      — EmojiCache, {key} expansion, 5-min refresh task
```

### Message Routing Order (main.rs)

For every incoming `Message`, routing happens in this exact order:

1. **addemoji link detection** — if text contains `t.me/addemoji/PackName` AND
   does NOT start with `/`, call `handle_addemoji_link` and skip everything else
2. **active flow handling** — if user has a non-Idle flow state, call
   `handle_emoji_flow_message`; if it returns `true`, skip everything else
3. **command dispatch** — `/emoji`, `/se`, `/start`, `/cookie_*`, YouTube URLs

Messages starting with `/` are never matched as addemoji links (step 1 is
skipped), so commands always reach step 3.

### Flow States

| State | Trigger | Exit |
|-------|---------|------|
| `AwaitingEmojis` | CB_ADD | cancel button, pack chosen, or any input transitions to `AwaitingPackChoice` |
| `AwaitingPackChoice` | emojis collected | pack name typed, inline pack button, or cancel |
| `AwaitingPackAlias` | set-alias button on pack detail | any text (sets or clears alias) |
| `AwaitingTestText` | CB_TEST | cancel button or `/emoji` |
| `AwaitingImportFile` | CB_IMPORT | cancel button or document sent → `AwaitingImportMode` |
| `AwaitingImportMode` | file analyzed | import mode button pressed |

### Adding Emojis — Accepted Input Types

From `AwaitingEmojis` state, the bot accepts **three** input forms:

| Input | How detected | API call |
|-------|-------------|----------|
| Custom emoji entities in message | `MessageEntityType::CustomEmoji` in entities | none needed |
| 19-digit number in text | `extract_19digit_ids()` — word of exactly 19 ASCII digits | `getCustomEmojiStickers` |
| `t.me/addemoji/PackName` link | `extract_addemoji_pack_name()` — matched BEFORE flow (works from any state) | `getStickerSet` |

For the addemoji link path, `fetch_pack_emojis()` calls `getStickerSet`, filters
stickers where `custom_emoji_id.is_some()`, and returns `Vec<PendingEmoji>`.
The flow is then set to `AwaitingPackChoice` regardless of previous state.

All three paths call `filter_duplicates()` which checks both the DB
(`existing_custom_emoji_ids`) and the current in-memory pending list.

### Pending Emoji Display

- Paginated: **30 items per page** (`PENDING_PAGE_SIZE` in `panel.rs`)
- Item numbers are **global** (page 2 starts at `31.`) so filter ops
  like `-31 -32` work correctly across pages
- Page info line `📄 صفحه N از M` shown in message text when `total_pages > 1`
- Prev/next nav buttons (`CB_PENDING_PAGE_PREFIX = "emoji:pendpg:"`) appear
  below the pack list in the keyboard; pressing them edits the message in-place
- `pending_total_pages(count)` helper in `panel.rs` computes page count

### Pack Choice Keyboard

Shown after emojis are collected, in `AwaitingPackChoice` state:

```
[این ایموجی‌ها از کجان؟ 🔗]    ← green btn, calls getCustomEmojiStickers
                                   groups by set_name, shows t.me/addemoji/ links
[PackName]                        ← btn_icon, icon = set_default(⭐) if default,
[PackName]                           pack_folder(📁) otherwise — all premium
[قبلی ⬅]  [➡ بعدی]               ← only if total_pages > 1
```

Typing a pack name creates a new pack if it doesn't exist.
`+N` / `-N` tokens edit the pending list (whitelist / blacklist by index).

### Callback Prefixes

All emoji callbacks start with `emoji:`. Full list of `CB_*` constants in
`src/emoji/panel.rs`:

| Constant | Value | Action |
|----------|-------|--------|
| `CB_ADD` | `emoji:add` | enter AwaitingEmojis |
| `CB_TEST` | `emoji:test` | enter AwaitingTestText |
| `CB_LIST` | `emoji:list` | show paginated emoji list |
| `CB_DELETE_PACK_MENU` | `emoji:delpack` | show packs for deletion |
| `CB_PACKS` | `emoji:packs` | show pack management list |
| `CB_IMPORT` | `emoji:import` | enter AwaitingImportFile |
| `CB_EXPORT` | `emoji:export` | generate and send SQL file |
| `CB_BACK` | `emoji:back` | return to main panel |
| `CB_CANCEL` | `emoji:cancel` | same as back |
| `CB_SHOW_PACK_LINKS` | `emoji:packlinks` | show source pack links |
| `CB_BACK_TO_PACK_CHOICE` | `emoji:backpick` | return from pack links to pending |
| `CB_PACK_OPEN_PREFIX` | `emoji:pack:` | open pack detail (+ pack id) |
| `CB_PACK_SET_DEFAULT_PREFIX` | `emoji:setdef:` | set pack as default |
| `CB_PACK_SET_ALIAS_PREFIX` | `emoji:setalias:` | enter AwaitingPackAlias |
| `CB_PACK_DELETE_PREFIX` | `emoji:packdel:` | delete pack |
| `CB_PICK_PACK_PREFIX` | `emoji:pickpack:` | pick pack to add emojis into |
| `CB_LIST_PAGE_PREFIX` | `emoji:listpg:` | navigate emoji list page |
| `CB_PENDING_PAGE_PREFIX` | `emoji:pendpg:` | navigate pending emojis page |
| `CB_IMPORT_REPLACE` | `emoji:import:replace` | execute replace import |
| `CB_IMPORT_MERGE` | `emoji:import:merge` | execute merge import |
| `CB_IMPORT_SMART` | `emoji:import:smart` | execute smart-merge import |

### Emoji List Format

```
• ![fallback](tg://emoji?id=ID) fallback = numeric_id | smart_name | alias
```

Rendered as MarkdownV2 with `tg://emoji` inline images (not entities).
Link preview disabled on all list messages.

### Export / Import

- **Export**: generates `emoji_{jalali-date}_{HH-MM}.sql` with
  `CREATE TABLE IF NOT EXISTS` + `INSERT` for the current user only.
  Sent as a Telegram document.
- **Import**: user sends the SQL file. Bot parses and analyzes it, shows a
  report with file stats + DB stats + duplicate count, then offers:
  - **جایگزین** — delete all current data, insert from file (replace mode)
  - **ادغام** — append to existing data, IDs continue (merge mode)
  - **ادغام هوشمند** — append, skip duplicate `custom_emoji_id`s (smart-merge)
  - If DB is empty, only a single confirm button is shown (always merge mode)
- Implemented in `src/emoji/import.rs`: `parse_sql` → `analyze` → `execute_replace` / `execute_merge`

### ID Sequence Reset

When a user deletes their last pack, both `emoji_packs_id_seq` and
`emoji_items_id_seq` are reset to 1 so the next pack starts from id=1.

### UX Notes

- The "pack source links" button is labeled **"این ایموجی‌ها از کجان؟ 🔗"** —
  answers what the user is thinking rather than describing the action.
- `icon_custom_emoji_id` on `InlineKeyboardButton` always renders to the
  **LEFT** of button text regardless of RTL — no API exists to change this.
- Pack buttons (`packs_keyboard`, `pack_choice_keyboard`) use `btn_icon` with
  `set_default` icon (⭐ premium) for default packs and `pack_folder` (📁
  premium) for others — plain unicode was replaced to get premium rendering.
- After adding emojis, the bot returns to the main panel. A future improvement
  would be to navigate directly to the target pack's detail view instead.

## Source Layout

```text
src/main.rs                          — event loop + routing (~160 lines)
src/config.rs                        — BOT_TOKEN / DATABASE_URL / ADMIN_USER_ID reading
src/bot.rs                           — send_text, send_text_md, send_start_button
src/cookie_pool.rs                   — CookiePool + format helpers + save_snapshot
src/youtube.rs                       — yt-dlp fetch + handle_youtube_url
src/i18n.rs                          — t() / tf() / entities_for_text() helpers, reads i18n.json
src/database/mod.rs
src/database/posfreSQL/postgresql.rs — PostgreSQL connection + cookie pool tables
src/database/posfreSQL/schema.sql    — CREATE TABLE statements
src/emoji/mod.rs
src/emoji/cache.rs                   — EmojiCache, {key} expansion, 5-min refresh task
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
