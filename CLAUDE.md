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

Fully implemented end-to-end. Source files:

```text
src/youtube/extract.rs          — YouTube URL detection
src/youtube/fetch.rs            — yt-dlp metadata fetch + format/codec/audio/subtitle parsing
src/youtube/format.rs           — preview caption/description formatting
src/youtube/handle.rs           — URL handler, analyzing reply, cookie retry, preview sending
src/youtube/quality_keyboard.rs — quality keyboard + cancel callback routing hub
src/youtube/selection.rs        — unified selection menu (codec/audio/subtitle/confirm)
src/youtube/download.rs         — request store, progress tracking, yt-dlp spawn, upload, cancel
src/youtube/lang_names.rs       — lang_name_fa(code) → Farsi language name (~130 entries)
src/youtube/trace.rs            — trace id generation + structured logs
src/youtube/types.rs            — VideoInfo, VideoCodec, VideoFormatOption, AudioLanguage, SubtitleLanguage
```

### Full flow

1. `main.rs` detects YouTube URL → creates `trace_id`.
2. `handle_youtube_url()` immediately replies with a spinning `⏳` premium emoji (analyzing message).
3. Firefox cookie selected from Cookie Pool. `fetch_video_info()` runs `yt-dlp --dump-single-json`.
4. Analyzing message is deleted. Preview thumbnail/caption + description chunks sent.
5. `send_quality_prompt()` stores a `YoutubeRequest` in `REQUESTS` (keyed by `request_id: u64`)
   and sends the quality inline keyboard.
6. User taps a quality → `enter_selection_menu()` edits the quality message to the unified
   selection menu (codec radio + audio radio + subtitle toggles + confirm button).
7. User tweaks selections (callbacks update `Arc<Mutex<Option<Selection>>>` on the request).
8. User taps confirm → `spawn_download()` registers a cancel token, spawns `run_download`.
9. `run_download` edits status message with live progress + red cancel button throughout.
10. On completion: video sent via local Bot API, status message deleted.
11. On cancel: yt-dlp child killed (or upload task aborted), status edited to cancelled, files cleaned up.

### yt-dlp metadata rules

- Always pass `--js-runtimes deno:/root/.deno/bin/deno`; systemd PATH does not include
  `/root/.deno/bin` and YouTube may return only storyboard formats without it.
- Format parsing reads both `formats` and `requested_formats` + top-level JSON object.
- A resolution is selectable only if it has a recognized video codec at that exact height.
- Recognized codecs: `avc1…` → H264, `hvc1…`/`dvh1…` → H265, `vp9`/`vp09…` → Vp9, `av01…` → Av1.
- Unknown/missing codecs are silently ignored. Never infer lower qualities from a max height.
- Audio languages: parsed from `formats[].language` field; marked original via
  `format_note.contains("original")` or `language_preference >= 10`.
- Subtitle languages: `subtitles{}` keys (is_auto=false) + `automatic_captions{}` keys (is_auto=true).

### Request store (`download.rs`)

```rust
static REQUESTS: OnceLock<Mutex<HashMap<u64, YoutubeRequest>>> = OnceLock::new();
```

- `store_request(req)` → returns a new `request_id: u64`
- `get_request(id)` → cloned copy (request stays in map until download starts)
- `take_request(id)` → removes and returns (called at download start)

`YoutubeRequest` carries: `trace_id`, `chat_id`, `user_id`, `webpage_url`, `cookie_spec`,
`title`, `duration`, `thumbnail_url`, `formats: Vec<VideoFormatOption>`,
`audio_languages`, `subtitle_languages`, `selection: Arc<Mutex<Option<Selection>>>`.

All clones of a `YoutubeRequest` share the same `selection` mutex via `Arc`.

### Selection state

```rust
pub struct Selection {
    pub height: u32, pub codec: VideoCodec,
    pub audio_lang: Option<String>, pub subtitle_langs: Vec<String>,
    pub view: SelectionView,   // Main | SubMenu(page)
}
```

- `init_selection(req, height)` → defaults: best codec (Av1>Vp9>H265>H264), original audio.
- `with_selection(req, |slot| …)` → locks mutex and runs closure.
- Selection menu callbacks mutate it in-place; keyboard is rebuilt via `EditMessageReplyMarkupParams`.

### Selection menu callback prefixes (`selection.rs`)

```text
yt:s:nop          — header no-op button
yt:s:c:{rid}:{codec_key}   — codec radio toggle
yt:s:a:{rid}:{idx}         — audio language radio toggle
yt:s:t:{rid}:{idx}         — subtitle toggle (multi-select)
yt:s:sm:{rid}              — open subtitle submenu (page 0)
yt:s:sb:{rid}              — back from subtitle submenu to main view
yt:s:sp:{rid}:{page}       — subtitle submenu page navigation
yt:s:go:{rid}              — confirm → spawn download
```

