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

echo "All tests passed successfully."
