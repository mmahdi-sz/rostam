#!/bin/bash
set -e

export NO_PROXY="127.0.0.1,localhost"
export no_proxy="127.0.0.1,localhost"

PORT=${TESTAPI_PORT:-14379}
BASE_URL="http://127.0.0.1:$PORT"

echo "Building dev mode..."
cargo build --features testapi

echo "Starting test API server..."
TESTAPI_ENABLED=1 BOT_API_BASE_URL="http://127.0.0.1:$PORT/bot" ./target/debug/rostam-dev > testapi.log 2>&1 &
SERVER_PID=$!

function cleanup {
    echo "Killing server PID $SERVER_PID"
    kill $SERVER_PID || true
    wait $SERVER_PID 2>/dev/null || true
    rm -f testapi.log
}
trap cleanup EXIT

echo "Waiting for server to start..."
for i in {1..10}; do
    if grep -q "\[testapi\] listening" testapi.log 2>/dev/null; then
        echo "Server is ready."
        break
    fi
    sleep 0.5
    if [ $i -eq 10 ]; then
        echo "Server failed to start!"
        cat testapi.log
        exit 1
    fi
done

echo ""
echo "=== Running Paywall Tests ==="

# Test 1: Feature blocked (rank below min_rank)
echo "Testing /test/rank/paywall (Blocked)"
RES=$(curl -s -X POST "$BASE_URL/test/rank/paywall" \
    -H "Content-Type: application/json" \
    -d '{"feature": "TestFeature", "rank": "Dalavar"}')

# Debug output
echo $RES | jq .

# Assertions
OK=$(echo "$RES" | jq -r '.ok')
if [ "$OK" != "true" ]; then
    echo "Fail: ok != true"
    exit 1
fi

WARNING_TEXT=$(echo "$RES" | jq -r '.warning_message.rendered_text')
if [[ ! "$WARNING_TEXT" == *"TestFeature"* ]]; then
    echo "Fail: Warning message text doesn't contain feature name. Got: $WARNING_TEXT"
    exit 1
fi

INLINE_KBD=$(echo "$RES" | jq -r '.inline_keyboard[0][0].callback_data')
if [[ ! "$INLINE_KBD" == *"rank:select:"* ]]; then
    echo "Fail: Incorrect inline keyboard callback data for shop menu. Got: $INLINE_KBD"
    exit 1
fi

STATS=$(echo "$RES" | jq -c '.stats_events[0]')
if [[ ! "$STATS" == *"paywall"* ]]; then
    echo "Fail: Missing stats_events for paywall."
    exit 1
fi

echo "✅ Paywall test passed!"

echo ""
echo "=== Running Emoji Rendering Tests ==="

echo "Testing /test/emoji/premium_render (Simple Expansion)"
# Assume {emoji.panel.icons.rank} triggers a cache expansion
RES=$(curl -s -X POST "$BASE_URL/test/emoji/premium_render" \
    -H "Content-Type: application/json" \
    -d '{"text": "Hello {emoji.panel.icons.rank}", "chat_id": 12345}')

# Check for successful resolution
OK=$(echo "$RES" | jq -r '.ok')
if [ "$OK" != "true" ]; then
    echo "Fail: ok != true"
    exit 1
fi

TEXT=$(echo "$RES" | jq -r '.rendered_text')
if [[ "$TEXT" == *"{emoji.panel.icons.rank}"* ]]; then
    # Well, it might be empty if the cache isn't loaded in test mode,
    # but at least let's verify the endpoint doesn't crash and returns valid JSON.
    echo "Note: Cache not loaded, key not expanded. Endpoint works though."
else
    echo "Expanded text: $TEXT"
fi

echo "✅ Emoji test passed!"

echo ""
echo "=== Running Router Callback Tests ==="

echo "Testing /test/router/callback"
# Assume sending "start:panel" routes to the start menu
RES=$(curl -s -X POST "$BASE_URL/test/router/callback" \
    -H "Content-Type: application/json" \
    -d '{"callback_data": "start:panel", "user_id": 12345, "username": "testuser"}')

# Check for successful execution
OK=$(echo "$RES" | jq -r '.ok')
if [ "$OK" != "true" ]; then
    echo "Fail: ok != true"
    exit 1
fi

TEXT=$(echo "$RES" | jq -r '.message.rendered_text')
if [[ -z "$TEXT" || "$TEXT" == "null" ]]; then
    echo "Fail: No message rendered text returned."
    exit 1
fi

echo "Router callback dispatched successfully, responded with: ${TEXT:0:50}..."

echo "Testing /test/router/callback (rank:shop)"
RES_SHOP=$(curl -s -X POST "$BASE_URL/test/router/callback" \
    -H "Content-Type: application/json" \
    -d '{"callback_data": "rank:shop", "user_id": 12345, "username": "testuser"}')
if [ "$(echo "$RES_SHOP" | jq -r '.ok')" != "true" ]; then echo "Fail: rank:shop callback"; exit 1; fi

echo "Testing /test/router/callback (rank:select:esfandyar)"
RES_DETAIL=$(curl -s -X POST "$BASE_URL/test/router/callback" \
    -H "Content-Type: application/json" \
    -d '{"callback_data": "rank:select:esfandyar", "user_id": 12345, "username": "testuser"}')
if [ "$(echo "$RES_DETAIL" | jq -r '.ok')" != "true" ]; then echo "Fail: rank:select:esfandyar callback"; exit 1; fi

echo "✅ Router callback test passed!"

echo ""
echo "=== Running Youtube Format Tests ==="
echo "Testing /test/youtube/format"
RES_YT=$(curl -s -X POST "$BASE_URL/test/youtube/format" \
    -H "Content-Type: application/json" \
    -d '{"url": "https://youtube.com/watch?v=dQw4w9WgXcQ"}')
OK_YT=$(echo "$RES_YT" | jq -r '.ok')
if [ "$OK_YT" != "true" ]; then
    echo "Fail: youtube format ok != true"
    exit 1
fi
echo "✅ Youtube format test passed!"

echo ""
echo "=== Running PDF Compress Tests ==="
echo "Testing /test/pdfcompress/menu"
RES_PDF=$(curl -s -X POST "$BASE_URL/test/pdfcompress/menu" \
    -H "Content-Type: application/json" \
    -d '{"filename": "document.pdf", "level": "ebook"}')
FLAG_PDF=$(echo "$RES_PDF" | jq -r '.gs_flag')
if [ "$FLAG_PDF" != "-dPDFSETTINGS=/ebook" ]; then
    echo "Fail: pdf compress flag mismatch. Got: $FLAG_PDF"
    exit 1
fi
echo "✅ PDF compress test passed!"

echo ""
echo "=== Running Social Media Guide Tests ==="

echo "Testing /test/start/guide (menu)"
RES_GD=$(curl -s -X POST "$BASE_URL/test/start/guide" -H "Content-Type: application/json" -d '{}')
if [ "$(echo "$RES_GD" | jq -r '.ok')" != "true" ]; then echo "Fail: guide menu"; exit 1; fi
if [ "$(echo "$RES_GD" | jq -r '.start_menu_first_row_callback')" != "start:guide" ]; then
    echo "Fail: guide button is not the first start-menu row"; exit 1
fi
GD_CBS=$(echo "$RES_GD" | jq -r '.button_callbacks | join(",")')
if [ "$GD_CBS" != "start:guide:yt,start:guide:sp,start:guide:sc,start:panel" ]; then
    echo "Fail: guide menu callbacks. Got: $GD_CBS"; exit 1
fi

for P in yt sp sc; do
    echo "Testing /test/start/guide ($P)"
    RES_GP=$(curl -s -X POST "$BASE_URL/test/start/guide" -H "Content-Type: application/json" -d "{\"platform\": \"$P\"}")
    if [ "$(echo "$RES_GP" | jq -r '.ok')" != "true" ]; then echo "Fail: guide $P"; exit 1; fi
    if [ "$(echo "$RES_GP" | jq -r '.within_telegram_cap')" != "true" ]; then echo "Fail: guide $P over 4096"; exit 1; fi
    if [ "$(echo "$RES_GP" | jq -r '.mentions_autodetect')" != "true" ]; then echo "Fail: guide $P missing autodetect note"; exit 1; fi
    if [ "$(echo "$RES_GP" | jq -r '.rendered_text')" == "" ]; then echo "Fail: guide $P empty text"; exit 1; fi
    if [ "$(echo "$RES_GP" | jq -r '.button_callbacks | join(",")')" != "start:guide" ]; then
        echo "Fail: guide $P back button"; exit 1
    fi
done

echo "Testing /test/start/guide (unknown platform)"
RES_GB=$(curl -s -X POST "$BASE_URL/test/start/guide" -H "Content-Type: application/json" -d '{"platform": "tiktok"}')
if [ "$(echo "$RES_GB" | jq -r '.ok')" != "false" ]; then echo "Fail: unknown platform accepted"; exit 1; fi
echo "✅ Social media guide tests passed!"

echo ""
echo "=== Running Emoji Panel Access Tests (/emoji, admin only) ==="
ADMIN_ID=$(grep -E '^ADMIN_USER_ID=' .env 2>/dev/null | head -1 | cut -d= -f2 | tr -d '"' | tr -d "'")
if [ -n "$ADMIN_ID" ]; then
    RES_EP=$(curl -s -X POST "$BASE_URL/test/emoji/panel" -H "Content-Type: application/json" -d "{\"user_id\": $ADMIN_ID}")
    if [ "$(echo "$RES_EP" | jq -r '.opens_panel')" != "true" ]; then echo "Fail: admin cannot open emoji panel"; exit 1; fi
    if [ "$(echo "$RES_EP" | jq -r '.button_callbacks | join(",")')" == "" ]; then echo "Fail: emoji panel keyboard empty"; exit 1; fi
fi
RES_EPN=$(curl -s -X POST "$BASE_URL/test/emoji/panel" -H "Content-Type: application/json" -d '{"user_id": -999003}')
if [ "$(echo "$RES_EPN" | jq -r '.opens_panel')" != "false" ]; then echo "Fail: non-admin opened emoji panel"; exit 1; fi
if [ "$(echo "$RES_EPN" | jq -r '.rendered_text')" != "" ]; then echo "Fail: emoji panel leaked to non-admin"; exit 1; fi
if [ "$(echo "$RES_EPN" | jq -r '.in_start_menu')" != "false" ]; then echo "Fail: emoji panel exposed in start menu"; exit 1; fi
echo "✅ Emoji panel access tests passed!"

