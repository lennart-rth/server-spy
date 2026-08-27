#!/usr/bin/env python3
"""Executes the shell-side steps of demo/script.json against the tmux
session used for recording (record-demo.sh). UI steps (key/keys/click_*/
sort/tab) are executed by the TUI itself via SERVER_SPY_SCRIPT and are
skipped here. The script file is the single source of truth for what the
demo recording does; tune timings and add steps by editing it.
"""
import json
import random
import re
import subprocess
import sys
import time

SHELL_ACTS = ("shell_type", "shell_enter", "wait_text", "wait_gone", "sleep")
rng = random.Random()


def tmux(*args):
    subprocess.run(["tmux", "send-keys", "-t", "rec", *args], check=False)


def type_slow(text, rate):
    rate = max(0.005, rate or 0.03)
    for ch in text:
        tmux("-l", ch)
        time.sleep(rate * (0.8 + 0.4 * rng.random()))


def pane_text():
    out = subprocess.run(
        ["tmux", "capture-pane", "-t", "rec", "-p"],
        capture_output=True,
        text=True,
    )
    return out.stdout or ""


def wait_pattern(pattern, timeout=90, absent=False):
    deadline = time.time() + timeout
    while time.time() < deadline:
        text = pane_text()
        hit = bool(re.search(pattern, text))
        if hit != absent:
            return True
        time.sleep(0.25)
    print(f"drive: pattern {pattern!r} {'gone' if absent else 'seen'} (timeout)",
          file=sys.stderr)
    return False


def main(path):
    steps = json.load(open(path))
    shell = [s for s in steps if s["act"] in SHELL_ACTS]
    if not shell:
        return
    t0 = shell[0]["t"]
    clock = 0.0
    for s in shell:
        wait = max(0.0, s["t"] - t0 - clock)
        if wait > 0:
            time.sleep(wait)
        clock += wait
        act = s["act"]
        if act == "shell_type":
            type_slow(s["text"], s.get("rate"))
        elif act == "shell_enter":
            tmux("Enter")
        elif act == "wait_text":
            wait_pattern(s["pattern"])
        elif act == "wait_gone":
            wait_pattern(s["pattern"], absent=True)
        # sleep: pacing is handled by the timestamps
    print(f"drive: {len(shell)} shell steps done")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "demo/script.json")
