# server-spy

> "If you can't beat them, join them."

Measure how much of the resources your experiment actually gets, and who is
stealing it from you. On smaller academic servers where multiple people run
jobs, coordination is often hard or not reliable — use this tool to see
exactly how much congestion was on the server while specific runs of yours
were active.

[Install](https://lennart-rth.github.io/server-spy/install.sh) ·
[GitHub](https://github.com/lennart-rth/server-spy)

## What it does

- **Auto-detect your experiment runs** — one row per distinct parameter
  combination of your worker, with the server resource conditions during each
  run and how much that run got affected
- **Easy and interpretable metrics** — SCI (a composite congestion score of
  CPU, memory and I/O pressure), CF (how many times longer a run took than on
  an empty server) and scheduler wait time — what each number means for your
  run
- **Debug what slows your experiment down** — which system component is the
  bottleneck on any given machine
- **Who is slowing you down** — which other users and processes interfered
  with your experiment the most, which specific runs got disturbed, and the
  reverse: which user or process interfered with any given experiment run
- **LaTeX paper-ready export** — statistics table and report template
  quantifying the server environment and the fairness of resources across all
  of your experiments
- **Detach &amp; reattach** — the daemon keeps recording in the background;
  close the TUI, come back later, attach again

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
