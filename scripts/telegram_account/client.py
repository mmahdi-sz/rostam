"""Telegram Dedicated Test Account Client & Safety Layer.

Provides process-locked MTProto interaction with Telegram bots using Telethon.
All bot operations enforce strict allowlist validation, entity type verification,
fail-closed authentication, and safe error translation without credential leakage.
"""

import asyncio
import fcntl
import getpass
import json
import logging
import math
import os
import re
import shutil
import stat
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Tuple, Union

import telethon
from telethon import TelegramClient, errors, functions, types

# Exit code constants
EXIT_SUCCESS = 0
EXIT_GENERIC_ERROR = 1
EXIT_CONFIG_ERROR = 2
EXIT_UNAUTHORIZED = 3
EXIT_FLOOD_WAIT = 4
EXIT_NETWORK_TIMEOUT = 5
EXIT_SESSION_LOCKED = 6
EXIT_TARGET_VIOLATION = 7


class AccountToolError(Exception):
    """Base exception for Telegram account tooling."""

    exit_code: int = EXIT_GENERIC_ERROR
    error_name: str = "generic_error"

    def __init__(
        self,
        message: str,
        exit_code: Optional[int] = None,
        error_name: Optional[str] = None,
    ):
        super().__init__(message)
        if exit_code is not None:
            self.exit_code = exit_code
        if error_name is not None:
            self.error_name = error_name


class ConfigError(AccountToolError):
    exit_code = EXIT_CONFIG_ERROR
    error_name = "config_error"


class NotAuthorizedError(AccountToolError):
    exit_code = EXIT_UNAUTHORIZED
    error_name = "unauthorized"


class FloodWaitError(AccountToolError):
    exit_code = EXIT_FLOOD_WAIT
    error_name = "flood_wait"

    def __init__(self, message: str, retry_after_seconds: int = 0):
        super().__init__(message)
        self.retry_after_seconds = retry_after_seconds


class NetworkTimeoutError(AccountToolError):
    exit_code = EXIT_NETWORK_TIMEOUT
    error_name = "network_timeout"


class SessionLockedError(AccountToolError):
    exit_code = EXIT_SESSION_LOCKED
    error_name = "session_locked"


class TargetViolationError(AccountToolError):
    exit_code = EXIT_TARGET_VIOLATION
    error_name = "target_violation"


class SecurityError(AccountToolError):
    exit_code = EXIT_GENERIC_ERROR
    error_name = "security_violation"


class UnsupportedButtonError(AccountToolError):
    exit_code = EXIT_GENERIC_ERROR
    error_name = "unsupported_button"


def mask_phone(phone: Optional[str]) -> str:
    """Mask phone number for safe display (e.g. +98*****1234)."""
    if not phone:
        return ""
    cleaned = str(phone).strip()
    if len(cleaned) <= 6:
        return "*" * len(cleaned)
    prefix_len = 3 if cleaned.startswith("+") else 2
    suffix_len = 4
    if len(cleaned) <= prefix_len + suffix_len:
        return cleaned[:prefix_len] + "*" * (len(cleaned) - prefix_len)
    mask_len = len(cleaned) - (prefix_len + suffix_len)
    return cleaned[:prefix_len] + ("*" * mask_len) + cleaned[-suffix_len:]


def parse_env_file(path: Union[str, Path]) -> Dict[str, str]:
    """Parse .env file safely into memory without altering os.environ."""
    p = Path(path)
    if not p.is_file():
        return {}
    values: Dict[str, str] = {}
    for line in p.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("export "):
            line = line[7:].strip()
        if "=" in line:
            k, v = line.split("=", 1)
            k = k.strip()
            v = v.strip()
            if len(v) >= 2 and (
                (v.startswith('"') and v.endswith('"'))
                or (v.startswith("'") and v.endswith("'"))
            ):
                v = v[1:-1]
            values[k] = v
    return values


def resolve_session_path(session_path: Union[str, Path]) -> Path:
    """Ensure session path matches Telethon suffix semantics (.session appended if missing)."""
    p_str = str(session_path)
    if not p_str.endswith(".session"):
        p_str += ".session"
    return Path(p_str)


def check_no_symlinks(path: Union[str, Path]):
    """Ensure path and all its ancestor components are regular directories/files and not symlinks."""
    p = Path(path)
    curr = p
    while True:
        if curr.is_symlink() or os.path.islink(curr):
            raise SecurityError(f"Unsafe symlink detected in path: {curr}")
        if curr == curr.parent:
            break
        curr = curr.parent


def validate_session_directory(session_dir: Path):
    """Reject shared system root or temp directories directly as leaf session directories."""
    dangerous = {
        Path("/"),
        Path("/tmp"),
        Path("/var"),
        Path("/var/tmp"),
        Path("/etc"),
        Path("/home"),
        Path("/usr"),
        Path("/bin"),
        Path("/lib"),
    }
    try:
        resolved = session_dir.resolve()
    except Exception:
        resolved = session_dir
    if resolved in dangerous:
        raise SecurityError(
            f"Session directory cannot be a shared system root or temp directory directly ({session_dir}). "
            "Use a dedicated private subdirectory such as '.telegram_account/' or '/var/lib/telegram-runner/sessions/'."
        )


