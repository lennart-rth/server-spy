//! A completely synthetic "machine" for demo recordings: no /proc scanning,
//! no real processes. A JSON scenario file defines the experiment runs and
//! the interfering processes with start/end times, load levels, per-load
//! noise and spikes. Each poll advances a fake clock and produces a Snapshot
//! exactly like the real collector would, so the TUI and the daemon protocol
//! stay unchanged.
//!
//! Filter changes still apply: only runs matching the current rules are
//! tracked, and finished runs are retained in full but shown only while they
//! match the current filter (see `collector::params_matches_rules`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::collector::{
    Antag, Control, PsiPct, RunAnt, RunRow, RunUser, Snapshot, TargetStatus, UserShare,
    MIN_CPU_SECS, MIN_RSS_BYTES, params_matches_rules,
};
use crate::conditions::build_conditions;
use crate::metrics::{congestion_factor, congestion_loss_pct, stall_secs};
use crate::procfs::{Process, PsiFile, PsiLine, PsiSet};

/// Synthetic pids for the fake processes and experiment runs, so the TUI and
/// the filter preview see stable identities.
fn proc_pid(i: usize) -> i32 {
    1000 + i as i32
}

fn run_pid(i: usize) -> i32 {
    2000 + i as i32
}

fn def_interval() -> f64 {
    1.0
}

fn def_cores() -> u64 {
    64
}

fn def_mem() -> u64 {
    128 * 1024
}

fn def_noise() -> f64 {
    0.3
}

/// The JSON scenario file. `runs` are the experiment runs (they only get
/// recorded once a filter matching them is active), `processes` are the
/// interfering processes that show up under "Other users / Processes".
#[derive(Deserialize)]
pub struct ScenarioSpec {
    #[serde(default = "def_interval")]
    interval: f64,
    /// End of the fake timeline; the daemon keeps producing idle snapshots
    /// afterwards.
    duration: f64,
    #[serde(default = "def_cores")]
    cores: u64,
    #[serde(default = "def_mem")]
    mem_total_mb: u64,
    #[serde(default)]
    target: String,
    #[serde(default)]
    runs: Vec<RunDef>,
    #[serde(default)]
    processes: Vec<ProcDef>,
}

/// Load levels shared by experiment runs and interfering processes.
#[derive(Deserialize)]
pub struct Load {
    start: f64,
    end: f64,
    #[serde(default)]
    cpu_cores: f64,
    #[serde(default)]
    wait_pct: f64,
    #[serde(default)]
    mem_mb: u64,
    #[serde(default = "def_noise")]
    noise: f64,
    #[serde(default)]
    spikes: Vec<Spike>,
}

#[derive(Deserialize)]
pub struct RunDef {
    params: String,
    #[serde(flatten)]
    load: Load,
    /// comm names of the processes that are attributed to this run (shown
    /// as its interferers when the run is highlighted). Empty/missing means
    /// every active process interferes, like the real collector.
    #[serde(default)]
    interference: Vec<String>,
}

#[derive(Deserialize)]
pub struct ProcDef {
    user: String,
    comm: String,
    cmdline: String,
    #[serde(flatten)]
    load: Load,
}

/// A temporary surge on top of a load's baseline (added to cpu/wait/mem
/// while `t` is inside `[at, at+len)`).
#[derive(Deserialize, Clone, Copy)]
pub struct Spike {
    at: f64,
    len: f64,
    #[serde(default)]
    cpu_cores: f64,
    #[serde(default)]
    wait_pct: f64,
    #[serde(default)]
    mem_mb: u64,
}

/// Deterministic xorshift64* so repeated takes of a recording look alike.
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        Rng(0x9E37_79B9_7F4A_7C15)
    }

    fn f64(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        ((x.wrapping_mul(0x2545_F491_4F6C_DD1D)) >> 11) as f64 / (1u64 << 53) as f64
    }
}

