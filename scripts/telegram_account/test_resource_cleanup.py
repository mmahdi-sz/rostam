import asyncio
from pathlib import Path
import stat
import tempfile
from unittest.mock import AsyncMock, Mock, patch

import client
from test_cli import NetworkGuardedTestCase


class ResourceCleanupTest(NetworkGuardedTestCase):
    def test_session_directory_is_rejected_without_permission_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "not-a-file.session"
            path.mkdir(mode=0o700)
            config = client.Config(api_id=12345, api_hash="a" * 32, session_path=path)
            tool = client.TelegramAccountClient(config)
            for operation in (
                lambda: tool.build_client(require_existing_session=True),
                lambda: client.ensure_private_session_storage(path),
            ):
                with self.assertRaises(client.SecurityError):
                    operation()
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o700)

    def test_cancellation_during_disconnect_closes_session_and_releases_lock(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "test.session"
            path.touch(mode=0o600)
            config = client.Config(api_id=12345, api_hash="a" * 32, session_path=path, timeout=0.03)
            tool = client.TelegramAccountClient(config)
            fake = Mock()
            fake.connect = AsyncMock()
            fake.is_user_authorized = AsyncMock(return_value=True)
            fake.is_connected.return_value = True

            async def hanging_disconnect():
                await asyncio.sleep(10)

            fake.disconnect = AsyncMock(side_effect=hanging_disconnect)
            action = AsyncMock(return_value={"success": True})
            with patch.object(tool, "build_client", return_value=fake):
                with self.assertRaises(client.NetworkTimeoutError):
                    asyncio.run(tool._execute_with_client(action))
            fake.session.close.assert_called_once()
            with client.SessionLock(path, timeout=0.03):
                pass
