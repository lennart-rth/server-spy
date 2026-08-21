#!/bin/sh
# server-spy demo: a believable shared-server scenario.
#   - realistic experiment runs (bench_ann.py), two at a time, endlessly
#   - fluctuating antagonists disguised as common CS workloads; alice is the
#     heavy user, with periodic CPU surges that wreck unlucky runs
#   - SERVER_SPY_DEMO=1 makes the TUI show the antagonists under fake
#     usernames ("other users") and hides everything owned by the real user
set -e
cd "$(dirname "$0")"
mkdir -p /tmp/server-spy-demo
chmod 777 /tmp/server-spy-demo
ANT_ARGS=${ANT_ARGS:---cpu 24 --mem 6000 --io 1500}
SPY=${SPY:-../target/release/server-spy}

export SERVER_SPY_DEMO=1
# the experiment owner — so the filter preview shows our workers properly
export SERVER_SPY_DEMO_USER=${SERVER_SPY_DEMO_USER:-eve}

python3 antagonists.py $ANT_ARGS >/dev/null 2>&1 &
ANTS=$!
# endless stream of experiment runs, two at a time
while :; do
    ./runner.py >/dev/null 2>&1
    sleep 2
done &
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