Subtitle submenu: 4 rows × 2 cols = 8 entries per page. Quick-access fa+en shown on main view.

### Quality keyboard callback prefixes (`quality_keyboard.rs`)

```text
yt:q:{rid}:{height}   — quality button → enter_selection_menu()
yt:c:…                — legacy codec callback (stale; acked silently)
yt:cancel:{rid}       — cancel active download
yt:s:…                — forwarded to handle_selection_callback()
```

Quality button colors: `>= 1080p` → Success (green), `720p`/`480p` → Primary, `<= 360p` → Danger (red).
Quality icons in `i18n.json` under `emoji.panel.icons`: `diamond` (4K/2K), `fire_yt` (1440p),
`sparkles` (1080p), `star_yt` (720p), `phone` (480p), `signal` (360p and below).

### Cancel system (`download.rs`)

```rust
static ACTIVE_DOWNLOADS: OnceLock<Mutex<HashMap<u64, Arc<Notify>>>> = OnceLock::new();
```

- `spawn_download(…)` calls `register_cancel(request_id)` → `Arc<Notify>`, then spawns task.
- `cancel_download(request_id)` → removes from map and calls `notify.notify_one()`.
- Inside `run_download`: cancel future is pinned once and selected alongside `rx.recv()` and
  upload tick. On fire: kills child / aborts send_task, edits status to cancelled, cleans up.
- `UnregisterGuard(request_id)` struct ensures `unregister_cancel` is called on every return path.
- Progress edits use `edit_progress_status()` which attaches cancel keyboard (`yt:cancel:{rid}`).
- Error/cancelled edits use plain `edit_status()` which removes the keyboard (no reply_markup).

### Download output

- Files saved to `DOWNLOAD_ROOT/{trace_id}/` (`/mnt/data/mahdidev/ros/dev/downloads/yt`).
- Format spec: `{format_id}+bestaudio/best`, merge to mp4.
- Progress parsed from `YT_PROGRESS|percent|downloaded|total|speed|eta|elapsed` lines.
- Progress bar: 10 cells, `●` filled / `○` empty.
- Upload via `SendVideoParams` with `FileUpload::InputFile` (local Bot API path).
- On upload success: status message deleted. On failure: new error message sent.
- Directory cleaned up (`remove_dir_all`) after every outcome.

## Prerequisites (Files & Runtime Dependencies)

Required files — all tracked in the repo under `files/`:

```text
files/runtime/libvosk.so            — Vosk native library (vosk crate FFI)
files/runtime/deep-filter            — DeepFilterNet3 statically-linked musl binary
files/models/vosk/vosk-model-fa-0.5  — Vosk Persian large model
files/realesrgan/realesrgan-ncnn-vulkan  — Real-ESRGAN NCNN Vulkan binary
files/realesrgan/models/realesr-animevideov3-x2.param/.bin  — Anime upscale x2
files/realesrgan/models/realesr-animevideov3-x3.param/.bin  — Anime upscale x3
files/realesrgan/models/realesr-animevideov3-x4.param/.bin  — Anime upscale x4
files/realesrgan/models/realesrgan-x4plus-anime.param/.bin  — Anime pro upscale x4
files/realesrgan/models/realesrgan-x4plus.param/.bin  — General upscale x4
files/models/vosk/vosk-model-fa-0.5-small  — Vosk Persian small model
files/models/vosk/vosk-model-en-us-0.42  — Vosk English large model (300MB+)
files/models/vosk/vosk-model-en-us-0.42-small  — Vosk English small model
files/models/deepfilter/DeepFilterNet3_onnx.tar.gz  — DeepFilterNet3 model (extracted at startup)
```

Build setup (build.rs):
- `cargo:rustc-link-search={manifest_dir}/files/runtime` so the vosk crate finds `libvosk.so` at link time
- `cargo:rustc-link-lib=vosk` links the native library

The deep-filter binary is called as a subprocess with `-m` model flag.
DeepFilterNet3 model tarball is extracted to `files/models/deepfilter/DeepFilterNet3/` on first run
(or manually: `tar xzf files/models/deepfilter/DeepFilterNet3_onnx.tar.gz -C files/models/deepfilter/`).

Required system packages:
- `ffmpeg` — audio conversion (16kHz mono 16-bit PCM WAV)
- `libvulkan1` + `mesa-vulkan-drivers` — Vulkan runtime for Real-ESRGAN NCNN (uses llvmpipe software rendering on CPU if no GPU)
- `libvosk.so` compatible with the `vosk = "0.3"` crate

## Project Summary

This project is a Rust Telegram bot named `ros-telegram-bot`.
It uses the `frankenstein` crate for Telegram Bot API access and runs as a
systemd service named `abc`.

