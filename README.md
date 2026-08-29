<img src="https://img.shields.io/crates/v/server-spy" alt="crates.io version"> <img src="https://img.shields.io/github/v/release/lennart-rth/server-spy" alt="GitHub release"> <img src="https://img.shields.io/github/actions/workflow/status/lennart-rth/server-spy/ci.yml?label=ci" alt="CI status"> <img src="https://img.shields.io/badge/license-GPLv3-blue" alt="license">

# [Server-Spy](https://lennart-rth.github.io/server-spy/)
*"If you can't beat them, join them"*

Measure how much resources your experiment actually gets, and who is stealing it from you.

On smaller academic servers where multiple people run jobs, coordination is often hard or not reliable. 
Use this tool to see exactly how much congestion was on the server while specific runs of yours were active and who is to blame for that.

## Demo

![Demo](site/demo.svg)


# Who is this for
- Researchers that run time critical experiments on servers with multiple users and no proper setup like slurm.
- If you ever saw your experiment results and wondered why that one run was way slower than all the others, and you suspected that the server was too busy at that moment.

# What it does
- **Auto-detect your experiment runs**: one row per distinct parameter combination of your worker, with the server resource conditions during that run and how much it got affected
- **Easy, interpretable metrics**: single congestion score, scheduler induced wait time and per system attribution
- **Debug what slows your experiment down**: which system component is the bottleneck on any given machine
- **Who is slowing you down**: which other users and processes interfered with your experiment the most, which specific runs got disturbed. Or, which user or process interfered with any given experiment run of yours
- **LaTeX paper-ready export**: statistics table and report template quantifying the server environment and the fairness of resources across all your experiments. REady to put in academic reports.
- **Detach & reattach**: Tmux style daemon keeps recording in the background. Close the TUI, let it monitor, attach again later.

# Metrics & scores

- **congestion index**: a composite of the CPU, memory and
  I/O pressure while your run was active, combined with
  the scheduler induced wait time. `0` = idle machine, `100` = fully saturated.
- **Scheduler wait (`wait%`)**: how long your run's processes sat in the CPU runqueue compared to their
  own work — `100%` means they waited just as long as they actually
  worked.
- **Attribution split (`cpu%` / `mem%` / `io%`)**: which resource caused
  the run's congestion.
- **Statistics pane**: distribution statistics (median, MAD, IQR, SD) of
  the congestion scores across all completed runs. To quantify how
  consistent the server conditions were.

# How to use
Set a regex filter that is used to classify what processes are your experiment runs. For example if you have a runner script that launches multiple sub processes that run experiments with different parameters, server-spy automatically detects all parameter combinations and shows your different runs in a list, together with how the server was utilized while that specific run was active.

The flow:
- press `f` to define/update the filter
- You can clik on single experiments runs to see what uses and proceses interferred with this one. and vice versa.
- detach the TUI (`d`)
- re-attach bu starting the tui again
- terminate (`q`) to quit and stop the monitoring
- save and load results (`s` / `l`)

## keys

| Key | Action |
|---|---|
| `f` | define/update the worker filter (regex; popup with live preview of matches, plus an exclude field) |
| `s` / `l` | save / load a snapshot via a file browser |
| `q` | **terminate** - stop the daemon and exit |
| `d` | **detach** - exit, daemon keeps recording in the background |
| `r` | restart recording (or `live` when viewing a loaded file) |
| `t` | stealth mode - rename our processes (e.g. to `htop`) so `ps`/`top` show something innocuous |
| `v` | toggle the users/processes lists between `overall` (accumulated) and `live` (right now) |
| `h` | help overlay - explains every metric, column and click action |
| `esc` | clear the current selection/highlight |


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


## Uninstall

```sh
server-spy stop                   # stop the background daemon
rm ~/.local/bin/server-spy        # or /usr/local/bin/server-spy when installed as root
rm -rf ~/.local/state/server-spy  # config and logs 
```

| Method | Uninstall command |
|---|---|
| Quick install (no sudo) | `rm ~/.local/bin/server-spy` and `rm -rf ~/.local/state/server-spy` |
| Quick install (as root) | `rm /usr/local/bin/server-spy` and `rm -rf /root/.local/state/server-spy` |
| crates.io | `cargo uninstall server-spy` |
| apt repo | `sudo apt remove server-spy` |
| Fedora / RHEL rpm | `sudo rpm -e server-spy` |
| Nix profile | `nix profile remove server-spy` (find the exact name with `nix profile list`) |


# Commands
`server-spy` to start or attach the TUI

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
| `--history N` | `1800` | history samples kept in memory |