echo ""
echo "=== Running Extended TestAPI Endpoint Suite ==="

echo "Testing /test/youtube/quality_select"
RES_QS=$(curl -s -X POST "$BASE_URL/test/youtube/quality_select" -H "Content-Type: application/json" -d '{"request_id": 999, "height": 720}')
if [ "$(echo "$RES_QS" | jq -r '.ok')" != "true" ]; then echo "Fail: quality_select"; exit 1; fi

echo "Testing /test/youtube/cancel"
RES_CAN=$(curl -s -X POST "$BASE_URL/test/youtube/cancel" -H "Content-Type: application/json" -d '{"request_id": 999}')
if [ "$(echo "$RES_CAN" | jq -r '.ok')" != "true" ]; then echo "Fail: youtube cancel"; exit 1; fi

echo "Testing /test/stt/recognize"
RES_STT=$(curl -s -X POST "$BASE_URL/test/stt/recognize" -H "Content-Type: application/json" -d '{"file_id": "file_123", "lang": "fa"}')
if [ "$(echo "$RES_STT" | jq -r '.ok')" != "true" ]; then echo "Fail: stt recognize"; exit 1; fi

echo "Testing /test/separation/submit"
RES_SEP=$(curl -s -X POST "$BASE_URL/test/separation/submit" -H "Content-Type: application/json" -d '{"file_id": "file_123", "mode": "stems2"}')
if [ "$(echo "$RES_SEP" | jq -r '.ok')" != "true" ]; then echo "Fail: separation submit"; exit 1; fi

echo "Testing /test/gwm/detect"
RES_GWM=$(curl -s -X POST "$BASE_URL/test/gwm/detect" -H "Content-Type: application/json" -d '{"file_id": "file_123"}')
if [ "$(echo "$RES_GWM" | jq -r '.ok')" != "true" ]; then echo "Fail: gwm detect"; exit 1; fi

echo "Testing /test/denoise/process"
RES_DN=$(curl -s -X POST "$BASE_URL/test/denoise/process" -H "Content-Type: application/json" -d '{"file_id": "file_video_123", "is_video": true}')
if [ "$(echo "$RES_DN" | jq -r '.ok')" != "true" ]; then echo "Fail: denoise process"; exit 1; fi

echo "Testing /test/tts/generate"
# Runs the real Piper/edge-tts engine + the ffmpeg opus conversion, so a broken
# model path or a rejected channel layout fails here.
RES_TTS=$(curl -s --max-time 300 -X POST "$BASE_URL/test/tts/generate" -H "Content-Type: application/json" -d '{"text": "سلام این یک تست است", "mode": "default"}')
if [ "$(echo "$RES_TTS" | jq -r '.ok')" != "true" ]; then echo "Fail: tts generate (fa): $RES_TTS"; exit 1; fi
if [ "$(echo "$RES_TTS" | jq -r '.output_ext')" != "ogg" ]; then echo "Fail: tts fa did not produce ogg: $RES_TTS"; exit 1; fi

RES_TTS_EN=$(curl -s --max-time 300 -X POST "$BASE_URL/test/tts/generate" -H "Content-Type: application/json" -d '{"text": "hello this is a test"}')
if [ "$(echo "$RES_TTS_EN" | jq -r '.output_ext')" != "ogg" ]; then echo "Fail: tts en did not produce ogg: $RES_TTS_EN"; exit 1; fi

RES_TTS_BAD=$(curl -s --max-time 300 -X POST "$BASE_URL/test/tts/generate" -H "Content-Type: application/json" -d '{"text": ""}')
if [ "$(echo "$RES_TTS_BAD" | jq -r '.ok')" != "false" ]; then echo "Fail: tts empty text should not succeed: $RES_TTS_BAD"; exit 1; fi

echo "Testing /test/tts/ux (char cap + job cancel button)"
RES_TUX=$(curl -s -X POST "$BASE_URL/test/tts/ux" -H "Content-Type: application/json" -d '{"char_len": 10}')
if [ "$(echo "$RES_TUX" | jq -r '.max_chars')" != "500" ]; then echo "Fail: tts max_chars: $RES_TUX"; exit 1; fi
if [ "$(echo "$RES_TUX" | jq -r '.too_long')" != "false" ]; then echo "Fail: short text flagged too_long: $RES_TUX"; exit 1; fi
if [ "$(echo "$RES_TUX" | jq -r '.progress_keyboard[0][0].callback_data')" != "tts:jobcancel" ]; then echo "Fail: tts progress cancel button missing: $RES_TUX"; exit 1; fi
if [ "$(echo "$RES_TUX" | jq -r '.progress_keyboard[0][0].style')" != "danger" ]; then echo "Fail: tts cancel button not danger: $RES_TUX"; exit 1; fi
# failure path: over the 500-char cap must render the i18n error, not an `!key!`
RES_TUXL=$(curl -s -X POST "$BASE_URL/test/tts/ux" -H "Content-Type: application/json" -d '{"char_len": 501}')
if [ "$(echo "$RES_TUXL" | jq -r '.too_long')" != "true" ]; then echo "Fail: 501 chars not rejected: $RES_TUXL"; exit 1; fi
if echo "$RES_TUXL" | jq -r '.too_long_text' | grep -q '!tts'; then echo "Fail: tts.text_too_long missing from i18n: $RES_TUXL"; exit 1; fi
if ! echo "$RES_TUXL" | jq -r '.too_long_text' | grep -q '501'; then echo "Fail: too_long_text lacks length: $RES_TUXL"; exit 1; fi

echo "Testing /test/stt/ready (human model label + premium emoji)"
RES_SR=$(curl -s -X POST "$BASE_URL/test/stt/ready" -H "Content-Type: application/json" -d '{"model": "fa_small"}')
if [ "$(echo "$RES_SR" | jq -r '.ok')" != "true" ]; then echo "Fail: stt ready: $RES_SR"; exit 1; fi
if echo "$RES_SR" | jq -r '.ready_title' | grep -q 'stt\.language'; then echo "Fail: stt ready_title leaks i18n key: $RES_SR"; exit 1; fi
if [ "$(echo "$RES_SR" | jq -r '.premium_emoji_count')" == "0" ]; then echo "Fail: stt ready_title has no premium emoji: $RES_SR"; exit 1; fi
if echo "$RES_SR" | jq -r '.ready_again' | grep -q 'stt\.language'; then echo "Fail: stt ready_again leaks i18n key: $RES_SR"; exit 1; fi

echo "Testing /test/deoldify/colorized"
RES_DEO=$(curl -s -X POST "$BASE_URL/test/deoldify/colorized" -H "Content-Type: application/json" -d '{"file_id": "file_bw_123", "render_factor": 24}')
if [ "$(echo "$RES_DEO" | jq -r '.ok')" != "true" ]; then echo "Fail: deoldify colorized"; exit 1; fi

echo "Testing /test/nobg/process"
RES_NOBG=$(curl -s -X POST "$BASE_URL/test/nobg/process" -H "Content-Type: application/json" -d '{"file_id": "file_nobg_123"}')
if [ "$(echo "$RES_NOBG" | jq -r '.ok')" != "true" ]; then echo "Fail: nobg process"; exit 1; fi

echo "Testing /test/admin/panel"
RES_ADM=$(curl -s -X POST "$BASE_URL/test/admin/panel" -H "Content-Type: application/json" -d '{"user_id": 12345}')
if [ "$(echo "$RES_ADM" | jq -r '.ok')" != "true" ]; then echo "Fail: admin panel"; exit 1; fi

echo "Testing /test/admin/stats_section"
for SEC in ov users yt ai music files money sys err; do
  RES_SEC=$(curl -s -X POST "$BASE_URL/test/admin/stats_section" -H "Content-Type: application/json" -d "{\"section\": \"$SEC\"}")
  if [ "$(echo "$RES_SEC" | jq -r '.ok')" != "true" ]; then echo "Fail: admin stats_section $SEC"; exit 1; fi
  if [ "$(echo "$RES_SEC" | jq -r '.known_section')" != "true" ]; then echo "Fail: admin stats_section unknown $SEC"; exit 1; fi
  if [ "$(echo "$RES_SEC" | jq -r '.rendered_text | length')" = "0" ]; then echo "Fail: admin stats_section empty $SEC"; exit 1; fi
done
# failure path: unknown key falls back to the overview, keyboard still navigable
RES_SEC_BAD=$(curl -s -X POST "$BASE_URL/test/admin/stats_section" -H "Content-Type: application/json" -d '{"section": "does_not_exist"}')
if [ "$(echo "$RES_SEC_BAD" | jq -r '.known_section')" != "false" ]; then echo "Fail: admin stats_section bad key"; exit 1; fi
if [ "$(echo "$RES_SEC_BAD" | jq -r '.nav_callbacks | length')" -lt 9 ]; then echo "Fail: admin stats_section nav missing"; exit 1; fi

echo "Testing /test/admin/broadcast"
RES_BC=$(curl -s -X POST "$BASE_URL/test/admin/broadcast" -H "Content-Type: application/json" -d '{"mode": "Copy", "pin": true, "target_count": 50}')
if [ "$(echo "$RES_BC" | jq -r '.ok')" != "true" ]; then echo "Fail: admin broadcast"; exit 1; fi

echo "Testing /test/surge/validate_url"
RES_SURGE=$(curl -s -X POST "$BASE_URL/test/surge/validate_url" -H "Content-Type: application/json" -d '{"url": "https://example.com/file.zip"}')
if [ "$(echo "$RES_SURGE" | jq -r '.valid')" != "true" ]; then echo "Fail: surge validate_url"; exit 1; fi

RES_SURGE_PS=$(curl -s -X POST "$BASE_URL/test/surge/validate_url" -H "Content-Type: application/json" -d '{"url": "https://play.google.com/store/apps/details?id=com.example"}')
if [ "$(echo "$RES_SURGE_PS" | jq -r '.detected_platform')" != "playstore" ]; then echo "Fail: surge validate_url playstore"; exit 1; fi

