#!/usr/bin/env python3
"""
Telegram MarkdownV2 i18n Incremental Validator
Tests message strings from i18n.json files by sending and deleting them via Telegram Bot API.
Supports persistent hashing/caching to only test new and modified keys on subsequent runs.
"""

import argparse
import hashlib
import json
import os
import re
import sys
import time
import requests

PLACEHOLDER_REGEX = re.compile(r'\{[^{}]+\}')
MD_MARKERS = re.compile(r'[*_`~|\\]|!\[|tg://')

SKIP_PREFIXES = (
    'buttons.', 'btn_', 'emojis.', '.button', '_btn', '.btn',
    'button_', '_button', 'emoji_', '_emoji', '.emoji', 'keyboards.',
    '_kb', '.kb', 'quota_weekly_limit', 'quota_daily_limit'
)

def extract_strings(data, prefix=''):
    """Recursively extracts all translatable strings from dict/list."""
    items = []
    if isinstance(data, dict):
        for key, val in data.items():
            new_prefix = f"{prefix}.{key}" if prefix else str(key)
            items.extend(extract_strings(val, new_prefix))
    elif isinstance(data, list):
        for idx, val in enumerate(data):
            new_prefix = f"{prefix}[{idx}]"
            items.extend(extract_strings(val, new_prefix))
    elif isinstance(data, str):
        items.append((prefix, data))
    return items

def is_button_or_ignored(key_path):
    lower_path = key_path.lower()
    return any(p in lower_path for p in SKIP_PREFIXES)

def normalize_text(text):
    # Replace all {placeholder} with safe text '1'
    return PLACEHOLDER_REGEX.sub('1', text)

def compute_hash(text):
    return hashlib.sha256(text.encode('utf-8')).hexdigest()[:16]

def test_telegram_mdv2(session, bot_token, chat_id, text, api_base="https://api.telegram.org", fast_mode=False):
    url_send = f"{api_base.rstrip('/')}/bot{bot_token}/sendMessage"
    url_delete = f"{api_base.rstrip('/')}/bot{bot_token}/deleteMessage"

    effective_chat_id = 0 if fast_mode else chat_id
    payload = {
        "chat_id": effective_chat_id,
        "text": text,
        "parse_mode": "MarkdownV2"
    }

    while True:
        try:
            resp = session.post(url_send, json=payload, timeout=10).json()
        except Exception as e:
            return False, f"Network error: {e}"

        if resp.get("ok"):
            msg_id = resp["result"]["message_id"]
            try:
                session.post(url_delete, json={"chat_id": effective_chat_id, "message_id": msg_id}, timeout=5)
            except Exception:
                pass
            return True, None

        error_code = resp.get("error_code")
        desc = resp.get("description", "Unknown error")

        # Fast mode against local bot API: chat not found means MarkdownV2 parsed successfully
        if fast_mode:
            if "can't parse entities" in desc:
                return False, desc
            if "chat not found" in desc or error_code == 400:
                return True, None

        # Rate limit handling
        if error_code == 429:
            retry_after = resp.get("parameters", {}).get("retry_after", 2)
            time.sleep(retry_after + 0.5)
            continue

        return False, desc

def load_cache(cache_file):
    if not os.path.isfile(cache_file):
        return None
    try:
        with open(cache_file, 'r', encoding='utf-8') as f:
            data = json.load(f)
            return data.get("hashes", {})
    except Exception:
        return None

def save_cache(cache_file, hashes_dict):
    try:
        os.makedirs(os.path.dirname(os.path.abspath(cache_file)), exist_ok=True)
        temp_file = f"{cache_file}.tmp"
        with open(temp_file, 'w', encoding='utf-8') as f:
            json.dump({"version": 1, "updated_at": int(time.time()), "hashes": hashes_dict}, f, indent=2)
        os.replace(temp_file, cache_file)
    except Exception as e:
        print(f"⚠️ Warning: Failed to save cache: {e}", file=sys.stderr)

