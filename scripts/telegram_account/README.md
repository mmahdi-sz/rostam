# Telegram Account Tooling & Automated Bot Testing CLI

This directory contains the Python tooling for managing a dedicated Telegram MTProto test account, interacting with development bots, executing automated smoke tests, and running offline CI workflows.

> [!NOTE]
> **Tooling Status**: Offline-ready local development and CI testing tooling. This is not a deployed production service or public bot feature.

---

## 1. Architecture Overview

The tooling consists of modular, cohesive components designed for strict security, process isolation, and zero secret exposure:

| Module | Purpose | Mode |
|---|---|---|
| `Apis.py` | Interactive helper to retrieve `TELEGRAM_TEST_API_ID` and `TELEGRAM_TEST_API_HASH` from `https://my.telegram.org` into `.env`. Preserves existing settings and sets `0600` permissions. | Interactive |
| `client.py` | Telethon client abstraction, POSIX process locking (`fcntl.flock`), target allowlist enforcement, noninteractive fail-closed authorization, callback button validation, literal text preservation, atomic download staging, and safe error translation. | Library |
| `cli.py` | Unified command-line interface supporting 8 subcommands with human-friendly text and machine-readable JSON output formats. | Noninteractive (except `login`) |
| `test_cli.py` | Comprehensive offline unit test suite enforcing zero outbound network access via socket interception. 38 offline tests covering all safety, operational, and timeout behaviors. | Offline Test |
| `test_apis.py` | Offline unit tests for `Apis.py` credential retrieval and atomic `.env` updates (6 tests). | Offline Test |
| `requirements.txt` | Pinned dependency specifications for runtime and testing. | Configuration |

---

## 2. Installation & Requirements

- Python 3.10+ (verified on Python 3.11 and Python 3.14)
- Dependencies (pinned in `scripts/telegram_account/requirements.txt`):
  - `Telethon==1.45.0`
  - `requests==2.34.2`
  - `pyaes==1.6.1`
  - `pyasn1==0.6.5`
  - `rsa==4.9.1`

Install into your virtual environment:

```bash
pip install -r scripts/telegram_account/requirements.txt
```

---

## 3. Canonical Configuration & Environment Variables

Configuration is loaded from CLI options, the specified `.env` file (`--env-file`, defaulting to repository root `.env`), or environment variables. CLI options take precedence over environment variables.

### Canonical Configuration Variables

| Variable Name | Required | Description | Example / Default |
|---|---|---|---|
| `TELEGRAM_TEST_ALLOWED_BOTS` | Yes (for bot ops) | Comma-separated list of allowlisted bot usernames. Fails closed if empty; no default production bot. | `rostam_dev_bot,my_test_bot` |
| `TELEGRAM_TEST_SESSION_PATH` | No | Path to the Telethon SQLite session file. Stored in a private `0700` directory with `0600` file permissions. | `dev/.telegram_account/session.session` |
| `TELEGRAM_TEST_PHONE` | Only for `login` | Dedicated test account international phone number in E.164 format. | `+989123456789` |
| `TELEGRAM_TEST_API_ID` | Yes | Positive integer MTProto application API ID from `my.telegram.org`. | `12345678` |
| `TELEGRAM_TEST_API_HASH` | Yes | 32-character hexadecimal MTProto API Hash from `my.telegram.org`. | `0123456789abcdef0123456789abcdef` |
| `TELEGRAM_TEST_TIMEOUT` | No | Positive finite operation timeout in seconds. | `30.0` |

---

## 4. Security Guarantees & Constraints

1. **Zero Secret Exposure**: Passwords, OTP codes, session keys, and API hashes are never printed to stdout/stderr or included in JSON output. `Config` marks `api_hash` and `phone` with `repr=False`. Unexpected internal exceptions emit a generic sanitized message (`{"error": "internal_error", ...}`). Phone numbers are always masked (e.g. `+98*****1234`).
2. **Fail-Closed Allowlisting**: Every bot operation validates the target username against `TELEGRAM_TEST_ALLOWED_BOTS` *before* contacting Telegram. Numeric IDs, phone numbers, unlisted bots, and human users (`entity.bot == False`) are rejected with exit code 7 (`TARGET_VIOLATION`).
3. **Session Storage & Process Locking**:
   - On initial `login`, a dedicated private leaf session directory (`0700`) and regular session file (`0600`) are created *before* Telethon initializes `SQLiteSession`.
   - Existing parent directories must already be private (`0700`, `drwx------`). The tool never alters permissions on existing shared directories; if an existing directory is not private (e.g. `0755`), it fails closed with `SecurityError`.
   - Shared system root or temp directories (such as `/tmp` or `/`) are rejected as leaf session directories.
   - Noninteractive commands never create files or directories for missing sessions.
   - Concurrent access is serialized using POSIX `fcntl.flock` with bounded timeout loop and cancellation-safe asynchronous polling. Lock files are opened with `O_NOFOLLOW` and enforced to `0600`.
