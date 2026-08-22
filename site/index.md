# server-spy

> "If you can't beat them, join them."

Measure how much of the resources your experiment actually gets, and who is
stealing it from you. On smaller academic servers where multiple people run
jobs, coordination is often hard or not reliable — use this tool to see
exactly how much congestion was on the server while specific runs of yours
were active.

[Install](https://lennart-rth.github.io/server-spy/install.sh) ·
[GitHub](https://github.com/lennart-rth/server-spy)

## What it shows you

- **Live congestion** — CPU, memory and I/O pressure (PSI stall metrics) and
  scheduler wait, as gauges with history sparklines
- **Schedule wait for your processes** — how much of the wall time your
  processes were blocked on the CPU scheduler
- **Resource utilization** — how CPU and memory are split between your
  workers and the rest of the machine
- **RSS** — peak memory of each run
- **Experiment Runs** — one row per distinct parameter combination of your
  worker: wall time, CPU time, wait, avg CPU%, peak RSS, PSI stall penalty,
  alive/done state, and the peak number of other active users during the run
- **Top users and top processes** that caused congestion while your
  experiments were supposed to run without disturbance
- Metrics that tell exactly which of your single runs were affected how much

## Quick start

- press `f` to define/update the worker filter (simple or regex, with a live
  preview of matching processes); confirm and recording starts
- detach with `d` — the daemon keeps recording in the background; attach
  again anytime with `server-spy tui`
- terminate with `q`
- save and load snapshots with `s` / `l` (plain CSV, file browser)

## Install

```sh
curl -fsSL https://lennart-rth.github.io/server-spy/install.sh | sh
```

Also available as an apt repository, rpm, `cargo install server-spy`, and a
Nix flake. Runs on any Linux; no runtime dependencies.
