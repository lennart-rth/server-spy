#!/usr/bin/env python3
import os
import subprocess
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WORKER = os.path.join(HERE, "worker.py")

RUNS = [
    (dict(algo="hnsw", M=16, ef=64), 0.0, 5.0, 800),
    (dict(algo="hnsw", M=32, ef=64), 1.5, 7.0, 1000),
    (dict(algo="hnsw", M=16, ef=256), 3.0, 6.0, 900),
    (dict(algo="bruteforce", ef=64), 4.5, 8.0, 1200),
    (dict(algo="ivf", nlist=100, nprobe=10), 6.0, 6.5, 1100),
    (dict(algo="hnsw", M=16, ef=64), 7.5, 5.0, 800),
]

procs = []
start = time.monotonic()
for params, delay, duration, mem in RUNS:
    wait = delay - (time.monotonic() - start)
    if wait > 0:
        time.sleep(wait)
    cmd = [WORKER, "--duration", str(duration), "--mem", str(mem)]
    for key, value in params.items():
        cmd.append(f"--{key}={value}")
    procs.append(subprocess.Popen(cmd))

for p in procs:
    p.wait()
