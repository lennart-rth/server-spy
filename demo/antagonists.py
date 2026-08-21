#!/usr/bin/env python3
import argparse
import os
import random
import signal
import subprocess
import sys
import time

IO_PATH = "/tmp/server-spy-demo/antagonist-io.bin"

# Fake users of the simulated shared server. alice is the heavy user (~70% of
# the load); the others only nibble at the machine.
BIG_USER = "alice"
SMALL_USERS = ["bob", "carol", "dave"]

# Realistic resource-hungry commands from the CS community. Each agent
# execs `python -c <core> <name> <args...>` so that /proc/<pid>/cmdline
# (and the "Other Processes" table) shows exactly these commands.
CPU_CMDS = [
    ("train.py", ["--model=resnet50", "--batch=64", "--epochs=100"]),
    ("train.py", ["--model=vit-base", "--batch=32", "--lr=1e-4"]),
    ("preprocess.py", ["--dataset=imagenet", "--shards=64"]),
    ("kmeans.py", ["--k=256", "--data=deep-10m", "--iters=50"]),
    ("make", ["-j16"]),
    ("gcc", ["-O2", "-march=native", "-o", "bench", "main.c"]),
    ("docker", ["build", "--no-cache", "-t", "exp-image", "."]),
    ("cargo", ["build", "--release"]),
    ("conda", ["env", "create", "-f", "env.yml"]),
    ("julia", ["-t", "8", "simulate.jl"]),
]

MEM_CMDS = [
    ("train.py", ["--model=resnet50", "--grad-accum=64", "--checkpoint=full"]),
    ("julia", ["--mem=48G", "-t", "4", "simulate.jl"]),
    ("Rscript", ["--mem=32G", "run_glm.R"]),
    ("preprocess.py", ["--dataset=imagenet", "--cache=ram"]),
]

IO_CMDS = [
    ("gzip", ["-9", "backup.sql"]),
    ("pigz", ["-9", "-p", "8", "corpus.tar"]),
    ("ffmpeg", ["-i", "input.mp4", "-c:v", "copy", "-f", "null", "-"]),
    ("tar", ["-cf", "archive.tar", "data/"]),
    ("rsync", ["-a", "--delete", "data/", "backup:data/"]),
]

# Short, intense CPU burn for the surge bursts and the fluctuating hogs.
CPU_BURN = r"""
import os, time
x = os.getpid() << 32 | 1
end = time.monotonic() + %(dur)f
while time.monotonic() < end:
    x = (x * 6364136223846793005 + 1442695040888963407) & ((1 << 64) - 1)
    x ^= x >> 33
"""

