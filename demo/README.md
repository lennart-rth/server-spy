# server-spy demo

The demo is **completely fake**: no real process is spawned or monitored.
Everything the TUI shows is synthesized by the daemon from `scenario.json`
(the fake machine) and, during recordings, driven by `script.json` (what
happens on screen).

| file | role |
|---|---|
| `scenario.json` | defines the fake machine: experiment runs + interfering processes |
| `script.json` | defines what happens during a recording (typing, clicks, sorts) |
| `run-demo.sh` | interactive demo: scenario daemon + TUI, you type the filter yourself |
| `record-demo.sh` | records an asciinema cast of the scripted demo for the website |
| `drive.py` | executes the *shell* steps of `script.json` against tmux during recording |
| `../src/scenario.rs` | the engine that turns `scenario.json` into snapshots |

---

## scenario.json — the fake machine

```jsonc
{
  "interval": 1.0,          // seconds per tick (how often snapshots are produced)
  "duration": 125.0,        // end of the timeline in scenario seconds
  "cores": 64,              // machine core count (displayed in the header/stats)
  "mem_total_mb": 131072,   // machine RAM
  "target": "",             // initial filter shown in the TUI header ("" = none)

  "runs": [                 // the experiment runs (only recorded once a
    {                       //   matching filter is active in the TUI)
      "params": "bench_ann.py --index=hnsw --M=16 --ef=64 --dataset=glove-100",
      "start": 6.0,         // scenario second the run starts
      "end": 10.0,          // scenario second the run ends (3-5s each is a good length)
      "cpu_cores": 6.0,     // CPU consumed while running (cores)
      "wait_pct": 4.0,      // scheduler wait as % of the run's CPU time
      "mem_mb": 800,        // RSS
      "noise": 0.3,         // 0 = steady, 0.5 = wild random variation
      "spikes": [           // optional surges during the run
        { "at": 7.0, "len": 1.5, "wait_pct": 45.0 }  // extra wait% between at..at+len
      ],
      "interference": ["make", "matlab"]  // OPTIONAL: which processes (by comm) are
                                          // attributed to this run when it is
                                          // highlighted; 1-2 keeps the highlight
                                          // clean. Missing/empty = every active
                                          // process interferes, like the real collector.
    }
    // ... more runs
  ],

  "processes": [            // the interfering processes (Other users / Processes)
    {
      "user": "alice",      // fake username shown in the users panel
      "comm": "make",       // process name shown in the processes panel
      "cmdline": "make -j32", // command shown in the cmdline column
      "start": 0.0,         // active from
      "end": 125.0,         // active until
      "cpu_cores": 12.0,    // CPU consumed per tick (cores)
      "wait_pct": 8.0,      // scheduler wait as % of CPU
      "mem_mb": 900,        // RSS
      "noise": 0.5,         // random variation
      "spikes": [           // optional surges (extra cpu/wait/mem)
        { "at": 40.0, "len": 6.0, "cpu_cores": 30.0, "wait_pct": 55.0, "mem_mb": 1500 }
      ]
    }
    // ... more processes
  ]
}
```

Notes:

- **The filter matters.** A run is only recorded if its `params` match the
  filter that is active in the TUI. With no filter, nothing is recorded.
  Runs recorded under an old filter stay in memory; changing the filter
  hides/shows them again (they are never deleted).
- **Run count & concurrency.** To keep at most N runs running at once, make
  each run shorter than the stagger between starts. E.g. runs of ~4s started
  every 2s overlap by at most 2. The scenario itself does not enforce this;
  your `start`/`end` values decide it.
- **Timing rule of thumb.** The first run should start ~3-5 scenario seconds
  after the daemon starts, so the TUI has time to load and the filter to be
  confirmed. In `record-demo.sh` the daemon starts ~2s before the shell
  starts typing.
- `user` names are arbitrary; the "idleuser" process (all-zero load) keeps a
  present-but-idle row in the live lists.

