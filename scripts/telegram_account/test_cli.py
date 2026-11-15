"""Offline unit test suite for Telegram Account CLI and client tooling.

Enforces zero outbound network access by patching socket.socket.connect and socket.create_connection.
Tests session serialization (fcntl.flock), target allowlist validation, entity resolution,
button clicking safety, atomic media download staging, error translation, CLI error sanitization,
and timeouts with bounded cleanup.
"""

import asyncio
import io
import json
import math
import os
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock, Mock, patch

# Ensure scripts/telegram_account directory is importable
SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import telethon
from telethon import errors, types
from telethon.sessions import SQLiteSession

from client import (
    EXIT_CONFIG_ERROR,
    EXIT_FLOOD_WAIT,
    EXIT_GENERIC_ERROR,
    EXIT_NETWORK_TIMEOUT,
    EXIT_SESSION_LOCKED,
    EXIT_SUCCESS,
    EXIT_TARGET_VIOLATION,
    EXIT_UNAUTHORIZED,
    AccountToolError,
    Config,
    ConfigError,
    FloodWaitError,
    NetworkTimeoutError,
    NotAuthorizedError,
    SecurityError,
    SessionLock,
    SessionLockedError,
    TargetViolationError,
    TelegramAccountClient,
    UnsupportedButtonError,
    check_no_symlinks,
    ensure_private_session_storage,
    mask_phone,
    parse_env_file,
    resolve_session_path,
    validate_session_directory,
)
import cli


# Network guard: prevent any outbound socket connections
_orig_connect = socket.socket.connect
_orig_create_connection = socket.create_connection


def guarded_connect(self, address):
    # Allow AF_UNIX for local asyncio loop socketpairs
    if self.family == socket.AF_UNIX:
        return _orig_connect(self, address)
    raise RuntimeError(f"Network access blocked in offline test: connect to {address}")


def guarded_create_connection(address, *args, **kwargs):
    raise RuntimeError(f"Network access blocked in offline test: create_connection to {address}")


class NetworkGuardedTestCase(unittest.TestCase):
    """Base test case enforcing zero outbound network access."""

    @classmethod
    def setUpClass(cls):
        socket.socket.connect = guarded_connect
        socket.create_connection = guarded_create_connection

    @classmethod
    def tearDownClass(cls):
        socket.socket.connect = _orig_connect
        socket.create_connection = _orig_create_connection