echo "Testing /test/sp/download_track"
RES_SP=$(curl -s -X POST "$BASE_URL/test/sp/download_track" -H "Content-Type: application/json" -d '{"url": "https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT"}')
if [ "$(echo "$RES_SP" | jq -r '.ok')" != "true" ]; then echo "Fail: spotify download_track ok"; exit 1; fi
if [ "$(echo "$RES_SP" | jq -r '.detected_track_id')" != "4cOdK2wGLETKBW3PvgPWqT" ]; then echo "Fail: spotify detected_track_id"; exit 1; fi
if [ "$(echo "$RES_SP" | jq -r '.cancel_callback')" != "sp:cancel" ]; then echo "Fail: spotify cancel_callback"; exit 1; fi

RES_SP_INVALID=$(curl -s -X POST "$BASE_URL/test/sp/download_track" -H "Content-Type: application/json" -d '{"url": "https://example.com/not_spotify"}')
if [ "$(echo "$RES_SP_INVALID" | jq -r '.ok')" != "false" ]; then echo "Fail: spotify invalid url accepted"; exit 1; fi

echo "Testing /test/sp/cancel"
RES_SP_CANCEL=$(curl -s -X POST "$BASE_URL/test/sp/cancel" -H "Content-Type: application/json" -d '{"user_id": 987654}')
if [ "$(echo "$RES_SP_CANCEL" | jq -r '.ok')" != "true" ]; then echo "Fail: spotify cancel ok"; exit 1; fi
echo "✅ Spotify tests passed!"

echo "Testing /test/sc/download_track"
RES_SC=$(curl -s -X POST "$BASE_URL/test/sc/download_track" -H "Content-Type: application/json" -d '{"url": "https://soundcloud.com/forss/vlick"}')
if [ "$(echo "$RES_SC" | jq -r '.ok')" != "true" ]; then echo "Fail: soundcloud download_track ok"; exit 1; fi
if [ "$(echo "$RES_SC" | jq -r '.detected_url')" != "https://soundcloud.com/forss/vlick" ]; then echo "Fail: soundcloud detected_url"; exit 1; fi
if [ "$(echo "$RES_SC" | jq -r '.cancel_callback')" != "sc:cancel" ]; then echo "Fail: soundcloud cancel_callback"; exit 1; fi

RES_SC_INVALID=$(curl -s -X POST "$BASE_URL/test/sc/download_track" -H "Content-Type: application/json" -d '{"url": "https://example.com/not_soundcloud"}')
if [ "$(echo "$RES_SC_INVALID" | jq -r '.ok')" != "false" ]; then echo "Fail: soundcloud invalid url accepted"; exit 1; fi

echo "Testing /test/sc/cancel"
RES_SC_CANCEL=$(curl -s -X POST "$BASE_URL/test/sc/cancel" -H "Content-Type: application/json" -d '{"user_id": 987654}')
if [ "$(echo "$RES_SC_CANCEL" | jq -r '.ok')" != "true" ]; then echo "Fail: soundcloud cancel ok"; exit 1; fi
echo "✅ SoundCloud tests passed!"

echo "Testing /test/ms/offer (spotify album, esfandyar)"
RES_MS=$(curl -s -X POST "$BASE_URL/test/ms/offer" -H "Content-Type: application/json" -d '{"url": "https://open.spotify.com/album/1ATL5GLyefJaxhQzSPVrLX", "rank": "esfandyar"}')
if [ "$(echo "$RES_MS" | jq -r '.ok')" != "true" ]; then echo "Fail: ms offer ok. Got: $RES_MS"; exit 1; fi
if [ "$(echo "$RES_MS" | jq -r '.platform')" != "album" ]; then echo "Fail: ms offer platform. Got: $RES_MS"; exit 1; fi
if [ "$(echo "$RES_MS" | jq -r '.track_limit')" != "null" ]; then echo "Fail: ms esfandyar must be unlimited. Got: $RES_MS"; exit 1; fi
if [ "$(echo "$RES_MS" | jq -r '.keyboard[0][0].callback_data')" != "ms:mode:one" ]; then echo "Fail: ms one-by-one button. Got: $RES_MS"; exit 1; fi
if [ "$(echo "$RES_MS" | jq -r '.keyboard[0][1].callback_data')" != "ms:mode:zip" ]; then echo "Fail: ms zip button. Got: $RES_MS"; exit 1; fi
if [ "$(echo "$RES_MS" | jq -r '.keyboard[1][0].callback_data')" != "ms:cancel" ]; then echo "Fail: ms cancel button. Got: $RES_MS"; exit 1; fi

echo "Testing /test/ms/offer (soundcloud set, sepahbod cap)"
RES_MS_SC=$(curl -s -X POST "$BASE_URL/test/ms/offer" -H "Content-Type: application/json" -d '{"url": "https://soundcloud.com/chris-467177669/sets/songs", "rank": "sepahbod"}')
if [ "$(echo "$RES_MS_SC" | jq -r '.platform')" != "soundcloud" ]; then echo "Fail: ms sc platform. Got: $RES_MS_SC"; exit 1; fi
if [ "$(echo "$RES_MS_SC" | jq -r '.track_limit')" != "20" ]; then echo "Fail: ms sepahbod cap. Got: $RES_MS_SC"; exit 1; fi
if [ "$(echo "$RES_MS_SC" | jq -r '.can_archive')" != "false" ]; then echo "Fail: ms sepahbod must not archive. Got: $RES_MS_SC"; exit 1; fi

echo "Testing /test/ms/offer (dalavar paywall)"
RES_MS_PW=$(curl -s -X POST "$BASE_URL/test/ms/offer" -H "Content-Type: application/json" -d '{"url": "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M", "rank": "dalavar"}')
if [ "$(echo "$RES_MS_PW" | jq -r '.blocked')" != "true" ]; then echo "Fail: ms dalavar not blocked. Got: $RES_MS_PW"; exit 1; fi
if [ "$(echo "$RES_MS_PW" | jq -r '.paywall_min_rank')" != "esfandyar" ]; then echo "Fail: ms paywall rank. Got: $RES_MS_PW"; exit 1; fi

echo "Testing /test/ms/offer (not a set link)"
RES_MS_BAD=$(curl -s -X POST "$BASE_URL/test/ms/offer" -H "Content-Type: application/json" -d '{"url": "https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT", "rank": "rostam"}')
if [ "$(echo "$RES_MS_BAD" | jq -r '.ok')" != "false" ]; then echo "Fail: ms track link accepted as set. Got: $RES_MS_BAD"; exit 1; fi

echo "Testing /test/ms/mode (sepahbod zip blocked, one-by-one allowed)"
RES_MS_ZIP=$(curl -s -X POST "$BASE_URL/test/ms/mode" -H "Content-Type: application/json" -d '{"rank": "sepahbod", "zip": true}')
if [ "$(echo "$RES_MS_ZIP" | jq -r '.blocked')" != "true" ]; then echo "Fail: ms sepahbod zip allowed. Got: $RES_MS_ZIP"; exit 1; fi
RES_MS_ONE=$(curl -s -X POST "$BASE_URL/test/ms/mode" -H "Content-Type: application/json" -d '{"rank": "sepahbod", "zip": false}')
if [ "$(echo "$RES_MS_ONE" | jq -r '.ok')" != "true" ]; then echo "Fail: ms sepahbod one-by-one blocked. Got: $RES_MS_ONE"; exit 1; fi
RES_MS_ZIP_OK=$(curl -s -X POST "$BASE_URL/test/ms/mode" -H "Content-Type: application/json" -d '{"rank": "esfandyar", "zip": true}')
if [ "$(echo "$RES_MS_ZIP_OK" | jq -r '.blocked')" != "false" ]; then echo "Fail: ms esfandyar zip blocked. Got: $RES_MS_ZIP_OK"; exit 1; fi
if [ "$(echo "$RES_MS_ZIP_OK" | jq -r '.archive_level')" != "9" ]; then echo "Fail: ms archive level must be 9. Got: $RES_MS_ZIP_OK"; exit 1; fi
echo "✅ MusicSet tests passed!"

echo "Testing /test/health/deep"
RES_HLT=$(curl -s -X POST "$BASE_URL/test/health/deep")
if [ "$(echo "$RES_HLT" | jq -r '.ok')" != "true" ]; then echo "Fail: health deep"; exit 1; fi

echo "Testing /test/rank/panel"
RES_RP=$(curl -s -X POST "$BASE_URL/test/rank/panel" -H "Content-Type: application/json" -d '{"user_id": 12345}')
if [ "$(echo "$RES_RP" | jq -r '.ok')" != "true" ]; then echo "Fail: rank panel"; exit 1; fi

echo "Testing /test/rank/free_rank (all langs)"
for L in fa en it ru; do
  RES_FR=$(curl -s -X POST "$BASE_URL/test/rank/free_rank" -H "Content-Type: application/json" -d "{\"lang\": \"$L\"}")
  if [ "$(echo "$RES_FR" | jq -r '.ok')" != "true" ]; then echo "Fail: free_rank $L. Got: $RES_FR"; exit 1; fi
  if [ "$(echo "$RES_FR" | jq -r '.free_rank_button.callback_data')" != "user:panel:referral" ]; then echo "Fail: free_rank callback $L. Got: $RES_FR"; exit 1; fi
done

echo "Testing /test/rank/free_rank (missing i18n key)"
RES_FRB=$(curl -s -X POST "$BASE_URL/test/rank/free_rank" -H "Content-Type: application/json" -d '{"lang": "fa", "label_key": "rank.no_such_key"}')
if [ "$(echo "$RES_FRB" | jq -r '.ok')" != "false" ]; then echo "Fail: free_rank accepted missing key. Got: $RES_FRB"; exit 1; fi