struct LiveRun {
    def: usize,
    wall: f64,
    cpu_secs: f64,
    wait_secs: f64,
    rss: u64,
    psi_stall: [f64; 3],
    /// pid -> (user, comm, cpu_secs, rss_peak) of every interfering process
    /// seen while this run was alive.
    ants: HashMap<i32, (String, String, f64, u64)>,
    users_max: usize,
    order: u64,
}

/// Accumulated lifetime totals of one interfering process.
#[derive(Clone, Default)]
struct Accum {
    cpu: f64,
    wait: f64,
    rss: u64,
}

/// One interval of load for a load definition: baseline + active spikes,
/// with bounded noise. Returns (cpu_secs, wait_secs, rss).
fn load_activity(rng: &mut Rng, load: &Load, t: f64, interval: f64) -> (f64, f64, u64) {
    if t < load.start || t >= load.end {
        return (0.0, 0.0, 0);
    }
    let mut cpu = load.cpu_cores;
    let mut wait = load.wait_pct;
    let mut mem = load.mem_mb;
    for s in &load.spikes {
        if t >= s.at && t < s.at + s.len {
            cpu += s.cpu_cores;
            wait += s.wait_pct;
            mem += s.mem_mb;
        }
    }
    let jitter = 1.0 + load.noise * (rng.f64() * 2.0 - 1.0);
    let cpu_secs = (cpu * interval * jitter).max(0.0);
    let wait_pct = (wait + load.noise * (rng.f64() * 2.0 - 1.0) * 10.0).max(0.0);
    let wait_secs = cpu_secs * wait_pct / 100.0;
    let mem_mb = (mem as f64 * (1.0 + load.noise * 0.3 * (rng.f64() * 2.0 - 1.0))).max(0.0);
    (cpu_secs, wait_secs, (mem_mb * 1024.0 * 1024.0) as u64)
}

pub struct Scenario {
    spec: ScenarioSpec,
    control: Arc<Control>,
    my_exe: String,
    t: f64,
    seq: u64,
    generation: u64,
    order: u64,
    rng: Rng,
    live: Vec<LiveRun>,
    done: Vec<RunRow>,
    /// Run defs whose window passed without ever matching the filter: they
    /// are never recorded.
    passed: HashSet<usize>,
    accum: Vec<Accum>,
    /// Last-interval activity per process: (cpu_secs, wait_secs, rss).
    activity: Vec<(f64, f64, u64)>,
    history: VecDeque<[f64; 4]>,
    history_len: usize,
    collecting_secs: f64,
    started_at: std::time::Instant,
    procs_shared: Option<Arc<std::sync::Mutex<Vec<Process>>>>,
}

impl Scenario {
    pub fn load(path: &str, control: Arc<Control>) -> io::Result<Scenario> {
        let text = fs::read_to_string(Path::new(path))?;
        let spec: ScenarioSpec = serde_json::from_str(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{path}: {e}")))?;
        if spec.duration <= 0.0 || spec.interval <= 0.0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{path}: duration and interval must be positive"),
            ));
        }
        Ok(Scenario::new(spec, control))
    }

    pub fn new(spec: ScenarioSpec, control: Arc<Control>) -> Scenario {
        let (_, generation) = control.get();
        let accum = vec![Accum { cpu: 0.0, wait: 0.0, rss: 0 }; spec.processes.len()];
        let activity = vec![(0.0, 0.0, 0); spec.processes.len()];
        Scenario {
            spec,
            control,
            my_exe: crate::collector::exe_name(),
            t: 0.0,
            seq: 0,
            generation,
            order: 0,
            rng: Rng::new(),
            live: Vec::new(),
            done: Vec::new(),
            passed: HashSet::new(),
            accum,
            activity,
            history: VecDeque::new(),
            history_len: 1800,
            collecting_secs: 0.0,
            started_at: std::time::Instant::now(),
            procs_shared: None,
        }
    }

    pub fn set_shared_procs(&mut self, shared: Arc<std::sync::Mutex<Vec<Process>>>) {
        self.procs_shared = Some(shared);
    }

    pub fn interval(&self) -> Duration {
        Duration::from_secs_f64(self.spec.interval)
    }

    /// Builds a RunRow for a live (alive=true) or just-finished (alive=false)