class ConfigAndUtilsTest(NetworkGuardedTestCase):
    """Test configuration loading, validation, phone masking, and secret redaction."""

    def test_phone_masking(self):
        self.assertEqual(mask_phone("+98912341234"), "+98*****1234")
        self.assertEqual(mask_phone("+989123456789"), "+98******6789")
        self.assertEqual(mask_phone("09123456789"), "09*****6789")
        self.assertEqual(mask_phone("123"), "***")
        self.assertEqual(mask_phone(""), "")
        self.assertEqual(mask_phone(None), "")

    def test_parse_env_file_safely(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            env_path = Path(tmpdir) / ".env"
            env_path.write_text(
                "# Comment line\n"
                "TELEGRAM_TEST_API_ID=123456\n"
                "TELEGRAM_TEST_API_HASH=\"abcdef0123456789abcdef0123456789\"\n"
                "export TELEGRAM_TEST_ALLOWED_BOTS='bot_a, @bot_b '\n"
                "EMPTY_VAL=\n"
                "\n"
            )
            parsed = parse_env_file(env_path)
            self.assertEqual(parsed.get("TELEGRAM_TEST_API_ID"), "123456")
            self.assertEqual(parsed.get("TELEGRAM_TEST_API_HASH"), "abcdef0123456789abcdef0123456789")
            self.assertEqual(parsed.get("TELEGRAM_TEST_ALLOWED_BOTS"), "bot_a, @bot_b ")
            self.assertEqual(parsed.get("EMPTY_VAL"), "")

    def test_config_load_canonical_names_only(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            env_path = Path(tmpdir) / ".env"
            env_path.write_text(
                "TELEGRAM_TEST_API_ID=98765\n"
                "TELEGRAM_TEST_API_HASH=abcdef0123456789abcdef0123456789\n"
                "TELEGRAM_TEST_ALLOWED_BOTS=@MyBot,OtherBot\n"
                "TELEGRAM_TEST_PHONE=+989120000000\n"
                "TELEGRAM_TEST_SESSION_PATH=/tmp/test.session\n"
                "TELEGRAM_TEST_TIMEOUT=15.5\n"
            )
            cfg = Config.load(env_file=env_path)
            self.assertEqual(cfg.api_id, 98765)
            self.assertEqual(cfg.api_hash, "abcdef0123456789abcdef0123456789")
            self.assertEqual(cfg.allowed_bots, ["mybot", "otherbot"])
            self.assertEqual(cfg.phone, "+989120000000")
            self.assertEqual(cfg.session_path, Path("/tmp/test.session"))
            self.assertEqual(cfg.timeout, 15.5)

    def test_config_ignores_legacy_fallbacks(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            env_path = Path(tmpdir) / ".env"
            env_path.write_text(
                "API_ID=555\n"
                "API_HASH=legacy_hash\n"
                "TELEGRAM_ALLOWED_BOTS=fallback_bot\n"
            )
            cfg = Config.load(env_file=env_path)
            self.assertIsNone(cfg.api_id)
            self.assertIsNone(cfg.api_hash)
            self.assertEqual(cfg.allowed_bots, [])

    def test_config_validates_positive_finite_timeout(self):
        for invalid in ["-5", "0", "nan", "inf", "-inf", "not_a_num"]:
            with self.assertRaises(ConfigError) as ctx:
                Config.load(timeout=invalid)
            self.assertIn("positive finite number", str(ctx.exception))

    def test_config_validates_positive_integer_api_id(self):
        for invalid in ["-10", "0", "abc", "12.34"]:
            with self.assertRaises(ConfigError) as ctx:
                Config.load(api_id=invalid)
            self.assertIn("positive integer", str(ctx.exception))
            # Verify input value is NOT echoed in the error
            self.assertNotIn(str(invalid), str(ctx.exception))

    def test_config_validates_32_hex_api_hash(self):
        # 31 chars, 33 chars, non-hex characters
        for invalid in ["a" * 31, "b" * 33, "z" * 32, "not-a-hex-hash!"]:
            with self.assertRaises(ConfigError) as ctx:
                Config.load(api_hash=invalid)
            self.assertIn("32 hexadecimal characters", str(ctx.exception))
            self.assertNotIn(invalid, str(ctx.exception))

    def test_config_repr_does_not_leak_secrets(self):
        cfg = Config(
            api_id=12345,
            api_hash="abcdef0123456789abcdef0123456789",
            phone="+989123456789",
        )
        r = repr(cfg)
        self.assertNotIn("abcdef0123456789abcdef0123456789", r)
        self.assertNotIn("+989123456789", r)

    def test_resolve_session_path_telethon_suffix_semantics(self):
        # Telethon appends .session if not ending with .session
        self.assertEqual(
            str(resolve_session_path("test.session")),
            "test.session",
        )
        self.assertEqual(
            str(resolve_session_path("test.custom")),
            "test.custom.session",
        )
        self.assertEqual(
            str(resolve_session_path(Path("/tmp/my.session"))),
            "/tmp/my.session",
        )


class SessionStorageAndLockTest(NetworkGuardedTestCase):
    """Test private session storage, permissions, and lock contention with fcntl.flock."""

    def test_ensure_private_session_storage_and_real_telethon_sqlite(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            sub = Path(tmpdir) / "private_sessions"
            sess_file = sub / "test.session"

            # 1. Ensure private session storage creates 0700 dir and 0600 file
            ensure_private_session_storage(sess_file)
            self.assertTrue(sub.exists())
            self.assertEqual(stat.S_IMODE(sub.stat().st_mode), 0o700)
            self.assertTrue(sess_file.exists())
            self.assertEqual(stat.S_IMODE(sess_file.stat().st_mode), 0o600)

            # 2. Verify real Telethon SQLiteSession opens it and retains 0600 permissions
            session = SQLiteSession(str(sess_file))
            session.save()
            session.close()
            self.assertEqual(stat.S_IMODE(sess_file.stat().st_mode), 0o600)

    def test_ensure_private_session_storage_rejects_unmodified_0755_dir(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            shared = Path(tmpdir) / "shared_dir"
            shared.mkdir(mode=0o755)
            session_file = shared / "test.session"

            with self.assertRaises(SecurityError) as ctx:
                ensure_private_session_storage(session_file)

            self.assertIn("not private", str(ctx.exception))
            # Existing 0755 directory must NOT be chmodded to 0700!
            self.assertEqual(stat.S_IMODE(shared.stat().st_mode), 0o755)
            # Session file must NOT be created!
            self.assertFalse(session_file.exists())

    def test_ensure_private_session_storage_accepts_existing_0700_dir(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            priv = Path(tmpdir) / "priv_dir"
            priv.mkdir(mode=0o700)
            session_file = priv / "test.session"

            ensure_private_session_storage(session_file)
            self.assertEqual(stat.S_IMODE(priv.stat().st_mode), 0o700)
            self.assertTrue(session_file.exists())
            self.assertEqual(stat.S_IMODE(session_file.stat().st_mode), 0o600)

    def test_session_lock_creation_and_permissions(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            sub = Path(tmpdir) / "sessions"
            sub.mkdir(mode=0o700)
            session_path = sub / "test.session"

            lock = SessionLock(session_path, timeout=2.0)
            with lock:
                self.assertTrue(lock.lock_path.exists())
                mode = stat.S_IMODE(lock.lock_path.stat().st_mode)
                self.assertEqual(mode, 0o600)

    def test_subprocess_lock_contention_timeout(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            sub = Path(tmpdir) / "sessions"
            sub.mkdir(mode=0o700)
            session_path = sub / "test.session"
            lock = SessionLock(session_path, timeout=0.2)

            # Spawn child process that holds exclusive flock on lock file
            p = subprocess.Popen([
                sys.executable, "-c",
                f"import fcntl, time, os; fd = os.open('{lock.lock_path}', os.O_CREAT|os.O_RDWR, 0o600); "
                f"fcntl.flock(fd, fcntl.LOCK_EX); time.sleep(1.0)"
            ])
            time.sleep(0.1)  # allow child process to acquire lock
            try:
                with self.assertRaises(SessionLockedError) as ctx:
                    lock.acquire()
                self.assertEqual(ctx.exception.exit_code, EXIT_SESSION_LOCKED)
            finally:
                p.terminate()
                p.wait()

    def test_session_lock_async_acquire_and_cancellation_releases_fd(self):
        async def _test():
            with tempfile.TemporaryDirectory() as tmpdir:
                sub = Path(tmpdir) / "sessions"
                sub.mkdir(mode=0o700)
                session_path = sub / "test.session"

                lock_a = SessionLock(session_path, timeout=5.0)
                await lock_a.async_acquire()
                self.assertIsNotNone(lock_a.fd)

                lock_b = SessionLock(session_path, timeout=10.0)

                async def acquire_b():
                    await lock_b.async_acquire()

                task = asyncio.create_task(acquire_b())
                # Allow task to enter polling loop
                await asyncio.sleep(0.06)
                self.assertFalse(task.done())

                # Cancel task B while waiting for lock
                task.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await task

                # Cancellation must have immediately closed fd and released lock
                self.assertIsNone(lock_b.fd)

                # Now release lock A
                lock_a.release()
                self.assertIsNone(lock_a.fd)

                # A new lock C must be able to acquire immediately without obstruction
                lock_c = SessionLock(session_path, timeout=1.0)
                await lock_c.async_acquire()
                self.assertIsNotNone(lock_c.fd)
                lock_c.release()

        asyncio.run(_test())

    def test_session_lock_async_acquire_timeout_releases_fd(self):
        async def _test():
            with tempfile.TemporaryDirectory() as tmpdir:
                sub = Path(tmpdir) / "sessions"
                sub.mkdir(mode=0o700)
                session_path = sub / "test.session"

                lock_a = SessionLock(session_path, timeout=5.0)
                await lock_a.async_acquire()

                lock_b = SessionLock(session_path, timeout=0.05)
                with self.assertRaises(SessionLockedError):
                    await lock_b.async_acquire()

                # Timeout must have closed fd
                self.assertIsNone(lock_b.fd)

                lock_a.release()

        asyncio.run(_test())

    def test_lock_rejects_symlink(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            sub = Path(tmpdir) / "sessions"
            sub.mkdir(mode=0o700)
            real_file = sub / "real.session.lock"
            real_file.touch()
            symlink_lock = sub / "link.session.lock"
            symlink_lock.symlink_to(real_file)

            lock = SessionLock(sub / "link.session", timeout=1.0)
            with self.assertRaises(SecurityError):
                lock.acquire()

    def test_lock_rejects_dangerous_system_directories(self):
        with self.assertRaises(SecurityError):
            validate_session_directory(Path("/tmp"))
        with self.assertRaises(SecurityError):
            validate_session_directory(Path("/"))

    def test_noninteractive_command_does_not_create_dirs_or_files_for_missing_session(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            ghost_dir = Path(tmpdir) / "nonexistent_sub"
            ghost_session = ghost_dir / "ghost.session"

            cfg = Config(
                api_id=12345,
                api_hash="abcdef0123456789abcdef0123456789",
                session_path=ghost_session,
                allowed_bots=["test_bot"],
            )
            client = TelegramAccountClient(cfg)
            loop = asyncio.new_event_loop()
            try:
                with self.assertRaises(NotAuthorizedError) as ctx:
                    loop.run_until_complete(client.get_status())
                self.assertEqual(ctx.exception.exit_code, EXIT_UNAUTHORIZED)
                # Verify ghost_dir and ghost_session were NOT created
                self.assertFalse(ghost_dir.exists())
                self.assertFalse(ghost_session.exists())
            finally:
                loop.close()


class BotValidationTest(NetworkGuardedTestCase):
    """Test target allowlist, non-bot rejection, numeric/phone rejection, and resolution."""

    def setUp(self):
        self.tmpdir = tempfile.TemporaryDirectory()
        sub = Path(self.tmpdir.name) / "sub"
        sub.mkdir(mode=0o700)
        self.session_file = sub / "test.session"
        self.session_file.touch(mode=0o600)
        self.config = Config(
            api_id=12345,
            api_hash="abcdef0123456789abcdef0123456789",
            session_path=self.session_file,
            allowed_bots=["allowed_bot", "second_bot"],
            timeout=5.0,
        )
        self.client_wrapper = TelegramAccountClient(self.config)

    def tearDown(self):
        self.tmpdir.cleanup()

    def test_empty_allowlist_fails_closed(self):
        self.config.allowed_bots = []
        with self.assertRaises(TargetViolationError) as ctx:
            self.client_wrapper.pre_validate_target("allowed_bot")
        self.assertEqual(ctx.exception.exit_code, EXIT_TARGET_VIOLATION)
        self.assertIn("Allowed bots list is empty", str(ctx.exception))

    def test_unlisted_bot_rejected(self):
        with self.assertRaises(TargetViolationError) as ctx:
            self.client_wrapper.pre_validate_target("evil_bot")
        self.assertEqual(ctx.exception.exit_code, EXIT_TARGET_VIOLATION)
        self.assertIn("not in allowed bots list", str(ctx.exception))

    def test_numeric_id_and_phone_rejected(self):
        for invalid in ["12345678", "-100123456789", "+989123456789"]:
            with self.assertRaises(TargetViolationError) as ctx:
                self.client_wrapper.pre_validate_target(invalid)
            self.assertEqual(ctx.exception.exit_code, EXIT_TARGET_VIOLATION)

    def test_human_user_rejected(self):
        mock_client = Mock()
        human_user = types.User(id=999, bot=False, username="allowed_bot")
        mock_client.get_entity = AsyncMock(return_value=human_user)

        loop = asyncio.new_event_loop()
        try:
            with self.assertRaises(TargetViolationError) as ctx:
                loop.run_until_complete(
                    self.client_wrapper.resolve_bot(mock_client, "allowed_bot")
                )
            self.assertEqual(ctx.exception.exit_code, EXIT_TARGET_VIOLATION)
            self.assertIn("human user, not a bot", str(ctx.exception))
        finally:
            loop.close()

    def test_username_mismatch_rejected(self):
        mock_client = Mock()
        spoofed_bot = types.User(id=888, bot=True, username="imposter_bot")
        mock_client.get_entity = AsyncMock(return_value=spoofed_bot)

        loop = asyncio.new_event_loop()
        try:
            with self.assertRaises(TargetViolationError) as ctx:
                loop.run_until_complete(
                    self.client_wrapper.resolve_bot(mock_client, "allowed_bot")
                )
            self.assertEqual(ctx.exception.exit_code, EXIT_TARGET_VIOLATION)
            self.assertIn("resolved to different username", str(ctx.exception))
        finally:
            loop.close()

    def test_valid_bot_resolved_successfully(self):
        mock_client = Mock()
        valid_bot = types.User(id=777, bot=True, username="allowed_bot")
        mock_client.get_entity = AsyncMock(return_value=valid_bot)

        loop = asyncio.new_event_loop()
        try:
            resolved = loop.run_until_complete(
                self.client_wrapper.resolve_bot(mock_client, "@allowed_bot")
            )
            self.assertEqual(resolved.id, 777)
            self.assertTrue(resolved.bot)
        finally:
            loop.close()


class LiteralTextAndOperationsTest(NetworkGuardedTestCase):
    """Test exact literal user text preservation, button clicking, and FloodWait mapping."""

    def setUp(self):
        self.tmpdir = tempfile.TemporaryDirectory()
        sub = Path(self.tmpdir.name) / "sub"
        sub.mkdir(mode=0o700)
        self.session_file = sub / "test.session"
        self.session_file.touch(mode=0o600)
        self.config = Config(
            api_id=12345,
            api_hash="abcdef0123456789abcdef0123456789",
            session_path=self.session_file,
            allowed_bots=["dev_bot"],
            timeout=5.0,
        )
        self.client_wrapper = TelegramAccountClient(self.config)

    def tearDown(self):
        self.tmpdir.cleanup()

    @patch("client.TelegramClient")
    def test_literal_text_preserves_persian_and_markdown_characters(self, mock_telethon_cls):
        mock_client = mock_telethon_cls.return_value
        mock_client.connect = AsyncMock()
        mock_client.is_user_authorized = AsyncMock(return_value=True)
        mock_client.is_connected = Mock(return_value=True)
        mock_client.disconnect = AsyncMock()

        bot_entity = types.User(id=2002, bot=True, username="dev_bot")
        mock_client.get_entity = AsyncMock(return_value=bot_entity)

        # Text with markdown characters, Persian characters, and backslashes
        literal_text = "سلام **ستاره** _زیرخط_ [لینک](http://example.com) `کد` \\معکوس"
        sent_msg = Mock(id=42, date=datetime.now(timezone.utc), text=literal_text)
        mock_client.send_message = AsyncMock(return_value=sent_msg)

        loop = asyncio.new_event_loop()
        try:
            res = loop.run_until_complete(self.client_wrapper.send_text("dev_bot", literal_text))
            self.assertTrue(res["success"])
            self.assertEqual(res["text"], literal_text)
            # Verify parse_mode=None was passed to client.send_message
            mock_client.send_message.assert_called_once_with(
                bot_entity, literal_text, parse_mode=None, link_preview=False
            )
        finally:
            loop.close()

    @patch("client.TelegramClient")
    def test_send_file_preserves_literal_caption(self, mock_telethon_cls):
        mock_client = mock_telethon_cls.return_value
        mock_client.connect = AsyncMock()
        mock_client.is_user_authorized = AsyncMock(return_value=True)
        mock_client.is_connected = Mock(return_value=True)
        mock_client.disconnect = AsyncMock()

        bot_entity = types.User(id=2002, bot=True, username="dev_bot")
        mock_client.get_entity = AsyncMock(return_value=bot_entity)

        dummy_file = Path(self.tmpdir.name) / "test.pdf"
        dummy_file.write_bytes(b"%PDF-1.4 dummy")

        caption = "فایل **مستند** [1] _نسخه_ 2.0"
        sent_msg = Mock(id=43, date=datetime.now(timezone.utc))
        mock_client.send_file = AsyncMock(return_value=sent_msg)

        loop = asyncio.new_event_loop()
        try:
            res = loop.run_until_complete(
                self.client_wrapper.send_file("dev_bot", dummy_file, caption=caption)
            )
            self.assertTrue(res["success"])
            mock_client.send_file.assert_called_once_with(
                bot_entity, str(dummy_file), caption=caption, parse_mode=None
            )
        finally:
            loop.close()

    @patch("client.TelegramClient")
    def test_button_clicking_and_unsupported_rejection(self, mock_telethon_cls):
        mock_client = mock_telethon_cls.return_value
        mock_client.connect = AsyncMock()
        mock_client.is_user_authorized = AsyncMock(return_value=True)
        mock_client.is_connected = Mock(return_value=True)
        mock_client.disconnect = AsyncMock()

        bot_entity = types.User(id=2002, bot=True, username="dev_bot")
        mock_client.get_entity = AsyncMock(return_value=bot_entity)

        cb_btn = Mock()
        cb_btn.text = "🇬🇧 English"
        cb_btn.data = b"lang:set:en"
        cb_btn.url = None
        cb_btn.inline_query = None
        cb_btn.click = AsyncMock(return_value=Mock(message="OK", alert=False))

        url_btn = Mock()
        url_btn.text = "URL"
        url_btn.data = None
        url_btn.url = "https://example.com"
        url_btn.inline_query = None

        mock_msg = Mock(spec=types.Message)
        mock_msg.id = 55
        mock_msg.buttons = [[cb_btn], [url_btn]]
        mock_client.get_messages = AsyncMock(return_value=mock_msg)

        loop = asyncio.new_event_loop()
        try:
            # Valid callback button
            res = loop.run_until_complete(
                self.client_wrapper.click_button("dev_bot", 55, button_data="lang:set:en")
            )
            self.assertTrue(res["success"])

            # URL button rejected
            with self.assertRaises(UnsupportedButtonError):
                loop.run_until_complete(
                    self.client_wrapper.click_button("dev_bot", 55, row=1, col=0)
                )
        finally:
            loop.close()

    @patch("client.TelegramClient")
    def test_flood_wait_mapping(self, mock_telethon_cls):
        mock_client = mock_telethon_cls.return_value
        mock_client.connect = AsyncMock()
        mock_client.is_user_authorized = AsyncMock(
            side_effect=errors.FloodWaitError(request=None, capture=180)
        )
        mock_client.is_connected = Mock(return_value=True)
        mock_client.disconnect = AsyncMock()

        loop = asyncio.new_event_loop()
        try:
            with self.assertRaises(FloodWaitError) as ctx:
                loop.run_until_complete(self.client_wrapper.get_status())
            self.assertEqual(ctx.exception.exit_code, EXIT_FLOOD_WAIT)
            self.assertEqual(ctx.exception.retry_after_seconds, 180)
            self.assertIn("180 seconds", str(ctx.exception))
        finally:
            loop.close()

    @patch("client.TelegramClient")
    def test_smoke_flow_orchestration(self, mock_telethon_cls):
        mock_client = mock_telethon_cls.return_value
        mock_client.connect = AsyncMock()
        mock_client.is_user_authorized = AsyncMock(return_value=True)
        mock_client.is_connected = Mock(return_value=True)
        mock_client.disconnect = AsyncMock()

        bot_entity = types.User(id=2002, bot=True, username="dev_bot")
        mock_client.get_entity = AsyncMock(return_value=bot_entity)

        guide_btn = Mock()
        guide_btn.text = "راهنما | Guide"
        guide_btn.data = b"start:guide"
        guide_btn.url = None
        guide_btn.inline_query = None

        start_response = Mock(spec=types.Message)
        start_response.id = 100
        start_response.text = "Welcome to Dev Bot"
        start_response.buttons = [[guide_btn]]

        back_btn = Mock()
        back_btn.text = "بازگشت | Back"
        back_btn.data = b"start:panel"
        back_btn.url = None
        back_btn.inline_query = None

        guide_response = Mock(spec=types.Message)
        guide_response.id = 100
        guide_response.text = "راهنمای استفاده"
        guide_response.buttons = [[back_btn]]

        panel_response = Mock(spec=types.Message)
        panel_response.id = 100
        panel_response.text = "Start Panel"
        panel_response.buttons = [[guide_btn]]

        async def on_guide():
            mock_client.get_messages = AsyncMock(return_value=guide_response)

        async def on_back():
            mock_client.get_messages = AsyncMock(return_value=panel_response)

        guide_btn.click = AsyncMock(side_effect=on_guide)
        back_btn.click = AsyncMock(side_effect=on_back)

        call_count = 0

        async def mock_iter(entity, limit=1, min_id=0):
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                yield Mock(id=50, text="Old message", buttons=None)
            else:
                yield start_response

        mock_client.iter_messages = mock_iter
        mock_client.send_message = AsyncMock(return_value=Mock(id=99))
        mock_client.get_messages = AsyncMock(return_value=start_response)

        loop = asyncio.new_event_loop()
        try:
            res = loop.run_until_complete(self.client_wrapper.smoke("dev_bot", timeout=4.0))
            self.assertTrue(res["success"])
            self.assertEqual(res["bot"], "dev_bot")
            self.assertTrue(any(s["step"] == "guide_button_detected" for s in res["steps"]))
            self.assertTrue(any(s["step"] == "panel_back_button_detected" for s in res["steps"]))
            self.assertTrue(any(s["step"] == "returned_to_start_panel" for s in res["steps"]))
        finally:
            loop.run_until_complete(loop.shutdown_asyncgens())
            loop.close()


class DownloadMediaAtomicStagingTest(NetworkGuardedTestCase):
    """Test download_media atomic staging, permissions, collision protection, and failure cleanup."""

    def setUp(self):
        self.tmpdir = tempfile.TemporaryDirectory()
        sub = Path(self.tmpdir.name) / "sub"
        sub.mkdir(mode=0o700)
        self.session_file = sub / "test.session"
        self.session_file.touch(mode=0o600)
        self.dest_dir = Path(self.tmpdir.name) / "downloads"
        self.dest_dir.mkdir(mode=0o700)

        self.config = Config(
            api_id=12345,
            api_hash="abcdef0123456789abcdef0123456789",
            session_path=self.session_file,
            allowed_bots=["dev_bot"],
            timeout=5.0,
        )
        self.client_wrapper = TelegramAccountClient(self.config)

    def tearDown(self):
        self.tmpdir.cleanup()

    @patch("client.TelegramClient")
    def test_download_media_success_and_modes(self, mock_telethon_cls):
        mock_client = mock_telethon_cls.return_value
        mock_client.connect = AsyncMock()
        mock_client.is_user_authorized = AsyncMock(return_value=True)
        mock_client.is_connected = Mock(return_value=True)
        mock_client.disconnect = AsyncMock()

        bot_entity = types.User(id=2002, bot=True, username="dev_bot")
        mock_client.get_entity = AsyncMock(return_value=bot_entity)

        mock_msg = Mock(spec=types.Message)
        mock_msg.id = 77
        mock_msg.media = Mock()
        file_mock = Mock()
        file_mock.name = "sample.mp3"
        file_mock.ext = ".mp3"
        mock_msg.file = file_mock
        mock_client.get_messages = AsyncMock(return_value=mock_msg)

        async def fake_download(msg, file):
            Path(file).write_bytes(b"sample audio content")
            return file

        mock_client.download_media = AsyncMock(side_effect=fake_download)

        loop = asyncio.new_event_loop()
        try:
            res = loop.run_until_complete(
                self.client_wrapper.download_media("dev_bot", 77, self.dest_dir)
            )
            self.assertTrue(res["success"])
            final_file = Path(res["file_path"])
            self.assertTrue(final_file.exists())
            self.assertEqual(final_file.name, "sample.mp3")
            self.assertEqual(stat.S_IMODE(final_file.stat().st_mode), 0o600)

            # Staging directories must be completely cleaned up
            staging_dirs = list(self.dest_dir.glob(".staging_*"))
            self.assertEqual(len(staging_dirs), 0)
        finally:
            loop.close()

    @patch("client.TelegramClient")
    def test_download_media_partial_cleanup_on_error(self, mock_telethon_cls):
        mock_client = mock_telethon_cls.return_value
        mock_client.connect = AsyncMock()
        mock_client.is_user_authorized = AsyncMock(return_value=True)
        mock_client.is_connected = Mock(return_value=True)
        mock_client.disconnect = AsyncMock()

        bot_entity = types.User(id=2002, bot=True, username="dev_bot")
        mock_client.get_entity = AsyncMock(return_value=bot_entity)

        mock_msg = Mock(spec=types.Message)
        mock_msg.id = 78
        mock_msg.media = Mock()
        file_mock = Mock()
        file_mock.name = "corrupted.mp3"
        file_mock.ext = ".mp3"
        mock_msg.file = file_mock
        mock_client.get_messages = AsyncMock(return_value=mock_msg)

        async def broken_download(msg, file):
            Path(file).write_bytes(b"partial bytes")
            raise RuntimeError("Network disconnected mid-stream")

        mock_client.download_media = AsyncMock(side_effect=broken_download)

        loop = asyncio.new_event_loop()
        try:
            with self.assertRaises(RuntimeError):
                loop.run_until_complete(
                    self.client_wrapper.download_media("dev_bot", 78, self.dest_dir)
                )
            # Assert no partial files or staging directories remain
            self.assertEqual(list(self.dest_dir.iterdir()), [])
        finally:
            loop.close()

    @patch("client.TelegramClient")
    def test_download_media_partial_cleanup_on_cancellation(self, mock_telethon_cls):
        mock_client = mock_telethon_cls.return_value
        mock_client.connect = AsyncMock()
        mock_client.is_user_authorized = AsyncMock(return_value=True)
        mock_client.is_connected = Mock(return_value=True)
        mock_client.disconnect = AsyncMock()

        bot_entity = types.User(id=2002, bot=True, username="dev_bot")
        mock_client.get_entity = AsyncMock(return_value=bot_entity)

        # Unrelated file in destination must NOT be touched
        unrelated = self.dest_dir / "unrelated_important.txt"
        unrelated.write_bytes(b"preserve this file")

        mock_msg = Mock(spec=types.Message)
        mock_msg.id = 80
        mock_msg.media = Mock()
        file_mock = Mock()
        file_mock.name = "cancelled.mp3"
        file_mock.ext = ".mp3"
        mock_msg.file = file_mock
        mock_client.get_messages = AsyncMock(return_value=mock_msg)

        async def cancel_download(msg, file):
            Path(file).write_bytes(b"partial cancellation bytes")
            raise asyncio.CancelledError()

        mock_client.download_media = AsyncMock(side_effect=cancel_download)

        loop = asyncio.new_event_loop()
        try:
            with self.assertRaises(asyncio.CancelledError):
                loop.run_until_complete(
                    self.client_wrapper.download_media("dev_bot", 80, self.dest_dir)
                )
            # Verify staging is cleaned up and unrelated file is preserved
            self.assertEqual(list(self.dest_dir.iterdir()), [unrelated])
            self.assertEqual(unrelated.read_bytes(), b"preserve this file")
        finally:
            loop.close()

    @patch("client.TelegramClient")
    def test_download_media_preexisting_symlink_collision(self, mock_telethon_cls):
        mock_client = mock_telethon_cls.return_value
        mock_client.connect = AsyncMock()
        mock_client.is_user_authorized = AsyncMock(return_value=True)
        mock_client.is_connected = Mock(return_value=True)
        mock_client.disconnect = AsyncMock()

        bot_entity = types.User(id=2002, bot=True, username="dev_bot")
        mock_client.get_entity = AsyncMock(return_value=bot_entity)

        # Create target as symlink in dest_dir
        target_name = "target.mp3"
        real_outside = Path(self.tmpdir.name) / "outside.txt"
        real_outside.write_text("outside")
        symlink_target = self.dest_dir / target_name
        symlink_target.symlink_to(real_outside)

        mock_msg = Mock(spec=types.Message)
        mock_msg.id = 81
        mock_msg.media = Mock()
        file_mock = Mock()
        file_mock.name = target_name
        file_mock.ext = ".mp3"
        mock_msg.file = file_mock
        mock_client.get_messages = AsyncMock(return_value=mock_msg)

        async def fake_download(msg, file):
            Path(file).write_bytes(b"downloaded")
            return file

        mock_client.download_media = AsyncMock(side_effect=fake_download)

        loop = asyncio.new_event_loop()
        try:
            with self.assertRaises(AccountToolError) as ctx:
                loop.run_until_complete(
                    self.client_wrapper.download_media("dev_bot", 81, self.dest_dir)
                )
            self.assertIn("refusing to overwrite", str(ctx.exception))
            # Outside file was not deleted or altered
            self.assertTrue(real_outside.exists())
            self.assertEqual(real_outside.read_text(), "outside")
        finally:
            loop.close()

    def test_build_client_has_auto_reconnect_disabled(self):
        client = self.client_wrapper.build_client(require_existing_session=True)
        try:
            self.assertFalse(client._auto_reconnect)
        finally:
            if hasattr(client, "session") and hasattr(client.session, "close"):
                client.session.close()


class TimeoutsAndCleanupTest(NetworkGuardedTestCase):
    """Test timeout deadlines on connect, auth, operation, and disconnect with guaranteed lock release."""

    def setUp(self):
        self.tmpdir = tempfile.TemporaryDirectory()
        sub = Path(self.tmpdir.name) / "sub"
        sub.mkdir(mode=0o700)
        self.session_file = sub / "test.session"
        self.session_file.touch(mode=0o600)

        self.config = Config(
            api_id=12345,
            api_hash="abcdef0123456789abcdef0123456789",
            session_path=self.session_file,
            allowed_bots=["dev_bot"],
            timeout=0.3,  # Short timeout for fast tests
        )
        self.client_wrapper = TelegramAccountClient(self.config)

    def tearDown(self):
        self.tmpdir.cleanup()

    @patch("client.TelegramClient")
    def test_hanging_connect_times_out_and_releases_lock(self, mock_telethon_cls):
        mock_client = mock_telethon_cls.return_value

        async def hang_forever():
            await asyncio.sleep(10.0)

        mock_client.connect = AsyncMock(side_effect=hang_forever)
        mock_client.is_connected = Mock(return_value=False)
        mock_client.disconnect = AsyncMock()

        loop = asyncio.new_event_loop()
        try:
            with self.assertRaises(NetworkTimeoutError) as ctx:
                loop.run_until_complete(self.client_wrapper.get_status())
            self.assertEqual(ctx.exception.exit_code, EXIT_NETWORK_TIMEOUT)

            # Assert lock file was released and is acquirable
            lock = SessionLock(self.session_file, timeout=0.1)
            with lock:
                self.assertTrue(True)
        finally:
            loop.close()

    @patch("client.TelegramClient")
    def test_hanging_auth_times_out_and_releases_lock(self, mock_telethon_cls):
        mock_client = mock_telethon_cls.return_value
        mock_client.connect = AsyncMock()

        async def hang_forever():
            await asyncio.sleep(10.0)

        mock_client.is_user_authorized = AsyncMock(side_effect=hang_forever)
        mock_client.is_connected = Mock(return_value=True)
        mock_client.disconnect = AsyncMock()

        loop = asyncio.new_event_loop()
        try:
            with self.assertRaises(NetworkTimeoutError) as ctx:
                loop.run_until_complete(self.client_wrapper.get_status())
            self.assertEqual(ctx.exception.exit_code, EXIT_NETWORK_TIMEOUT)

            lock = SessionLock(self.session_file, timeout=0.1)
            with lock:
                self.assertTrue(True)
        finally:
            loop.close()

    @patch("client.TelegramClient")
    def test_hanging_disconnect_bounded(self, mock_telethon_cls):
        mock_client = mock_telethon_cls.return_value
        mock_client.connect = AsyncMock()
        mock_client.is_user_authorized = AsyncMock(return_value=True)
        mock_client.is_connected = Mock(return_value=True)

        async def hang_disconnect():
            await asyncio.sleep(10.0)

        mock_client.disconnect = AsyncMock(side_effect=hang_disconnect)
        mock_me = types.User(id=111, bot=False, username="user", phone="98912341234")
        mock_client.get_me = AsyncMock(return_value=mock_me)

        loop = asyncio.new_event_loop()
        try:
            res = loop.run_until_complete(self.client_wrapper.get_status())
            self.assertTrue(res["success"])
            # Assert lock is still released despite hanging disconnect
            lock = SessionLock(self.session_file, timeout=0.1)
            with lock:
                self.assertTrue(True)
        finally:
            loop.close()


class CLIE2EAndSanitizationTest(NetworkGuardedTestCase):
    """Test CLI argument parsing, JSON error outputs, exit codes, and secret sanitization."""

    def setUp(self):
        self.tmpdir = tempfile.TemporaryDirectory()
        sub = Path(self.tmpdir.name) / "sub"
        sub.mkdir(mode=0o700)
        self.session_file = sub / "test.session"
        self.session_file.touch(mode=0o600)
        self.env_file = Path(self.tmpdir.name) / ".env"
        self.env_file.write_text(
            "TELEGRAM_TEST_API_ID=11111\n"
            "TELEGRAM_TEST_API_HASH=abcdef0123456789abcdef0123456789\n"
            "TELEGRAM_TEST_ALLOWED_BOTS=test_bot\n"
        )

    def tearDown(self):
        self.tmpdir.cleanup()

    @patch("client.TelegramClient")
    def test_cli_status_json_success(self, mock_telethon_cls):
        mock_client = mock_telethon_cls.return_value
        mock_client.connect = AsyncMock()
        mock_client.is_user_authorized = AsyncMock(return_value=True)
        mock_client.is_connected = Mock(return_value=True)
        mock_client.disconnect = AsyncMock()
        mock_me = types.User(id=333, bot=False, username="ci_user", phone="98912341234")
        mock_client.get_me = AsyncMock(return_value=mock_me)

        out = io.StringIO()
        with patch("sys.stdout", out):
            code = cli.main([
                "--env-file", str(self.env_file),
                "--session", str(self.session_file),
                "--json",
                "status",
            ])

        self.assertEqual(code, EXIT_SUCCESS)
        data = json.loads(out.getvalue())
        self.assertTrue(data["success"])
        self.assertEqual(data["user_id"], 333)
        self.assertEqual(data["phone_masked"], "98*****1234")

    def test_cli_missing_session_unauthorized_exit_code(self):
        missing_session = Path(self.tmpdir.name) / "missing.session"
        out = io.StringIO()
        with patch("sys.stdout", out):
            code = cli.main([
                "--env-file", str(self.env_file),
                "--session", str(missing_session),
                "--json",
                "status",
            ])

        self.assertEqual(code, EXIT_UNAUTHORIZED)
        data = json.loads(out.getvalue())
        self.assertFalse(data["success"])
        self.assertEqual(data["error"], "unauthorized")

    def test_cli_disallowed_bot_exit_code_7(self):
        out = io.StringIO()
        with patch("sys.stdout", out):
            code = cli.main([
                "--env-file", str(self.env_file),
                "--session", str(self.session_file),
                "--json",
                "send-text",
                "--bot", "unauthorized_bot",
                "--text", "hello",
            ])

        self.assertEqual(code, EXIT_TARGET_VIOLATION)
        data = json.loads(out.getvalue())
        self.assertFalse(data["success"])
        self.assertEqual(data["error"], "target_violation")

    @patch("client.TelegramClient")
    def test_cli_unexpected_exception_sanitizes_secrets(self, mock_telethon_cls):
        mock_client = mock_telethon_cls.return_value
        # Inject exception containing simulated secret
        dummy_secret = "SECRET_SUPER_CONFIDENTIAL_KEY_9999"
        mock_client.connect = AsyncMock(side_effect=RuntimeError(f"Database crash with {dummy_secret}"))
        mock_client.is_connected = Mock(return_value=False)
        mock_client.disconnect = AsyncMock()

        stdout_buf = io.StringIO()
        stderr_buf = io.StringIO()
        with patch("sys.stdout", stdout_buf), patch("sys.stderr", stderr_buf):
            code = cli.main([
                "--env-file", str(self.env_file),
                "--session", str(self.session_file),
                "--json",
                "status",
            ])

        self.assertEqual(code, EXIT_GENERIC_ERROR)
        out = stdout_buf.getvalue()
        err = stderr_buf.getvalue()

        # Assert secret is absent from both stdout and stderr!
        self.assertNotIn(dummy_secret, out)
        self.assertNotIn(dummy_secret, err)

        # Check structure
        data = json.loads(out)
        self.assertFalse(data["success"])
        self.assertEqual(data["error"], "internal_error")
        self.assertEqual(data["exception_class"], "RuntimeError")
        self.assertEqual(data["message"], "An unexpected internal error occurred.")

    def test_cli_rejects_allowed_bots_flag(self):
        stderr_buf = io.StringIO()
        with patch("sys.stderr", stderr_buf):
            with self.assertRaises(SystemExit) as ctx:
                cli.build_parser().parse_args([
                    "status",
                    "--allowed-bots", "bot1,bot2",
                ])
            self.assertNotEqual(ctx.exception.code, 0)
        self.assertIn("unrecognized arguments: --allowed-bots", stderr_buf.getvalue())


if __name__ == "__main__":
    unittest.main()
