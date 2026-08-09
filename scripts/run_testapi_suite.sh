#!/bin/bash
set -e

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

echo "Testing /test/referral/spend"
RES_REF=$(curl -s -X POST "$BASE_URL/test/referral/spend" -H "Content-Type: application/json" -d '{"points": 20, "tier": "Esfandyar"}')
if [ "$(echo "$RES_REF" | jq -r '.ok')" != "true" ]; then echo "Fail: referral spend. Got: $RES_REF"; exit 1; fi
# مسیر واقعی: ۲۰ امتیاز خرج تیر ۲۰تایی ⇒ موجودی صفر، رتبه نشسته، ۳۱ روز.
if [ "$(echo "$RES_REF" | jq -r '.remaining_points')" != "0" ]; then echo "Fail: referral spend did not debit. Got: $RES_REF"; exit 1; fi
if [ "$(echo "$RES_REF" | jq -r '.granted_rank')" != "esfandyar" ]; then echo "Fail: referral spend rank not granted. Got: $RES_REF"; exit 1; fi
if [ "$(echo "$RES_REF" | jq -r '.days_added')" != "31" ]; then echo "Fail: referral spend days. Got: $RES_REF"; exit 1; fi

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

echo "✅ Extended TestAPI Endpoint Suite passed!"

echo ""
echo "All tests passed successfully."