/// run, mirroring `collector::build_row`.
    fn build_row(&self, r: &LiveRun, alive: bool) -> RunRow {
        let wall = r.wall.max(0.001);
        let wait_pct = if r.cpu_secs > 0.0 {
            Some(r.wait_secs / r.cpu_secs * 100.0)
        } else {
            None
        };
        let psi = [
            r.psi_stall[0] / wall * 100.0,
            r.psi_stall[1] / wall * 100.0,
            r.psi_stall[2] / wall * 100.0,
        ];
        let mem_stall = stall_secs(psi[1], wall);
        let io_stall = stall_secs(psi[2], wall);
        let cf = congestion_factor(r.cpu_secs, r.wait_secs, mem_stall, io_stall);
        let cl = congestion_loss_pct(wall, r.wait_secs, mem_stall, io_stall);
        let cpu_pct = r.cpu_secs / (wall * self.spec.cores as f64) * 100.0;
        let mut ants: Vec<RunAnt> = r
            .ants
            .iter()
            .filter(|(_, (_, _, cpu, _))| *cpu >= MIN_CPU_SECS)
            .map(|(pid, (_, comm, cpu, rss))| RunAnt {
                pid: *pid,
                comm: comm.clone(),
                cpu_secs: *cpu,
                rss: *rss,
            })
            .collect();
        ants.sort_by(|a, b| b.cpu_secs.total_cmp(&a.cpu_secs));
        // per-run users mirror the shown processes
        let mut by_user: HashMap<&str, (f64, u64, usize)> = HashMap::new();
        for (user, _, cpu, rss) in r.ants.values() {
            if *cpu < MIN_CPU_SECS {
                continue;
            }
            let e = by_user.entry(user.as_str()).or_default();
            e.0 += cpu;
            e.1 = e.1.max(*rss);
            e.2 += 1;
        }
        let mut run_users: Vec<RunUser> = by_user
            .into_iter()
            .map(|(user, (cpu_secs, rss, procs))| RunUser {
                user: user.to_string(),
                cpu_secs,
                rss,
                procs,
            })
            .collect();
        run_users.sort_by(|a, b| b.cpu_secs.total_cmp(&a.cpu_secs));
        let def = &self.spec.runs[r.def];
        RunRow {
            params: def.params.clone(),
            roots: vec![run_pid(r.def)],
            wall,
            cpu_secs: r.cpu_secs,
            wait_secs: r.wait_secs,
            wait_pct,
            cpu_pct,
            rss: r.rss,
            psi,
            alive,
            order: r.order,
            users: r.users_max,
            cf,
            cl,
            ants,
            run_users,
        }
    }

    fn close_live(&mut self) {
        let rows: Vec<RunRow> = self.live.iter().map(|r| self.build_row(r, false)).collect();
        self.live.clear();
        self.done.extend(rows);
        while self.done.len() > 1000 {
            self.done.remove(0);
        }
    }

    fn synthetic_procs(&self) -> Vec<Process> {
        let mut out: Vec<Process> = self
            .spec
            .processes
            .iter()
            .enumerate()
            .map(|(i, p)| Process {
                pid: proc_pid(i),
                ppid: 1,
                comm: p.comm.clone(),
                cmdline: p.cmdline.split_whitespace().map(String::from).collect(),
                uid: 1000,
                ticks: 0,
                rss: 0,
                start_secs: 0.0,
                demo_user: p.user.clone(),
                tty: 0,
                last_cpu: 0,
            })
            .collect();
        // live runs appear as processes too, so the filter preview counts
        // them like real experiment processes
        for (i, r) in self.live.iter().enumerate() {
            let def = &self.spec.runs[r.def];
            let comm = def
                .params
                .split_whitespace()
                .next()
                .unwrap_or("worker")
                .to_string();
            out.push(Process {
                pid: run_pid(i),
                ppid: 1,
                comm: comm.clone(),
                cmdline: def.params.split_whitespace().map(String::from).collect(),
                uid: 1000,
                ticks: 0,
                rss: 0,
                start_secs: 0.0,
                demo_user: std::env::var("SERVER_SPY_DEMO_USER").unwrap_or_default(),
                tty: 0,
                last_cpu: 0,
            });
        }
        out
    }

    pub fn poll(&mut self) -> Snapshot {
        let dt = self.spec.interval;
        self.t += dt;

        let (rules, generation) = self.control.get();
        if generation != self.generation {
            self.generation = generation;
            // filter change: close the current runs as done (retained), then
            // re-evaluate which defs to record, like the real collector does
            self.close_live();
        }

        // start runs whose def is active and that match the current filter;
        // defs whose window passes without ever matching are never recorded
        for i in 0..self.spec.runs.len() {
            if self.passed.contains(&i) || self.live.iter().any(|r| r.def == i) {
                continue;
            }
            let def = &self.spec.runs[i];
            if self.t >= def.load.end {
                self.passed.insert(i);
            } else if self.t >= def.load.start
                && params_matches_rules(&def.params, &rules, &self.my_exe)
            {
                self.order += 1;
                self.live.push(LiveRun {
                    def: i,
                    wall: 0.0,
                    cpu_secs: 0.0,
                    wait_secs: 0.0,
                    rss: 0,
                    psi_stall: [0.0; 3],
                    ants: HashMap::new(),
                    users_max: 0,
                    order: self.order,
                });
            }
        }

        // whether any experiment run is alive during this interval; the
        // "other processes/users" totals and collecting_secs only grow while
        // we are recording, exactly like the real collector — otherwise the
        // util% (cpu_secs / collecting_secs) explodes right after a run starts
        let collecting = !self.live.is_empty();
        if collecting {
            self.collecting_secs += dt;
        }
        // the cores our runs use: scales the util% of users/processes the
        // same way the real collector scopes it to the runs' cores
        let our_cores: u32 = if collecting {
            self.live
                .iter()
                .map(|r| self.spec.runs[r.def].load.cpu_cores)
                .sum::<f64>()
                .round()
                .max(1.0) as u32
        } else {
            0
        };

        // per-process activity for this interval
        for (i, p) in self.spec.processes.iter().enumerate() {
            let a = load_activity(&mut self.rng, &p.load, self.t, dt);
            self.activity[i] = a;
            if collecting {
                let acc = &mut self.accum[i];
                acc.cpu += a.0;
                acc.wait += a.1;
                acc.rss = acc.rss.max(a.2);
            }
        }

        // machine-wide pressure from every active load
        let mut used_cores = 0.0;
        let mut total_cpu = 0.0;
        let mut total_wait = 0.0;
        let mut used_mem_mb = 0.0;
        let mut run_cores = 0.0;
        let mut run_mem_mb = 0.0;
        for (cpu, wait, rss) in &self.activity {
            used_cores += cpu / dt;
            total_cpu += cpu;
            total_wait += wait;
            used_mem_mb += *rss as f64 / (1024.0 * 1024.0);
        }
        let mut run_activity = vec![(0.0, 0.0, 0u64); self.live.len()];
        for (k, r) in self.live.iter().enumerate() {
            let a = load_activity(
                &mut self.rng,
                &self.spec.runs[r.def].load,
                self.t,
                dt,
            );
            run_activity[k] = a;
            used_cores += a.0 / dt;
            total_cpu += a.0;
            run_cores += a.0 / dt;
            run_mem_mb += a.2 as f64 / (1024.0 * 1024.0);
        }
        let load_frac = used_cores / self.spec.cores.max(1) as f64;
        // pressure: a linear ramp with the load, plus a saturating term once
        // the machine starts getting over-committed, so baseline load reads
        // as moderate while spikes push the gauges up sharply
        let mut psi_cpu = load_frac * 30.0
            + 100.0 * (1.0 - (-((load_frac - 0.5).max(0.0) * 3.0)).exp());
        let mut psi_mem = (used_mem_mb / self.spec.mem_total_mb.max(1) as f64) * 100.0 * 0.9;
        let wait_avg = if total_cpu > 0.0 {
            total_wait / total_cpu * 100.0
        } else {
            0.0
        };
        let mut psi_io = 0.5 + wait_avg * 0.3;
        psi_cpu += self.rng.f64() * 1.5;
        psi_mem += self.rng.f64() * 0.5;
        psi_io += self.rng.f64() * 1.0;
        psi_cpu = psi_cpu.min(99.0);
        psi_mem = psi_mem.min(99.0);
        psi_io = psi_io.min(99.0);
        let sys_wait = if total_cpu > 0.0 {
            Some(wait_avg.min(99.0))
        } else {
            None
        };

        // update live runs
        for (k, r) in self.live.iter_mut().enumerate() {
            let (cpu, wait, mem) = run_activity[k];
            r.wall += dt;
            r.cpu_secs += cpu;
            r.wait_secs += wait;
            r.rss = r.rss.max(mem);
            r.psi_stall[0] += psi_cpu / 100.0 * dt;
            r.psi_stall[1] += psi_mem / 100.0 * dt;
            r.psi_stall[2] += psi_io / 100.0 * dt;
            // only the processes named in the run's `interference` list are
            // attributed to it (and count toward its "usr" max); without the
            // list, every active process interferes, like the real collector
            let interferers = &self.spec.runs[r.def].interference;
            let mut users: HashSet<&str> = HashSet::new();
            for (i, (pcpu, _, prss)) in self.activity.iter().enumerate() {
                if *pcpu == 0.0 && *prss == 0 {
                    continue;
                }
                let p = &self.spec.processes[i];
                if !interferers.is_empty() && !interferers.iter().any(|c| c == &p.comm) {
                    continue;
                }
                users.insert(p.user.as_str());
                let e = r
                    .ants
                    .entry(proc_pid(i))
                    .or_insert_with(|| (p.user.clone(), p.comm.clone(), 0.0, 0));
                e.2 += pcpu;
                e.3 = e.3.max(*prss);
            }
            r.users_max = r.users_max.max(users.len());
        }

        // finalize runs that ended this interval
        let ended: Vec<usize> = self
            .live
            .iter()
            .filter(|r| self.t >= self.spec.runs[r.def].load.end)
            .map(|r| r.def)
            .collect();
        let done_rows: Vec<RunRow> = self
            .live
            .iter()
            .filter(|r| ended.contains(&r.def))
            .map(|r| self.build_row(r, false))
            .collect();
        self.live.retain(|r| !ended.contains(&r.def));
        self.done.extend(done_rows);
        while self.done.len() > 1000 {
            self.done.remove(0);
        }

        // overall "other processes / users" lists (lifetime totals)
        let mut ants: Vec<Antag> = self
            .spec
            .processes
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let a = &self.accum[i];
                if a.cpu < MIN_CPU_SECS && a.rss < MIN_RSS_BYTES {
                    return None;
                }
                Some(Antag {
                    pid: proc_pid(i),
                    user: p.user.clone(),
                    comm: p.comm.clone(),
                    cmdline: p.cmdline.clone(),
                    cpu_secs: a.cpu,
                    wait_secs: a.wait,
                    rss: a.rss,
                })
            })
            .collect();
        ants.sort_by(|a, b| b.cpu_secs.total_cmp(&a.cpu_secs));
        let mut by_user: HashMap<String, (f64, f64, u64, usize)> = HashMap::new();
        for a in &ants {
            let e = by_user.entry(a.user.clone()).or_default();
            e.0 += a.cpu_secs;
            e.1 += a.wait_secs;
            e.2 += a.rss;
            e.3 += 1;
        }
        let mut users: Vec<UserShare> = by_user
            .into_iter()
            .map(|(user, (cpu_secs, wait_secs, rss, procs))| UserShare {
                user,
                cpu_secs,
                wait_secs,
                rss,
                procs,
            })
            .collect();
        users.sort_by(|a, b| b.cpu_secs.total_cmp(&a.cpu_secs));

        // live lists: every process present this interval, idle ones last
        let mut live_ants: Vec<Antag> = self
            .spec
            .processes
            .iter()
            .enumerate()
            .filter(|(_, p)| self.t >= p.load.start && self.t < p.load.end)
            .map(|(i, p)| {
                let (cpu, wait, rss) = self.activity[i];
                Antag {
                    pid: proc_pid(i),
                    user: p.user.clone(),
                    comm: p.comm.clone(),
                    cmdline: p.cmdline.clone(),
                    cpu_secs: cpu,
                    wait_secs: wait,
                    rss,
                }
            })
            .collect();
        live_ants.sort_by(|a, b| b.cpu_secs.total_cmp(&a.cpu_secs));
        let mut live_by_user: HashMap<String, (f64, f64, u64, usize)> = HashMap::new();
        for a in &live_ants {
            let e = live_by_user.entry(a.user.clone()).or_default();
            e.0 += a.cpu_secs;
            e.1 += a.wait_secs;
            e.2 = e.2.max(a.rss);
            e.3 += 1;
        }
        let mut live_users: Vec<UserShare> = live_by_user
            .into_iter()
            .map(|(user, (cpu_secs, wait_secs, rss, procs))| UserShare {
                user,
                cpu_secs,
                wait_secs,
                rss,
                procs,
            })
            .collect();
        live_users.sort_by(|a, b| b.cpu_secs.total_cmp(&a.cpu_secs));

        // visible runs: retained done runs matching the filter + live runs
        let mut rows: Vec<RunRow> = self
            .done
            .iter()
            .filter(|r| params_matches_rules(&r.params, &rules, &self.my_exe))
            .cloned()
            .collect();
        for r in &self.live {
            rows.push(self.build_row(r, true));
        }
        rows.sort_by_key(|r| (!r.alive, r.order));

        let mem_total = self.spec.mem_total_mb * 1024 * 1024;
        let used_mem = ((used_mem_mb) * 1024.0 * 1024.0) as u64;
        let cores_f = self.spec.cores.max(1) as f64;
        let share_cpu = [
            (run_cores / cores_f * 100.0).clamp(0.0, 100.0),
            ((used_cores - run_cores) / cores_f * 100.0).clamp(0.0, 100.0),
            (100.0 - used_cores / cores_f * 100.0).max(0.0),
        ];
        let mem_f = self.spec.mem_total_mb as f64;
        let share_mem = [
            (run_mem_mb / mem_f * 100.0).clamp(0.0, 100.0),
            ((used_mem_mb - run_mem_mb) / mem_f * 100.0).clamp(0.0, 100.0),
            (100.0 - used_mem_mb / mem_f * 100.0).max(0.0),
        ];

        self.seq += 1;
        let sys_wait_pct = sys_wait.unwrap_or(0.0);
        self.history.push_back([psi_cpu, psi_mem, psi_io, sys_wait_pct]);
        while self.history.len() > self.history_len {
            self.history.pop_front();
        }

        let visible_done: Vec<RunRow> = self
            .done
            .iter()
            .filter(|r| params_matches_rules(&r.params, &rules, &self.my_exe))
            .cloned()
            .collect();
        let conditions = build_conditions(&visible_done, self.spec.cores);

        if let Some(shared) = &self.procs_shared {
            *shared.lock().unwrap() = self.synthetic_procs();
        }

        let status = if !rules.iter().any(|r| !r.exclude) {
            TargetStatus::NoTarget
        } else if !self.live.is_empty() {
            TargetStatus::Active(self.live.len())
        } else if self.order > 0 {
            TargetStatus::Exited
        } else {
            TargetStatus::Searching
        };
        let psi = |v: f64| PsiFile {
            some: PsiLine {
                avg10: v,
                avg60: v,
                avg300: v,
                total: (v / 100.0 * dt * 1e6) as u64,
            },
            full: None,
        };

        Snapshot {
            seq: self.seq,
            history: self.history.iter().copied().collect(),
            target: self.spec.target.clone(),
            rules: rules.clone(),
            status,
            psi: PsiSet {
                cpu: psi(psi_cpu),
                mem: psi(psi_mem),
                io: psi(psi_io),
            },
            psi_pct: PsiPct {
                cpu_some: psi_cpu,
                mem_some: psi_mem,
                io_some: psi_io,
            },
            sys_wait,
            rss_total: used_mem,
            mem_total,
            mem_avail: mem_total.saturating_sub(used_mem),
            runs: rows,
            share_cpu,
            share_mem,
            antagonists: ants,
            users,
            live_ants,
            live_users,
            live_dt: dt,
            conditions,
            collecting,
            cores: self.spec.cores,
            our_cores,
            collecting_secs: self.collecting_secs,
            rec_secs: self.started_at.elapsed().as_secs_f64(),
            scanned: self.spec.processes.len() + self.live.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::Rule;

    fn spec() -> ScenarioSpec {
        serde_json::from_str(
            r#"{
                "duration": 60.0,
                "cores": 64,
                "mem_total_mb": 131072,
                "runs": [
                    {"params": "bench_ann.py --M=16 --dataset=glove-100", "start": 4.0, "end": 14.0, "cpu_cores": 6.0, "wait_pct": 5.0, "mem_mb": 800},
                    {"params": "bench_ann.py --M=32 --dataset=sift-1m", "start": 10.0, "end": 20.0, "cpu_cores": 8.0, "wait_pct": 10.0, "mem_mb": 1200}
                ],
                "processes": [
                    {"user": "alice", "comm": "make", "cmdline": "make -j32", "start": 0.0, "end": 60.0, "cpu_cores": 12.0, "wait_pct": 8.0, "mem_mb": 900},
                    {"user": "alice", "comm": "cc1", "cmdline": "cc1 -O2", "start": 0.0, "end": 60.0, "cpu_cores": 2.0, "wait_pct": 4.0, "mem_mb": 300},
                    {"user": "bob", "comm": "python3", "cmdline": "python3 prep.py", "start": 0.0, "end": 60.0, "cpu_cores": 3.0, "wait_pct": 8.0, "mem_mb": 2000}
                ]
            }"#,
        )
        .unwrap()
    }

    fn scen() -> Scenario {
        let c = Arc::new(Control::new(String::new()));
        c.set_rules(vec![Rule {
            pattern: "bench_ann".into(),
            regex: true,
            exclude: false,
        }]);
        Scenario::new(spec(), c)
    }

    #[test]
    fn scenario_tracks_only_filtered_runs_and_derives_users_from_procs() {
        let mut s = scen();
        let mut snap = s.poll();
        // no run may start before its start time
        assert_eq!(snap.runs.len(), 0);
        // run 1 starts at t=4
        for _ in 0..4 {
            snap = s.poll();
        }
        assert_eq!(snap.runs.len(), 1);
        assert!(snap.runs[0].alive);
        assert!(snap.runs[0].params.contains("glove-100"));
        // antagonist lists are consistent: every user has a process
        let procs: HashSet<&str> = snap.antagonists.iter().map(|a| a.user.as_str()).collect();
        let users: HashSet<&str> = snap.users.iter().map(|u| u.user.as_str()).collect();
        assert_eq!(users, procs);
        assert!(users.contains("alice"));
        assert!(users.contains("bob"));
        assert!(!snap.users.iter().any(|u| u.user.is_empty()));
        // live lists too
        let lprocs: HashSet<&str> = snap.live_ants.iter().map(|a| a.user.as_str()).collect();
        let lusers: HashSet<&str> = snap.live_users.iter().map(|u| u.user.as_str()).collect();
        assert_eq!(lusers, lprocs);
    }

    #[test]
    fn scenario_run_attributes_only_designated_interferers() {
        // run 1 only lists "make" as interferer; run 2 has no list (all)
        let spec: ScenarioSpec = serde_json::from_str(
            r#"{
                "duration": 60.0,
                "cores": 64,
                "mem_total_mb": 131072,
                "runs": [
                    {"params": "bench_ann.py --M=16", "start": 2.0, "end": 12.0, "cpu_cores": 6.0, "interference": ["make"]},
                    {"params": "bench_ann.py --M=32", "start": 14.0, "end": 24.0, "cpu_cores": 8.0}
                ],
                "processes": [
                    {"user": "alice", "comm": "make", "cmdline": "make -j32", "start": 0.0, "end": 60.0, "cpu_cores": 12.0, "mem_mb": 900},
                    {"user": "alice", "comm": "cc1", "cmdline": "cc1", "start": 0.0, "end": 60.0, "cpu_cores": 2.0, "mem_mb": 300},
                    {"user": "bob", "comm": "python3", "cmdline": "python3 prep.py", "start": 0.0, "end": 60.0, "cpu_cores": 3.0, "mem_mb": 2000}
                ]
            }"#,
        )
        .unwrap();
        let c = Arc::new(Control::new(String::new()));
        c.set_rules(vec![Rule {
            pattern: "bench_ann".into(),
            regex: true,
            exclude: false,
        }]);
        let mut s = Scenario::new(spec, c);
        for _ in 0..25 {
            s.poll();
        }
        let snap = s.poll();
        let done: Vec<&RunRow> = snap.runs.iter().filter(|r| !r.alive).collect();
        let r1 = done.iter().find(|r| r.params.contains("M=16")).unwrap();
        assert_eq!(
            r1.ants.iter().map(|a| a.comm.as_str()).collect::<Vec<_>>(),
            vec!["make"],
            "run 1 only attributes its designated interferer"
        );
        assert_eq!(r1.run_users.len(), 1);
        assert_eq!(r1.run_users[0].user, "alice");
        let r2 = done.iter().find(|r| r.params.contains("M=32")).unwrap();
        assert_eq!(
            r2.ants.len(),
            3,
            "run 2 has no interference list: all active processes count"
        );
    }

    #[test]
    fn scenario_finalizes_runs_and_retains_filtered_ones() {
        let mut s = scen();
        for _ in 0..30 {
            s.poll();
        }
        // both runs finished by t=30; both match the filter
        let snap = s.poll();
        let done: Vec<&RunRow> = snap.runs.iter().filter(|r| !r.alive).collect();
        assert_eq!(done.len(), 2, "both runs done and visible");
        for r in &done {
            assert!(r.cf.is_some());
            assert!(r.cl.is_some());
            assert!(r.wall > 9.0);
            assert!(r.cpu_secs > 0.0);
        }
        assert_eq!(snap.conditions.n, 2);
        // narrowing the filter hides the second run but keeps it retained
        s.control.set_rules(vec![Rule {
            pattern: "glove-100".into(),
            regex: true,
            exclude: false,
        }]);
        let snap = s.poll();
        assert_eq!(
            snap.runs.iter().filter(|r| !r.alive).count(),
            1,
            "non-matching done runs hidden"
        );
        assert_eq!(snap.conditions.n, 1);
        // widening it again brings the retained run back
        s.control.set_rules(vec![Rule {
            pattern: "bench_ann".into(),
            regex: true,
            exclude: false,
        }]);
        let snap = s.poll();
        assert_eq!(snap.runs.iter().filter(|r| !r.alive).count(), 2);
        assert_eq!(snap.conditions.n, 2);
    }

    #[test]
    fn scenario_snapshot_shape() {
        let mut s = scen();
        let snap = s.poll();
        assert_eq!(snap.cores, 64);
        assert_eq!(snap.scanned, 3);
        assert!(snap.mem_total > snap.mem_avail);
        assert_eq!(snap.status, TargetStatus::Searching);
        assert!(snap.share_cpu[2] >= 0.0);
    }
}