4. **Fail-Closed Authorization**: Only the explicit `login` command prompts for OTP/2FA. Noninteractive commands require an existing session file and valid authorization; missing or unauthorized sessions exit immediately with code 3 (`UNAUTHORIZED`).
5. **Exact Literal Text Preservation**: Message text and captions are delivered with `parse_mode=None`, preventing markdown parsing errors, stripping, or unintended entity transformation of Persian or special characters (`**`, `_`, `[]`, `\`).
6. **Atomic Media Download Staging**:
   - Media downloads stage into a private temporary directory (`tempfile.mkdtemp` with mode `0700`) under the destination directory.
   - Download is written into a `0600` staging file.
   - Overwrite protection: if a file or symlink with the target name already exists, the download fails without modifying existing files.
   - Partial downloads and staging directories are completely wiped in `finally:` on error and cancellation (`asyncio.CancelledError`).
   - Upon completion, the file is atomically moved to destination with `0600` permissions.
7. **Bounded Operation Deadlines**: The entire command pipeline (lock acquisition, connect, auth check, operation, bounded disconnect, and lock release) is wrapped in an overall timeout deadline. Auto-reconnect is disabled (`auto_reconnect=False`).

---

## 5. CLI Usage & Subcommands

Run the CLI using Python:

```bash
python3 scripts/telegram_account/cli.py [GLOBAL OPTIONS] <SUBCOMMAND> [SUBCOMMAND OPTIONS]
```

### Global Options
- `--env-file PATH`: Path to `.env` file (defaults to `dev/.env`).
- `--session PATH` (or `--session-path`): Path to SQLite session file (defaults to `.telegram_account/session.session`).
- `--timeout SECONDS`: Network timeout in seconds (default: 30).
- `--json`: Format all output as machine-readable JSON to stdout.

*Note*: Bot allowlisting is strictly environment-driven via `TELEGRAM_TEST_ALLOWED_BOTS` to prevent command-line overriding.

### Subcommand Reference

#### 1. `login` (Interactive)
Authenticates a new session. Prompts on console for one-time code and 2FA password if configured:
```bash
python3 scripts/telegram_account/cli.py login --phone "+989123456789"
```

#### 2. `status` (Noninteractive)
Verifies session validity and authorization status. Works without phone number configured if session exists:
```bash
python3 scripts/telegram_account/cli.py status --json
```
Output:
```json
{
  "success": true,
  "authorized": true,
  "user_id": 987654321,
  "username": "tester",
  "phone_masked": "+98*****1234"
}
```

#### 3. `send-text`
Sends literal text to an allowlisted bot (full UTF-8, markdown characters, and Persian RTL support):
```bash
python3 scripts/telegram_account/cli.py send-text --bot rostam_dev_bot --text "سلام **ستاره** دنیا!"
```

#### 4. `send-file`
Sends a local document or media file with literal caption:
```bash
python3 scripts/telegram_account/cli.py send-file --bot rostam_dev_bot --file "/path/to/doc.pdf" --caption "Test [PDF]"
```

#### 5. `read-messages`
Fetches recent messages and renders complete 2D inline keyboard trees with callback data:
```bash
python3 scripts/telegram_account/cli.py read-messages --bot rostam_dev_bot --limit 3 --json
```

#### 6. `click-button`
Clicks an inline callback button on a specific message. Selection can be by button text, callback data string, or coordinate (`--coord ROW COL` or `--row R --col C`):
```bash
# Click by callback data:
python3 scripts/telegram_account/cli.py click-button --bot rostam_dev_bot --message-id 105 --button-data "start:guide"

# Click by visible label:
python3 scripts/telegram_account/cli.py click-button --bot rostam_dev_bot --message-id 105 --button-text "🇬🇧 English"

# Click by coordinates (row 0, col 1):
python3 scripts/telegram_account/cli.py click-button --bot rostam_dev_bot --message-id 105 --coord 0 1
```

#### 7. `download-media`
Downloads an attachment from a bot message to a controlled local folder via atomic staging:
```bash
python3 scripts/telegram_account/cli.py download-media --bot rostam_dev_bot --message-id 120 --dest-dir "./downloads/telegram_e2e"
```

#### 8. `smoke`
Executes an automated end-to-end smoke verification flow:
1. Queries the latest message ID from the bot.
2. Sends `/start`.
3. Awaits fresh response correlated with the request.
4. If language picker is presented, clicks English (`lang:set:en`) and verifies message update.
5. Clicks the guide button (`start:guide`) and verifies menu transition.
6. Clicks the back button (`start:panel`) and verifies return to the start menu.

```bash
python3 scripts/telegram_account/cli.py smoke --bot rostam_dev_bot --timeout 30 --json
```

---

## 6. Exit Codes

| Code | Constant | Meaning |
|:---:|---|---|
| `0` | `SUCCESS` | Requested operation completed successfully. |
| `1` | `GENERIC_ERROR` | Invalid arguments, unhandled error, missing local file, or security violation. |
| `2` | `CONFIG_ERROR` | Missing or invalid MTProto configuration (`api_id`, `api_hash`, `timeout`). |
| `3` | `UNAUTHORIZED` | Session file not found, unauthorized, or revoked (noninteractive fail-closed). |
| `4` | `FLOOD_WAIT` | Telegram rate limit hit (`FloodWaitError` with `retry_after_seconds`). |
| `5` | `NETWORK_TIMEOUT` | Network operation timed out without response. |
| `6` | `SESSION_LOCKED` | SQLite session file locked by another process (after timeout). |
| `7` | `TARGET_VIOLATION` | Target bot unlisted, numeric chat ID, or human user entity. |

---

## 7. Offline Unit Tests

Unit tests execute with **strictly zero network access**, enforced by socket interception:

```bash
python3 -m unittest discover -s scripts/telegram_account -p "test_*.py"
```

The offline test suite covers:
- Target allowlist enforcement, numeric rejection, and non-bot detection.
- Session lock serialization under contention (`fcntl.flock`).
- Button resolution by text, data, and row/column.
- Rejection of invalid button types (URL buttons, switches, lack of callback data).
- Atomic staging download safety, partial file cleanup on error/cancellation, and overwrite protection.
- Literal text and caption preservation without markdown entity parsing (`parse_mode=None`).
- Bounded deadlines on connect, auth check, operations, and disconnect with guaranteed lock and session release.
- Telethon FloodWait mapping with `retry_after_seconds` and exit code 4.
- Masked phone numbers in JSON outputs and secret sanitization on internal errors.

---

## 8. CI Integration & Self-Hosted Runner Setup

The project uses a two-tier testing model:

### Tier 1: Public Offline CI (`.github/workflows/ci.yml`)
- **Trigger**: Automatic on all pushes and PRs to `dev` and `master`.
- **Runner**: Public GitHub runner (`ubuntu-latest`).
- **Isolation**: Runs without credentials, secrets, or network access (`test_*.py`).

### Tier 2: Manual Live Telegram E2E (`.github/workflows/telegram-e2e.yml`)
- **Trigger**: Manual `workflow_dispatch` only.
- **Branch Restriction**: Strictly restricted to `refs/heads/dev`.
- **Runner**: Dedicated self-hosted trusted runner (`runs-on: [self-hosted, linux, telegram-e2e]`). Public runners are forbidden.
- **Concurrency**: Serialized via workflow concurrency group `telegram-live-e2e-session` (`cancel-in-progress: false`).

### Self-Hosted Runner Host Configuration
1. **User**: Run runner agent under an unprivileged user (e.g. `github-runner`).
2. **External Session Directory**:
   ```bash
   sudo mkdir -p /var/lib/telegram-runner/sessions
   sudo chown -R github-runner:github-runner /var/lib/telegram-runner/sessions
   sudo chmod 700 /var/lib/telegram-runner/sessions
   ```
3. **Repository Secrets & Variables**:
   - `TELEGRAM_TEST_API_ID` (Secret)
   - `TELEGRAM_TEST_API_HASH` (Secret)
   - `TELEGRAM_TEST_ALLOWED_BOTS` (Secret / Var)
   - `TELEGRAM_TEST_SESSION_PATH` = `/var/lib/telegram-runner/sessions/telethon_e2e.session` (Secret / Var)
4. **Pre-Flight Enforcement**: CI verifies that `TELEGRAM_TEST_SESSION_PATH` resides outside the git checkout directory and that the session file exists before starting.

---

## 9. Current Operational Status & Live Prerequisites

> [!IMPORTANT]
> **Telegram Rate-Limiting Notice**: Live MTProto authentication and requests are currently temporarily rate-limited by Telegram (`Too many tries...`). 
> No live requests or authentication retries should be made until this restriction clears.

When rate limits clear and authorized by the operator:
1. Log into the trusted runner host as `github-runner`.
2. Perform one-time interactive login:
   ```bash
   python3 scripts/telegram_account/cli.py login \
     --session /var/lib/telegram-runner/sessions/telethon_e2e.session \
     --phone "+98..."
   ```
3. Set file permissions: `chmod 600 /var/lib/telegram-runner/sessions/telethon_e2e.session`.
4. Trigger the manual E2E workflow from GitHub Actions.

---

## 10. Advanced Testing Note: YouTube Automation

YouTube hardsub / download testing is excluded from the basic automated smoke test because:
1. Hardsubbing is paywalled behind `Esfandyar` rank in the bot (`src/youtube/`). Lower ranks encounter payment prompts.
2. It triggers background worker jobs and CPU Broker processes (`separation.service` on port 6589).
3. It requires external YouTube video downloads and valid browser cookie pools.

For advanced manual E2E testing once a high-rank test account is provisioned:
```bash
# 1. Send YouTube URL
python3 scripts/telegram_account/cli.py send-text --bot rostam_dev_bot --text "https://www.youtube.com/watch?v=..."

# 2. Inspect selection menu
python3 scripts/telegram_account/cli.py read-messages --bot rostam_dev_bot --limit 2 --wait-seconds 10 --json

# 3. Click hardsub format button
python3 scripts/telegram_account/cli.py click-button --bot rostam_dev_bot --message-id <MSG_ID> --button-data "<CB_DATA>"
```
