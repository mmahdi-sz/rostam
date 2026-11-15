#!/usr/bin/env python3
"""Retrieve Telegram application credentials and save them to the project's .env.

Workflow reference: https://github.com/esfelurm/Apis-Telegram/blob/main/Apis.py
"""

import argparse
from html.parser import HTMLParser
import os
from pathlib import Path
import re
import tempfile
import secrets
from urllib.parse import urlparse

import requests


DEFAULT_ENV = Path(__file__).resolve().parents[2] / ".env"


def check_response(response, stage, sensitive=()):
    if response.status_code < 400:
        return
    try:
        body = response.json()
        detail = body.get("text", "") if isinstance(body, dict) else ""
    except ValueError:
        detail = response.text if "<" not in response.text else ""
    detail = str(detail)
    for value in sensitive:
        if value:
            detail = detail.replace(value, "[redacted]")
    detail = re.sub(r"[0-9a-fA-F]{32,}", "[redacted]", detail)
    detail = re.sub(r"\+?[0-9]{7,15}", "[redacted]", detail)
    detail = " ".join(detail.split())[:250]
    hint = " Do not repeatedly request codes; check https://my.telegram.org/auth in your browser."
    raise RuntimeError(f"Telegram {stage}: HTTP {response.status_code}. {detail or 'Request rejected.'}{hint}")


class CredentialPage(HTMLParser):
    def __init__(self, page):
        super().__init__()
        self.text = []
        self.values = []
        self.ignored = 0
        self.new_application = False
        self.creation_hash = None
        self.in_creation_form = False
        self.feed(page)

    def handle_starttag(self, tag, attrs):
        if tag == "form" and dict(attrs).get("id") == "app_create_form":
            self.new_application = True
            self.in_creation_form = True
        if tag == "input" and self.in_creation_form:
            fields = dict(attrs)
            if fields.get("name") == "hash":
                self.creation_hash = fields.get("value")
        if tag in ("script", "style"):
            self.ignored += 1
        if not self.ignored:
            for key, value in attrs:
                if key in ("data-value", "data-copy") and value:
                    self.values.append(value.strip())

    def handle_endtag(self, tag):
        if tag == "form":
            self.in_creation_form = False
        if tag in ("script", "style") and self.ignored:
            self.ignored -= 1

    def handle_data(self, data):
        if not self.ignored:
            self.text.append(data)


def extract_credentials(page):
    parsed = CredentialPage(page)
    if parsed.new_application:
        raise RuntimeError("This account has no Telegram API application. Create one at https://my.telegram.org/apps.")
    plain = "\n".join(parsed.text)
    values = []
    for field in ("api_id", "api_hash"):
        label = field.replace("_", r"[_\s]+")
        pattern = r"[1-9][0-9]{0,9}" if field == "api_id" else r"[0-9a-fA-F]{32}"
        match = re.search(
            rf"\b(?:App\s+)?{label}\s*:?\s*({pattern})\b",
            plain,
            re.IGNORECASE,
        )
        if match:
            values.append(match.group(1))
            continue
        # The current website keeps hidden/copyable credentials in attributes.
        candidates = {value for value in parsed.values if re.fullmatch(pattern, value)}
        label_present = re.search(rf"\b{label}\b", plain, re.IGNORECASE)
        if label_present and len(candidates) == 1:
            values.append(candidates.pop())
        else:
            raise RuntimeError(
                "Application credentials could not be read. The application may be missing "
                "or the page format may be unsupported; .env was not changed."
            )
    api_id, api_hash = values
    if not 0 < int(api_id) <= 2**31 - 1 or not re.fullmatch(
        r"[0-9a-fA-F]{32}", api_hash
    ):
        raise RuntimeError("Telegram returned invalid application credentials; .env was not changed.")
    return api_id, api_hash


def create_application(session, page):
    parsed = CredentialPage(page)
    if not parsed.new_application or not parsed.creation_hash:
        raise RuntimeError("Application creation form is missing its security token; .env was not changed.")
    response = session.post(
        "https://my.telegram.org/apps/create",
        data={
            "hash": parsed.creation_hash,
            "app_title": "Rostam Test Automation",
            "app_shortname": "rostamtest" + secrets.token_hex(4),
            "app_url": "",
            "app_platform": "other",
            "app_desc": "Dedicated Telegram bot integration testing account.",
        },
        headers={"Referer": "https://my.telegram.org/apps"},
        timeout=30,
    )
    check_response(response, "application creation", (parsed.creation_hash,))
    if response.text.strip() not in ("", "true"):
        raise RuntimeError("Telegram rejected application creation; .env was not changed. No duplicate request was sent.")


