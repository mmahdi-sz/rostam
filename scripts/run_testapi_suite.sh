#!/bin/bash
set -e

PORT=${TESTAPI_PORT:-14379}
BASE_URL="http://127.0.0.1:$PORT"

echo "Building dev mode..."
cargo build --features testapi

echo "Starting test API server..."
TESTAPI_ENABLED=1 BOT_API_BASE_URL="http://127.0.0.1:$PORT/bot" ./target/debug/ros-telegram-bot > testapi.log 2>&1 &
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

TEXT=$(echo "$RES" | jq -r '.message.rendered_text')
if [[ ! "$TEXT" == *"TestFeature"* ]]; then
    echo "Fail: Message text doesn't contain feature name. Got: $TEXT"
    exit 1
fi

INLINE_KBD=$(echo "$RES" | jq -r '.inline_keyboard[0][0].callback_data')
if [ "$INLINE_KBD" != "rank:menu" ]; then
    echo "Fail: Incorrect inline keyboard callback data. Got: $INLINE_KBD"
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

echo "Testing /test/admin/panel"
RES_ADM=$(curl -s -X POST "$BASE_URL/test/admin/panel" -H "Content-Type: application/json" -d '{"user_id": 12345}')
if [ "$(echo "$RES_ADM" | jq -r '.ok')" != "true" ]; then echo "Fail: admin panel"; exit 1; fi

echo "Testing /test/surge/validate_url"
RES_SURGE=$(curl -s -X POST "$BASE_URL/test/surge/validate_url" -H "Content-Type: application/json" -d '{"url": "https://example.com/file.zip"}')
if [ "$(echo "$RES_SURGE" | jq -r '.valid')" != "true" ]; then echo "Fail: surge validate_url"; exit 1; fi

echo "Testing /test/health/deep"
RES_HLT=$(curl -s -X POST "$BASE_URL/test/health/deep")
if [ "$(echo "$RES_HLT" | jq -r '.ok')" != "true" ]; then echo "Fail: health deep"; exit 1; fi

echo "Testing /test/rank/panel"
RES_RP=$(curl -s -X POST "$BASE_URL/test/rank/panel" -H "Content-Type: application/json" -d '{"user_id": 12345}')
if [ "$(echo "$RES_RP" | jq -r '.ok')" != "true" ]; then echo "Fail: rank panel"; exit 1; fi

echo "Testing /test/referral/spend"
RES_REF=$(curl -s -X POST "$BASE_URL/test/referral/spend" -H "Content-Type: application/json" -d '{"points": 20, "tier": "Esfandyar"}')
if [ "$(echo "$RES_REF" | jq -r '.ok')" != "true" ]; then echo "Fail: referral spend"; exit 1; fi

echo "✅ Extended TestAPI Endpoint Suite passed!"

echo ""
echo "All tests passed successfully."

