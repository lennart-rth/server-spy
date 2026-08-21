#!/bin/sh
# server-spy demo: realistic experiment runs (bench_ann.py) plus noisy
# antagonists. Run as root/sudo to simulate the antagonists running as
# another user (they appear under "Other users" in the TUI).
set -e
cd "$(dirname "$0")"
mkdir -p /tmp/server-spy-demo
chmod 777 /tmp/server-spy-demo
ANT_ARGS=${ANT_ARGS:---cpu 14 --mem 14000 --io 2000}
SPY=${SPY:-../target/release/server-spy}

if [ "$(id -u)" = "0" ]; then
    echo "running antagonists as user 'nobody' (simulates other users on the box)"
    setpriv --reuid=nobody --regid=nogroup --clear-groups \
        python3 antagonists.py $ANT_ARGS >/dev/null 2>&1 &
else
    echo "note: run with sudo so the antagonists appear as a different user"
    python3 antagonists.py $ANT_ARGS >/dev/null 2>&1 &
fi
ANTS=$!
./runner.py >/dev/null 2>&1 &
RUNNER=$!
cleanup() {
    set +e
    "$SPY" stop >/dev/null 2>&1
    kill $ANTS $RUNNER 2>/dev/null
    sleep 0.3
    pkill -f "$(pwd)/antagonists.py" 2>/dev/null
    pkill -f "$(pwd)/bench_ann.py" 2>/dev/null
    pkill -f "$(pwd)/runner.py" 2>/dev/null
    rm -rf /tmp/server-spy-demo
}
trap cleanup EXIT INT TERM HUP
sleep 1
"$SPY" start --target bench_ann --interval 1
"$SPY" tui