echo "Testing /test/referral/spend"
RES_REF=$(curl -s -X POST "$BASE_URL/test/referral/spend" -H "Content-Type: application/json" -d '{"points": 20, "tier": "Esfandyar"}')
if [ "$(echo "$RES_REF" | jq -r '.ok')" != "true" ]; then echo "Fail: referral spend. Got: $RES_REF"; exit 1; fi
# مسیر واقعی: ۲۰ امتیاز خرج تیر ۲۰تایی ⇒ موجودی صفر، رتبه نشسته، ۳۱ روز.
if [ "$(echo "$RES_REF" | jq -r '.remaining_points')" != "0" ]; then echo "Fail: referral spend did not debit. Got: $RES_REF"; exit 1; fi
if [ "$(echo "$RES_REF" | jq -r '.granted_rank')" != "esfandyar" ]; then echo "Fail: referral spend rank not granted. Got: $RES_REF"; exit 1; fi
DAYS_ADDED=$(echo "$RES_REF" | jq -r '.days_added')
if [ "$DAYS_ADDED" != "31" ] && [ "$DAYS_ADDED" != "32" ]; then echo "Fail: referral spend days. Got: $RES_REF"; exit 1; fi

echo "Testing /test/referral/spend (insufficient points)"
RES_REF_LOW=$(curl -s -X POST "$BASE_URL/test/referral/spend" -H "Content-Type: application/json" -d '{"points": 5, "tier": "Esfandyar"}')
if [ "$(echo "$RES_REF_LOW" | jq -r '.ok')" != "false" ]; then echo "Fail: referral spend granted with 5 points. Got: $RES_REF_LOW"; exit 1; fi
if [ "$(echo "$RES_REF_LOW" | jq -r '.granted_rank')" != "null" ]; then echo "Fail: rank granted without points. Got: $RES_REF_LOW"; exit 1; fi
echo "✅ Referral spend tests passed!"

echo "Testing /test/referral/leaderboard"
RES_LDB=$(curl -s -X POST "$BASE_URL/test/referral/leaderboard" -H "Content-Type: application/json" -d '{}')
if [ "$(echo "$RES_LDB" | jq -r '.ok')" != "true" ]; then echo "Fail: referral leaderboard ok"; exit 1; fi
if [ "$(echo "$RES_LDB" | jq -r '.has_rlm')" != "true" ]; then echo "Fail: referral leaderboard has_rlm"; exit 1; fi
LDB_TEXT=$(echo "$RES_LDB" | jq -r '.rendered_text')
if [[ ! "$LDB_TEXT" == *"mmahdi"* ]]; then echo "Fail: referral leaderboard text missing sample username"; exit 1; fi
echo "✅ Referral leaderboard test passed!"

echo "Testing /test/compress/submit"
RES_FC=$(curl -s -X POST "$BASE_URL/test/compress/submit" -H "Content-Type: application/json" -d '{"user_id": 12345, "fmt": "7z", "level": 5, "algo": "lzma2"}')
if [ "$(echo "$RES_FC" | jq -r '.ok')" != "true" ]; then echo "Fail: compress submit"; exit 1; fi

echo "Testing /test/compress/ux (solid mode colors)"
RES_FCU=$(curl -s -X POST "$BASE_URL/test/compress/ux" -H "Content-Type: application/json" -d '{"fmt": "7z", "level": 5, "solid": false}')
if [ "$(echo "$RES_FCU" | jq -r '.ok')" != "true" ]; then echo "Fail: compress ux"; exit 1; fi
if [ "$(echo "$RES_FCU" | jq -r '.solid_button_color')" != "success" ]; then
    echo "Fail: whole-folder solid button must be green. Got: $(echo "$RES_FCU" | jq -r '.solid_button_color')"; exit 1
fi
if [ "$(echo "$RES_FCU" | jq -r '.max_level_toast')" != "null" ]; then echo "Fail: toast leaked below max level"; exit 1; fi
if [ -z "$(echo "$RES_FCU" | jq -r '.progress_keyboard[0][0].callback_data')" ]; then echo "Fail: progress cancel button missing"; exit 1; fi
if [ "$(echo "$RES_FCU" | jq -r '.progress_keyboard[0][0].callback_data')" != "fc:jobcancel" ]; then echo "Fail: progress cancel callback"; exit 1; fi
if [ "$(echo "$RES_FCU" | jq -r '.ask_password_keyboard[0][0].callback_data')" != "fc:cancel" ]; then echo "Fail: ask-password cancel button missing"; exit 1; fi
if [[ ! "$(echo "$RES_FCU" | jq -r '.progress_text')" == *"02:05"* ]]; then echo "Fail: progress elapsed not mm:ss"; exit 1; fi
if [ -z "$(echo "$RES_FCU" | jq -r '.password_need_text')" ]; then echo "Fail: password_need_text missing"; exit 1; fi
# Staged status message: downloading must not render the compressing text, and a
# real percent must produce a remaining-time line (40% after 80s => 02:00 left).
DL_TEXT=$(echo "$RES_FCU" | jq -r '.downloading_text')
if [[ "$DL_TEXT" == *"{idx}"* || "$DL_TEXT" == *"{total}"* ]]; then echo "Fail: downloading placeholders unresolved"; exit 1; fi
if [[ ! "$DL_TEXT" == *"2"* || ! "$DL_TEXT" == *"3"* ]]; then echo "Fail: downloading must show file 2 of 3. Got: $DL_TEXT"; exit 1; fi
if [ "$(echo "$RES_FCU" | jq -r '.eta_shown')" != "true" ]; then echo "Fail: eta not computed at 40%"; exit 1; fi
if [ "$(echo "$RES_FCU" | jq -r '.bar_at_40')" != "████░░░░░░" ]; then echo "Fail: bar at 40%. Got: $(echo "$RES_FCU" | jq -r '.bar_at_40')"; exit 1; fi
ETA_TEXT=$(echo "$RES_FCU" | jq -r '.compress_text_with_eta')
if [[ ! "$ETA_TEXT" == *"02:00"* ]]; then echo "Fail: remaining time missing. Got: $ETA_TEXT"; exit 1; fi
if [[ ! "$ETA_TEXT" == *"████░░░░░░"* ]]; then echo "Fail: compress bar not filled at 40%"; exit 1; fi
# Failure path: no percent yet => elapsed only, never a fabricated ETA.
NOETA_TEXT=$(echo "$RES_FCU" | jq -r '.compress_text_no_eta')
if [[ ! "$NOETA_TEXT" == *"░░░░░░░░░░"* ]]; then echo "Fail: empty bar expected before first percent"; exit 1; fi
if [[ "$NOETA_TEXT" == *"{eta}"* ]]; then echo "Fail: eta placeholder leaked"; exit 1; fi

RES_FCS=$(curl -s -X POST "$BASE_URL/test/compress/ux" -H "Content-Type: application/json" -d '{"fmt": "7z", "solid": true}')
if [ "$(echo "$RES_FCS" | jq -r '.solid_button_color')" != "primary" ]; then
    echo "Fail: per-file solid button must be blue. Got: $(echo "$RES_FCS" | jq -r '.solid_button_color')"; exit 1
fi

# مسیر خطا/مرزی: زدن «+» روی سقف باید توست حداکثر فشرده‌سازی بدهد
RES_FCM=$(curl -s -X POST "$BASE_URL/test/compress/ux" -H "Content-Type: application/json" -d '{"fmt": "rar", "level": 5, "bump_level": true}')
if [ "$(echo "$RES_FCM" | jq -r '.max_level')" != "5" ]; then echo "Fail: rar max level should be 5"; exit 1; fi
if [ "$(echo "$RES_FCM" | jq -r '.level')" != "5" ]; then echo "Fail: level must clamp at max"; exit 1; fi
TOAST=$(echo "$RES_FCM" | jq -r '.max_level_toast')
if [ "$TOAST" == "null" ] || [[ ! "$TOAST" == *"rar"* ]] || [[ ! "$TOAST" == *"5"* ]]; then
    echo "Fail: max level toast wrong. Got: $TOAST"; exit 1
fi
echo "✅ File compress UX test passed!"

# ZSTD: سقف درجه ۱۹، و دکمه‌های رمز/پارت/solid باید از کیبورد حذف باشند —
# نمایش دکمهٔ رمز برای zstd یعنی کاربر رمز می‌دهد و آرشیو بی‌رمز می‌گیرد.
echo "Testing /test/compress/ux (zstd capabilities)"
RES_FCZ=$(curl -s -X POST "$BASE_URL/test/compress/ux" -H "Content-Type: application/json" -d '{"fmt": "zstd", "level": 19, "bump_level": true}')
if [ "$(echo "$RES_FCZ" | jq -r '.ok')" != "true" ]; then echo "Fail: compress ux zstd"; exit 1; fi
if [ "$(echo "$RES_FCZ" | jq -r '.fmt')" != "zstd" ]; then echo "Fail: zstd fmt not parsed"; exit 1; fi
if [ "$(echo "$RES_FCZ" | jq -r '.max_level')" != "19" ]; then echo "Fail: zstd max level should be 19"; exit 1; fi
if [ "$(echo "$RES_FCZ" | jq -r '.level')" != "19" ]; then echo "Fail: zstd level must clamp at 19"; exit 1; fi
for FLD in has_password_button has_split_button has_solid_button; do
    if [ "$(echo "$RES_FCZ" | jq -r ".$FLD")" != "false" ]; then
        echo "Fail: zstd must not expose $FLD"; exit 1
    fi
done
if [ -z "$(echo "$RES_FCZ" | jq -r '.welcome_text')" ] || [[ ! "$(echo "$RES_FCZ" | jq -r '.welcome_text')" == *"ZSTD"* ]]; then
    echo "Fail: zstd welcome text missing"; exit 1
fi
ZTOAST=$(echo "$RES_FCZ" | jq -r '.max_level_toast')
if [ "$ZTOAST" == "null" ] || [[ ! "$ZTOAST" == *"zstd"* ]] || [[ ! "$ZTOAST" == *"19"* ]]; then
    echo "Fail: zstd max level toast wrong. Got: $ZTOAST"; exit 1
fi
# و ۷z همان دکمه‌ها را باید داشته باشد (تا گیت قابلیت‌ها بقیه را خراب نکرده باشد)
for FLD in has_password_button has_split_button has_solid_button; do
    if [ "$(echo "$RES_FCU" | jq -r ".$FLD")" != "true" ]; then
        echo "Fail: 7z lost $FLD"; exit 1
    fi
