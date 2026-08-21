<img src="https://img.shields.io/crates/v/server-spy" alt="crates.io version"> <img src="https://img.shields.io/github/v/release/lennart-rth/server-spy" alt="GitHub release"> <img src="https://img.shields.io/github/actions/workflow/status/lennart-rth/server-spy/ci.yml?label=ci" alt="CI status"> <img src="https://img.shields.io/badge/license-MIT-blue" alt="license">

# Server-Spy
"If you can't beat them, join them"
Measure how much of the resources your experiment actually gets, and who is stealing it from you.

On smaller academic servers where multiple people run jobs, coordination is often hard or not reliable. 
Use this tool to see exactly how much congestion was on the server while specific runs of yours were active.

[ we will embed a demo video here ]

# Who is this for
- Researchers that run time or performance critical experiments on servers with multiple users and no slurm installed.
- If you ever saw your experiment results and wondered why that one run was way slower than all the others, and you suspected that the server was busy at that moment.

# What it shows you:
- **Live congestion** - CPU, memory and I/O pressure (PSI stall metrics) and scheduler wait
- **Schedule wait for your processes** - how much of the wall time your processes were blocked on the CPU scheduler
- **Resource utilization** - how CPU and memory are split between your workers and the rest of the machine
- **RSS** - peak memory of each run
- **Experiment Runs** - one row per distinct parameter combination of your worker, with wall time, CPU time, wait, avg CPU%, peak RSS, PSI stall penalty and alive/done state
- **Top users and top processes** that lead to any congestion while YOUR experiments are supposed to run without disturbance
- **Metrics that tell exactly which of your single runs were affected how much**

# How to use
Set a simple or regex filter that is used to classify what processes are your experiment runs. For example if you have a runner script that launches multiple sub processes that run experiments with different parameters, server-spy automatically detects all parameter combinations and shows your different runs in a list, together with how the server was utilized while that specific run was active.

The flow:
- press `f` to define/update the filter
- detach the TUI (`d`)
- terminate (`q`) to quit and stop the monitoring
- save and load results (`s` / `l`)

## keys

| Key | Action |
|---|---|
| `f` | update the worker filter (popup with live preview, simple/regex) |
| `s` / `l` | save / load a snapshot via a file browser |
| `q` | **terminate** - stop the daemon and exit |
| `d` | **detach** - exit, daemon keeps recording in the background |
| `r` | restart recording (or `live` when viewing a loaded file) |
| `t` | stealth mode - rename our processes (e.g. to `htop`) so `ps`/`top` show something innocuous |


# Installation

The universal installer needs no sudo — it downloads the prebuilt static
binary into `~/.local/bin` (or `/usr/local/bin` when run as root):

```sh
curl -fsSL https://raw.githubusercontent.com/lennart-rth/server-spy/master/install.sh | sh
```

| Method | Command |
|---|---|
| Any Linux (no sudo) | `curl -fsSL https://raw.githubusercontent.com/lennart-rth/server-spy/master/install.sh \| sh` |
| crates.io | `cargo install server-spy` |
| Debian / Ubuntu (apt repo) | `curl -fsSL https://raw.githubusercontent.com/lennart-rth/server-spy/master/install-apt.sh \| sudo sh` |
| Prebuilt static binary | download `server-spy-<ver>-<arch>-unknown-linux-musl.tar.gz` from the [GitHub releases](https://github.com/lennart-rth/server-spy/releases) - works on any Linux (incl. Alpine), just unpack and run |
| Fedora / RHEL | `rpm -Uvh server-spy-<ver>-1.x86_64.rpm` (also `aarch64` on [GitHub releases](https://github.com/lennart-rth/server-spy/releases)) |
| Nix / NixOS | `nix run github:lennart-rth/server-spy` or `nix profile install github:lennart-rth/server-spy` |


# Commands
`server-spy` to start the TUI

| Command | What it does |
|---|---|
| `server-spy tui` | attach the TUI (starts the daemon if needed) |
| `server-spy start` | start the background daemon |
| `server-spy stop` | stop the daemon (collected data is discarded) |
| `server-spy dump` | print a snapshot as text and exit (`--count N` polls, `--via-daemon` reads the running daemon) |
| `server-spy daemon` | run the daemon in the foreground (debugging) |

# Options
Common options for `tui` / `start` / `daemon`:

| Option | Default | Meaning |
|---|---|---|
| `--interval SECS` | `1` | poll interval |
| `--target NAME` | empty | initial worker filter |
| `--history N` | `1800` |  for timeline plots |