The bot currently supports:

- Telegram long polling with `getUpdates` (offset persisted per-update)
- `/start` command with AI Lab, YouTube, and Emoji buttons
- Reading `BOT_TOKEN` from `.env`, `/etc/default/abc`, or process env
- Firefox Cookie Pool management for user `mahdi`
- Optional PostgreSQL persistence for Cookie Pool state
- systemd deployment through `abc.service`
- Local bare Git server under `git-server/ros-telegram-bot.git`
- Full emoji management panel (`/emoji`)
- Full YouTube downloader: URL detection → preview → quality/codec/audio/subtitle selection → yt-dlp download → upload via local Bot API → cancel button
- AI Lab submenu: Speech-to-Text (Vosk ASR + DeepFilterNet3), noise removal (DeepFilterNet3), image upscale (Real-ESRGAN NCNN Vulkan)

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

Marks the last selected cookie as rate-limited and moves it to a 30-minute
cooldown. After 30 minutes the profile is auto-refreshed and re-added to pool.

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

4. **STT audio handling** — if user has `AwaitingSttAudio` and message has
   voice/audio/document, call `handle_stt_audio()` and skip everything else
5. **command dispatch** — `/emoji`, `/se`, `/start`, `/cookie_*`, YouTube URLs

Messages starting with `/` are never matched as addemoji links (step 1 is
skipped), so commands always reach step 5.

### Flow States

| State | Trigger | Exit |
|-------|---------|------|
| `AwaitingEmojis` | CB_ADD | cancel button, pack chosen, or any input transitions to `AwaitingPackChoice` |
| `AwaitingPackChoice` | emojis collected | pack name typed, inline pack button, or cancel |
| `AwaitingPackAlias` | set-alias button on pack detail | any text (sets or clears alias) |
| `AwaitingTestText` | CB_TEST | cancel button or `/emoji` |
| `AwaitingImportFile` | CB_IMPORT | cancel button or document sent → `AwaitingImportMode` |
| `AwaitingImportMode` | file analyzed | import mode button pressed |
| `AwaitingSttConfig` | `ai:stt` button | lang/size/denoise chosen → `AwaitingSttAudio` |
| `AwaitingSttAudio` | lang chosen in config | audio message received → transcribe |

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
src/main.rs                          — event loop + routing
src/config.rs                        — BOT_TOKEN / DATABASE_URL / ADMIN_USER_ID reading
src/bot.rs                           — send_text, send_text_md, send_start_button
src/cookie_pool.rs                   — CookiePool + format helpers + save_snapshot
src/i18n/mod.rs                      — t() / tf() / entities_for_text() / apply_premium_to_md()
src/i18n/emoji_map.rs                — EMOJI_MAP: visible char → emoji.panel.icons key
src/i18n/entities.rs                 — entities_for_text(): UTF-16 CustomEmoji entities
src/i18n/premium_md.rs              — apply_premium_to_md(): emoji char → tg://emoji MarkdownV2
src/youtube/mod.rs                   — YouTube module exports
src/youtube/extract.rs               — YouTube URL detection
src/youtube/fetch.rs                 — yt-dlp metadata fetch + codec/audio/subtitle parsing
src/youtube/format.rs                — preview caption/description formatting
src/youtube/handle.rs                — URL flow, analyzing reply, cookie retry, preview send
src/youtube/quality_keyboard.rs      — callback routing hub (quality/selection/cancel)
src/youtube/selection.rs             — unified selection menu (codec/audio/subtitle/confirm)
src/youtube/download.rs              — request store, cancel system, yt-dlp, progress, upload
src/youtube/lang_names.rs            — lang_name_fa(code): language code → Farsi name
src/youtube/trace.rs                 — trace id generation + structured logs
src/youtube/types.rs                 — VideoInfo, VideoCodec, VideoFormatOption, AudioLanguage, SubtitleLanguage
src/database/mod.rs
src/database/posfreSQL/postgresql.rs — PostgreSQL connection + cookie pool tables
src/database/posfreSQL/schema.sql    — CREATE TABLE statements
src/stt/mod.rs                       — STT module exports
src/stt/types.rs                     — SttConfig, SttLang, SttModelSize
src/stt/vosk.rs                      — Vosk transcribe: WAV → text via vosk crate
src/stt/deepfilter.rs                — DeepFilterNet3 noise reduction subprocess
src/stt/config.rs                    — CB_STT_* constants + keyboard builders
src/stt/handle.rs                    — enter_stt_config, handle_stt_callback, handle_stt_audio
src/emoji/mod.rs
src/emoji/cache/                     — EmojiCache, {key} expansion, 5-min refresh task
src/emoji/flow.rs
src/emoji/handler/                   — all callback + message handlers
src/upscale/mod.rs                   — upscale module exports
src/upscale/handle.rs                — upscale flow: model selection, image processing, report
src/emoji/panel/                     — keyboard builders, text formatters, CB_* constants, btn_* helpers
src/emoji/store/                     — all DB queries
src/emoji/smart_name.rs
src/emoji/import/                    — SQL parse, analyze, execute import modes
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