def secure_file(path: Path):
    """Enforce private 0600 permissions and reject unsafe symlinks."""
    check_no_symlinks(path)
    if path.exists():
        if not stat.S_ISREG(path.stat().st_mode):
            raise SecurityError("Session path must be a regular file.")
        try:
            os.chmod(path, 0o600)
            mode = stat.S_IMODE(path.stat().st_mode)
            if mode != 0o600:
                raise SecurityError(f"Failed to enforce 0600 mode on file: {path}")
        except OSError as e:
            raise SecurityError(f"Failed to set 0600 permissions on file {path}: {e}")


def ensure_private_session_storage(session_file: Path):
    """Create dedicated private session directory (0700) and .session file (0600) before Telethon client init."""
    check_no_symlinks(session_file)
    session_dir = session_file.parent
    validate_session_directory(session_dir)

    # 1. Directory private setup
    if not session_dir.exists():
        try:
            session_dir.mkdir(parents=True, mode=0o700, exist_ok=True)
            os.chmod(session_dir, 0o700)
        except OSError as e:
            raise SecurityError(f"Failed to create private session directory {session_dir}: {e}")
    else:
        current_mode = stat.S_IMODE(session_dir.stat().st_mode)
        if current_mode & 0o077 != 0:
            raise SecurityError(
                f"Session directory '{session_dir}' has permissions {oct(current_mode)}, which is not private. "
                "Existing directories must have mode 0700 (drwx------). Secure it manually or specify a dedicated private session directory."
            )

    # 2. File private setup (mode 0600)
    if not session_file.exists():
        try:
            flags = os.O_CREAT | os.O_EXCL | os.O_RDWR
            if hasattr(os, "O_NOFOLLOW"):
                flags |= os.O_NOFOLLOW
            fd = os.open(str(session_file), flags, 0o600)
            try:
                os.fchmod(fd, 0o600)
                if stat.S_IMODE(os.fstat(fd).st_mode) != 0o600:
                    raise SecurityError(f"Failed to enforce 0600 mode on session file: {session_file}")
            finally:
                os.close(fd)
        except OSError as e:
            raise SecurityError(f"Failed to pre-create private session file {session_file}: {e}")
    else:
        secure_file(session_file)


