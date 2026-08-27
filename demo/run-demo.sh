#!/bin/sh
# server-spy demo: a completely fake shared-server scenario. No real process
# is spawned or monitored — scenario.json defines the workloads (with
# spikes and interference) and the experiment runs; the daemon synthesizes
# snapshots from it, so the demo is fully reproducible and tunable by hand.
set -e
cd "$(dirname "$0")"
SPY=${SPY:-../target/release/server-spy}
SCENARIO=${SCENARIO:-scenario.json}
# resolve to an absolute path: the detached daemon inherits our cwd, so a
# relative path would be looked up from the wrong directory and the daemon
# would silently exit (start then times out)
case "$SCENARIO" in
    /*) ;;
    *) SCENARIO="$(pwd)/$SCENARIO" ;;
esac

export SERVER_SPY_DEMO=1
# the experiment owner — shown as the owner of the experiment processes
export SERVER_SPY_DEMO_USER=${SERVER_SPY_DEMO_USER:-eve}
export SERVER_SPY_SCENARIO="$SCENARIO"

"$SPY" stop >/dev/null 2>&1 || true
"$SPY" start --target "" --interval 1
"$SPY" tui