CORE_TMPL = r"""
import os, random, signal, subprocess, sys, time
rng = random.Random(os.getpid())
mode = %(mode)r
mem = %(mem)d
io_path = %(io_path)r
dur = %(dur)f
stop = time.monotonic() + dur
page = 1024 * 1024
os.makedirs(os.path.dirname(io_path), exist_ok=True)

def cleanup(signum, frame):
    try:
        os.remove(io_path)
    except OSError:
        pass
    sys.exit(0)

signal.signal(signal.SIGTERM, cleanup)
signal.signal(signal.SIGINT, cleanup)

if mode == "cpu":
    # burn hard for a while, then go quiet — so contention fluctuates
    while time.monotonic() < stop:
        burn_end = time.monotonic() + rng.uniform(8, 30)
        x = os.getpid() << 32 | 1
        while time.monotonic() < burn_end:
            x = (x * 6364136223846793005 + 1442695040888963407) & ((1 << 64) - 1)
            x ^= x >> 33
        time.sleep(rng.uniform(1, 5))

elif mode == "mem":
    # hold a chunk of RAM and churn it; occasionally drop and regrow
    chunks = []
    while sum(len(c) for c in chunks) < mem * page:
        chunks.append(bytearray(b"\xaa" * (8 * page)))
    while time.monotonic() < stop:
        c = chunks[rng.randrange(len(chunks))]
        off = rng.randrange(0, len(c) - page)
        c[off : off + page] = b"\xbb" * page
        if rng.random() < 0.04:
            for c in rng.sample(chunks, max(1, len(chunks) // 5)):
                chunks.remove(c)
            while sum(len(c) for c in chunks) < mem * page // 2:
                chunks.append(bytearray(b"\xaa" * (8 * page)))
        time.sleep(0.02)

elif mode == "io":
    # write bursts, then idle — so I/O pressure comes in waves
    data = bytearray(8 * page)
    while time.monotonic() < stop:
        burst = int(rng.uniform(200, 900)) * page
        written = 0
        with open(io_path, "wb", buffering=0) as f:
            while written < burst:
                f.write(data)
                written += len(data)
                if written %% (64 * page) == 0:
                    os.fsync(f.fileno())
        try:
            os.remove(io_path)
        except OSError:
            pass
        time.sleep(rng.uniform(3, 15))

elif mode == "surge":
    # periodically unleash a swarm of short-lived CPU hogs — the "super
    # spike" that wrecks whichever experiment run is unlucky enough to
    # be running at that moment
    while True:
        time.sleep(rng.uniform(35, 70))
        n = rng.randint(18, 26)
        for _ in range(n):
            name, args = rng.choice(CPU_CMDS)
            env = os.environ.copy()
            env["SERVER_SPY_DEMO_USER"] = rng.choice(%(surge_users)r)
            subprocess.Popen(
                [sys.executable, "-c", CPU_BURN %% {"dur": rng.uniform(12, 22)},
                 name, *args],
                env=env,
                stdin=subprocess.DEVNULL,
            )
"""

children = []


def _spawn(mode, name, args, user, mem=0):
    env = os.environ.copy()
    env["SERVER_SPY_DEMO_USER"] = user
    code = CORE_TMPL % {
        "mode": mode,
        "mem": mem,
        "io_path": IO_PATH,
        "dur": random.uniform(240, 600),
        "surge_users": SMALL_USERS,
    }
    child = subprocess.Popen(
        [sys.executable, "-c", code, name, *args],
        env=env,
        stdin=subprocess.DEVNULL,
    )
    children.append(child)


def spawn_cpu(user):
    name, args = random.choice(CPU_CMDS)
    _spawn("cpu", name, args, user)


def spawn_mem(user, mb):
    name, args = random.choice(MEM_CMDS)
    _spawn("mem", name, args, user, mem=mb)


def spawn_io(user):
    name, args = random.choice(IO_CMDS)
    _spawn("io", name, args, user)


def spawn_surge():
    name, args = random.choice(CPU_CMDS)
    _spawn("surge", name, args, SMALL_USERS[0])


def main():
    p = argparse.ArgumentParser(description="fluctuating server load antagonists")
    p.add_argument("--cpu", type=int, default=16, help="persistent CPU hogs (mostly for alice)")
    p.add_argument("--mem", type=int, default=4000, help="total MiB spread over mem agents")
    p.add_argument("--io", type=int, default=800, help="MiB per io burst")
    args = p.parse_args()

    rng = random.Random()
    # alice owns the biggest share but stays under half the processes, so
    # bob/carol/dave are clearly visible in the users list
    n_big = max(args.cpu // 2, args.cpu - 8)
    for i in range(args.cpu):
        user = BIG_USER if i < n_big else rng.choice(SMALL_USERS)
        spawn_cpu(user)
    for _ in range(rng.randint(2, 3)):
        spawn_mem(rng.choice(SMALL_USERS), args.mem // rng.randint(2, 3))
    for _ in range(rng.randint(1, 2)):
        spawn_io(rng.choice(SMALL_USERS))
    spawn_surge()

    def shutdown(signum, frame):
        for child in children:
            try:
                child.terminate()
            except OSError:
                pass
        for child in children:
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill()
        if os.path.exists(IO_PATH):
            os.remove(IO_PATH)
        sys.exit(0)

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)
    while True:
        if children and all(c.poll() is not None for c in children):
            break
        time.sleep(0.5)
    shutdown(None, None)


if __name__ == "__main__":
    main()