def main():
    parser = argparse.ArgumentParser(description="Test i18n JSON strings against Telegram MarkdownV2 parser with diff-caching")
    parser.add_argument("-t", "--token", default=os.getenv("BOT_TOKEN"), help="Telegram Bot Token (or BOT_TOKEN env)")
    parser.add_argument("-c", "--chat-id", default=os.getenv("CHAT_ID"), help="Target Chat ID (or CHAT_ID env)")
    parser.add_argument("-f", "--file", required=True, help="Path to i18n JSON file")
    parser.add_argument("--api-url", default=os.getenv("TG_API_URL", "https://api.telegram.org"), help="Telegram Bot API URL base")
    parser.add_argument("--cache-file", help="Path to cache file (default: .<filename>.md-cache.json)")
    parser.add_argument("--delay", type=float, default=0.03, help="Delay between requests in seconds (default 0.03)")
    parser.add_argument("--limit", type=int, default=0, help="Limit number of items to test (0 = all)")
    parser.add_argument("--key", type=str, default="", help="Filter by key substring")
    parser.add_argument("--all", action="store_true", help="Don't skip button/emoji keys")
    parser.add_argument("--force", action="store_true", help="Force test all strings, ignoring cache")
    parser.add_argument("--fast", action="store_true", help="Fast validation via local Bot API parsing without sending to user")
    parser.add_argument("--md-only", action="store_true", help="Only test strings containing Markdown formatting or escapes")
    parser.add_argument("--namespaces", type=str, default="", help="Comma-separated namespaces to test (e.g. start,studio)")

    args = parser.parse_args()

    if not args.token:
        sys.exit("Error: Telegram bot token required (--token or BOT_TOKEN env).")
    if not args.chat_id and not args.fast:
        sys.exit("Error: Target chat_id required (--chat-id or CHAT_ID env, or use --fast).")
    if not os.path.isfile(args.file):
        sys.exit(f"Error: File not found: {args.file}")

    abs_file = os.path.abspath(args.file)
    dir_name, base_name = os.path.split(abs_file)
    cache_path = args.cache_file or os.path.join(dir_name, f".{base_name}.md-cache.json")

    with open(args.file, "r", encoding="utf-8") as f:
        try:
            data = json.load(f)
        except Exception as e:
            sys.exit(f"Invalid JSON in {args.file}: {e}")

    all_strings = extract_strings(data)
    candidates = {}
    ns_list = tuple(ns.strip().rstrip('.') + '.' for ns in args.namespaces.split(',') if ns.strip()) if args.namespaces else ()

    for key_path, val in all_strings:
        stem = '.'.join(key_path.split('.')[1:]) if '.' in key_path else key_path
        if ns_list and not any(stem.startswith(ns) for ns in ns_list):
            continue
        if args.key and args.key.lower() not in key_path.lower():
            continue
        if not args.all and is_button_or_ignored(key_path):
            continue
        if args.md_only and not MD_MARKERS.search(val):
            continue
        if not val.strip():
            continue
        candidates[key_path] = val

    total_candidates = len(candidates)
    cached_hashes = None if args.force else load_cache(cache_path)

    new_keys = []
    mod_keys = []
    to_test = []

    if cached_hashes is None:
        print(f"ℹ️ Cache not found or --force used. Validating ALL {total_candidates} message strings...")
        for k, v in candidates.items():
            to_test.append((k, v))
        cached_hashes = {}
    else:
        for k, v in candidates.items():
            cur_hash = compute_hash(v)
            if k not in cached_hashes:
                new_keys.append(k)
                to_test.append((k, v))
            elif cached_hashes[k] != cur_hash:
                mod_keys.append(k)
                to_test.append((k, v))

        if not to_test:
            print(f"⚡ All {total_candidates} strings verified via cache '{os.path.basename(cache_path)}' (0 diffs). Instant PASS.")
            sys.exit(0)

        print(f"🔍 Diff detected: {len(to_test)} strings to test ({len(new_keys)} new, {len(mod_keys)} modified, {total_candidates - len(to_test)} cached).")

    if args.limit > 0:
        to_test = to_test[:args.limit]

    total = len(to_test)
    failed = []
    passed = 0
    session = requests.Session()

    for idx, (key_path, raw_text) in enumerate(to_test, 1):
        test_text = normalize_text(raw_text)
        ok, err = test_telegram_mdv2(
            session, args.token, args.chat_id, test_text,
            api_base=args.api_url, fast_mode=args.fast
        )

        if ok:
            passed += 1
            cached_hashes[key_path] = compute_hash(raw_text)
            print(f"[{idx}/{total}] OK: {key_path}", flush=True)
        else:
            failed.append((key_path, raw_text, err))
            print(f"[{idx}/{total}] FAILED: {key_path} -> {err}", flush=True)

        if args.delay > 0 and not args.fast:
            time.sleep(args.delay)

    # Clean up deleted keys from cache
    active_keys = set(candidates.keys())
    clean_cache = {k: h for k, h in cached_hashes.items() if k in active_keys}

    if failed:
        # Save partial successes so next run only tests failed ones
        save_cache(cache_path, clean_cache)
        print("\n" + "=" * 50)
        print(f"RESULT SUMMARY: {passed} passed, {len(failed)} failed (out of {total})")
        print("=" * 50)
        print("\nFAILED KEYS:")
        for k, text, err in failed:
            print(f"\n- Key: {k}")
            print(f"  Error: {err}")
            print(f"  Text:  {text}")
        sys.exit(1)
    else:
        save_cache(cache_path, clean_cache)
        print("\n" + "=" * 50)
        print(f"RESULT SUMMARY: All {passed} strings passed! Cache updated.")
        print("=" * 50)

if __name__ == "__main__":
    main()
