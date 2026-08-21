#!/bin/sh
set -e
cd "$(dirname "$0")"
mkdir -p /tmp/server-spy-demo
ANT_ARGS=${ANT_ARGS:---cpu 14 --mem 14000 --io 2000}
SPY=${SPY:-../target/release/server-spy}
python3 antagonists.py $ANT_ARGS >/dev/null 2>&1 &
ANTS=$!
./runner.py >/dev/null 2>&1 &
RUNNER=$!
cleanup() {
    set +e
    "$SPY" stop >/dev/null 2>&1
    kill $ANTS $RUNNER 2>/dev/null
    sleep 0.3
    pkill -f "$(pwd)/antagonists.py" 2>/dev/null
    pkill -f "$(pwd)/worker.py" 2>/dev/null
    pkill -f "$(pwd)/runner.py" 2>/dev/null
    rm -rf /tmp/server-spy-demo
}
trap cleanup EXIT INT TERM HUP
sleep 1
"$SPY" start --target worker.py --interval 1
"$SPY" tui
