#!/bin/sh
# Records the server-spy demo as an asciinema cast and embeds it into the
# website. The recording shows a typical server shell, someone typing
# `server-spy`, the TUI starting with no filter, typing the worker filter
# into the popup, and detaching.
#
# Usage:
#   demo/record-demo.sh [out.cast]        # out.cast defaults to site/demo.cast
#
# Env:
#   SPY=path/to/server-spy                # default ../target/release/server-spy
#   ANT_ARGS="--cpu N --mem N --io N"     # default full demo load
#   NO_EMBED=1                            # skip re-embedding into site/index.html
set -e

cd "$(dirname "$0")/.."
SPY=${SPY:-./target/release/server-spy}
OUT=${1:-site/demo.cast}
ANT_ARGS=${ANT_ARGS:---cpu 24 --mem 6000 --io 1500}
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
# the experiment owner — so the filter preview shows our workers properly
export SERVER_SPY_DEMO_USER=${SERVER_SPY_DEMO_USER:-eve}
# so the typed `server-spy` command resolves to the built binary
BIN_DIR=$(cd "$(dirname "$SPY")" && pwd)
export PATH="$BIN_DIR:$PATH"

cleanup() {
    set +e
    "$SPY" stop >/dev/null 2>&1
    tmux kill-session -t rec 2>/dev/null
    pkill -f "server-spy daemo[n]" 2>/dev/null
    pkill -f "server-spy tu[i]" 2>/dev/null
    pkill -f "antagonists.py --c[pu]" 2>/dev/null
    pkill -f "bench_ann.py --duratio[n]" 2>/dev/null
    pkill -f "runn[er].py" 2>/dev/null
    pkill -f "asciinema re[c]" 2>/dev/null
    rm -f /tmp/server-spy-*.sock
    rm -rf /tmp/server-spy-demo
}
trap cleanup EXIT INT TERM HUP

# fresh state, no target set yet
cleanup
sleep 0.5

# a fresh environment (CI) has no first-run marker yet, which would pop up
# the welcome notice and swallow the scripted keystrokes below; mark it seen
mkdir -p "$HOME/.local/state/server-spy" 2>/dev/null || true
touch "$HOME/.local/state/server-spy/first-run" 2>/dev/null || true

# start the demo scenario
setsid python3 demo/antagonists.py $ANT_ARGS </dev/null >/dev/null 2>&1 &
setsid python3 demo/runner.py </dev/null >/dev/null 2>&1 &
"$SPY" start --target "" --interval 0.5
sleep 4

# record a shell session and drive it like a human would
tmux new-session -d -s rec -x 180 -y 54 \
    "asciinema rec -i 0.15 --overwrite '$OUT' -c '$SHELL_CMD'"

type_slow() {
    string="$1"
    len=${#string}
    i=0
    while [ "$i" -lt "$len" ]; do
        tmux send-keys -t rec "$(printf '%s' "$string" | cut -c$((i+1)))"
        # POSIX-safe random 0.01-0.02 s per keystroke (no bash $RANDOM in CI)
        sleep "0.0$(($(od -An -N1 -tu1 /dev/urandom) % 2 + 1))"
        i=$((i + 1))
    done
}

# wait until the pane shows some expected text (so keystrokes are never
# swallowed by a shell/TUI that is still starting up)
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

# the command line (match any common prompt glyph: zsh/p10k, sh, root, csh)
wait_for_text '[❯$#%]' || echo "record-demo: shell prompt not seen" >&2
type_slow "server-spy"
sleep 0.1
tmux send-keys -t rec Enter
wait_for_text "Worker Filter" || echo "record-demo: TUI did not appear" >&2
sleep 0.5

# no filter yet — define it right away in the popup
tmux send-keys -t rec 'f'
sleep 0.5
type_slow "bench_ann"
sleep 1.2
tmux send-keys -t rec Enter
sleep 15

# detach back to the shell, then end the session by killing the shell —
# asciinema saves the cast, and nothing gets typed on the prompt
tmux send-keys -t rec 'd'
sleep 0.8
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
    python3 - "$OUT" << 'EOF'
import base64, re, sys
cast = sys.argv[1]
b64 = base64.b64encode(open(cast, 'rb').read()).decode()
path = 'site/index.html'
html = open(path).read()
m = re.search(r"data:application/x-asciicast;base64,[A-Za-z0-9+/=]+", html)
if not m:
    sys.exit("record-demo: no cast data URL found in site/index.html")
html = html.replace(m.group(0), f'data:application/x-asciicast;base64,{b64}')
open(path, 'w').write(html)
print(f"record-demo: embedded {len(b64)} bytes into {path}")
EOF
fi
