#!/usr/bin/env python3
import argparse
import os
import random
import signal
import subprocess
import sys
import time

IO_PATH = "/tmp/server-spy-demo/antagonist-io.bin"
children = []


def cpu_hog(seed):
    x = seed
    while True:
        x = (x * 6364136223846793005 + 1442695040888963407) & ((1 << 64) - 1)
        x ^= x >> 33
        x = (x * 0xFF51AFD7ED558CCD) & ((1 << 64) - 1)
        if x % 97 == 0:
            x ^= 0x9E3779B97F4A7C15


def mem_hog(mb):
    rng = random.Random(42)
    target = mb * 1024 * 1024
    chunks = []

    def alloc_more():
        while sum(len(c) for c in chunks) < target:
            chunks.append(bytearray(b"\xaa" * (8 * 1024 * 1024)))

    alloc_more()
    while True:
        time.sleep(0.05)
        chunk = chunks[rng.randrange(len(chunks))]
        off = rng.randrange(0, len(chunk) - 1024 * 1024)
        chunk[off : off + 1024 * 1024] = b"\xbb" * (1024 * 1024)
        if rng.random() < 0.02:
            dropped = rng.sample(chunks, max(1, len(chunks) // 8))
            for c in dropped:
                chunks.remove(c)
            alloc_more()


def io_hog(mb):
    os.makedirs(os.path.dirname(IO_PATH), exist_ok=True)

    def cleanup(signum, frame):
        if os.path.exists(IO_PATH):
            os.remove(IO_PATH)
        sys.exit(0)

    signal.signal(signal.SIGTERM, cleanup)
    signal.signal(signal.SIGINT, cleanup)
    data = bytearray(8 * 1024 * 1024)
    while True:
        written = 0
        with open(IO_PATH, "wb", buffering=0) as f:
            while written < mb * 1024 * 1024:
                f.write(data)
                written += len(data)
                if written % (64 * 1024 * 1024) == 0:
                    os.fsync(f.fileno())
                time.sleep(0.005)
        os.remove(IO_PATH)


ROLES = {"cpu": cpu_hog, "mem": mem_hog, "io": io_hog}


def spawn(role, arg):
    child = subprocess.Popen(
        [sys.executable, os.path.abspath(__file__), "--role", role, "--arg", str(arg)]
    )
    children.append(child)
    return child


def main():
    parser = argparse.ArgumentParser(description="server load antagonists")
    parser.add_argument("--cpu", type=int, default=6)
    parser.add_argument("--mem", type=int, default=1500)
    parser.add_argument("--io", type=int, default=400)
    parser.add_argument("--role", choices=list(ROLES))
    parser.add_argument("--arg", type=str)
    args = parser.parse_args()

    if args.role:
        ROLES[args.role](int(args.arg))
        return

    for i in range(args.cpu):
        spawn("cpu", i)
    spawn("mem", args.mem)
    spawn("io", args.io)

    def shutdown(signum, frame):
        for child in children:
            child.terminate()
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
        if any(c.poll() is not None for c in children):
            break
        time.sleep(0.5)
    shutdown(None, None)


if __name__ == "__main__":
    main()
