#!/usr/bin/env python3
import argparse
import json
import os
import random
import time

OUTDIR = "/tmp/server-spy-demo"


def main():
    p = argparse.ArgumentParser(description="fake ANN benchmark run")
    p.add_argument("--index", default="hnsw", choices=["hnsw", "ivf", "bruteforce"])
    p.add_argument("--M", type=int, default=16)
    p.add_argument("--ef", type=int, default=64)
    p.add_argument("--nlist", type=int, default=1024)
    p.add_argument("--nprobe", type=int, default=16)
    p.add_argument("--dataset", default="glove-100")
    p.add_argument("--batch", type=int, default=1000)
    p.add_argument("--duration", type=float, default=5.0)
    p.add_argument("--mem", type=int, default=256)
    args = p.parse_args()

    os.makedirs(OUTDIR, exist_ok=True)
    rng = random.Random(args.M * 7919 + args.ef * 104729 + args.nlist + args.nprobe)
    size = args.mem * 1024 * 1024
    buf = bytearray(size)
    view = memoryview(buf)
    page = 1024 * 1024
    for off in range(0, size, page):
        view[off : off + page] = b"\x00" * min(page, size - off)

    chunks_per_query = 16 if args.index == "bruteforce" else 8
    io_interval = 0.3 if args.index == "ivf" else 1.0
    queries = 0
    touched = 0
    last_io = 0.0
    start = time.monotonic()
    result_path = os.path.join(OUTDIR, f"results-{args.dataset}-{args.index}.jsonl")
    result = open(result_path, "a")

    while True:
        elapsed = time.monotonic() - start
        if elapsed >= args.duration:
            break
        for _ in range(chunks_per_query):
            n = 4096
            off = rng.randrange(0, max(1, len(view) - n))
            s = 0
            for b in view[off : off + n]:
                s += b
            queries += 1
            touched += n
        if rng.random() < 0.12:
            time.sleep(rng.uniform(0.05, 0.5))
        if elapsed - last_io >= io_interval:
            payload = os.urandom(8192)
            result.write(
                json.dumps(
                    {
                        "index": args.index,
                        "M": args.M,
                        "ef": args.ef,
                        "dataset": args.dataset,
                        "payload": payload.hex()[:64],
                    }
                )
                + "\n"
            )
            result.flush()
            os.fsync(result.fileno())
            last_io = elapsed

    result.close()
    print(
        json.dumps(
            {
                "index": args.index,
                "M": args.M,
                "ef": args.ef,
                "dataset": args.dataset,
                "queries": queries,
                "touched_mb": touched / (1024 * 1024),
                "elapsed_s": round(time.monotonic() - start, 2),
            }
        )
    )


if __name__ == "__main__":
    main()