## Cookie Pool & Auto-Refresh

Implemented in `src/cookie_pool/` and `src/modules/cookie_refresher.rs`.

### معماری کلی

- پروفایل‌های فایرفاکس از `/home/mahdi/.mozilla/firefox` کشف می‌شن (max 20)
- هر پروفایل = یک اکانت Gmail لاگین‌شده در فایرفاکس
- yt-dlp مستقیم از `cookies.sqlite` هر پروفایل می‌خونه
- کپی کش‌شده پروفایل‌ها در `cookie_profiles_cache/` قرار داره
- انتخاب کوکی رندوم از pool، با حذف `last_used_cookie` و کوکی‌های در cooldown

### فایل لینک‌ها

- مسیر: `files/youtube_links.txt`
- هر خط یک لینک یوتیوب (خطوط خالی و `#` نادیده گرفته می‌شن)
- `cookie_refresher` رندوم 3 لینک از فایل انتخاب می‌کنه

### چرخه refresh (cookie_refresher)

- هر ۶ ساعت یکبار برای همه پروفایل‌ها اجرا می‌شه (`COOKIE_REFRESH_INTERVAL_SECS`)
- پروفایل‌ها **۳ تا ۳ تا parallel** اجرا می‌شن (`futures::future::join_all` روی chunks of 3)
- برای هر پروفایل، ترتیب این‌هاست:
  1. **kill_existing_firefox**: `pkill -f "firefox.*{profile_path}"` + صبر 3 ثانیه + `pkill -9 -f ...` + صبر 2 ثانیه + حذف `{profile_path}/.parentlock` و `{profile_path}/lock`
  2. **check_login**: چک وجود و non-empty بودن `cookies.sqlite` در `profile_path`
  3. **open_firefox**: `sudo -u mahdi firefox --profile {profile_path} {url1}` با `DISPLAY=:10` و `XDG_RUNTIME_DIR=/run/user/1002` (X11، نه Wayland)
  4. باز کردن 2 لینک اضافی با `--new-tab` (هر کدام با تاخیر 1 ثانیه)
  5. **firefox_wait**: هر 5 ثانیه `/proc/{pid}` چک می‌شه؛ اگه crash کنه → `firefox_crashed`
  6. بعد از 3600 ثانیه (1 ساعت): `firefox_timeout` — `kill -TERM` + صبر 3 ثانیه + `kill -KILL` در صورت نیاز
  7. **refresh_cache**: کپی `cookies.sqlite` + `cookies.sqlite-wal` + `cookies.sqlite-shm` از `source_profile_dir` به `cache_dir`

### مدیریت 429 (auto-refresh)

وقتی yt-dlp خطای 429 برگردونه:

1. `mark_last_rate_limited()` → کوکی با cooldown **4 ساعت** (safety net) از pool خارج می‌شه
2. `CookieSource` از طریق `rate_limit_tx` channel به event loop اصلی فرستاده می‌شه
3. یک task جداگانه spawn می‌شه که **30 دقیقه** صبر می‌کنه
4. بعد از 30 دقیقه، `cookie_refresher::run()` فقط برای همون پروفایل اجرا می‌شه
5. بعد از اتمام refresh (موفق یا ناموفق)، `remove_from_cooldown()` صدا زده می‌شه
6. event loop از `cooldown_done_rx` می‌خونه و کوکی دوباره به pool active اضافه می‌شه

### لاگ‌ها

```bash
journalctl -u abc -f | grep cookie_refresh
```

فرمت لاگ‌ها:
```
[cookie_refresh profile=xyz event=<name>] key=val ...
```

eventهای اصلی: `start`, `login_check`, `kill_existing`, `firefox_open`, `firefox_tab`,
`firefox_wait`, `firefox_timeout`, `firefox_crashed`, `firefox_kill_term`,
`firefox_kill_force`, `cache_copy`, `done`, `cooldown_refresh_scheduled`,
`cooldown_refresh_start`, `cooldown_refresh_done`

### اضافه کردن پروفایل جدید

1. یک پروفایل فایرفاکس جدید بساز و با اکانت Google لاگین کن
2. مطمئن شو `cookies.sqlite` در پروفایل وجود داره
3. `systemctl restart abc` — کشف خودکار انجام می‌شه

## Git Server

```text
origin -> git-server/ros-telegram-bot.git
branch: master
```