## script.json — what happens during a recording

Each step has a timestamp `t` (seconds) and an action `act`. Timestamps are
relative to each side's own start: the first *shell* step is t=0 for the
shell side, the first *UI* step is t=0 for the TUI side. You only need one
timeline — each side ignores the other's steps.

### Shell steps (executed by `drive.py` via tmux)

```jsonc
{ "t": 0.0, "act": "shell_type", "text": "server-spy", "rate": 0.01 },
{ "t": 0.4, "act": "shell_enter" },                          // press Enter
{ "t": 0.7, "act": "wait_text", "pattern": "Experiment Filter" }, // wait until the TUI shows this text
{ "t": 0.9, "act": "wait_gone", "pattern": "Experiment Filter" }  // wait until this text disappears (TUI left)
```

- `shell_type` types `text` into the shell, one character per `rate` seconds
  (smaller = faster).
- `wait_text` polls the terminal until `pattern` appears (max 90s).
- `wait_gone` polls until `pattern` is gone — used to wait for the TUI to
  detach at the end.
- `sleep` is a no-op (timestamps already pace the steps); keep it only as a
  marker.

### UI steps (executed by the TUI itself)

```jsonc
{ "t": 2.0, "act": "key", "key": "f" },                       // single key
{ "t": 2.1, "act": "keys", "text": "bench_ann", "rate": 0.03 }, // type a string
{ "t": 2.6, "act": "key", "key": "enter" },
{ "t": 5.0, "act": "sort", "table": "runs", "col": 1 },       // click a column header
{ "t": 7.0, "act": "click_user", "name": "alice" },           // click a user row
{ "t": 12.0, "act": "click_proc", "name": "make" },           // click a process row (by comm)
{ "t": 18.0, "act": "click_run", "order": 2 },                // click an experiment run (by order)
{ "t": 23.0, "act": "key", "key": "v" },                      // toggle overall/live
{ "t": 74.0, "act": "key", "key": "d" }                       // detach (ends the TUI)
```

- `key` accepts: `enter`, `esc`, `tab`, `backspace`, `delete`, `up`,
  `down`, `left`, `right`, `home`, `end`, or any single character (`f`,
  `v`, `h`, `d`, …).
- `keys` expands into per-character `key` steps spaced by `rate`.
- `sort` clicks the header of `table` (`runs`, `users`, `ants`) at column
  index `col`. Column indices (0-based):
  - runs: 0 params, 1 congestion, 2 cpu%, 3 mem%, 4 io%, 5 wait%, 6 util%,
    7 wall, 8 usr, 9 state
  - users: 0 #, 1 user, 2 util%, 3 wait%, 4 runs, 5 share
  - ants: 0 #, 1 user, 2 comm, 3 util%, 4 wait%, 5 runs, 6 cmdline
- `click_user`/`click_proc`/`click_run` click the row of the named user /
  process / run. `click_run` takes the run's order either as `"order": 2`
  or as `"name": "2"`. Clicks are **retried until the target exists on
  screen** (up to 20s), so they work even if the timeline drifts a little.
  The list is scrolled automatically if the row is below the fold.
- Pacing rule of thumb: keep at least 0.5s between the end of one command
  and the start of the next, and use a slow `rate` (~0.15) so the typing
  looks human.
- `click` with `x`/`y` clicks absolute terminal coordinates (fragile — use
  the named clicks instead).
- `tab` clicks the `[overall]`/`[live]` tabs: `{ "act": "tab", "table":
  "users", "mode": "live" }`.

### Adding a new demo

1. Edit `scenario.json` runs/processes (keep the first run after the filter
   gets confirmed, stagger the rest).
2. Edit `script.json`: type the filter early (`key f` + `keys` + `enter`),
   then sort / click / toggle at whatever seconds you like.
3. Run `./demo/run-demo.sh` to try it interactively, or
   `./demo/record-demo.sh` to re-record the website cast.
