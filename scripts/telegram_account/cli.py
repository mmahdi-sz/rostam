#!/usr/bin/env python3
"""CLI interface for Telegram Dedicated Test Account Tooling.

Provides subcommands for session management, bot interactions, and automated smoke testing.
"""

import argparse
import asyncio
import json
import os
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional

# Ensure scripts/telegram_account directory is importable
SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from client import (
    AccountToolError,
    Config,
    ConfigError,
    FloodWaitError,
    TelegramAccountClient,
    EXIT_GENERIC_ERROR,
    EXIT_SUCCESS,
)


def build_parser() -> argparse.ArgumentParser:
    """Build CLI argument parser with global flags and subcommands."""
    common_p = argparse.ArgumentParser(add_help=False)
    common_p.add_argument(
        "--env-file",
        type=str,
        default=argparse.SUPPRESS,
        help="Path to .env configuration file (defaults to dev/.env if present)",
    )
    common_p.add_argument(
        "--session",
        "--session-path",
        dest="session_path",
        type=str,
        default=argparse.SUPPRESS,
        help="Path to Telethon SQLite session file (defaults to .telegram_account/session.session)",
    )
    common_p.add_argument(
        "--timeout",
        type=float,
        default=argparse.SUPPRESS,
        help="Timeout in seconds for operations (default: 30s)",
    )
    common_p.add_argument(
        "--json",
        action="store_true",
        default=argparse.SUPPRESS,
        help="Format all output as machine-readable JSON to stdout",
    )

    parser = argparse.ArgumentParser(
        prog="telegram-account",
        description="Telegram Dedicated Test Account CLI and Automated Smoke Tooling",
        parents=[common_p],
    )

    subparsers = parser.add_subparsers(dest="subcommand", required=True, help="Subcommand to execute")

    # login
    login_p = subparsers.add_parser("login", parents=[common_p], help="Interactive one-time login authentication")
    login_p.add_argument("--phone", type=str, default=None, help="International phone number (+98...)")

    # status
    subparsers.add_parser("status", parents=[common_p], help="Verify session validity and authorization status")

    # send-text
    send_text_p = subparsers.add_parser("send-text", parents=[common_p], help="Send text message to an allowlisted bot")
    send_text_p.add_argument("--bot", type=str, required=True, help="Allowlisted target bot username")
    send_text_p.add_argument("--text", type=str, required=True, help="Message text content")

    # send-file
    send_file_p = subparsers.add_parser("send-file", parents=[common_p], help="Send document or photo to an allowlisted bot")
    send_file_p.add_argument("--bot", type=str, required=True, help="Allowlisted target bot username")
    send_file_p.add_argument("--file", type=str, required=True, help="Local file path")
    send_file_p.add_argument("--caption", type=str, default=None, help="Optional message caption")

    # read-messages
    read_p = subparsers.add_parser(
        "read-messages", parents=[common_p], help="Read messages and inline button tree from an allowlisted bot"
    )
    read_p.add_argument("--bot", type=str, required=True, help="Allowlisted target bot username")
    read_p.add_argument("--limit", type=int, default=5, help="Maximum number of messages to fetch (default: 5)")
    read_p.add_argument("--min-id", type=int, default=0, help="Fetch messages with ID strictly greater than min-id")
    read_p.add_argument("--wait-seconds", type=float, default=None, help="Optional polling wait for new message (seconds)")

    # click-button
    click_p = subparsers.add_parser(
        "click-button", parents=[common_p], help="Click an inline callback button on an existing message"
    )
    click_p.add_argument("--bot", type=str, required=True, help="Allowlisted target bot username")
    click_p.add_argument("--message-id", type=int, required=True, help="Target message ID")
    selector = click_p.add_mutually_exclusive_group(required=True)
    selector.add_argument("--button-text", type=str, help="Visible button label text")
    selector.add_argument("--button-data", type=str, help="Raw callback data string (e.g. 'start:guide')")
    selector.add_argument(
        "--coord", nargs=2, type=int, metavar=("ROW", "COL"), help="Row and column indices (0-indexed)"
    )
    # Support separate --row and --col as non-exclusive optional args
    click_p.add_argument("--row", type=int, default=None, help="Button row index (0-indexed)")
    click_p.add_argument("--col", type=int, default=None, help="Button column index (0-indexed)")

    # download-media
    dl_p = subparsers.add_parser(
        "download-media", parents=[common_p], help="Download media from a bot message to a controlled local folder"
    )
    dl_p.add_argument("--bot", type=str, required=True, help="Allowlisted target bot username")
    dl_p.add_argument("--message-id", type=int, required=True, help="Message ID containing media")
    dl_p.add_argument("--dest-dir", type=str, required=True, help="Target destination directory (0600 file permissions)")

    # smoke
    smoke_p = subparsers.add_parser(
        "smoke", parents=[common_p], help="Execute automated start/guide smoke flow against an allowlisted bot"
    )
    smoke_p.add_argument("--bot", type=str, required=True, help="Allowlisted target bot username")

    return parser


