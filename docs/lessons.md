# Lessons — why the rules in CLAUDE.md exist

Narrative context pulled out of `CLAUDE.md` so that file stays a spec sheet.
Read a section here when the corresponding rule is unclear or you're tempted to skip it.

## Local Bot API file storage — first suspect for "دانلود فایل با خطا مواجه شد"

There are two **separate** Bot API servers, not one:

| Service | `--dir` | Port | Users |
|---|---|---|---|
| `telegram-bot-api.service` | `/mnt/data/telegram-bot-api/data` | 8081 | dev + several unrelated bot tokens |
| `telegram-bot-api-rostam.service` | `/mnt/data/telegram-bot-api-rostam/data` | 8082 | prod only (+ `…-rostam-watchdog.service`) |

Restarting one does not touch the other.

In `--local` mode `getFile` returns an absolute path, and `download_telegram_file`
(`src/bot/files.rs`) just `std::fs::copy`s it. So a download failure is far more often a
**filesystem permission** problem than a code bug. The server creates each token directory
`0750`, owned by whatever user the service runs as; if that isn't the bot's own user, **every**
download-based feature (stt, denoise, gwm, pdfcompress, upscale…) fails at once with
`Permission denied (os error 13)`.

Fixed 2026-08-08: the 8081 server ran as root while `rostam_dev.service` runs as `mahdi`. Now
`/etc/systemd/system/telegram-bot-api.service.d/override.conf` sets `User=mahdi` / `Group=mahdi`,
and the whole tree was `chown -R mahdi:mahdi`'d. Root-running consumers on 8081 are unaffected
(root reads anything). Prod's server still runs as root, matching `rostam.service`, which also
has no `User=`.

Diagnose before reading code:

```bash
journalctl -u rostam_dev -n 300 | rg 'download_failed|os error 13'
ls -la /mnt/data/telegram-bot-api/data/
```

A bare `chown` regresses as soon as the server writes new files — the service's `User=` is what
has to match the bot's.

## Long jobs — the rule that broke most often

`deoldify` and `filecompress` both rendered a static `00:00` and shipped without a cancel button,
while `stt` already had a working ticker right next door. A dead progress bar never shows up in
code review, only in real use — that's why it's a commit gate, not a style preference.

Cancel must do three things, not one. Discarding the output after the CPU is already burnt is not
a cancel: the flag has to reach the subprocess wait loop and `start_kill()` it, the reserved quota
has to be refunded, and the work dir has to be cleaned.

Re-arm exists because nobody sends one file. Forcing a menu round-trip per item is what kills the
UX. A spawned task that arms the flow must hold a cloned `FlowManager` and set state directly —
the old `flow_clear_tx` channel was drained at the top of the update loop and silently erased
late arms.

Acknowledgement/counter edits during upload ingest go through `spawn_user_task` because the update
loop is sequential: awaited Telegram round-trips make a burst of forwarded files arrive one at a
time.

## CPU Broker — "quick" is a guess, not a measurement

Any op over ~500ms of CPU (image/audio/video, ONNX, ffmpeg, Ghostscript, RAR splitting) must go
through the broker on `separation.service` (port 6589). This is the easiest rule to forget on a new
feature, because the operation always *feels* fast. Check it explicitly instead of assuming.
Core-scaling policy and Redis key layout live in the service source — read there for exact numbers.

## Premium emoji / MarkdownV2 mismatch is invisible in review

`stt.ready_title` shipped with a missing `apply_premium_to_md` + `parse_mode(MarkdownV2)` pair and
rendered plain emoji. Nothing in the diff looks wrong. Same class of bug: an unescaped interpolated
value — a label like «فارسی (سریع)» carries parens, so it needs `md_escape` before it reaches the
template, and the template's own literal `.`/`-`/`(` must be escaped in all four languages.

## A promise in an i18n string is a spec

`tts.enter_text_default` advertised «حداکثر ۵۰۰ کاراکتر» for months while nothing checked the
length. When a user-facing string names a limit, count, size, or format, grep for the constant that
enforces it; if there isn't one, that's the bug. Count characters (`chars().count()`), not bytes —
Persian text hits a byte cap at half the advertised length.

## `/start` ↔ `/ref` ↔ `/panel` drift

They're not identical menus — `/start` is the entry hub (status summary + quick-access buttons,
including a direct referral button), `/ref` is the dedicated referral/reward screen, `/panel` is
admin. But rank names, referral rewards, and any feature exposed in one must stay consistent. This
is the rule most likely to get forgotten when a new feature is bolted on, so it belongs in the
commit gate, not in an afterthought pass.
