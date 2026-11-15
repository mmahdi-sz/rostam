import stat
import io
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import Apis


PAGE = '''<label class="label">App api_id:</label><div><span>12345</span></div>
<label>App api_hash:</label><div><span>0123456789abcdef0123456789abcdef</span></div>'''


class CredentialsTest(unittest.TestCase):
    def test_actual_creation_form_is_not_mistaken_for_existing_credentials(self):
        page = '<form id="app_create_form"><input name="hash" value="private-csrf-token"></form><p>Use your api_id and api_hash to develop applications.</p>'
        with self.assertRaisesRegex(RuntimeError, "no Telegram API application"):
            Apis.extract_credentials(page)

    @patch.object(Apis.requests, "Session")
    def test_page_reload_keeps_login_without_requesting_another_code(self, factory):
        session = factory.return_value.__enter__.return_value
        challenge = Mock(status_code=200)
        challenge.json.return_value = {"random_hash": "challenge"}
        session.post.side_effect = [challenge, Mock(status_code=200, text="true")]
        session.get.side_effect = [
            Mock(status_code=200),
            Mock(status_code=200, text="Create application", url="https://my.telegram.org/apps/new"),
            Mock(status_code=200, text=PAGE, url="https://my.telegram.org/apps"),
        ]
        read_retry = Mock(return_value="")
        with patch("sys.stdout", new_callable=io.StringIO) as output:
            self.assertEqual(Apis.retrieve("+989123456789", lambda _: "otp", read_retry), Apis.extract_credentials(PAGE))
        self.assertEqual(session.post.call_count, 2)
        self.assertEqual(session.get.call_count, 3)
        read_retry.assert_called_once()
        self.assertNotIn("0123456789abcdef0123456789abcdef", output.getvalue())

    @patch.object(Apis.requests, "Session")
    def test_lost_website_session_does_not_request_another_code(self, factory):
        session = factory.return_value.__enter__.return_value
        challenge = Mock(status_code=200)
        challenge.json.return_value = {"random_hash": "challenge"}
        session.post.side_effect = [challenge, Mock(status_code=200, text="true")]
        session.get.side_effect = [Mock(status_code=200), Mock(status_code=200, text="Login required", url="https://my.telegram.org/auth")]
        read_retry = Mock()
        with patch("sys.stdout", new_callable=io.StringIO):
            with self.assertRaisesRegex(RuntimeError, "session was not retained"):
                Apis.retrieve("+989123456789", lambda _: "otp", read_retry)
        read_retry.assert_not_called()
        self.assertEqual(session.post.call_count, 2)

    @patch.object(Apis.requests, "Session")
    def test_missing_application_is_created_in_same_login_session(self, factory):
        session = factory.return_value.__enter__.return_value
        challenge = Mock(status_code=200)
        challenge.json.return_value = {"random_hash": "challenge"}
        session.post.side_effect = [challenge, Mock(status_code=200, text="true"), Mock(status_code=200, text="")]
        session.get.side_effect = [Mock(status_code=200), Mock(status_code=200, text='<form id="app_create_form"><input name="hash" value="csrf-secret"></form>'), Mock(status_code=200, text=PAGE)]
        with patch("sys.stdout", new_callable=io.StringIO) as output:
            self.assertEqual(Apis.retrieve("+989123456789", lambda _: "otp"), Apis.extract_credentials(PAGE))
        creation = session.post.call_args_list[-1]
        self.assertEqual(creation.args[0], "https://my.telegram.org/apps/create")
        self.assertEqual(creation.kwargs["data"]["hash"], "csrf-secret")
        self.assertEqual(creation.kwargs["data"]["app_platform"], "other")
        self.assertEqual(session.post.call_count, 3)
        self.assertNotIn("csrf-secret", output.getvalue())

    def test_creation_failure_is_not_retried_or_exposed(self):
        session = Mock()
        session.post.return_value = Mock(status_code=200, text="ERROR private-token")
        page = '<form id="app_create_form"><input name="hash" value="private-token"></form>'
        with self.assertRaisesRegex(RuntimeError, "rejected application creation") as error:
            Apis.create_application(session, page)
        self.assertNotIn("private-token", str(error.exception))
        self.assertEqual(session.post.call_count, 1)
        session.reset_mock()
        with self.assertRaisesRegex(RuntimeError, "security token"):
            Apis.create_application(session, '<form id="app_create_form"></form>')
        session.post.assert_not_called()

    def test_current_page_hidden_hash_and_copyable_id(self):
        page = '''<div>API ID</div><button data-copy="12345">Copy</button>
        <div>API Hash</div><span class="spoiler" data-value="0123456789abcdef0123456789abcdef">••••</span>'''
        self.assertEqual(Apis.extract_credentials(page), ("12345", "0123456789abcdef0123456789abcdef"))

    def test_ambiguous_or_unlabelled_hidden_secrets_are_rejected(self):
        for page in (
            '<span data-value="' + 'a' * 32 + '">hidden</span>',
            '<div>API ID 12345</div><div>API Hash</div><span data-value="' + 'a' * 32 + '"></span><span data-value="' + 'b' * 32 + '"></span>',
        ):
            with self.assertRaises(RuntimeError):
                Apis.extract_credentials(page)

    def test_extract_and_reject_missing_or_invalid_credentials(self):
        self.assertEqual(Apis.extract_credentials(PAGE), ("12345", "0123456789abcdef0123456789abcdef"))
        for page in ("Login required", PAGE.replace("12345", "invalid")):
            with self.assertRaises(RuntimeError):
                Apis.extract_credentials(page)

    def test_save_preserves_settings_replaces_duplicates_and_protects_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / ".env"
            path.write_text("# Keep this\nBOT_TOKEN=existing\nTELEGRAM_TEST_API_ID=old\nexport TELEGRAM_TEST_API_ID=duplicate\nOTHER=value")
            values = {"TELEGRAM_TEST_API_ID": "12345", "TELEGRAM_TEST_API_HASH": "a" * 32}
            Apis.save_env(path, values)
            first = path.read_text()
            self.assertIn("# Keep this\nBOT_TOKEN=existing\n", first)
            self.assertIn("OTHER=value\n", first)
            self.assertEqual(first.count("TELEGRAM_TEST_API_ID="), 1)
            self.assertNotIn("old", first)
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            Apis.save_env(path, values)
            self.assertEqual(path.read_text(), first)

    def test_failed_atomic_replace_preserves_original(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / ".env"
            path.write_text("BOT_TOKEN=original\n")
            with patch.object(Apis.os, "replace", side_effect=OSError("write failed")):
                with self.assertRaises(OSError):
                    Apis.save_env(path, {"TELEGRAM_TEST_API_ID": "12345"})
            self.assertEqual(path.read_text(), "BOT_TOKEN=original\n")
            self.assertEqual(list(Path(directory).iterdir()), [path])

    @patch.object(Apis.requests, "Session")
    def test_login_success_and_expired_code(self, session_factory):
        session = session_factory.return_value.__enter__.return_value
        challenge = Mock(status_code=200)
        challenge.json.return_value = {"random_hash": "challenge"}
        session.post.side_effect = [challenge, Mock(status_code=200, text="true")]
        session.get.return_value = Mock(status_code=200, text=PAGE)
        self.assertEqual(Apis.retrieve("+989123456789", lambda _: "otp"), Apis.extract_credentials(PAGE))
        session.get.assert_any_call("https://my.telegram.org/auth", timeout=30)
        session.headers.update.assert_called_once()
        session.post.side_effect = [challenge, Mock(status_code=200, text="Invalid code")]
        session.get.reset_mock()
        with self.assertRaises(RuntimeError):
            Apis.retrieve("+989123456789", lambda _: "expired")
        self.assertEqual(session.get.call_count, 1)

    @patch.object(Apis.requests, "Session")
    def test_rejected_code_request_reports_reason_without_retry_or_code_prompt(self, session_factory):
        session = session_factory.return_value.__enter__.return_value
        session.get.return_value = Mock(status_code=200)
        response = Mock(status_code=400)
        response.json.return_value = {"text": "Too many tries for +989123456789. Please try again later."}
        session.post.return_value = response
        read_code = Mock()
        with self.assertRaisesRegex(RuntimeError, "Too many tries") as result:
            Apis.retrieve("+989123456789", read_code)
        self.assertNotIn("+989123456789", str(result.exception))
        read_code.assert_not_called()
        self.assertEqual(session.post.call_count, 1)

    def test_plain_text_and_html_errors_are_handled(self):
        for text, expected in [("PHONE_NUMBER_INVALID", "PHONE_NUMBER_INVALID"), ("<html>private</html>", "Request rejected")]:
            response = Mock(status_code=400, text=text)
            response.json.side_effect = ValueError()
            with self.assertRaisesRegex(RuntimeError, expected):
                Apis.check_response(response, "code request")


if __name__ == "__main__":
    unittest.main()
