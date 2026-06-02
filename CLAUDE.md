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

## Debug & Trace Logging (MUST FOLLOW)

Every non-trivial bot flow must have enough operator-facing logs to trace one
user action from routing to final Telegram/API response. Logs are hardcoded
dev/operator text and do NOT belong in `i18n.json`.

Rules:

- Add a stable per-action trace id for multi-step flows. Keep the same trace id
  across routing, handler calls, external commands/API calls, parsing,
  Telegram sends/edits, callbacks, retries, and failure branches.
- Use structured, grep-friendly log lines with a domain prefix and event name,
  e.g. `[youtube trace=12 event=fetch_parsed] heights=[144, 240, 720]`.
- Log routing inputs: `user_id`, `chat_id`, command/callback prefix, URL or
  identifier, and the selected branch/handler. This is how we verify whether a
  message reached the intended flow.
- Log important function boundaries: handler start, inputs passed to helper
  functions, outputs returned by helpers, and which next function receives that
  output.
- Log external work: command/API name, sanitized args, exit status, parse
  summary, retry decisions, selected cookie/profile id, and rate-limit/bad-cookie
  branches.
- Log Telegram operations: send/edit/callback-answer attempts, success events,
  and Telegram error descriptions when they fail.
- Log dynamic UI decisions: which buttons were built, callback data prefixes,
  detected formats/qualities, page numbers, item ids, and why a panel was
  skipped.
- Do not log secrets: bot tokens, raw cookie values, full database URLs, or
  private file contents. Local profile ids/paths and Telegram user/chat ids are
  acceptable for operator debugging in this private deployment.
- Keep logs concise but complete enough that `journalctl -u abc --no-pager -n
  300 | rg "domain|trace|event"` can show where the flow broke.

Current YouTube example:

```text
[youtube trace=1 event=route_youtube_url] user_id=... chat_id=... url=...
[youtube trace=1 event=fetch_parsed] format_count=28 requested_format_count=2 heights=[144, 240, 360, 480, 720]
[youtube trace=1 event=quality_prompt_buttons] available_heights=[144, 240, 360, 480, 720] button_heights=[720, 480, 360, 240, 144]
```

## Emoji Template System

