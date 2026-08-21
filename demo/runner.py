#!/usr/bin/env python3
import os
import subprocess
import time

HERE = os.path.dirname(os.path.abspath(__file__))
WORKER = os.path.join(HERE, "bench_ann.py")

# (params, start delay s, duration s, mem MiB) — like a real ANN benchmark
# campaign across datasets and index configs, with some runs overlapping.
RUNS = [
    (dict(index="hnsw", M=16, ef=64, dataset="glove-100", batch=1000), 0.0, 6.0, 800),
    (dict(index="hnsw", M=32, ef=128, dataset="glove-100", batch=1000), 1.0, 8.0, 1000),
    (dict(index="hnsw", M=16, ef=256, dataset="sift-1m", batch=500), 2.5, 7.0, 900),
    (dict(index="ivf", nlist=1024, nprobe=32, dataset="deep-10m", batch=500), 4.0, 9.0, 1200),
    (dict(index="bruteforce", dataset="glove-100", batch=1000), 6.0, 6.5, 1100),
    (dict(index="ivf", nlist=4096, nprobe=16, dataset="sift-1m", batch=500), 7.5, 7.5, 1000),
    (dict(index="hnsw", M=32, ef=64, dataset="deep-10m", batch=1000), 9.0, 8.0, 1200),
    (dict(index="hnsw", M=16, ef=64, dataset="glove-100", batch=1000), 10.5, 6.0, 800),
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