def format_output(result: Dict[str, Any], is_json: bool):
    """Format and print output to stdout."""
    if is_json:
        print(json.dumps(result, indent=2, ensure_ascii=False))
        return

    # Human-readable formatting
    if "authorized" in result:
        status_label = "AUTHORIZED" if result.get("authorized") else "UNAUTHORIZED"
        if result.get("already_authorized"):
            status_label = "ALREADY AUTHORIZED"
        print(f"Session Status: {status_label}")
        print(f"User ID:        {result.get('user_id')}")
        print(f"Username:       @{result.get('username')}" if result.get("username") else "Username:       None")
        print(f"Phone:          {result.get('phone_masked')}")
    elif "count" in result and "messages" in result:
        print(f"Read {result['count']} messages from @{result.get('bot')}:")
        for m in result.get("messages", []):
            print(f"\n--- Message ID: {m.get('id')} ({m.get('date')}) ---")
            print(m.get("text") or "(no text)")
            if m.get("buttons"):
                print("  Buttons:")
                for row in m["buttons"]:
                    row_strs = [f"[{b.get('text')}] (data={b.get('data')})" for b in row]
                    print("    " + " | ".join(row_strs))
    elif "steps" in result:
        print(f"Smoke Test against @{result.get('bot')}: SUCCESS")
        print("Executed steps:")
        for s in result.get("steps", []):
            print(f"  - {s.get('step')}: {json.dumps(s, ensure_ascii=False)}")
        final = result.get("final_message", {})
        print(f"Final Message ID {final.get('id')}: {final.get('text')[:60]}...")
    elif "file_path" in result:
        print(f"Downloaded media from @{result.get('bot')} (msg {result.get('message_id')}):")
        print(f"  Path: {result.get('file_path')} ({result.get('file_size')} bytes)")
    elif "button_text" in result or "button_data" in result:
        print(f"Clicked button on @{result.get('bot')} (msg {result.get('message_id')}):")
        print(f"  Label:  {result.get('button_text')}")
        print(f"  Data:   {result.get('button_data')}")
        if result.get("answer"):
            print(f"  Answer: {result.get('answer')}")
    elif "text" in result:
        print(f"Sent text to @{result.get('bot')} (msg {result.get('message_id')}): {result.get('text')}")
    else:
        for k, v in result.items():
            print(f"{k}: {v}")


def format_error(err: Exception, is_json: bool):
    """Format and print error to appropriate stream without secret leakage."""
    if is_json:
        if isinstance(err, AccountToolError):
            data: Dict[str, Any] = {
                "success": False,
                "error": err.error_name,
                "message": str(err),
            }
            if isinstance(err, FloodWaitError):
                data["retry_after_seconds"] = err.retry_after_seconds
        else:
            data = {
                "success": False,
                "error": "internal_error",
                "exception_class": type(err).__name__,
                "message": "An unexpected internal error occurred.",
            }
        print(json.dumps(data, indent=2, ensure_ascii=False))
    else:
        if isinstance(err, AccountToolError):
            print(f"Error [{err.error_name}]: {err}", file=sys.stderr)
        else:
            print(
                f"Error [internal_error]: ({type(err).__name__}) An unexpected internal error occurred.",
                file=sys.stderr,
            )


async def async_main(args: argparse.Namespace) -> int:
    """Execute parsed CLI command asynchronously."""
    # Load configuration
    env_file = getattr(args, "env_file", None)
    session_path = getattr(args, "session_path", None)
    timeout = getattr(args, "timeout", None)
    is_json = getattr(args, "json", False)

    config = Config.load(
        env_file=env_file,
        session_path=session_path,
        timeout=timeout,
    )

    client = TelegramAccountClient(config)
    cmd = args.subcommand

    if cmd == "login":
        result = await client.interactive_login(phone=getattr(args, "phone", None))
    elif cmd == "status":
        result = await client.get_status()
    elif cmd == "send-text":
        result = await client.send_text(target_bot=args.bot, text=args.text)
    elif cmd == "send-file":
        result = await client.send_file(target_bot=args.bot, file_path=args.file, caption=getattr(args, "caption", None))
    elif cmd == "read-messages":
        result = await client.read_messages(
            target_bot=args.bot,
            limit=args.limit,
            min_id=args.min_id,
            wait_seconds=getattr(args, "wait_seconds", None),
        )
    elif cmd == "click-button":
        row, col = None, None
        if getattr(args, "coord", None):
            row, col = args.coord[0], args.coord[1]
        elif getattr(args, "row", None) is not None and getattr(args, "col", None) is not None:
            row, col = args.row, args.col

        result = await client.click_button(
            target_bot=args.bot,
            message_id=args.message_id,
            button_text=getattr(args, "button_text", None),
            button_data=getattr(args, "button_data", None),
            row=row,
            col=col,
        )
    elif cmd == "download-media":
        result = await client.download_media(
            target_bot=args.bot,
            message_id=args.message_id,
            dest_dir=args.dest_dir,
        )
    elif cmd == "smoke":
        result = await client.smoke(target_bot=args.bot, timeout=getattr(args, "timeout", None))
    else:
        raise AccountToolError(f"Unknown subcommand: {cmd}", exit_code=EXIT_GENERIC_ERROR)

    format_output(result, is_json=is_json)
    return EXIT_SUCCESS


def main(argv: Optional[List[str]] = None) -> int:
    """Entry point for CLI execution."""
    parser = build_parser()
    args = parser.parse_args(argv)
    is_json = getattr(args, "json", False)

    try:
        return asyncio.run(async_main(args))
    except AccountToolError as e:
        format_error(e, is_json=is_json)
        return e.exit_code
    except Exception as e:
        format_error(e, is_json=is_json)
        return EXIT_GENERIC_ERROR


if __name__ == "__main__":
    sys.exit(main())