`{key}` placeholders in any text are expanded at send time using the global
emoji cache (loaded from `ADMIN_USER_ID`'s DB). Each `{key}` is replaced with
a randomly chosen emoji from the matching group.

### Key matching rules (checked in order)

All keys resolve via a flat `HashMap` pre-built at cache load time.

**Global keys (default pack context):**

1. **Exact smart_name** — `{fire1}` matches only the item named `fire1`
2. **Prefix group** — `{fire}` matches all items whose smart_name starts with
   `fire` followed by digits (e.g. `fire1`, `fire2`, `fire3`)
3. **Alias group** — `{boss}` matches all items with alias `boss`
4. **Item DB id** — `{43}` matches the item whose `id = 43` (shown in the list
   as the number before the `|`, e.g. `🔥 = 43 | fire4 | blue_fire`)
5. **Raw Telegram emoji id** — `{5188481279963715781}` (19-digit number,
   passes through as a raw `tg://emoji?id=...` link without a cache lookup)

**Pack-scoped keys (`{pack_ident:item_key}`):**

Use a colon to scope the lookup to a specific pack. The pack identifier can be:
- Pack **name** — `{terraria:stone}`
- Pack **alias** — `{terra:stone}` (if pack alias is `terra`)
- Pack **numeric id** — `{2:stone}`

The item key after the colon follows the same rules as global keys:
- `{terraria:stone1}` — exact smart_name in pack
- `{terraria:stone}` — prefix group in pack (random from stone1, stone2 …)
- `{terraria:boss}` — alias group in pack
- `{terraria:43}` or `{2:43}` — item by DB id in pack

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
- Implementation: `src/emoji/cache/` (mod, loader, render, types), global `CACHE: OnceLock<Arc<RwLock<EmojiCache>>>`
- `loader.rs` JOINs `emoji_packs` to build pack-scoped keys at load time

## Premium Emoji System

All UI emoji are premium custom emoji managed via `i18n.json`.

### How it works

- **IDs**: stored in `emoji.panel.icons.*` in `i18n.json` (24 emoji, e.g. `"cancel": "5215204871422093648"`)
- **Static UI emoji map**: `src/i18n/emoji_map.rs` maps visible emoji
  chars (e.g. `✅`, `❌`, `📁`) to icon keys in `emoji.panel.icons.*`.
  Keep variation-selector forms first when needed (e.g. `⭐️` before `⭐`).
- **Plain text messages**: `send_text()` in `src/bot.rs` calls
  `expand_and_entify()`, which expands dynamic `{key}` templates from the
  emoji cache and then calls `entities_for_text()` in `src/i18n/entities.rs`.
  Any known UI emoji char inside an `i18n.json` string becomes a premium
  `MessageEntity::CustomEmoji` automatically, with UTF-16 offsets computed
  by code.
- **Inline keyboard buttons**: use `icon_custom_emoji_id` on
  `InlineKeyboardButton`. In emoji panel code, use `btn_icon(text,
  callback_data, "icon_key")` from `src/emoji/panel/buttons.rs`; it looks up
  `emoji.panel.icons.icon_key` in `i18n.json` and applies
  `ButtonStyle::Primary`. Use/extend the local helpers for other colors
  (`ButtonStyle::Success`, `Danger`, `Primary`) instead of hardcoding button
  structs everywhere.
- **Reply keyboard buttons**: use `icon_custom_emoji_id` on `KeyboardButton`.
  Current code builds these as struct literals in
  `src/emoji/panel/keyboards.rs` because the builder has typestate friction.
- **MarkdownV2 messages**: `send_text_md()` does NOT add premium emoji
  entities. For MarkdownV2 text that needs premium UI emoji, use
  `apply_premium_to_md()` from `src/i18n/premium_md.rs`, which converts known
  UI emoji into `![emoji](tg://emoji?id=ID)`. Do not mix Telegram entities
  with MarkdownV2 unless the code is deliberately handling both.

### i18n.json rules for premium emoji

- All user-facing Telegram strings still belong in `i18n.json`, including
  captions, button labels, status messages, and user-visible errors.
- To show a fixed premium UI emoji in a plain `send_text()` message, put the
  visible emoji char directly in the `i18n.json` value, and make sure that char
  exists in `EMOJI_MAP` with a matching ID in `emoji.panel.icons.*`.
- To use a dynamic emoji from the user's/admin's emoji DB, put a `{key}`
  placeholder in the `i18n.json` value. It is expanded only when sent through
  `send_text()` and only if the global emoji cache is loaded.
- For inline/reply keyboard icons, do NOT put the emoji char in the button
  label just to get premium rendering. Pass the icon key and let
  `icon_custom_emoji_id` render the icon. Telegram renders inline button icons
  to the left of the text even in RTL.
- If an icon key lookup returns an empty string or `!missing.key!`, helpers
  should omit the icon rather than sending an invalid custom emoji ID.

### Adding a new premium emoji

1. Add `"key": "ID"` to `emoji.panel.icons` in `i18n.json`
2. Add `("🔥", "key")` to `EMOJI_MAP` in `src/i18n/emoji_map.rs`
   (longer/variation-selector forms first)
3. Use `btn_icon(text, CB_FOO, "key")` for inline buttons, a
   `KeyboardButton { icon_custom_emoji_id: Some(t("emoji.panel.icons.key")),
   ... }` for reply keyboards, or just put the emoji char in any plain text
   message sent through `send_text()`

### YouTube downloader UI reminder

When adding the YouTube downloader flow, keep all user-visible downloader
strings under `youtube.*` in `i18n.json`. Use colored inline buttons with
`icon_custom_emoji_id` for actions such as download, audio/video choice,
quality choice, cancel, retry, and back. Plain status/error messages should go
through `send_text()` so `{key}` templates and UI premium emoji render
automatically; MarkdownV2 captions need explicit Markdown premium handling.

## YouTube Downloader Current State

Implemented across:

```text
src/youtube/extract.rs          — YouTube URL detection
src/youtube/fetch.rs            — yt-dlp metadata fetch + format/codec parsing
src/youtube/format.rs           — preview caption/description formatting
src/youtube/handle.rs           — URL handler, cookie retry flow, preview sending
src/youtube/quality_keyboard.rs — quality + codec inline keyboards/callbacks
src/youtube/trace.rs            — trace id generation + structured logs
src/youtube/types.rs            — VideoInfo, VideoCodec, VideoFormatOption
```

Current flow:

1. `main.rs` detects YouTube URLs in ordinary text and creates a `trace_id`.
2. `handle_youtube_url()` selects a Firefox cookie from the Cookie Pool.
3. `fetch_video_info()` runs `yt-dlp --dump-single-json --no-download`.
4. The bot sends the preview thumbnail/caption and long description chunks.
5. `send_quality_prompt()` sends the quality-selection inline keyboard.

### yt-dlp metadata rules

- `fetch_video_info()` must keep passing
  `--js-runtimes deno:/root/.deno/bin/deno`; systemd's PATH does not include
  `/root/.deno/bin`, and without the explicit Deno runtime YouTube may return
  only storyboard formats.
- Format parsing reads both `formats` and `requested_formats`, then also checks
  the top-level JSON object.
- A YouTube resolution is considered selectable only if there is a video format
  for that height with a recognized video codec.
- Recognized codecs:
  - `avc1...` -> `VideoCodec::H264`
  - `hvc1...` or `dvh1...` -> `VideoCodec::H265`
  - `vp9` or `vp09...` -> `VideoCodec::Vp9`
  - `av01...` -> `VideoCodec::Av1`
- Unknown/missing codecs are ignored. If a resolution only has unknown codecs,
  do not show that resolution.
- Do not infer lower qualities from a max height. The keyboard should show only
  exact heights present in `VideoInfo.video_formats` with recognized codecs.

### Quality and codec UI

Quality labels live in `i18n.json` under `youtube.quality.buttons.*`.
Codec labels live under `youtube.codec.buttons.*`.

Quality button colors:

- `>= 1080p`: `ButtonStyle::Success` (green)
- `720p` and `480p`: `ButtonStyle::Primary`
- `<= 360p`: `ButtonStyle::Danger` (red)

Current callback prefixes:

```text
yt:quality:{height}:{codec_keys}
yt:codec:{height}:{codec_key}
```

Examples:

```text
yt:quality:1080:h264,vp9,av1
yt:codec:1080:vp9
```

Current callback behavior:

- Clicking a quality with multiple codecs edits the quality message into a
  codec-selection message.
- Clicking a quality with one codec currently answers with
  `youtube.quality.not_ready`.
- Clicking a codec currently answers with `youtube.quality.not_ready`.
- Actual download is not implemented yet.

### Downloader implementation notes for the next agent

Telegram callback data is limited and cannot safely carry a YouTube URL,
cookie spec, title, or format id. Before implementing real downloads, replace
the current callback payload shape with a short request id, for example:

```text
yt:quality:{request_id}:{height}
yt:codec:{request_id}:{height}:{codec_key}
```

Store the request in memory when `send_quality_prompt()` is called. The stored
request should include at least:

- original `trace_id`
- `chat_id` and requesting `user_id`
- `webpage_url`
- selected cookie `yt_dlp_browser_spec`
- title
- `Vec<VideoFormatOption>` including `format_id`, `height`, and `codec`

When the user chooses the final codec/quality:

- find the matching `VideoFormatOption` by `height + codec`
- run download in a spawned task so long polling is not blocked
- select the format as `{format_id}+bestaudio/best`
- use the same explicit Deno runtime and same cookie spec
- log command start, sanitized args, progress updates, output path, send start,
  send success/failure, cleanup, and all Telegram edit failures
- upload via the local Bot API using a local file path (`FileUpload::InputFile`)
  so large files use the local `telegram-bot-api` server

The requested progress text should be an i18n template, not hardcoded:

```text
دانلود از یوتیوب (مرحله دریافت ویدیو)
⏳ در حال دانلود از یوتیوب...
🎬 کیفیت انتخابی: {quality}
📊 پیشرفت: {percent}
🔘 نوار پیشرفت: {bar}
📥 حجم: {downloaded} از {total}
🚀 سرعت: {speed}
⏱️ سپری‌شده: {elapsed}
⌛ باقی‌مانده: {eta}
```

Progress bars use 10 cells: filled `●`, empty `○`.

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

Optional local Telegram Bot API server:

```text
BOT_API_BASE_URL=http://127.0.0.1:8081
```

When `BOT_API_BASE_URL` is set, the bot builds the Frankenstein API URL as
`{BOT_API_BASE_URL}/bot{BOT_TOKEN}` via `Bot::new_url(...)`. In
`frankenstein` v0.50 this is the correct constructor; there is no
`with_base_url` helper in the current crate.

If `BOT_API_BASE_URL` contains `127.0.0.1` or `localhost`, startup first calls
`logOut` against the official Telegram Bot API using `Bot::new(token)`, then
switches to the local Bot API URL. This is required before using a local
`telegram-bot-api` server.

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
src/youtube/mod.rs                   — YouTube module exports
src/youtube/fetch.rs                 — yt-dlp metadata fetch + codec parsing
src/youtube/handle.rs                — URL flow, cookie retry, preview send
src/youtube/quality_keyboard.rs      — quality/codec inline UI callbacks
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
