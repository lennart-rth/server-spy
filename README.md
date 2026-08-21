<img src="https://img.shields.io/crates/v/server-spy" alt="crates.io version">
<img src="https://img.shields.io/github/v/release/lennart-rth/server-spy" alt="GitHub release">
<img src="https://img.shields.io/github/actions/workflow/status/lennart-rth/server-spy/ci.yml?label=ci" alt="CI status">
<img src="https://img.shields.io/badge/license-MIT-blue" alt="license">

# Server-Spy
"if you can't beat them, join them"

A monitoring tool to measure resource congestion for time critical experiments, on servers that don't have slurm for "ReAsOnS".
On smaller academic servers where multiple people run jobs, coordination is often hard or not reliable. So if you suspect that your measurements got affected by someone else running stuff, use this tool to see exactly how much congestion was on the server while specific runs of yours were active.

[ we will embed a demo video here ]


# Who is this for
Researchers that run time or performance critical experiments on servers where multiple users share the box and no proper coordination is used.
If you ever saw your experiment results and wondered why that one random run was way slower than all the others, and you suspected that the server was very busy at that moment.

# What it shows you:
- **Live congestion** — CPU, memory and I/O pressure (PSI stall metrics) and scheduler wait, as gauges with history sparklines
- **Schedule wait for your processes** — how much of the wall time your processes were blocked on the CPU scheduler
- **Resource utilization** — how CPU and memory are split between your workers and the rest of the machine
- **RSS** — peak memory of each run
- **Experiment Runs** — one row per distinct parameter combination of your worker, with wall time, CPU time, wait, avg CPU%, peak RSS, PSI stall penalty and alive/done state
- **Top users and top processes** that lead to any congestion while YOUR experiments are supposed to run without disturbance
- **Metrics that tell exactly which of your single runs were affected how much**

# How to use
Set a simple or regex filter that is used to classify what processes are your experiment runs. For example if you have a runner script that launches multiple sub processes that run experiments with different parameters, server-spy automatically detects all parameter combinations and shows your different runs in a list, together with how the server was utilized while that specific run was active.

The flow:
- press `f` to define/update the filter — the popup shows a live preview of which processes match, confirm and recording starts
- detach the TUI (`d`) — the monitoring runs in a detached background daemon; starting the tool again attaches back to it to visualize
- terminate (`q`) to quit and stop the monitoring
- save and load results (`s` / `l`) — a file browser saves/loads plain CSV snapshots you can open in a spreadsheet

## keys

| Key | Action |
|---|---|
| `f` | update the worker filter (popup with live preview, simple/regex) |
| `s` / `l` | save / load a snapshot via a file browser |
| `q` | **terminate** — stop the daemon and exit |
| `d` | **detach** — exit, daemon keeps recording in the background |
| `r` | restart recording (or `live` when viewing a loaded file) |
| `t` | stealth mode — rename our processes (e.g. to `htop`) so `ps`/`top` show something innocuous |
| `j`/`k`, arrows | scroll lists |
| mouse | click headers to sort, scroll wheel to scroll, click buttons/popups |

# Installation

server-spy is a single self-contained Linux binary (no runtime dependencies). Pick whichever fits your machine:

| Method | Command |
|---|---|
| crates.io | `cargo install server-spy` |
| Prebuilt static binary | download `server-spy-<ver>-<arch>-unknown-linux-musl.tar.gz` from the [GitHub releases](https://github.com/lennart-rth/server-spy/releases) — works on any Linux (incl. Alpine), just unpack and run |
| cargo-binstall | `cargo binstall server-spy` |
| Debian / Ubuntu | `dpkg -i server-spy_<ver>_amd64.deb` |
| Fedora / RHEL | `rpm -Uvh server-spy-<ver>-1.x86_64.rpm` |
| Arch (AUR) | `paru -S server-spy` |
| Nix / NixOS | `nix run github:lennart-rth/server-spy` or `nix profile install github:lennart-rth/server-spy` |

# Commands
`server-spy tui` just starts the TUI — all the other things can be done inside it too.

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
| `--history N` | `1800` | sparkline samples (~30 min at 1 s) |



# Roadmap
- [x] automatic releases pipeline (GitHub Actions: static binaries, .deb, .rpm on every tag)
- [x] tidy up readme
- [ ] record demo video
- [ ] write github pages webpage that promotes the tool