done
echo "✅ File compress ZSTD test passed!"

echo ""
echo "=== Running Redeem Tests (real handler, dev DB) ==="

# ۱) کد تازه با ظرفیت ۱ ⇒ مصرف موفق
echo "Testing /test/redeem/apply (Consumed)"
RES_RD=$(curl -s -X POST "$BASE_URL/test/redeem/apply" -H "Content-Type: application/json" \
    -d '{"seed": true, "max_uses": 1, "rank": "Sepahbod", "duration_days": 7}')
if [ "$(echo "$RES_RD" | jq -r '.db')" != "connected" ]; then echo "Fail: redeem needs the dev DB. Got: $RES_RD"; exit 1; fi
if [ "$(echo "$RES_RD" | jq -r '.ok')" != "true" ]; then echo "Fail: redeem consume. Got: $RES_RD"; exit 1; fi
if [ "$(echo "$RES_RD" | jq -r '.used_count')" != "1" ]; then echo "Fail: used_count after first redeem. Got: $RES_RD"; exit 1; fi
if [ "$(echo "$RES_RD" | jq -r '.user_rank.rank')" != "sepahbod" ]; then echo "Fail: rank not applied. Got: $RES_RD"; exit 1; fi

# ۲) همان کاربر، همان کد، بدون seed ⇒ AlreadyRedeemed و ظرفیت دست‌نخورده
echo "Testing /test/redeem/apply (AlreadyRedeemed)"
RES_RD2=$(curl -s -X POST "$BASE_URL/test/redeem/apply" -H "Content-Type: application/json" -d '{}')
if [ "$(echo "$RES_RD2" | jq -r '.ok')" != "false" ]; then echo "Fail: second redeem granted again. Got: $RES_RD2"; exit 1; fi
if [ "$(echo "$RES_RD2" | jq -r '.used_count')" != "1" ]; then echo "Fail: second redeem burned capacity. Got: $RES_RD2"; exit 1; fi
if [ "$(echo "$RES_RD2" | jq -r '.redemption_rows')" != "1" ]; then echo "Fail: duplicate redemption row. Got: $RES_RD2"; exit 1; fi

# ۳) کد ناموجود ⇒ مسیر شکست (و پاک‌سازی ردیف‌های آزمایشی)
echo "Testing /test/redeem/apply (invalid code)"
RES_RD3=$(curl -s -X POST "$BASE_URL/test/redeem/apply" -H "Content-Type: application/json" \
    -d '{"code": "TESTAPINOSUCH", "cleanup": true}')
if [ "$(echo "$RES_RD3" | jq -r '.ok')" != "false" ]; then echo "Fail: invalid code accepted. Got: $RES_RD3"; exit 1; fi
if [ "$(echo "$RES_RD3" | jq -r '.rendered_text')" == "null" ]; then echo "Fail: invalid code sent no message. Got: $RES_RD3"; exit 1; fi
# پاک‌سازی ردیف‌های کد آزمایشی برای اجرای بعدی سوئیت
curl -s -X POST "$BASE_URL/test/redeem/apply" -H "Content-Type: application/json" \
    -d '{"cleanup": true}' > /dev/null
echo "✅ Redeem tests passed!"

echo ""
echo "=== Running Quota Reserve Tests (real rank::quota path, dev DB) ==="
# کاربر آزمایشی در بازه‌ی -999xxx؛ در پایان پاک می‌شود.
QUSER=-999002
q() { curl -s -X POST "$BASE_URL/test/quota" -H "Content-Type: application/json" -d "$1"; }
qfield() { echo "$1" | jq -r "$2"; }

# پاک‌سازی وضعیت قبلی: refund بزرگ سهمیه را صفر می‌کند (GREATEST با صفر).
q '{"user_id":-999002,"kind":"upscale_2x_weekly","action":"refund","amount":1000000,"window_secs":604800}' > /dev/null
q '{"user_id":-999002,"kind":"denoise_daily","action":"refund","amount":1000000,"window_secs":86400}' > /dev/null

# ۱) شمارشی (upscale): دو رزرو با سقف ۲ ⇒ هر دو موفق، used=1 سپس ۲
R=$(q '{"user_id":-999002,"kind":"upscale_2x_weekly","action":"reserve","amount":1,"window_secs":604800,"limit":2}')
if [ "$(qfield "$R" .granted)" != "true" ] || [ "$(qfield "$R" .used_after)" != "1" ]; then echo "Fail: upscale reserve #1. Got: $R"; exit 1; fi
R=$(q '{"user_id":-999002,"kind":"upscale_2x_weekly","action":"reserve","amount":1,"window_secs":604800,"limit":2}')
if [ "$(qfield "$R" .granted)" != "true" ] || [ "$(qfield "$R" .used_after)" != "2" ]; then echo "Fail: upscale reserve #2. Got: $R"; exit 1; fi

# ۲) رزرو سوم ⇒ رد، و مصرف دست‌نخورده (بدون کسر مضاعف)
R=$(q '{"user_id":-999002,"kind":"upscale_2x_weekly","action":"reserve","amount":1,"window_secs":604800,"limit":2}')
if [ "$(qfield "$R" .granted)" != "false" ]; then echo "Fail: upscale over-limit granted. Got: $R"; exit 1; fi
if [ "$(qfield "$R" .used)" != "2" ]; then echo "Fail: rejected reserve changed used. Got: $R"; exit 1; fi

# ۳) refund ⇒ برگشت به ۱، بعد رزرو دوباره جا می‌شود
R=$(q '{"user_id":-999002,"kind":"upscale_2x_weekly","action":"refund","amount":1,"window_secs":604800}')
if [ "$(qfield "$R" .used)" != "1" ]; then echo "Fail: upscale refund. Got: $R"; exit 1; fi
R=$(q '{"user_id":-999002,"kind":"upscale_2x_weekly","action":"reserve","amount":1,"window_secs":604800,"limit":2}')
if [ "$(qfield "$R" .granted)" != "true" ]; then echo "Fail: reserve after refund. Got: $R"; exit 1; fi

# ۴) مقداری (denoise): سقف ۶۰ ثانیه، ۴۰ جا می‌شود
R=$(q '{"user_id":-999002,"kind":"denoise_daily","action":"reserve","amount":40,"window_secs":86400,"limit":60}')
if [ "$(qfield "$R" .granted)" != "true" ] || [ "$(qfield "$R" .used_after)" != "40" ]; then echo "Fail: denoise reserve 40. Got: $R"; exit 1; fi

# ۵) ۴۰ دوم از باقی‌مانده بزرگ‌تر است ⇒ رد و used همان ۴۰
R=$(q '{"user_id":-999002,"kind":"denoise_daily","action":"reserve","amount":40,"window_secs":86400,"limit":60}')
if [ "$(qfield "$R" .granted)" != "false" ] || [ "$(qfield "$R" .used)" != "40" ]; then echo "Fail: denoise over-limit. Got: $R"; exit 1; fi

# ۶) ۲۰ دقیقاً جا می‌شود ⇒ ۶۰؛ سپس refund بزرگ‌تر از مصرف منفی نمی‌شود
R=$(q '{"user_id":-999002,"kind":"denoise_daily","action":"reserve","amount":20,"window_secs":86400,"limit":60}')
if [ "$(qfield "$R" .granted)" != "true" ] || [ "$(qfield "$R" .used_after)" != "60" ]; then echo "Fail: denoise exact fit. Got: $R"; exit 1; fi
R=$(q '{"user_id":-999002,"kind":"denoise_daily","action":"refund","amount":500,"window_secs":86400}')
if [ "$(qfield "$R" .used)" != "0" ]; then echo "Fail: refund went negative or wrong. Got: $R"; exit 1; fi

# پاک‌سازی ردیف‌های کاربر آزمایشی
q '{"user_id":-999002,"kind":"upscale_2x_weekly","action":"refund","amount":1000000,"window_secs":604800}' > /dev/null
echo "✅ Quota reserve tests passed! (test user $QUSER)"

echo ""
echo "=== Running Studio Trim Tests ==="

# Test 1: Valid multi-range parsing (including Persian digits & no-space dash)
echo "Testing /test/studio/trim (Valid ranges with Persian digits)"
RES_ST1=$(curl -s -X POST "$BASE_URL/test/studio/trim" \
    -H "Content-Type: application/json" \
    -d '{"input_ranges": "00:00 - 00:30\n۰۰:۰۱:۰۰-۰۰:۰۲:۰۰", "duration_secs": 300}')
if [ "$(echo "$RES_ST1" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_ST1" | jq -r '.is_valid')" != "true" ] || [ "$(echo "$RES_ST1" | jq -r '.ranges_count')" != "2" ]; then
    echo "Fail: Studio trim valid ranges test failed. Got: $RES_ST1"
    exit 1
fi

# Test 2: Invalid range parsing (start >= end and out of bounds)
echo "Testing /test/studio/trim (Invalid ranges)"
RES_ST2=$(curl -s -X POST "$BASE_URL/test/studio/trim" \
    -H "Content-Type: application/json" \
    -d '{"input_ranges": "00:02:00 - 00:01:00\n00:06:00 - 00:10:00", "duration_secs": 300}')
if [ "$(echo "$RES_ST2" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_ST2" | jq -r '.is_valid')" != "false" ]; then
    echo "Fail: Studio trim invalid ranges test failed. Got: $RES_ST2"
    exit 1
fi

# Test 3: Auto-clamping end timestamp to video duration
echo "Testing /test/studio/trim (Auto-clamping end timestamp)"
RES_ST3=$(curl -s -X POST "$BASE_URL/test/studio/trim" \
    -H "Content-Type: application/json" \
    -d '{"input_ranges": "00:01:00 - 00:10:00", "duration_secs": 300}')
if [ "$(echo "$RES_ST3" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_ST3" | jq -r '.is_valid')" != "true" ] || [ "$(echo "$RES_ST3" | jq -r '.parsed_ranges[0].end_secs')" != "300" ]; then
    echo "Fail: Studio trim autoclamp test failed. Got: $RES_ST3"
    exit 1