def save_env(path, values):
    """Replace only our keys, keeping the rest of the file intact."""
    path = path.resolve()
    original = path.read_text() if path.exists() else ""
    lines = original.splitlines(keepends=True)
    written = set()
    result = []
    for line in lines:
        match = re.match(r"\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=", line)
        key = match.group(1) if match else None
        if key in values:
            if key not in written:
                result.append(f"{key}={values[key]}\n")
                written.add(key)
        else:
            result.append(line)
    if result and not result[-1].endswith("\n"):
        result.append("\n")
    result.extend(f"{key}={value}\n" for key, value in values.items() if key not in written)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, delete=False) as handle:
            temporary = Path(handle.name)
            os.fchmod(handle.fileno(), 0o600)
            handle.write("".join(result))
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def retrieve(phone, read_code=input, read_retry=input):
    with requests.Session() as session:
        session.headers.update({
            "User-Agent": "Mozilla/5.0 TelegramAccountSetup/1.0",
            "Origin": "https://my.telegram.org",
            "Referer": "https://my.telegram.org/auth",
            "X-Requested-With": "fetch",
        })
        # Initialize the same website cookies used by the official login page.
        response = session.get("https://my.telegram.org/auth", timeout=30)
        check_response(response, "login page")
        response = session.post(
            "https://my.telegram.org/auth/send_password", data={"phone": phone}, timeout=30
        )
        check_response(response, "code request", (phone,))
        try:
            challenge = response.json()
        except ValueError:
            raise RuntimeError("Telegram did not send a login code. Try again later.") from None
        if not isinstance(challenge, dict) or not challenge.get("random_hash"):
            raise RuntimeError("Telegram rejected the login request. Try again later.")
        code = read_code("One-time code received in Telegram: ").strip()
        if not code:
            raise RuntimeError("No login code entered; .env was not changed.")
        response = session.post(
            "https://my.telegram.org/auth/login",
            data={"phone": phone, "random_hash": challenge["random_hash"], "password": code},
            timeout=30,
        )
        check_response(response, "login", (phone, code, challenge["random_hash"]))
        if response.text.strip() != "true":
            raise RuntimeError("Login failed or the code expired; .env was not changed.")
        creation_attempted = False
        for attempt in range(3):
            response = session.get("https://my.telegram.org/apps", timeout=30)
            check_response(response, "application page", (phone,))
            if CredentialPage(response.text).new_application:
                if creation_attempted:
                    raise RuntimeError("Application was not created; .env was not changed. No duplicate request was sent.")
                creation_attempted = True
                print("Creating the Telegram API application in this authenticated session...")
                create_application(session, response.text)
                response = session.get("https://my.telegram.org/apps", timeout=30)
                check_response(response, "application page", (phone,))
                if CredentialPage(response.text).new_application:
                    raise RuntimeError("Telegram still returned the creation form; .env was not changed. No duplicate request was sent.")
            try:
                return extract_credentials(response.text)
            except RuntimeError as error:
                path = urlparse(response.url).path if isinstance(response.url, str) else "unknown"
                print(f"Could not read application credentials (page: {path}).")
                print(str(error))
                if path.startswith("/auth"):
                    raise RuntimeError("Website session was not retained; .env was not changed.") from None
                if attempt == 2 or read_retry(
                    "Session remains open in memory. Create/check the application at "
                    "https://my.telegram.org/apps; press Enter to reload, or q to quit: "
                ).strip().lower() == "q":
                    raise RuntimeError("Credentials were not saved; website session closed.") from None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env-file", type=Path, default=DEFAULT_ENV)
    args = parser.parse_args()
    try:
        phone = input("Telegram phone number with country code (e.g. +98...): ").strip()
        if not re.fullmatch(r"\+[1-9][0-9]{6,14}", phone):
            raise RuntimeError("Enter a valid international phone number starting with +.")
        api_id, api_hash = retrieve(phone)
        save_env(args.env_file, {
            "TELEGRAM_TEST_PHONE": phone,
            "TELEGRAM_TEST_API_ID": api_id,
            "TELEGRAM_TEST_API_HASH": api_hash,
        })
    except (requests.RequestException, RuntimeError, OSError) as error:
        parser.exit(1, f"Error: {error}\n")
    except (KeyboardInterrupt, EOFError):
        parser.exit(1, "\nCancelled; credentials were not saved.\n")
    print(f"Credentials saved to {args.env_file.resolve()} (permissions 0600).")
    print("The one-time code and website login session were not saved.")


if __name__ == "__main__":
    main()
