#!/usr/bin/env python3
import argparse
import os
import random
import subprocess
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WORKER = os.path.join(HERE, "bench_ann.py")

DATASETS = ["glove-100", "sift-1m", "deep-10m", "laion-10m"]
INDEXES = [
    dict(index="hnsw", M=16, ef=64),
    dict(index="hnsw", M=32, ef=128),
    dict(index="hnsw", M=16, ef=256),
    dict(index="ivf", nlist=1024, nprobe=32),
    dict(index="ivf", nlist=4096, nprobe=16),
    dict(index="bruteforce"),
]

rng = random.Random()

# 14 runs, quick (6-12 s), shuffled. Two run at a time: the next one starts
# as soon as one finishes, so runs keep cycling and 1-2 are always alive.
RUNS = []
for _ in range(14):
    params = dict(rng.choice(INDEXES))
    params["dataset"] = rng.choice(DATASETS)
    params["batch"] = rng.choice([250, 500, 1000])
    duration = rng.uniform(6, 12)
    mem = rng.choice([300, 500, 800])
    RUNS.append((params, duration, mem))
rng.shuffle(RUNS)


def spawn(params, duration, mem):
    cmd = [WORKER, "--duration", f"{duration:.1f}", "--mem", str(mem)]
    for key, value in params.items():
        cmd.append(f"--{key}={value}")
    return subprocess.Popen(cmd)


def main():
    p = argparse.ArgumentParser(description="spawn experiment runs, two at a time")
    p.add_argument("--delay", type=float, default=7.0,
                   help="seconds to wait before the first run starts")
    args = p.parse_args()

    if args.delay > 0:
        time.sleep(args.delay)

    pending = list(RUNS)
    pool = []
    while pending or pool:
        while len(pool) < 2 and pending:
            params, duration, mem = pending.pop()
            pool.append(spawn(params, duration, mem))
        time.sleep(0.5)
        pool = [p for p in pool if p.poll() is None]


if __name__ == "__main__":
    main()