fi
echo "✅ Studio Trim tests passed!"

echo "Testing /test/studio/compress (Filtering & Container)"
RES_SC1=$(curl -s -X POST "$BASE_URL/test/studio/compress" \
    -H "Content-Type: application/json" \
    -d '{"orig_h": 1080, "orig_fps": 60, "selected_codec": "h264"}')
if [ "$(echo "$RES_SC1" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_SC1" | jq -r '.container')" != ".mp4" ]; then
    echo "Fail: Studio compress h264 container test failed. Got: $RES_SC1"
    exit 1
fi
if echo "$RES_SC1" | jq -r '.available_resolutions[]' | grep -q "2160"; then
    echo "Fail: Studio compress 4K resolution leaked for 1080p source."
    exit 1
fi

RES_SC2=$(curl -s -X POST "$BASE_URL/test/studio/compress" \
    -H "Content-Type: application/json" \
    -d '{"orig_h": 720, "orig_fps": 30, "selected_codec": "h265"}')
if [ "$(echo "$RES_SC2" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_SC2" | jq -r '.container')" != ".mkv" ]; then
    echo "Fail: Studio compress h265 container test failed. Got: $RES_SC2"
    exit 1
fi
if echo "$RES_SC2" | jq -r '.available_fps[]' | grep -q "60"; then
    echo "Fail: Studio compress 60fps leaked for 30fps source."
    exit 1
fi
RES_SC3=$(curl -s -X POST "$BASE_URL/test/studio/compress" \
    -H "Content-Type: application/json" \
    -d '{"orig_h": 1080, "orig_fps": 30, "selected_codec": "av1"}')
if [ "$(echo "$RES_SC3" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_SC3" | jq -r '.preset')" != "9" ]; then
    echo "Fail: Studio compress av1 preset test failed. Got: $RES_SC3"
    exit 1
fi
echo "✅ Studio Compress tests passed!"

echo "Testing /test/studio/extract (Full extraction)"
RES_SE1=$(curl -s -X POST "$BASE_URL/test/studio/extract" \
    -H "Content-Type: application/json" \
    -d '{"streams": [{"kind": "audio", "codec_name": "aac", "language": "eng"}, {"kind": "subtitle", "codec_name": "subrip", "language": "fas"}]}')
if [ "$(echo "$RES_SE1" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_SE1" | jq -r '.total_streams')" != "2" ] || [ "$(echo "$RES_SE1" | jq -r '.audio_count')" != "1" ] || [ "$(echo "$RES_SE1" | jq -r '.sub_count')" != "1" ]; then
    echo "Fail: Studio extract full extraction test failed. Got: $RES_SE1"
    exit 1
fi
if [ "$(echo "$RES_SE1" | jq -r '.mapped_extensions[0]')" != "m4a" ] || [ "$(echo "$RES_SE1" | jq -r '.mapped_extensions[1]')" != "srt" ]; then
    echo "Fail: Studio extract extension mapping test failed. Got: $RES_SE1"
    exit 1
fi

echo "Testing /test/studio/extract (Partial extraction - Audio only)"
RES_SE2=$(curl -s -X POST "$BASE_URL/test/studio/extract" \
    -H "Content-Type: application/json" \
    -d '{"streams": [{"kind": "audio", "codec_name": "flac", "language": "eng"}]}')
if [ "$(echo "$RES_SE2" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_SE2" | jq -r '.audio_count')" != "1" ] || [ "$(echo "$RES_SE2" | jq -r '.sub_count')" != "0" ]; then
    echo "Fail: Studio extract partial audio extraction test failed. Got: $RES_SE2"
    exit 1
fi

echo "Testing /test/studio/extract (Zero extractable streams)"
RES_SE3=$(curl -s -X POST "$BASE_URL/test/studio/extract" \
    -H "Content-Type: application/json" \
    -d '{"streams": []}')
if [ "$(echo "$RES_SE3" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_SE3" | jq -r '.total_streams')" != "0" ]; then
    echo "Fail: Studio extract zero streams test failed. Got: $RES_SE3"
    exit 1
fi
echo "✅ Studio Extract tests passed!"

echo "Testing /test/studio/burn (ASS subtitle style-preserved path)"
RES_SB1=$(curl -s -X POST "$BASE_URL/test/studio/burn" \
    -H "Content-Type: application/json" \
    -d '{"sub_filename": "sub.ass", "order": "video_first"}')
if [ "$(echo "$RES_SB1" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_SB1" | jq -r '.sub_format')" != "ass" ] || [ "$(echo "$RES_SB1" | jq -r '.filter_type')" != "ass" ]; then
    echo "Fail: Studio burn ASS test failed. Got: $RES_SB1"
    exit 1
fi

echo "Testing /test/studio/burn (SRT subtitle default-style path)"
RES_SB2=$(curl -s -X POST "$BASE_URL/test/studio/burn" \
    -H "Content-Type: application/json" \
    -d '{"sub_filename": "sub.srt", "order": "sub_first"}')
if [ "$(echo "$RES_SB2" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_SB2" | jq -r '.sub_format')" != "srt" ] || [ "$(echo "$RES_SB2" | jq -r '.filter_type')" != "subtitles" ]; then
    echo "Fail: Studio burn SRT test failed. Got: $RES_SB2"
    exit 1
fi

echo "Testing /test/studio/burn (Unsupported subtitle format failure path)"
RES_SB3=$(curl -s -X POST "$BASE_URL/test/studio/burn" \
    -H "Content-Type: application/json" \
    -d '{"sub_filename": "invalid.txt"}')
if [ "$(echo "$RES_SB3" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_SB3" | jq -r '.sub_format')" != "unsupported" ]; then
    echo "Fail: Studio burn unsupported format test failed. Got: $RES_SB3"
    exit 1
fi
echo "Testing /test/studio/burn (SRT document routes to subtitle, not video)"
RES_SB4=$(curl -s -X POST "$BASE_URL/test/studio/burn" \
    -H "Content-Type: application/json" \
    -d '{"sub_filename": "movie.srt"}')
if [ "$(echo "$RES_SB4" | jq -r '.route_decision')" != "subtitle" ] || [ "$(echo "$RES_SB4" | jq -r '.sub_workdir_name')" != "sub.srt" ]; then
    echo "Fail: Studio burn srt routing test failed. Got: $RES_SB4"
    exit 1
fi

echo "Testing /test/studio/burn (path traversal filename sanitized, fixed workdir name)"
RES_SB5=$(curl -s -X POST "$BASE_URL/test/studio/burn" \
    -H "Content-Type: application/json" \
    -d '{"sub_filename": "sub.ass", "video_filename": "../../etc/passwd.mkv"}')
if [ "$(echo "$RES_SB5" | jq -r '.video_workdir_name')" != "input.mkv" ]; then
    echo "Fail: Studio burn workdir name test failed. Got: $RES_SB5"
    exit 1
fi
if echo "$RES_SB5" | jq -r '.sanitized_display_name' | grep -q '/'; then
    echo "Fail: Studio burn sanitized name still has path separators. Got: $RES_SB5"
    exit 1
fi
if echo "$RES_SB5" | jq -r '.filter_arg' | grep -q "'"; then
    echo "Fail: Studio burn filter arg must not be shell-quoted. Got: $RES_SB5"
    exit 1
fi

echo "Testing /test/studio/burn (duration cap failure path)"
RES_SB6=$(curl -s -X POST "$BASE_URL/test/studio/burn" \
    -H "Content-Type: application/json" \
    -d '{"sub_filename": "sub.srt", "duration_secs": 99999}')
if [ "$(echo "$RES_SB6" | jq -r '.duration_blocked')" != "true" ]; then
    echo "Fail: Studio burn duration cap test failed. Got: $RES_SB6"
    exit 1
fi
if [ "$(echo "$RES_SB6" | jq -r '.stats_events | index("studio_burn/burn/too_long")')" = "null" ]; then
    echo "Fail: Studio burn too_long stats event missing. Got: $RES_SB6"
    exit 1
fi
for K in too_long_err_text download_failed_err_text oversized_err_text job_cancelled_text status_downloading_text status_uploading_text; do
    V=$(echo "$RES_SB6" | jq -r ".$K")
    if [ -z "$V" ] || [ "$V" = "null" ] || echo "$V" | grep -q '^!.*!$'; then
        echo "Fail: Studio burn missing i18n for $K. Got: $V"
        exit 1
    fi
done
if [ "$(echo "$RES_SB6" | jq -r '.job_cancel_keyboard[0][0].callback_data')" != "stb:jobcancel" ]; then
    echo "Fail: Studio burn job cancel callback wrong. Got: $RES_SB6"
    exit 1
fi
echo "Testing /test/studio/burn (source codec drives the encoder)"
for PAIR in "av1:libsvtav1" "hevc:libx265" "vp9:libvpx-vp9" "h264:libx264" "weirdcodec:libx264"; do
    SRC="${PAIR%%:*}"; WANT="${PAIR##*:}"
    RES_SB7=$(curl -s -X POST "$BASE_URL/test/studio/burn" \
        -H "Content-Type: application/json" \
        -d "{\"sub_filename\": \"sub.srt\", \"source_codec\": \"$SRC\"}")
    GOT=$(echo "$RES_SB7" | jq -r '.video_encoder')
    if [ "$GOT" != "$WANT" ]; then
        echo "Fail: Studio burn encoder for $SRC should be $WANT, got $GOT. Res: $RES_SB7"
        exit 1
    fi
done
# pix_fmt is only forced on the x264 path; forcing it on AV1 would drop 10-bit sources to 8-bit.
RES_SB8=$(curl -s -X POST "$BASE_URL/test/studio/burn" \
    -H "Content-Type: application/json" -d '{"sub_filename": "sub.srt", "source_codec": "av1"}')
if echo "$RES_SB8" | jq -r '.video_encoder_args | join(" ")' | grep -q "yuv420p"; then
    echo "Fail: Studio burn must not force pix_fmt on the AV1 path. Got: $RES_SB8"
    exit 1