class SessionLock:
    """POSIX process lock using fcntl.flock to serialize SQLite session access."""

    def __init__(self, session_path: Union[str, Path], timeout: float = 10.0):
        self.session_file = resolve_session_path(session_path)
        self.lock_path = Path(f"{self.session_file}.lock")
        self.timeout = float(timeout)
        self.fd: Optional[int] = None

    def _open_lock_file(self):
        if self.fd is not None:
            return
        check_no_symlinks(self.lock_path)
        check_no_symlinks(self.session_file)

        session_dir = self.session_file.parent
        validate_session_directory(session_dir)

        flags = os.O_CREAT | os.O_RDWR
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW

        try:
            self.fd = os.open(str(self.lock_path), flags, 0o600)
        except OSError as e:
            raise SecurityError(f"Failed to open lock file safely at {self.lock_path}: {e}")

        try:
            st = os.fstat(self.fd)
            if not stat.S_ISREG(st.st_mode):
                os.close(self.fd)
                self.fd = None
                raise SecurityError(f"Lock path is not a regular file: {self.lock_path}")
            os.fchmod(self.fd, 0o600)
            if stat.S_IMODE(os.fstat(self.fd).st_mode) != 0o600:
                os.close(self.fd)
                self.fd = None
                raise SecurityError(f"Failed to enforce 0600 mode on lock file: {self.lock_path}")
        except OSError as e:
            if self.fd is not None:
                os.close(self.fd)
                self.fd = None
            raise SecurityError(f"Lock file fchmod failed: {e}")

    def acquire(self) -> "SessionLock":
        self._open_lock_file()
        start_time = time.monotonic()
        while True:
            try:
                fcntl.flock(self.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return self
            except (BlockingIOError, OSError):
                if time.monotonic() - start_time >= self.timeout:
                    self.release()
                    raise SessionLockedError(
                        f"Timed out after {self.timeout}s waiting for session lock at {self.lock_path}"
                    )
                time.sleep(0.1)

    async def async_acquire(self, timeout: Optional[float] = None) -> "SessionLock":
        effective_timeout = float(timeout) if timeout is not None else self.timeout
        self._open_lock_file()
        start_time = time.monotonic()
        try:
            while True:
                try:
                    fcntl.flock(self.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    return self
                except (BlockingIOError, OSError):
                    if time.monotonic() - start_time >= effective_timeout:
                        self.release()
                        raise SessionLockedError(
                            f"Timed out after {effective_timeout}s waiting for session lock at {self.lock_path}"
                        )
                    await asyncio.sleep(0.02)
        except BaseException:
            self.release()
            raise

    def release(self):
        if self.fd is not None:
            try:
                fcntl.flock(self.fd, fcntl.LOCK_UN)
            except OSError:
                pass
            try:
                os.close(self.fd)
            except OSError:
                pass
            self.fd = None

    def __enter__(self) -> "SessionLock":
        return self.acquire()

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.release()


@dataclass
class Config:
    """Canonical test account configuration."""

    api_id: Optional[int] = None
    api_hash: Optional[str] = field(default=None, repr=False)
    phone: Optional[str] = field(default=None, repr=False)
    session_path: Path = field(
        default_factory=lambda: Path("dev/.telegram_account/session.session")
        if Path("dev").is_dir()
        else Path(".telegram_account/session.session")
    )
    allowed_bots: List[str] = field(default_factory=list)
    timeout: float = 30.0
    env_file: Optional[Path] = None

    @classmethod
    def load(
        cls,
        env_file: Optional[Union[str, Path]] = None,
        session_path: Optional[Union[str, Path]] = None,
        allowed_bots: Optional[Union[str, List[str]]] = None,
        phone: Optional[str] = None,
        api_id: Optional[Union[int, str]] = None,
        api_hash: Optional[str] = None,
        timeout: Optional[Union[float, int, str]] = None,
    ) -> "Config":
        target_env = None
        if env_file:
            target_env = Path(env_file)
        else:
            default_candidate = Path(__file__).resolve().parents[2] / ".env"
            if default_candidate.is_file():
                target_env = default_candidate

        env_data = parse_env_file(target_env) if target_env and target_env.is_file() else {}

        # 1. API ID (must be positive integer)
        raw_api_id = api_id if api_id is not None else env_data.get("TELEGRAM_TEST_API_ID")
        if raw_api_id is None:
            raw_api_id = os.environ.get("TELEGRAM_TEST_API_ID")
        resolved_api_id = None
        if raw_api_id is not None and str(raw_api_id).strip() != "":
            try:
                resolved_api_id = int(str(raw_api_id).strip())
                if resolved_api_id <= 0:
                    raise ConfigError("Invalid API ID: must be a positive integer")
            except ValueError:
                raise ConfigError("Invalid API ID: must be a positive integer")

        # 2. API Hash (must be 32 hexadecimal characters)
        raw_api_hash = api_hash if api_hash is not None else env_data.get("TELEGRAM_TEST_API_HASH")
        if raw_api_hash is None:
            raw_api_hash = os.environ.get("TELEGRAM_TEST_API_HASH")
        resolved_api_hash = None
        if raw_api_hash is not None and str(raw_api_hash).strip() != "":
            cleaned_hash = str(raw_api_hash).strip()
            if not re.match(r"^[0-9a-fA-F]{32}$", cleaned_hash):
                raise ConfigError("Invalid API hash format: must be 32 hexadecimal characters")
            resolved_api_hash = cleaned_hash

        # 3. Phone (only needed for login)
        raw_phone = phone if phone is not None else env_data.get("TELEGRAM_TEST_PHONE")
        if raw_phone is None:
            raw_phone = os.environ.get("TELEGRAM_TEST_PHONE")
        resolved_phone = str(raw_phone).strip() if raw_phone else None

        # 4. Session Path
        raw_session = session_path if session_path is not None else env_data.get("TELEGRAM_TEST_SESSION_PATH")
        if raw_session is None:
            raw_session = os.environ.get("TELEGRAM_TEST_SESSION_PATH")
        if raw_session:
            resolved_session = Path(raw_session)
        else:
            default_dir = Path("dev/.telegram_account") if Path("dev").is_dir() else Path(".telegram_account")
            resolved_session = default_dir / "session.session"

        # 5. Allowed Bots
        raw_bots = allowed_bots if allowed_bots is not None else env_data.get("TELEGRAM_TEST_ALLOWED_BOTS")
        if raw_bots is None:
            raw_bots = os.environ.get("TELEGRAM_TEST_ALLOWED_BOTS")
        resolved_bots: List[str] = []
        if raw_bots:
            parts = raw_bots.split(",") if isinstance(raw_bots, str) else list(raw_bots)
            for b in parts:
                cleaned = str(b).strip().lstrip("@").lower()
                if cleaned and cleaned not in resolved_bots:
                    resolved_bots.append(cleaned)

        # 6. Timeout (positive finite float)
        raw_timeout = timeout if timeout is not None else env_data.get("TELEGRAM_TEST_TIMEOUT")
        if raw_timeout is None:
            raw_timeout = os.environ.get("TELEGRAM_TEST_TIMEOUT")
        if raw_timeout is None:
            raw_timeout = 30.0
        try:
            resolved_timeout = float(raw_timeout)
            if not math.isfinite(resolved_timeout) or resolved_timeout <= 0:
                raise ConfigError("Invalid timeout: must be a positive finite number")
        except (ValueError, TypeError):
            raise ConfigError("Invalid timeout: must be a positive finite number")

        return cls(
            api_id=resolved_api_id,
            api_hash=resolved_api_hash,
            phone=resolved_phone,
            session_path=resolved_session,
            allowed_bots=resolved_bots,
            timeout=resolved_timeout,
            env_file=target_env,
        )

    def validate_credentials(self):
        """Ensure api_id and api_hash are present."""
        if not self.api_id or not self.api_hash:
            raise ConfigError(
                "Missing required MTProto API credentials. "
                "Configure TELEGRAM_TEST_API_ID and TELEGRAM_TEST_API_HASH in .env or via CLI options."
            )


class TelegramAccountClient:
    """Manages Telethon connection, process locking, bot validation, and operations."""

    def __init__(self, config: Config):
        self.config = config
        self.session_file = resolve_session_path(config.session_path)

    def build_client(self, require_existing_session: bool = True) -> TelegramClient:
        """Construct a configured Telethon client."""
        self.config.validate_credentials()
        check_no_symlinks(self.session_file)

        if require_existing_session:
            if not self.session_file.exists():
                raise NotAuthorizedError(
                    f"Session file not found at {self.session_file}. Run 'login' command first."
                )
            secure_file(self.session_file)
        else:
            ensure_private_session_storage(self.session_file)

        logging.getLogger("telethon").setLevel(logging.WARNING)

        # Telethon 1.45.0 quirk: session must be passed as str, not Path.
        # Explicitly set auto_reconnect=False to prevent infinite background reconnects.
        return TelegramClient(
            str(self.session_file),
            api_id=self.config.api_id,
            api_hash=self.config.api_hash,
            auto_reconnect=False,
            flood_sleep_threshold=0,
            request_retries=0,
            connection_retries=1,
            retry_delay=1,
            timeout=self.config.timeout,
        )

    async def _execute_with_client(
        self,
        coro_fn: Callable[[TelegramClient], Any],
        require_existing_session: bool = True,
    ) -> Any:
        """Acquire process lock, initialize client, run operation, and safely clean up under deadline."""
        # Noninteractive check BEFORE creating lock or dirs
        if require_existing_session:
            check_no_symlinks(self.session_file)
            if not self.session_file.exists():
                raise NotAuthorizedError(
                    f"Session file not found at {self.session_file}. Run 'login' command first."
                )
            secure_file(self.session_file)

        deadline = self.config.timeout
        lock = SessionLock(self.session_file, timeout=deadline)

        async def _runner():
            await lock.async_acquire(timeout=deadline)
            client: Optional[TelegramClient] = None
            try:
                client = self.build_client(require_existing_session=require_existing_session)
                await client.connect()

                if require_existing_session:
                    if not await client.is_user_authorized():
                        raise NotAuthorizedError(
                            "Session is not authorized or expired. Run 'login' command first."
                        )

                return await coro_fn(client)
            finally:
                try:
                    if client is not None and client.is_connected():
                        try:
                            await asyncio.wait_for(
                                client.disconnect(),
                                timeout=min(1.0, max(0.05, deadline / 4)),
                            )
                        except (Exception, asyncio.TimeoutError, TimeoutError):
                            pass
                finally:
                    if client is not None:
                        try:
                            if hasattr(client, "session") and hasattr(client.session, "close"):
                                client.session.close()
                        except Exception:
                            pass
                    lock.release()

        try:
            return await asyncio.wait_for(_runner(), timeout=deadline)
        except telethon.errors.FloodWaitError as e:
            raise FloodWaitError(
                f"Telegram FloodWait: must wait {e.seconds} seconds before retrying",
                retry_after_seconds=e.seconds,
            ) from e
        except (
            telethon.errors.AuthKeyUnregisteredError,
            telethon.errors.AuthKeyInvalidError,
            telethon.errors.UserDeactivatedError,
            telethon.errors.SessionRevokedError,
            telethon.errors.SessionExpiredError,
        ) as e:
            raise NotAuthorizedError(
                f"Telegram session is invalid or expired ({e.__class__.__name__}). Run 'login' command first."
            ) from e
        except SessionLockedError:
            raise
        except (asyncio.TimeoutError, TimeoutError) as e:
            lock.release()
            raise NetworkTimeoutError(f"Operation timed out after {deadline}s") from e
        except BaseException:
            lock.release()
            raise

    def pre_validate_target(self, target_bot: str) -> str:
        """Validate target bot before contacting Telegram."""
        if not target_bot or not isinstance(target_bot, str):
            raise TargetViolationError("Target bot must be a non-empty string username")

        cleaned = target_bot.strip().lstrip("@")
        if not cleaned:
            raise TargetViolationError("Target bot username cannot be empty")

        if cleaned.isdigit() or cleaned.startswith("-") or cleaned.startswith("+"):
            raise TargetViolationError(
                f"Target '{target_bot}' is invalid: numeric IDs and phone numbers are not allowed"
            )

        if not self.config.allowed_bots:
            raise TargetViolationError(
                "Allowed bots list is empty. Configure TELEGRAM_TEST_ALLOWED_BOTS (fails closed)"
            )

        if cleaned.lower() not in [b.lower() for b in self.config.allowed_bots]:
            raise TargetViolationError(
                f"Target bot '{cleaned}' is not in allowed bots list: {self.config.allowed_bots}"
            )

        return cleaned

    async def resolve_bot(self, client: TelegramClient, target_bot: str) -> types.User:
        """Validate target bot against allowlist and verify resolved entity properties."""
        cleaned = self.pre_validate_target(target_bot)

        try:
            entity = await client.get_entity(cleaned)
        except Exception as e:
            raise TargetViolationError(f"Failed to resolve target bot '{cleaned}': {e}") from e

        if not isinstance(entity, types.User):
            raise TargetViolationError(
                f"Target '{cleaned}' resolved to non-user entity: {type(entity).__name__}"
            )

        if not getattr(entity, "bot", False):
            raise TargetViolationError(f"Target '{cleaned}' is a human user, not a bot")

        resolved_username = (getattr(entity, "username", "") or "").lower()
        if resolved_username != cleaned.lower():
            raise TargetViolationError(
                f"Target '{cleaned}' resolved to different username '{resolved_username}'"
            )

        return entity

    async def get_status(self) -> Dict[str, Any]:
        """Verify session validity and return masked status."""
        async def _action(client: TelegramClient):
            me = await client.get_me()
            raw_phone = getattr(me, "phone", "") or self.config.phone or ""
            return {
                "success": True,
                "authorized": True,
                "user_id": getattr(me, "id", None),
                "username": getattr(me, "username", None),
                "first_name": getattr(me, "first_name", None),
                "phone_masked": mask_phone(raw_phone),
            }

        return await self._execute_with_client(_action, require_existing_session=True)

    async def send_text(self, target_bot: str, text: str) -> Dict[str, Any]:
        """Send exact literal text message to an allowlisted bot (parse_mode=None)."""
        self.pre_validate_target(target_bot)
        if not text:
            raise AccountToolError("Message text cannot be empty", exit_code=EXIT_GENERIC_ERROR)

        async def _action(client: TelegramClient):
            entity = await self.resolve_bot(client, target_bot)
            # pass parse_mode=None to guarantee exact literal user text preservation
            msg = await client.send_message(entity, text, parse_mode=None, link_preview=False)
            return {
                "success": True,
                "bot": target_bot.lstrip("@"),
                "message_id": msg.id,
                "date": msg.date.isoformat() if hasattr(msg.date, "isoformat") else str(msg.date),
                "text": msg.text,
            }

        return await self._execute_with_client(_action, require_existing_session=True)

    async def send_file(
        self,
        target_bot: str,
        file_path: Union[str, Path],
        caption: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Send local file to an allowlisted bot with path validation and literal caption."""
        self.pre_validate_target(target_bot)
        path = Path(file_path)
        if not path.exists():
            raise AccountToolError(f"File not found: {file_path}", exit_code=EXIT_GENERIC_ERROR)
        if path.is_symlink():
            raise SecurityError(f"Refusing to send symlink file: {file_path}")
        if not path.is_file():
            raise AccountToolError(f"Path is not a regular file: {file_path}", exit_code=EXIT_GENERIC_ERROR)
        if not os.access(path, os.R_OK):
            raise AccountToolError(f"File is not readable: {file_path}", exit_code=EXIT_GENERIC_ERROR)

        async def _action(client: TelegramClient):
            entity = await self.resolve_bot(client, target_bot)
            # pass parse_mode=None to guarantee exact literal caption preservation
            msg = await client.send_file(entity, str(path), caption=caption, parse_mode=None)
            return {
                "success": True,
                "bot": target_bot.lstrip("@"),
                "message_id": msg.id,
                "date": msg.date.isoformat() if hasattr(msg.date, "isoformat") else str(msg.date),
                "file_name": path.name,
                "caption": caption,
            }

        return await self._execute_with_client(_action, require_existing_session=True)

    async def read_messages(
        self,
        target_bot: str,
        limit: int = 5,
        min_id: int = 0,
        wait_seconds: Optional[float] = None,
    ) -> Dict[str, Any]:
        """Read recent messages and full inline button trees from an allowlisted bot."""
        self.pre_validate_target(target_bot)
        async def _action(client: TelegramClient):
            entity = await self.resolve_bot(client, target_bot)

            if wait_seconds and wait_seconds > 0:
                start_wait = time.monotonic()
                found_any = False
                while time.monotonic() - start_wait < wait_seconds:
                    async for m in client.iter_messages(entity, limit=1, min_id=min_id):
                        found_any = True
                        break
                    if found_any:
                        break
                    await asyncio.sleep(0.5)

            messages = []
            async for msg in client.iter_messages(entity, limit=limit, min_id=min_id):
                buttons_data = []
                if msg.buttons:
                    for r_idx, row in enumerate(msg.buttons):
                        row_data = []
                        for c_idx, btn in enumerate(row):
                            data_val = None
                            btn_data = getattr(btn, "data", None)
                            if btn_data is not None:
                                try:
                                    data_val = btn_data.decode("utf-8")
                                except Exception:
                                    data_val = btn_data.hex() if hasattr(btn_data, "hex") else str(btn_data)
                            row_data.append({
                                "row": r_idx,
                                "col": c_idx,
                                "text": btn.text,
                                "data": data_val,
                                "url": getattr(btn, "url", None),
                                "type": "callback" if btn_data is not None else "other",
                            })
                        buttons_data.append(row_data)

                messages.append({
                    "id": msg.id,
                    "date": msg.date.isoformat() if hasattr(msg.date, "isoformat") else str(msg.date),
                    "text": msg.text or "",
                    "has_media": bool(msg.media),
                    "buttons": buttons_data,
                })

            return {
                "success": True,
                "bot": target_bot.lstrip("@"),
                "count": len(messages),
                "messages": messages,
            }

        return await self._execute_with_client(_action, require_existing_session=True)

    async def click_button(
        self,
        target_bot: str,
        message_id: int,
        button_text: Optional[str] = None,
        button_data: Optional[str] = None,
        row: Optional[int] = None,
        col: Optional[int] = None,
    ) -> Dict[str, Any]:
        """Click an actual inline callback button on a bot message."""
        self.pre_validate_target(target_bot)
        async def _action(client: TelegramClient):
            entity = await self.resolve_bot(client, target_bot)
            msg = await client.get_messages(entity, ids=message_id)
            if not msg:
                raise AccountToolError(f"Message ID {message_id} not found", exit_code=EXIT_GENERIC_ERROR)

            if not msg.buttons:
                raise AccountToolError(
                    f"Message ID {message_id} has no inline buttons", exit_code=EXIT_GENERIC_ERROR
                )

            target_btn = None
            target_row, target_col = None, None

            if row is not None and col is not None:
                if row < 0 or row >= len(msg.buttons):
                    raise AccountToolError(
                        f"Row {row} out of range (total rows: {len(msg.buttons)})",
                        exit_code=EXIT_GENERIC_ERROR,
                    )
                if col < 0 or col >= len(msg.buttons[row]):
                    raise AccountToolError(
                        f"Col {col} out of range (row {row} has {len(msg.buttons[row])} columns)",
                        exit_code=EXIT_GENERIC_ERROR,
                    )
                target_btn = msg.buttons[row][col]
                target_row, target_col = row, col
            elif button_text:
                cleaned_target = button_text.strip().lower()
                for r_idx, r in enumerate(msg.buttons):
                    for c_idx, b in enumerate(r):
                        if b.text.strip().lower() == cleaned_target:
                            target_btn = b
                            target_row, target_col = r_idx, c_idx
                            break
                    if target_btn:
                        break
                if not target_btn:
                    raise AccountToolError(
                        f"Button with text '{button_text}' not found in message {message_id}",
                        exit_code=EXIT_GENERIC_ERROR,
                    )
            elif button_data:
                target_bytes = button_data.encode("utf-8")
                for r_idx, r in enumerate(msg.buttons):
                    for c_idx, b in enumerate(r):
                        b_data = getattr(b, "data", None)
                        if b_data is not None:
                            if b_data == target_bytes or (
                                hasattr(b_data, "decode")
                                and b_data.decode("utf-8", errors="ignore") == button_data
                            ):
                                target_btn = b
                                target_row, target_col = r_idx, c_idx
                                break
                    if target_btn:
                        break
                if not target_btn:
                    raise AccountToolError(
                        f"Button with data '{button_data}' not found in message {message_id}",
                        exit_code=EXIT_GENERIC_ERROR,
                    )
            else:
                raise AccountToolError(
                    "Must specify button selector: button_text, button_data, or row and col",
                    exit_code=EXIT_GENERIC_ERROR,
                )

            # Security verification: MUST be a real callback button
            b_data = getattr(target_btn, "data", None)
            if b_data is None or not isinstance(b_data, (bytes, bytearray)):
                raise UnsupportedButtonError(
                    f"Button '{target_btn.text}' has no callback data (type: {type(target_btn).__name__}). "
                    "Only actual callback buttons are allowed; contact sharing, location, URL auth, "
                    "and inline switch buttons are forbidden."
                )

            if getattr(target_btn, "url", None) is not None:
                raise UnsupportedButtonError(
                    f"Button '{target_btn.text}' is a URL button, which is not an allowable callback button."
                )

            if getattr(target_btn, "inline_query", None) is not None:
                raise UnsupportedButtonError(
                    f"Button '{target_btn.text}' is an inline switch button, which is not an allowable callback button."
                )

            res = await target_btn.click()
            answer_text = getattr(res, "message", None) if res else None
            has_alert = getattr(res, "alert", False) if res else False

            b_data_str = None
            try:
                b_data_str = b_data.decode("utf-8")
            except Exception:
                b_data_str = b_data.hex() if hasattr(b_data, "hex") else str(b_data)

            return {
                "success": True,
                "bot": target_bot.lstrip("@"),
                "message_id": message_id,
                "row": target_row,
                "col": target_col,
                "button_text": target_btn.text,
                "button_data": b_data_str,
                "answer": answer_text,
                "alert": has_alert,
            }

        return await self._execute_with_client(_action, require_existing_session=True)

    async def download_media(
        self,
        target_bot: str,
        message_id: int,
        dest_dir: Union[str, Path],
    ) -> Dict[str, Any]:
        """Download media attachment with private atomic staging, no overwrite, and 0600 permissions."""
        self.pre_validate_target(target_bot)
        check_no_symlinks(dest_dir)
        dest_path = Path(dest_dir)

        if not dest_path.exists():
            try:
                dest_path.mkdir(parents=True, mode=0o700, exist_ok=True)
                os.chmod(dest_path, 0o700)
            except OSError as e:
                raise SecurityError(f"Failed to create destination directory: {e}")
        else:
            if not dest_path.is_dir():
                raise AccountToolError(
                    f"Destination path exists and is not a directory: {dest_dir}",
                    exit_code=EXIT_GENERIC_ERROR,
                )
            if dest_path.is_symlink() or os.path.islink(dest_dir):
                raise SecurityError(f"Destination directory cannot be a symlink: {dest_dir}")

        async def _action(client: TelegramClient):
            entity = await self.resolve_bot(client, target_bot)
            msg = await client.get_messages(entity, ids=message_id)
            if not msg:
                raise AccountToolError(f"Message ID {message_id} not found", exit_code=EXIT_GENERIC_ERROR)
            if not getattr(msg, "media", None):
                raise AccountToolError(
                    f"Message ID {message_id} has no media attachment", exit_code=EXIT_GENERIC_ERROR
                )

            # 1. Create known private staging directory under dest_path BEFORE network
            staging_dir = Path(tempfile.mkdtemp(dir=str(dest_path), prefix=".staging_"))
            try:
                os.chmod(staging_dir, 0o700)
            except OSError as e:
                shutil.rmtree(staging_dir, ignore_errors=True)
                raise SecurityError(f"Failed to set 0700 permissions on staging dir: {e}")

            staging_file = staging_dir / "download.tmp"

            # Pre-create staging file with 0600 permissions
            try:
                fd = os.open(str(staging_file), os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600)
                os.close(fd)
            except OSError as e:
                shutil.rmtree(staging_dir, ignore_errors=True)
                raise SecurityError(f"Failed to create staging file: {e}")

            try:
                # Download directly into staging_file
                res_path = await client.download_media(msg, file=str(staging_file))
                if not res_path or not staging_file.exists():
                    raise AccountToolError("Media download returned empty result", exit_code=EXIT_GENERIC_ERROR)

                os.chmod(staging_file, 0o600)

                # Determine safe target filename
                orig_name = None
                file_obj = getattr(msg, "file", None)
                if file_obj and getattr(file_obj, "name", None) and isinstance(file_obj.name, str):
                    orig_name = Path(file_obj.name).name
                if not orig_name:
                    ext = getattr(file_obj, "ext", "") if file_obj and isinstance(getattr(file_obj, "ext", None), str) else ""
                    orig_name = f"media_{message_id}{ext}"

                safe_name = "".join(c for c in orig_name if c.isalnum() or c in "._-")
                if not safe_name or safe_name in (".", ".."):
                    safe_name = f"media_{message_id}.bin"

                final_path = dest_path / safe_name

                # Verify no overwrite!
                if final_path.exists() or os.path.islink(final_path):
                    raise AccountToolError(
                        f"Destination file already exists (refusing to overwrite): {final_path.name}",
                        exit_code=EXIT_GENERIC_ERROR,
                    )

                # Atomically move from staging to final path
                os.replace(staging_file, final_path)
                os.chmod(final_path, 0o600)

                return {
                    "success": True,
                    "bot": target_bot.lstrip("@"),
                    "message_id": message_id,
                    "file_path": str(final_path),
                    "file_size": final_path.stat().st_size,
                }
            except BaseException:
                # Clean up all staging artifacts on error or cancellation
                raise
            finally:
                # Always remove private staging directory
                shutil.rmtree(staging_dir, ignore_errors=True)

        return await self._execute_with_client(_action, require_existing_session=True)

    async def smoke(
        self,
        target_bot: str,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """Execute automated start/guide smoke test against allowlisted bot."""
        self.pre_validate_target(target_bot)
        smoke_timeout = float(timeout) if timeout else self.config.timeout

        async def _action(client: TelegramClient):
            entity = await self.resolve_bot(client, target_bot)
            steps: List[Dict[str, Any]] = []

            # 1. Query latest message ID to differentiate fresh responses
            latest_id = 0
            async for m in client.iter_messages(entity, limit=1):
                latest_id = m.id
            steps.append({"step": "query_latest_id", "latest_id": latest_id})

            # 2. Send /start (with parse_mode=None for literal delivery)
            start_msg = await client.send_message(entity, "/start", parse_mode=None)
            steps.append({"step": "send_start", "message_id": start_msg.id})

            # 3. Wait for fresh response from bot
            start_time = time.monotonic()
            fresh_msg = None
            while time.monotonic() - start_time < smoke_timeout:
                async for m in client.iter_messages(entity, limit=1):
                    if m.id > latest_id and m.id != start_msg.id:
                        fresh_msg = m
                        break
                if fresh_msg:
                    break
                await asyncio.sleep(0.5)

            if not fresh_msg:
                raise NetworkTimeoutError(
                    f"Timed out after {smoke_timeout}s waiting for bot response to /start"
                )

            steps.append({
                "step": "received_start_response",
                "message_id": fresh_msg.id,
                "text": fresh_msg.text or "",
                "has_buttons": bool(fresh_msg.buttons),
            })

            curr_msg = fresh_msg

            def find_btn(m, data_str=None):
                if not m or not m.buttons:
                    return None
                for row in m.buttons:
                    for b in row:
                        bd = getattr(b, "data", None)
                        if bd is not None:
                            try:
                                bd_str = bd.decode("utf-8")
                            except Exception:
                                bd_str = ""
                            if bd_str == data_str:
                                return b
                return None

            # 4. Check for language selection callback (e.g. lang:set:en)
            lang_btn = find_btn(curr_msg, data_str="lang:set:en")
            if lang_btn:
                steps.append({"step": "language_picker_detected", "button": "lang:set:en"})
                await lang_btn.click()

                wait_lang = time.monotonic()
                updated_msg = None
                orig_text = curr_msg.text
                while time.monotonic() - wait_lang < min(10.0, smoke_timeout):
                    m_check = await client.get_messages(entity, ids=curr_msg.id)
                    if m_check and (m_check.text != orig_text or m_check.buttons != curr_msg.buttons):
                        updated_msg = m_check
                        break
                    async for nm in client.iter_messages(entity, limit=1):
                        if nm.id > curr_msg.id:
                            updated_msg = nm
                            break
                    if updated_msg:
                        break
                    await asyncio.sleep(0.5)

                if updated_msg:
                    curr_msg = updated_msg
                    steps.append({
                        "step": "language_updated",
                        "message_id": curr_msg.id,
                        "text": curr_msg.text or "",
                        "has_buttons": bool(curr_msg.buttons),
                    })

            # 5. Check for Guide button ('start:guide')
            guide_btn = find_btn(curr_msg, data_str="start:guide")
            if guide_btn:
                steps.append({"step": "guide_button_detected", "button": "start:guide"})
                await guide_btn.click()

                wait_guide = time.monotonic()
                updated_msg = None
                orig_text = curr_msg.text
                while time.monotonic() - wait_guide < min(10.0, smoke_timeout):
                    m_check = await client.get_messages(entity, ids=curr_msg.id)
                    if m_check and (m_check.text != orig_text or m_check.buttons != curr_msg.buttons):
                        updated_msg = m_check
                        break
                    async for nm in client.iter_messages(entity, limit=1):
                        if nm.id > curr_msg.id:
                            updated_msg = nm
                            break
                    if updated_msg:
                        break
                    await asyncio.sleep(0.5)

                if updated_msg:
                    curr_msg = updated_msg
                    steps.append({
                        "step": "guide_view_loaded",
                        "message_id": curr_msg.id,
                        "text": curr_msg.text or "",
                        "has_buttons": bool(curr_msg.buttons),
                    })

                    # 6. Check for Back button ('start:panel')
                    back_btn = find_btn(curr_msg, data_str="start:panel")
                    if back_btn:
                        steps.append({"step": "panel_back_button_detected", "button": "start:panel"})
                        await back_btn.click()

                        wait_panel = time.monotonic()
                        returned_msg = None
                        orig_text = curr_msg.text
                        while time.monotonic() - wait_panel < min(10.0, smoke_timeout):
                            m_check = await client.get_messages(entity, ids=curr_msg.id)
                            if m_check and (m_check.text != orig_text or m_check.buttons != curr_msg.buttons):
                                returned_msg = m_check
                                break
                            async for nm in client.iter_messages(entity, limit=1):
                                if nm.id > curr_msg.id:
                                    returned_msg = nm
                                    break
                            if returned_msg:
                                break
                            await asyncio.sleep(0.5)

                        if returned_msg:
                            curr_msg = returned_msg
                            steps.append({
                                "step": "returned_to_start_panel",
                                "message_id": curr_msg.id,
                                "text": curr_msg.text or "",
                            })

            return {
                "success": True,
                "bot": target_bot.lstrip("@"),
                "steps": steps,
                "final_message": {
                    "id": curr_msg.id,
                    "text": curr_msg.text or "",
                    "has_buttons": bool(curr_msg.buttons),
                },
            }

        return await self._execute_with_client(_action, require_existing_session=True)

    async def interactive_login(self, phone: Optional[str] = None) -> Dict[str, Any]:
        """Perform one-time interactive login authentication with private storage setup."""
        target_phone = phone or self.config.phone
        if not target_phone:
            raise ConfigError(
                "Phone number is required for login. Set TELEGRAM_TEST_PHONE or pass --phone."
            )

        # Pre-create private session storage BEFORE Telethon client construction
        ensure_private_session_storage(self.session_file)

        deadline = self.config.timeout
        lock = SessionLock(self.session_file, timeout=deadline)
        await lock.async_acquire(timeout=deadline)
        client: Optional[TelegramClient] = None
        try:
            client = self.build_client(require_existing_session=False)
            await client.connect()

            if await client.is_user_authorized():
                secure_file(self.session_file)
                me = await client.get_me()
                return {
                    "success": True,
                    "already_authorized": True,
                    "user_id": getattr(me, "id", None),
                    "username": getattr(me, "username", None),
                    "phone_masked": mask_phone(target_phone),
                }

            await client.send_code_request(target_phone)
            code = input(f"Enter Telegram login code for {mask_phone(target_phone)}: ").strip()
            if not code:
                raise AccountToolError("Login cancelled: empty code entered", exit_code=EXIT_GENERIC_ERROR)

            try:
                await client.sign_in(phone=target_phone, code=code)
            except errors.SessionPasswordNeededError:
                password = getpass.getpass("Enter 2FA password: ")
                await client.sign_in(password=password)

            secure_file(self.session_file)
            me = await client.get_me()
            return {
                "success": True,
                "authorized": True,
                "user_id": getattr(me, "id", None),
                "username": getattr(me, "username", None),
                "phone_masked": mask_phone(target_phone),
            }
        except telethon.errors.FloodWaitError as e:
            raise FloodWaitError(
                f"Telegram FloodWait: must wait {e.seconds} seconds before retrying",
                retry_after_seconds=e.seconds,
            ) from e
        finally:
            if client is not None:
                try:
                    if client.is_connected():
                        await asyncio.wait_for(client.disconnect(), timeout=min(2.0, deadline))
                except Exception:
                    pass
                try:
                    if hasattr(client, "session") and hasattr(client.session, "close"):
                        client.session.close()
                except Exception:
                    pass
            lock.release()
