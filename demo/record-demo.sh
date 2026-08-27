#!/bin/sh
# Records the server-spy demo as an asciinema cast and embeds it into the
# website. The whole demo is completely fake: no real process is monitored,
# demo/scenario.json feeds synthetic snapshots (workloads, interference,
# spikes) and demo/script.json defines what happens during the recording —
# the shell typing, the filter edits, the sorts and the clicks. The shell
# steps are driven here via tmux (demo/drive.py); the TUI executes the UI
# steps itself (SERVER_SPY_SCRIPT), so every interaction is reproducible and
# tunable by hand.
#
# Usage:
#   demo/record-demo.sh [out.cast]        # out.cast defaults to site/demo.cast
#
# Env:
#   SPY=path/to/server-spy                # default ../target/release/server-spy
#   SCRIPT=demo/script.json               # the recording script
#   SCENARIO=demo/scenario.json           # the fake system definition
#   NO_EMBED=1                            # skip re-embedding into site/index.html
#                                         # and regenerating site/demo.svg
#
# Regenerating site/demo.svg needs svg-term-cli (npm install -g svg-term-cli).
set -e

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
SITE_DIR="$ROOT/site"
SPY=${SPY:-./target/release/server-spy}
OUT=${1:-"$SITE_DIR/demo.cast"}
SCRIPT=${SCRIPT:-demo/script.json}
SCENARIO=${SCENARIO:-demo/scenario.json}
PS1_PROMPT=${PS1_PROMPT:-"$ "}

for cmd in tmux asciinema python3; do
    command -v "$cmd" >/dev/null 2>&1 || { echo "record-demo: '$cmd' not found" >&2; exit 1; }
done

# dash exits silently on EOF (no "exit" echoed) and shows the custom PS1 via
# the POSIX ENV startup file; bash falls back but echoes "exit" on EOF
if command -v dash >/dev/null 2>&1; then
    SHELL_CMD=dash
    printf 'PS1="%s"\n' "$PS1_PROMPT" > /tmp/server-spy-dashrc
    export ENV=/tmp/server-spy-dashrc
else
    SHELL_CMD=bash
    export PS1="$PS1_PROMPT"
fi

export SERVER_SPY_DEMO=1
# the experiment owner — shown as the owner of the experiment processes
export SERVER_SPY_DEMO_USER=${SERVER_SPY_DEMO_USER:-eve}
# completely fake: the daemon synthesizes snapshots from the scenario file
export SERVER_SPY_SCENARIO="$SCENARIO"
# the TUI replays the UI steps of the recording script by itself
export SERVER_SPY_SCRIPT="$SCRIPT"
# so the typed `server-spy` command resolves to the built binary
BIN_DIR=$(cd "$(dirname "$SPY")" && pwd)
export PATH="$BIN_DIR:$PATH"
# svg-term-cli is usually installed via `npm install -g svg-term-cli`
export PATH="$HOME/.npm-global/bin:$PATH"

cleanup() {
    set +e
    "$SPY" stop >/dev/null 2>&1
    tmux kill-session -t rec 2>/dev/null
    pkill -f "server-spy daemo[n]" 2>/dev/null
    pkill -f "server-spy tu[i]" 2>/dev/null
    pkill -f "asciinema re[c]" 2>/dev/null
    rm -f /tmp/server-spy-*.sock
}
trap cleanup EXIT INT TERM HUP

# fresh state
cleanup
sleep 0.5

# start the fake scenario daemon
"$SPY" start --target "" --interval 1
sleep 1

# record a shell session and drive it like a human would
tmux new-session -d -s rec -x 180 -y 54 \
    "asciinema rec -i 0.15 --overwrite '$OUT' -c '$SHELL_CMD'"

wait_for_text() {
    pattern="$1"
    i=0
    while [ "$i" -lt 40 ]; do
        if tmux capture-pane -t rec -p 2>/dev/null | grep -qE "$pattern"; then
            return 0
        fi
        sleep 0.25
        i=$((i + 1))
    done
    return 1
}

# wait for the shell prompt, then run the recording script: drive.py handles
# the shell steps (typing `server-spy`, entering, waiting for the TUI), the
# TUI handles the UI steps and detaches itself; drive.py's final wait_gone
# step waits until the TUI has left the pane
wait_for_text '[❯$#%]' || echo "record-demo: shell prompt not seen" >&2
python3 demo/drive.py "$SCRIPT"
sleep 1

# end the session by killing the pane's shell — asciinema saves the cast
PANEPID=$(tmux list-panes -t rec -F '#{pane_pid}' 2>/dev/null | head -1)
PID=$PANEPID
i=0
while [ -n "$PID" ] && [ "$i" -lt 5 ]; do
    CHILD=$(pgrep -P "$PID" 2>/dev/null | head -1)
    [ -n "$CHILD" ] || break
    PID=$CHILD
    i=$((i + 1))
done
[ -n "$PID" ] && kill "$PID" 2>/dev/null
sleep 1

[ -f "$OUT" ] || { echo "record-demo: no cast produced" >&2; exit 1; }
echo "record-demo: recorded $OUT"

if [ -z "$NO_EMBED" ]; then
    python3 - "$OUT" "$SITE_DIR" << 'EOF'
import base64, os, re, sys
cast = sys.argv[1]
b64 = base64.b64encode(open(cast, 'rb').read()).decode()
path = os.path.join(sys.argv[2], "index.html")
html = open(path).read()
m = re.search(r"data:application/x-asciicast;base64,[A-Za-z0-9+/=]+", html)
if not m:
    sys.exit("record-demo: no cast data URL found in site/index.html")
html = html.replace(m.group(0), f'data:application/x-asciicast;base64,{b64}')
open(path, 'w').write(html)
print(f"record-demo: embedded {len(b64)} bytes into {path}")
EOF
    # keep the README's demo.svg in sync with the freshly recorded cast:
    # svg-term-cli (the tool that always rendered the demo) only reads
    # asciicast v1/v2, so rewrite the v3 delta timestamps to absolute ones
    python3 - "$OUT" /tmp/server-spy-demo-v2.cast << 'EOF'
import json, sys
src, dst = sys.argv[1], sys.argv[2]
with open(src) as f:
    header = json.loads(f.readline())
    events = [json.loads(l) for l in f if l.strip()]
out = [{"version": 2,
        "width": header.get("term", {}).get("cols") or header.get("width"),
        "height": header.get("term", {}).get("rows") or header.get("height")}]
monotonic = all(e[0] >= events[i - 1][0] for i, e in enumerate(events) if i)
acc = 0.0
for t, k, d in events:
    acc += max(0.0, t)
    out.append([acc if not monotonic else t, k, d])
with open(dst, "w") as f:
    for e in out:
        f.write(json.dumps(e) + "\n")
print(f"record-demo: cast rewritten to v2 for svg-term ({len(out) - 1} events)")
EOF
    svg-term --in /tmp/server-spy-demo-v2.cast --out "$SITE_DIR/demo.svg" \
        --width 180 --height 54 --no-cursor
    echo "record-demo: regenerated $SITE_DIR/demo.svg"
fi