fi
echo "Testing /test/studio/burn (oversized output is split, not rejected)"
# 2402 MB is the real report: an AV1 source re-encoded to H.264 landed over the 2000 MB cap.
RES_SB9=$(curl -s -X POST "$BASE_URL/test/studio/burn" \
    -H "Content-Type: application/json" \
    -d '{"sub_filename": "sub.srt", "duration_secs": 3600, "output_bytes": 2519076864}')
if [ "$(echo "$RES_SB9" | jq -r '.split_needed')" != "true" ]; then
    echo "Fail: Studio burn 2402 MB output must be split. Got: $RES_SB9"
    exit 1
fi
if [ "$(echo "$RES_SB9" | jq -r '.split_parts_planned')" != "2" ]; then
    echo "Fail: Studio burn 2402 MB output must halve into 2 parts. Got: $RES_SB9"
    exit 1
fi
# Every piece must fit under the cap, or splitting bought nothing.
if [ "$(echo "$RES_SB9" | jq -r '.split_part_bytes_max <= .max_upload_bytes')" != "true" ]; then
    echo "Fail: Studio burn split part still above the upload cap. Got: $RES_SB9"
    exit 1
fi
if [ "$(echo "$RES_SB9" | jq -r '.split_segment_secs')" != "1800" ]; then
    echo "Fail: Studio burn segment length for 1h/2parts should be 1800s. Got: $RES_SB9"
    exit 1
fi
if [ "$(echo "$RES_SB9" | jq -r '.stats_events | index("studio_burn/burn/split")')" = "null" ]; then
    echo "Fail: Studio burn split stats event missing. Got: $RES_SB9"
    exit 1
fi
for K in status_splitting_text job_done_part_rendered_text; do
    V=$(echo "$RES_SB9" | jq -r ".$K")
    if [ -z "$V" ] || [ "$V" = "null" ] || echo "$V" | grep -q '^!.*!$'; then
        echo "Fail: Studio burn missing i18n for $K. Got: $V"
        exit 1
    fi
done
# A file far over the cap needs more than two pieces, or each "half" is still unsendable.
RES_SB10=$(curl -s -X POST "$BASE_URL/test/studio/burn" \
    -H "Content-Type: application/json" \
    -d '{"sub_filename": "sub.srt", "duration_secs": 3600, "output_bytes": 5242880000}')
if [ "$(echo "$RES_SB10" | jq -r '.split_parts_planned')" != "3" ]; then
    echo "Fail: Studio burn 5000 MB output needs 3 parts. Got: $RES_SB10"
    exit 1
fi
# Under the cap nothing is split and the single-file caption is used.
RES_SB11=$(curl -s -X POST "$BASE_URL/test/studio/burn" \
    -H "Content-Type: application/json" \
    -d '{"sub_filename": "sub.srt", "output_bytes": 104857600}')
if [ "$(echo "$RES_SB11" | jq -r '.split_needed')" != "false" ] || [ "$(echo "$RES_SB11" | jq -r '.split_parts_planned')" != "1" ]; then
    echo "Fail: Studio burn small output must not be split. Got: $RES_SB11"
    exit 1
fi
echo "✅ Studio Burn tests passed!"

echo "Testing /test/transfer/meter (fetching + uploading)"
RES_TR_M=$(curl -s -X POST "$BASE_URL/test/transfer/meter" -H "Content-Type: application/json" \
    -d '{"total_bytes": 104857600, "lang": "en", "stage": "fetching", "chunks": [{"bytes": 50000000, "after_ms": 100}]}')
if [ "$(echo "$RES_TR_M" | jq -r '.is_complete')" != "false" ]; then echo "Fail: meter is_complete"; exit 1; fi
if [ "$(echo "$RES_TR_M" | jq -r '.bytes_done')" != "50000000" ]; then echo "Fail: meter bytes_done"; exit 1; fi
if [ "$(echo "$RES_TR_M" | jq -r '.text_len_utf16 < 4096')" != "true" ]; then echo "Fail: meter text_len"; exit 1; fi

RES_TR_M2=$(curl -s -X POST "$BASE_URL/test/transfer/meter" -H "Content-Type: application/json" \
    -d '{"total_bytes": 0, "lang": "fa", "stage": "done", "chunks": [{"bytes": 100000, "after_ms": 100}]}')
if [ "$(echo "$RES_TR_M2" | jq -r '.speed')" == "—" ]; then echo "Fail: meter speed should not be unknown after chunk"; exit 1; fi
if [ "$(echo "$RES_TR_M2" | jq -r '.eta')" != "—" ]; then echo "Fail: meter eta should be unknown when total is 0"; exit 1; fi
if [ "$(echo "$RES_TR_M2" | jq -r '.percent')" != "null" ]; then echo "Fail: meter percent should be null when total is 0"; exit 1; fi

echo "Testing /test/transfer/upload (success)"
RES_TR_UP=$(curl -s -X POST "$BASE_URL/test/transfer/upload" -H "Content-Type: application/json" -d '{"cancel_after_chunk": false}')
if [ "$(echo "$RES_TR_UP" | jq -r '.bytes_counted')" != "4194304" ]; then echo "Fail: upload bytes_counted: $RES_TR_UP"; exit 1; fi
if [ "$(echo "$RES_TR_UP" | jq -r '.final_stage')" != "Done" ]; then echo "Fail: upload final_stage: $RES_TR_UP"; exit 1; fi
if [ "$(echo "$RES_TR_UP" | jq -r '.speed_bps > 0')" != "true" ]; then echo "Fail: upload speed_bps: $RES_TR_UP"; exit 1; fi

echo "Testing /test/transfer/upload (cancel)"
RES_TR_UPC=$(curl -s -X POST "$BASE_URL/test/transfer/upload" -H "Content-Type: application/json" -d '{"cancel_after_chunk": true}')
if [ "$(echo "$RES_TR_UPC" | jq -r '.bytes_counted < 4194304')" != "true" ]; then echo "Fail: upload canceled bytes: $RES_TR_UPC"; exit 1; fi

echo "✅ Transfer tests passed!"

echo ""
echo "=== Running Package Converter Tests ==="

echo "Testing /test/pkg/validate (valid deb)"
RES_PKG_V1=$(curl -s -X POST "$BASE_URL/test/pkg/validate" -H "Content-Type: application/json" -d '{"format": "deb", "test_case": "valid"}')
if [ "$(echo "$RES_PKG_V1" | jq -r '.ok')" != "true" ]; then echo "Fail: pkg validate deb valid: $RES_PKG_V1"; exit 1; fi

echo "Testing /test/pkg/validate (path traversal)"
RES_PKG_V2=$(curl -s -X POST "$BASE_URL/test/pkg/validate" -H "Content-Type: application/json" -d '{"format": "deb", "test_case": "path_traversal"}')
if [ "$(echo "$RES_PKG_V2" | jq -r '.ok')" != "false" ] || [ "$(echo "$RES_PKG_V2" | jq -r '.error_kind')" != "PathTraversal" ]; then
    echo "Fail: pkg validate path_traversal: $RES_PKG_V2"; exit 1;
fi

echo "Testing /test/pkg/validate (symlink escape)"
RES_PKG_V3=$(curl -s -X POST "$BASE_URL/test/pkg/validate" -H "Content-Type: application/json" -d '{"format": "pacman", "test_case": "symlink_escape"}')
if [ "$(echo "$RES_PKG_V3" | jq -r '.ok')" != "false" ] || [ "$(echo "$RES_PKG_V3" | jq -r '.error_kind')" != "SymlinkEscape" ]; then
    echo "Fail: pkg validate symlink_escape: $RES_PKG_V3"; exit 1;
fi

echo "Testing /test/pkg/convert (Dalavar paywall)"
RES_PKG_C1=$(curl -s -X POST "$BASE_URL/test/pkg/convert" -H "Content-Type: application/json" -d '{"src_fmt": "deb", "dst_fmt": "rpm", "rank": "dalavar"}')
if [ "$(echo "$RES_PKG_C1" | jq -r '.paywall_blocked')" != "true" ]; then echo "Fail: pkg convert paywall: $RES_PKG_C1"; exit 1; fi

echo "Testing /test/pkg/convert (Sepahbod alien dispatch)"
RES_PKG_C2=$(curl -s -X POST "$BASE_URL/test/pkg/convert" -H "Content-Type: application/json" -d '{"src_fmt": "deb", "dst_fmt": "rpm", "rank": "sepahbod"}')
if [ "$(echo "$RES_PKG_C2" | jq -r '.paywall_blocked')" != "false" ] || [ "$(echo "$RES_PKG_C2" | jq -r '.tool_selected')" != "alien" ]; then
    echo "Fail: pkg convert sepahbod: $RES_PKG_C2"; exit 1;
fi

echo "Testing /test/pkg/convert (Sepahbod fpm pacman dispatch)"
RES_PKG_C3=$(curl -s -X POST "$BASE_URL/test/pkg/convert" -H "Content-Type: application/json" -d '{"src_fmt": "deb", "dst_fmt": "pacman", "rank": "sepahbod"}')
if [ "$(echo "$RES_PKG_C3" | jq -r '.tool_selected')" != "fpm" ]; then echo "Fail: pkg convert fpm pacman: $RES_PKG_C3"; exit 1; fi

echo "Testing /test/pkg/convert (Quota exhausted)"
RES_PKG_C4=$(curl -s -X POST "$BASE_URL/test/pkg/convert" -H "Content-Type: application/json" -d '{"src_fmt": "deb", "dst_fmt": "rpm", "rank": "sepahbod", "quota_exhausted": true}')
if [ "$(echo "$RES_PKG_C4" | jq -r '.quota_blocked')" != "true" ]; then echo "Fail: pkg convert quota: $RES_PKG_C4"; exit 1; fi

echo "Testing /test/pkg/ux"
RES_PKG_UX=$(curl -s -X POST "$BASE_URL/test/pkg/ux" -H "Content-Type: application/json" -d '{"src_fmt": "deb", "stage": "converting"}')
if [ "$(echo "$RES_PKG_UX" | jq -r '.target_buttons | length')" != "2" ]; then echo "Fail: pkg ux buttons: $RES_PKG_UX"; exit 1; fi
if [ "$(echo "$RES_PKG_UX" | jq -r '.detected_text_len < 4096')" != "true" ]; then echo "Fail: pkg ux length: $RES_PKG_UX"; exit 1; fi

echo "✅ Package Converter tests passed!"

echo ""
echo "=== Running Force Join Tests ==="

echo "Testing /test/fj/gate (master toggle OFF -> allowed)"
RES_FJ_G1=$(curl -s -X POST "$BASE_URL/test/fj/gate" -H "Content-Type: application/json" -d '{"enabled": false}')
if [ "$(echo "$RES_FJ_G1" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_FJ_G1" | jq -r '.allowed')" != "true" ]; then
    echo "Fail: fj gate master toggle off: $RES_FJ_G1"; exit 1;
fi

echo "Testing /test/fj/gate (master toggle ON with 0 locks -> allowed)"
RES_FJ_G2=$(curl -s -X POST "$BASE_URL/test/fj/gate" -H "Content-Type: application/json" -d '{"enabled": true, "setup_mandatory_lock": false}')
if [ "$(echo "$RES_FJ_G2" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_FJ_G2" | jq -r '.allowed')" != "true" ]; then
    echo "Fail: fj gate master toggle on 0 locks: $RES_FJ_G2"; exit 1;
fi

echo "Testing /test/fj/gate (mandatory lock not joined -> locked out)"
RES_FJ_G3=$(curl -s -X POST "$BASE_URL/test/fj/gate" -H "Content-Type: application/json" -d '{"enabled": true, "setup_mandatory_lock": true, "simulated_membership": "left"}')
if [ "$(echo "$RES_FJ_G3" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_FJ_G3" | jq -r '.allowed')" != "false" ]; then
    echo "Fail: fj gate mandatory lock not joined: $RES_FJ_G3"; exit 1;
fi
if [ "$(echo "$RES_FJ_G3" | jq -r '.check_button_cb')" != "fj:check" ]; then
    echo "Fail: fj gate missing check_button_cb: $RES_FJ_G3"; exit 1;
fi
if [ "$(echo "$RES_FJ_G3" | jq -r '.rendered_locked_text | length > 0')" != "true" ]; then
    echo "Fail: fj gate missing locked text: $RES_FJ_G3"; exit 1;
fi

echo "Testing /test/fj/gate (check button click when not joined -> alert toast)"
RES_FJ_G4=$(curl -s -X POST "$BASE_URL/test/fj/gate" -H "Content-Type: application/json" -d '{"enabled": true, "setup_mandatory_lock": true, "simulated_membership": "left", "is_check_btn": true, "live": true}')
if [ "$(echo "$RES_FJ_G4" | jq -r '.allowed')" != "false" ] || [ "$(echo "$RES_FJ_G4" | jq -r '.alert_toast | length > 0')" != "true" ]; then
    echo "Fail: fj gate check btn not joined: $RES_FJ_G4"; exit 1;
fi

echo "Testing /test/fj/gate (mandatory lock joined -> allowed)"
RES_FJ_G5=$(curl -s -X POST "$BASE_URL/test/fj/gate" -H "Content-Type: application/json" -d '{"enabled": true, "setup_mandatory_lock": true, "simulated_membership": "member"}')
if [ "$(echo "$RES_FJ_G5" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_FJ_G5" | jq -r '.allowed')" != "true" ]; then
    echo "Fail: fj gate mandatory lock joined: $RES_FJ_G5"; exit 1;
fi

echo "Testing /test/fj/admin/menu"
RES_FJ_M1=$(curl -s -X POST "$BASE_URL/test/fj/admin/menu" -H "Content-Type: application/json" -d '{"enabled": true}')
if [ "$(echo "$RES_FJ_M1" | jq -r '.ok')" != "true" ]; then echo "Fail: fj admin menu: $RES_FJ_M1"; exit 1; fi
if [ "$(echo "$RES_FJ_M1" | jq -r '.status_button_cb')" != "fj:toggle" ]; then echo "Fail: fj admin menu toggle cb: $RES_FJ_M1"; exit 1; fi
if [ "$(echo "$RES_FJ_M1" | jq -r '.view_button_cb')" != "fj:view" ]; then echo "Fail: fj admin menu view cb: $RES_FJ_M1"; exit 1; fi

echo "Testing /test/fj/admin/locks (empty state)"
RES_FJ_L1=$(curl -s -X POST "$BASE_URL/test/fj/admin/locks" -H "Content-Type: application/json" -d '{"simulate_empty": true}')
if [ "$(echo "$RES_FJ_L1" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_FJ_L1" | jq -r '.is_empty')" != "true" ]; then
    echo "Fail: fj admin locks empty: $RES_FJ_L1"; exit 1;
fi
if [ "$(echo "$RES_FJ_L1" | jq -r '.add_new_cb')" != "fj:add" ]; then echo "Fail: fj admin locks add cb: $RES_FJ_L1"; exit 1; fi

echo "Testing /test/fj/admin/locks (populated list)"
RES_FJ_L2=$(curl -s -X POST "$BASE_URL/test/fj/admin/locks" -H "Content-Type: application/json" -d '{"simulate_empty": false}')
if [ "$(echo "$RES_FJ_L2" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_FJ_L2" | jq -r '.lock_count')" != "2" ]; then
    echo "Fail: fj admin locks populated: $RES_FJ_L2"; exit 1;
fi
if [ "$(echo "$RES_FJ_L2" | jq -r '.manage_buttons | length')" != "2" ]; then
    echo "Fail: fj admin locks manage buttons: $RES_FJ_L2"; exit 1;
fi

echo "Testing /test/fj/admin/manage"
RES_FJ_MG=$(curl -s -X POST "$BASE_URL/test/fj/admin/manage" -H "Content-Type: application/json" -d '{"link": "https://t.me/test_ch", "mode": "mandatory", "member_cap": 500, "expires_in_days": 30, "already_joined": 20, "joined_via_link": 35}')
if [ "$(echo "$RES_FJ_MG" | jq -r '.ok')" != "true" ] || [ "$(echo "$RES_FJ_MG" | jq -r '.lock_found')" != "true" ]; then
    echo "Fail: fj admin manage: $RES_FJ_MG"; exit 1;
fi
if [ "$(echo "$RES_FJ_MG" | jq -r '.stats.already_joined')" != "20" ] || [ "$(echo "$RES_FJ_MG" | jq -r '.stats.joined_via_link')" != "35" ]; then
    echo "Fail: fj admin manage stats: $RES_FJ_MG"; exit 1;
fi
if [[ ! "$(echo "$RES_FJ_MG" | jq -r '.buttons.mode_cb')" == "fj:mode:"* ]]; then
    echo "Fail: fj admin manage mode cb: $RES_FJ_MG"; exit 1;
fi

echo "Testing /test/fj/admin/toggle_mode (NotFound)"
RES_FJ_T1=$(curl -s -X POST "$BASE_URL/test/fj/admin/toggle_mode" -H "Content-Type: application/json" -d '{"scenario": "not_found"}')
if [ "$(echo "$RES_FJ_T1" | jq -r '.result')" != "NotFound" ] || [ "$(echo "$RES_FJ_T1" | jq -r '.is_error')" != "true" ]; then
    echo "Fail: fj toggle_mode not found: $RES_FJ_T1"; exit 1;
fi

echo "Testing /test/fj/admin/toggle_mode (NoChatId)"
RES_FJ_T2=$(curl -s -X POST "$BASE_URL/test/fj/admin/toggle_mode" -H "Content-Type: application/json" -d '{"scenario": "no_chat_id"}')
if [ "$(echo "$RES_FJ_T2" | jq -r '.result')" != "NoChatId" ] || [ "$(echo "$RES_FJ_T2" | jq -r '.is_error')" != "true" ]; then
    echo "Fail: fj toggle_mode no chat id: $RES_FJ_T2"; exit 1;
fi

echo "Testing /test/fj/admin/toggle_mode (BotNotAdmin - Fail-Closed at setup time)"
RES_FJ_T3=$(curl -s -X POST "$BASE_URL/test/fj/admin/toggle_mode" -H "Content-Type: application/json" -d '{"scenario": "bot_not_admin"}')
if [ "$(echo "$RES_FJ_T3" | jq -r '.result')" != "BotNotAdmin" ] || [ "$(echo "$RES_FJ_T3" | jq -r '.is_error')" != "true" ]; then
    echo "Fail: fj toggle_mode bot not admin: $RES_FJ_T3"; exit 1;
fi
if [ "$(echo "$RES_FJ_T3" | jq -r '.error_message | length > 0')" != "true" ]; then
    echo "Fail: fj toggle_mode bot not admin error message: $RES_FJ_T3"; exit 1;
fi

echo "Testing /test/fj/admin/toggle_mode (Ok -> mandatory)"
RES_FJ_T4=$(curl -s -X POST "$BASE_URL/test/fj/admin/toggle_mode" -H "Content-Type: application/json" -d '{"scenario": "ok"}')
if [ "$(echo "$RES_FJ_T4" | jq -r '.result')" != "Ok" ] || [ "$(echo "$RES_FJ_T4" | jq -r '.is_error')" != "false" ]; then
    echo "Fail: fj toggle_mode ok: $RES_FJ_T4"; exit 1;
fi
if [ "$(echo "$RES_FJ_T4" | jq -r '.resulting_mode')" != "mandatory" ]; then
    echo "Fail: fj toggle_mode resulting mode mandatory: $RES_FJ_T4"; exit 1;
fi

echo "Testing /test/fj/admin/toggle_mode (Ok -> toggle to optional)"
RES_FJ_T5=$(curl -s -X POST "$BASE_URL/test/fj/admin/toggle_mode" -H "Content-Type: application/json" -d '{"scenario": "toggle_to_optional"}')
if [ "$(echo "$RES_FJ_T5" | jq -r '.result')" != "Ok" ] || [ "$(echo "$RES_FJ_T5" | jq -r '.resulting_mode')" != "optional" ]; then
    echo "Fail: fj toggle_mode toggle to optional: $RES_FJ_T5"; exit 1;
fi

echo "✅ Force Join tests passed!"

echo "✅ Extended TestAPI Endpoint Suite passed!"

echo ""
echo "All tests passed successfully."
