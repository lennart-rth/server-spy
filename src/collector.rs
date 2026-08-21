use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::metrics::{cpu_pct, psi_penalty_pct, wait_overhead_pct};
use crate::procfs::{self, Process, PsiFile, PsiSet, SysInfo};

pub struct Control {
    target: Mutex<String>,
    regex: AtomicBool,
    stealth: Mutex<String>,
    peer: AtomicI32,
    generation: AtomicU64,
}

impl Control {
    pub fn new(name: String) -> Self {
        Self {
            target: Mutex::new(name),
            regex: AtomicBool::new(false),
            stealth: Mutex::new(String::new()),
            peer: AtomicI32::new(-1),
            generation: AtomicU64::new(1),
        }
    }

    pub fn set(&self, name: String, regex: bool) {
        *self.target.lock().unwrap() = name;
        self.regex.store(regex, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn get(&self) -> (String, bool, u64) {
        let name = self.target.lock().unwrap().clone();
        (
            name,
            self.regex.load(Ordering::SeqCst),
            self.generation.load(Ordering::SeqCst),
        )
    }

    pub fn set_stealth(&self, name: String) {
        *self.stealth.lock().unwrap() = name;
    }

    pub fn get_stealth(&self) -> String {
        self.stealth.lock().unwrap().clone()
    }

    pub fn set_peer(&self, pid: i32) {
        self.peer.store(pid, Ordering::SeqCst);
    }

    pub fn get_peer(&self) -> i32 {
        self.peer.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetStatus {
    NoTarget,
    Searching,
    Active(usize),
    Exited,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PsiPct {
    pub cpu_some: f64,
    pub mem_some: f64,
    pub io_some: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRow {
    pub params: String,
    pub roots: Vec<i32>,
    pub wall: f64,
    pub cpu_secs: f64,
    pub wait_secs: f64,
    pub wait_pct: Option<f64>,
    pub cpu_pct: f64,
    pub rss: u64,
    pub psi: [f64; 3],
    pub alive: bool,
    pub order: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Antag {
    pub pid: i32,
    pub user: String,
    pub comm: String,
    pub cmdline: String,
    pub cpu_secs: f64,
    pub wait_secs: f64,
    pub rss: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserShare {
    pub user: String,
    pub cpu_secs: f64,
    pub wait_secs: f64,
    pub rss: u64,
    pub procs: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchInfo {
    pub pid: i32,
    pub user: String,
    pub comm: String,
    pub cmdline: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub seq: u64,
    pub history: Vec<[f64; 4]>,
    pub target: String,
    pub status: TargetStatus,
    pub psi: PsiSet,
    pub psi_pct: PsiPct,
    pub sys_wait: Option<f64>,
    pub rss_total: u64,
    pub mem_total: u64,
    pub mem_avail: u64,
    pub runs: Vec<RunRow>,
    pub share_cpu: [f64; 3],
    pub share_mem: [f64; 3],
    pub antagonists: Vec<Antag>,
    pub users: Vec<UserShare>,
    pub collecting: bool,
    pub cores: u64,
    pub collecting_secs: f64,
    pub rec_secs: f64,
    pub scanned: usize,
}

#[derive(Debug, Clone, Copy)]
struct PidLast {
    ticks: u64,
    sched: Option<(u64, u64)>,
}

struct PidAccum {
    uid: u32,
    user: String,
    comm: String,
    cmdline: String,
    owned: bool,
    ticks: u64,
    wait_ns: u64,
    rss_peak: u64,
}

struct RunState {
    params: String,
    roots: HashSet<i32>,
    start: f64,
    last_seen: f64,
    psi_base: [u64; 3],
    cpu_ns: u64,
    wait_ns: u64,
    ticks: u64,
    rss: u64,
    order: u64,
}

pub struct Collector {
    pub interval: Duration,
    pub sys: SysInfo,
    pub seq: u64,
    control: Arc<Control>,
    my_exe: String,
    boot: f64,
    generation: u64,
    last_tick: Instant,
    pid_last: HashMap<i32, PidLast>,
    pid_accum: HashMap<i32, PidAccum>,
    runs: HashMap<String, RunState>,
    finalized: Vec<RunRow>,
    history: VecDeque<[f64; 4]>,
    history_len: usize,
    run_order: u64,
    psi_last: Option<PsiSet>,
    g_sched_last: Option<HashMap<i32, (u64, u64)>>,
    had_parents: bool,
    collecting_secs: f64,
    started: Instant,
    shared_procs: Option<Arc<Mutex<Vec<Process>>>>,
}

impl Collector {
    pub fn new(interval: Duration, control: Arc<Control>, history_len: usize) -> Self {
        let sys = SysInfo::detect();
        let boot = procfs::boot_secs();
        Self {
            interval,
            sys,
            seq: 0,
            control,
            my_exe: exe_name(),
            boot,
            generation: 0,
            last_tick: Instant::now(),
            pid_last: HashMap::new(),
            pid_accum: HashMap::new(),
            runs: HashMap::new(),
            finalized: Vec::new(),
            history: VecDeque::new(),
            history_len,
            run_order: 0,
            psi_last: None,
            g_sched_last: None,
            had_parents: false,
            collecting_secs: 0.0,
            started: Instant::now(),
            shared_procs: None,
        }
    }

    pub fn set_shared_procs(&mut self, shared: Arc<Mutex<Vec<Process>>>) {
        self.shared_procs = Some(shared);
    }

    pub fn poll(&mut self) -> Snapshot {
        let tick = Instant::now();
        let dt = (tick - self.last_tick).as_secs_f64();
        self.last_tick = tick;
        let now_wall = epoch_now();

        let (name, is_regex, generation) = self.control.get();
        if generation != self.generation {
            self.generation = generation;
            self.pid_last.clear();
            self.runs.clear();
            self.finalized.clear();
            self.g_sched_last = None;
            self.had_parents = false;
        }

        let psi = procfs::read_psi();
        let (mem_total, mem_avail) = procfs::meminfo();
        let procs = procfs::scan_processes(&self.sys, self.boot);
        if let Some(shared) = &self.shared_procs {
            *shared.lock().unwrap() = procs.clone();
        }
        let stealth = self.control.get_stealth();
        let peer = self.control.get_peer();
        let self_pid = std::process::id() as i32;
        let vis: Vec<&Process> = procs
            .iter()
            .filter(|p| {
                p.pid != self_pid
                    && p.pid != peer
                    && !is_stealth(p, &stealth)
            })
            .collect();
        let by_pid: HashMap<i32, &Process> = procs.iter().map(|p| (p.pid, p)).collect();

        let mut adjacency: HashMap<i32, Vec<i32>> = HashMap::new();
        for p in &procs {
            adjacency.entry(p.ppid).or_default().push(p.pid);
        }

        let mut tree: HashSet<i32> = HashSet::new();
        let matched = if name.is_empty() {
            HashSet::new()
        } else {
            let raw: HashSet<i32> = if is_regex {
                match regex::Regex::new(&name) {
                    Ok(re) => vis
                        .iter()
                        .filter(|p| matches_regex(p, &re, &self.my_exe))
                        .map(|p| p.pid)
                        .collect(),
                    Err(_) => HashSet::new(),
                }
            } else {
                vis.iter()
                    .filter(|p| matches_name(p, &name, &self.my_exe))
                    .map(|p| p.pid)
                    .collect()
            };
            let mut leaves = raw.clone();
            for &p in &raw {
                let mut stack: Vec<i32> = adjacency.get(&p).cloned().unwrap_or_default();
                let mut seen: HashSet<i32> = HashSet::from([p]);
                let mut has_desc = false;
                while let Some(c) = stack.pop() {
                    if raw.contains(&c) {
                        has_desc = true;
                        break;
                    }
                    if seen.insert(c)
                        && let Some(ch) = adjacency.get(&c)
                    {
                        stack.extend(ch.iter().copied());
                    }
                }
                if has_desc {
                    leaves.remove(&p);
                }
            }
            leaves
        };
        let mut stack: Vec<i32> = matched.iter().copied().collect();
        while let Some(pid) = stack.pop() {
            if tree.insert(pid)
                && let Some(ch) = adjacency.get(&pid)
            {
                stack.extend(ch.iter().copied());
            }
        }
        let roots: Vec<i32> = matched.iter().copied().collect();

        let root_set: HashSet<i32> = roots.iter().copied().collect();
        let mut owner: HashMap<i32, i32> = HashMap::new();
        let mut queue: VecDeque<i32> = VecDeque::new();
        for &r in &roots {
            owner.insert(r, r);
            queue.push_back(r);
        }
        while let Some(p) = queue.pop_front() {
            let o = owner[&p];
            if let Some(ch) = adjacency.get(&p) {
                for &c in ch {
                    if root_set.contains(&c) {
                        continue;
                    }
                    if let std::collections::hash_map::Entry::Vacant(e) = owner.entry(c) {
                        e.insert(o);
                        queue.push_back(c);
                    }
                }
            }
        }

        let mut sched_map: HashMap<i32, (u64, u64)> = HashMap::new();
        if !tree.is_empty() {
            for pid in &tree {
                if let Some(s) = procfs::read_schedstat_sum(*pid) {
                    sched_map.insert(*pid, s);
                }
            }
        }

        let mut by_key: HashMap<String, Vec<i32>> = HashMap::new();
        for &r in &roots {
            let p = by_pid[&r];
            by_key.entry(cmdline_key(p)).or_default().push(r);
        }
        for entry in self.runs.values_mut() {
            entry.roots.retain(|r| by_pid.contains_key(r));
            let mut rss = 0u64;
            for (pid, o) in &owner {
                if !entry.roots.contains(o) {
                    continue;
                }
                let p = match by_pid.get(pid) {
                    Some(p) => p,
                    None => continue,
                };
                entry.ticks += delta_ticks(&self.pid_last, *pid, p.ticks);
                if let Some((cpu, wait)) = sched_map.get(pid) {
                    let (pcpu, pwait) = self
                        .pid_last
                        .get(pid)
                        .and_then(|l| l.sched)
                        .unwrap_or((0, 0));
                    entry.cpu_ns += cpu.saturating_sub(pcpu);
                    entry.wait_ns += wait.saturating_sub(pwait);
                }
                rss += p.rss;
            }
            entry.rss = entry.rss.max(rss);
            if entry.roots.is_empty() {
                let wall = (entry.last_seen - entry.start).max(0.0);
                let row = build_row(entry, wall, &psi, &self.sys);
                self.finalized.push(row);
                if self.finalized.len() > 1000 {
                    self.finalized.remove(0);
                }
            } else {
                entry.last_seen = now_wall;
            }
        }
        self.runs.retain(|_, e| !e.roots.is_empty());

        for (key, new_roots) in by_key {
            if !self.runs.contains_key(&key) {
                let start = new_roots
                    .iter()
                    .map(|r| by_pid[r].start_secs)
                    .fold(f64::INFINITY, f64::min);
                self.run_order += 1;
                self.runs.insert(
                    key.clone(),
                    RunState {
                        params: key.clone(),
                        roots: HashSet::new(),
                        start,
                        last_seen: now_wall,
                        psi_base: psi_base(&psi),
                        cpu_ns: 0,
                        wait_ns: 0,
                        ticks: 0,
                        rss: 0,
                        order: self.run_order,
                    },
                );
            }
            if let Some(entry) = self.runs.get_mut(&key) {
                for r in new_roots {
                    entry.roots.insert(r);
                }
            }
        }

        let mut psi_pct = PsiPct::default();
        if let Some(last) = &self.psi_last {
            let us = dt * 1e6;
            psi_pct.cpu_some = line_pct(last.cpu.some.total, psi.cpu.some.total, us);
            psi_pct.mem_some = line_pct(last.mem.some.total, psi.mem.some.total, us);
            psi_pct.io_some = line_pct(last.io.some.total, psi.io.some.total, us);
        }
        self.psi_last = Some(psi);

        let hz = self.sys.clk_tck;
        let cores = self.sys.cores;
        let denom = (dt * hz as f64 * cores as f64).max(1.0);
        let tree_ticks: u64 = tree
            .iter()
            .map(|pid| {
                by_pid
                    .get(pid)
                    .map(|p| delta_ticks(&self.pid_last, *pid, p.ticks))
                    .unwrap_or(0)
            })
            .sum();
        let all_ticks: u64 = procs
            .iter()
            .map(|p| delta_ticks(&self.pid_last, p.pid, p.ticks))
            .sum();
        let cpu_used = all_ticks as f64 / denom * 100.0;
        let cpu_target = tree_ticks as f64 / denom * 100.0;
        let cpu_others = (cpu_used - cpu_target).max(0.0);
        let cpu_idle = (100.0 - cpu_used).max(0.0);

        let tree_rss: u64 = tree
            .iter()
            .map(|pid| by_pid.get(pid).map(|p| p.rss).unwrap_or(0))
            .sum();
        let used_mem = mem_total.saturating_sub(mem_avail);
        let mem_total_f = mem_total.max(1) as f64;
        let mem_target = tree_rss as f64 / mem_total_f * 100.0;
        let mem_others = (used_mem.saturating_sub(tree_rss) as f64 / mem_total_f * 100.0).max(0.0);
        let mem_free = mem_avail as f64 / mem_total_f * 100.0;

        let mut rows: Vec<RunRow> = self.finalized.clone();
        for e in self.runs.values() {
            let wall = (now_wall - e.start).max(0.0);
            rows.push(build_row(e, wall, &psi, &self.sys));
        }
        rows.sort_by_key(|r| (!r.alive, r.order));

        let collecting = !self.runs.is_empty();
        if collecting {
            self.collecting_secs += dt;
        }
        for p in &procs {
            let e = self.pid_accum.entry(p.pid).or_insert_with(|| PidAccum {
                uid: p.uid,
                user: procfs::username(p.uid),
                comm: p.comm.clone(),
                cmdline: fmt_cmdline(&p.cmdline),
                owned: false,
                ticks: 0,
                wait_ns: 0,
                rss_peak: 0,
            });
            if tree.contains(&p.pid) {
                e.owned = true;
            }
            if collecting {
                e.ticks += delta_ticks(&self.pid_last, p.pid, p.ticks);
                e.rss_peak = e.rss_peak.max(p.rss);
            }
        }
        if self.pid_accum.len() > 3000 {
            let mut keys: Vec<i32> = self.pid_accum.keys().copied().collect();
            keys.sort_by_key(|k| self.pid_accum[k].ticks);
            for k in keys.iter().take(self.pid_accum.len() - 3000) {
                self.pid_accum.remove(k);
            }
        }

        let euid = unsafe { libc::geteuid() };
        let mut ants: Vec<Antag> = Vec::new();
        for (pid, a) in &self.pid_accum {
            if tree.contains(pid) || a.owned || *pid == self_pid || *pid == peer {
                continue;
            }
            ants.push(Antag {
                pid: *pid,
                user: a.user.clone(),
                comm: a.comm.clone(),
                cmdline: a.cmdline.clone(),
                cpu_secs: a.ticks as f64 / hz as f64,
                wait_secs: a.wait_ns as f64 / 1e9,
                rss: a.rss_peak,
            });
        }
        ants.sort_by(|a, b| b.cpu_secs.total_cmp(&a.cpu_secs));
        ants.retain(|a| a.cpu_secs >= MIN_CPU_SECS || a.rss >= MIN_RSS_BYTES);
        ants.truncate(8);

        let mut by_user: HashMap<String, (f64, f64, u64, usize)> = HashMap::new();
        for (pid, a) in &self.pid_accum {
            if tree.contains(pid)
                || a.owned
                || *pid == self_pid
                || *pid == peer
                || a.uid == euid
            {
                continue;
            }
            let e = by_user.entry(a.user.clone()).or_default();
            e.0 += a.ticks as f64 / hz as f64;
            e.1 += a.wait_ns as f64 / 1e9;
            e.2 += a.rss_peak;
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
        users.retain(|u| u.cpu_secs >= MIN_CPU_SECS || u.rss >= MIN_RSS_BYTES);
        users.truncate(12);

        let status = if name.is_empty() {
            TargetStatus::NoTarget
        } else if matched.is_empty() {
            if self.had_parents {
                TargetStatus::Exited
            } else {
                TargetStatus::Searching
            }
        } else {
            self.had_parents = true;
            TargetStatus::Active(matched.len())
        };

        for p in &procs {
            let sched = sched_map.get(&p.pid).copied();
            self.pid_last.insert(
                p.pid,
                PidLast {
                    ticks: p.ticks,
                    sched,
                },
            );
        }
        self.pid_last.retain(|pid, _| by_pid.contains_key(pid));

        let rss_total: u64 = procs.iter().map(|p| p.rss).sum();

        let mut g_sched: HashMap<i32, (u64, u64)> = HashMap::new();
        let mut g_wait = 0u64;
        let mut g_cpu = 0u64;
        if let Some(last) = &self.g_sched_last {
            for p in &procs {
                if let Some((cpu, wait)) = procfs::read_schedstat(p.pid) {
                    g_sched.insert(p.pid, (cpu, wait));
                    let (lc, lw) = last.get(&p.pid).copied().unwrap_or((cpu, wait));
                    let dc = cpu.saturating_sub(lc);
                    let dw = wait.saturating_sub(lw);
                    g_cpu += dc;
                    g_wait += dw;
                    if collecting
                        && let Some(e) = self.pid_accum.get_mut(&p.pid)
                    {
                        e.wait_ns += dw;
                    }
                }
            }
        } else {
            for p in &procs {
                if let Some((cpu, wait)) = procfs::read_schedstat(p.pid) {
                    g_sched.insert(p.pid, (cpu, wait));
                }
            }
        }
        self.g_sched_last = Some(g_sched);
        let sys_wait = wait_overhead_pct(g_wait, g_cpu);

        self.seq += 1;
        self.history.push_back([
            psi_pct.cpu_some,
            psi_pct.mem_some,
            psi_pct.io_some,
            sys_wait.unwrap_or(0.0),
        ]);
        while self.history.len() > self.history_len {
            self.history.pop_front();
        }

        Snapshot {
            seq: self.seq,
            history: self.history.iter().copied().collect(),
            target: name,
            status,
            psi,
            psi_pct,
            sys_wait,
            rss_total,
            mem_total,
            mem_avail,
            runs: rows,
            share_cpu: [cpu_target, cpu_others, cpu_idle],
            share_mem: [mem_target, mem_others, mem_free],
            antagonists: ants,
            users,
            collecting,
            cores: self.sys.cores,
            collecting_secs: self.collecting_secs,
            rec_secs: self.started.elapsed().as_secs_f64(),
            scanned: procs.len(),
        }
    }
}

fn build_row(e: &RunState, wall: f64, psi: &PsiSet, sys: &SysInfo) -> RunRow {
        let hz = sys.clk_tck;
        let cores = sys.cores;
        let wait_pct = wait_overhead_pct(e.wait_ns, e.cpu_ns);
        let pct = cpu_pct(e.ticks, wall, hz, cores);
        let psi_c = psi_penalty_pct(psi.cpu.some.total.saturating_sub(e.psi_base[0]), wall);
        let psi_m = psi_penalty_pct(full_total(&psi.mem).saturating_sub(e.psi_base[1]), wall);
        let psi_i = psi_penalty_pct(full_total(&psi.io).saturating_sub(e.psi_base[2]), wall);
        let mut rootp: Vec<i32> = e.roots.iter().copied().collect();
        rootp.sort_unstable();
        RunRow {
            params: e.params.clone(),
            roots: rootp,
            wall,
            cpu_secs: e.cpu_ns as f64 / 1e9,
            wait_secs: e.wait_ns as f64 / 1e9,
            wait_pct,
            cpu_pct: pct,
            rss: e.rss,
            psi: [psi_c, psi_m, psi_i],
            alive: !e.roots.is_empty(),
            order: e.order,
        }
}

pub fn exe_name() -> String {
    std::fs::read_link("/proc/self/exe")
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "server-spy".to_string())
}

pub(crate) fn is_stealth(p: &Process, stealth: &str) -> bool {
    !stealth.is_empty()
        && p.cmdline
            .first()
            .map(|a| basename(a) == stealth)
            .unwrap_or(false)
}

pub(crate) fn matches_name(p: &Process, name: &str, my_exe: &str) -> bool {
    if p.pid == std::process::id() as i32 {
        return false;
    }
    if p.cmdline
        .first()
        .map(|a| basename(a) == my_exe)
        .unwrap_or(false)
    {
        return false;
    }
    if p.comm.contains(name) {
        return true;
    }
    p.cmdline.join(" ").contains(name)
}

pub(crate) fn matches_regex(p: &Process, re: &regex::Regex, my_exe: &str) -> bool {
    if p.pid == std::process::id() as i32 {
        return false;
    }
    if p.cmdline
        .first()
        .map(|a| basename(a) == my_exe)
        .unwrap_or(false)
    {
        return false;
    }
    if re.is_match(&p.comm) {
        return true;
    }
    re.is_match(&p.cmdline.join(" "))
}

fn basename(s: &str) -> String {
    s.rsplit('/').next().unwrap_or(s).to_string()
}

fn cmdline_key(p: &Process) -> String {
    if p.cmdline.is_empty() {
        return p.comm.clone();
    }
    let base = basename(&p.cmdline[0]);
    if p.cmdline.len() > 1 && INTERPRETERS.contains(&base.as_str()) {
        if p.cmdline[1] == "-c" && p.cmdline.len() > 2 {
            return p.cmdline[2..].join(" ");
        }
        let script = basename(&p.cmdline[1]);
        let rest = p.cmdline[2..].join(" ");
        return if rest.is_empty() {
            script
        } else {
            format!("{script} {rest}")
        };
    }
    let rest = p.cmdline[1..].join(" ");
    if rest.is_empty() {
        base
    } else {
        format!("{base} {rest}")
    }
}

fn short_token(t: &str) -> String {
    if let Some(eq) = t.find('=') {
        let (k, v) = t.split_at(eq + 1);
        if v.contains('/') {
            return format!("{k}{}", v.rsplit('/').next().unwrap_or(v));
        }
        return t.to_string();
    }
    if t.contains('/') {
        t.rsplit('/').next().unwrap_or(t).to_string()
    } else {
        t.to_string()
    }
}

fn fmt_cmdline(cmdline: &[String]) -> String {
    let mut out = Vec::new();
    for t in cmdline {
        let s = short_token(t);
        if s.len() > 32 {
            continue;
        }
        out.push(s);
        if out.len() >= 8 {
            break;
        }
    }
    let joined = out.join(" ");
    if joined.len() > 70 {
        let cut: String = joined.chars().take(69).collect();
        format!("{cut}…")
    } else {
        joined
    }
}

const INTERPRETERS: [&str; 12] = [
    "python", "python2", "python3", "sh", "bash", "dash", "ksh", "zsh", "fish", "node",
    "perl", "ruby",
];

const MIN_CPU_SECS: f64 = 1.0;
const MIN_RSS_BYTES: u64 = 1024 * 1024 * 1024;

fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn delta_ticks(last: &HashMap<i32, PidLast>, pid: i32, now: u64) -> u64 {
    last.get(&pid)
        .map(|l| now.saturating_sub(l.ticks))
        .unwrap_or(0)
}

fn line_pct(prev: u64, cur: u64, us: f64) -> f64 {
    if us <= 0.0 {
        return 0.0;
    }
    (cur as i64 - prev as i64).max(0) as f64 / us * 100.0
}

fn full_total(f: &PsiFile) -> u64 {
    f.full.map(|l| l.total).unwrap_or(f.some.total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procfs::Process;

    fn proc(cmdline: Vec<String>) -> Process {
        Process {
            pid: 1,
            ppid: 0,
            comm: "x".into(),
            cmdline,
            uid: 1000,
            ticks: 0,
            rss: 0,
            start_secs: 0.0,
        }
    }

    #[test]
    fn key_strips_interpreter() {
        let p = proc(vec![
            "/usr/bin/python3".into(),
            "/exp/worker.py".into(),
            "--algo=hnsw".into(),
            "--M=16".into(),
        ]);
        assert_eq!(cmdline_key(&p), "worker.py --algo=hnsw --M=16");
    }

    #[test]
    fn key_conann_style() {
        let p = proc(vec![
            "/home/lennart/new/conann/conann/build/eval/error_annoy".into(),
            "--M".into(),
            "16".into(),
            "--ef".into(),
            "64".into(),
            "-t".into(),
        ]);
        assert_eq!(cmdline_key(&p), "error_annoy --M 16 --ef 64 -t");
    }

    #[test]
    fn key_handles_sh_c() {
        let p = proc(vec![
            "/bin/sh".into(),
            "-c".into(),
            "worker --algo hnsw --M 16".into(),
        ]);
        assert_eq!(cmdline_key(&p), "worker --algo hnsw --M 16");
    }

    #[test]
    fn key_keeps_non_interpreter() {
        let p = proc(vec!["/usr/bin/sleep".into(), "5".into()]);
        assert_eq!(cmdline_key(&p), "sleep 5");
    }

    #[test]
    fn matches_phrase_substring() {
        let p = proc(vec![
            "/home/lennart/new/conann/conann/build/eval/error_annoy".into(),
            "--M".into(),
            "16".into(),
        ]);
        assert!(matches_name(&p, "error_annoy", "server-spy"));
        assert!(matches_name(&p, "conann/build/eval", "server-spy"));
        assert!(matches_name(&p, "error_annoy --M", "server-spy"));
        assert!(!matches_name(&p, "worker", "server-spy"));
    }

    #[test]
    fn matches_comm_substring() {
        let mut p = proc(vec!["worker".into()]);
        p.comm = "worker".into();
        assert!(matches_name(&p, "worker", "server-spy"));
        assert!(matches_name(&p, "orke", "server-spy"));
    }

    #[test]
    fn skips_own_binary_instances() {
        let p = proc(vec![
            "/usr/local/bin/htop".into(),
            "tui".into(),
            "--target".into(),
            "worker".into(),
        ]);
        assert!(!matches_name(&p, "worker", "htop"));
    }

    #[test]
    fn key_uses_comm_when_no_cmdline() {
        let mut p = proc(vec![]);
        p.comm = "kthreadd".into();
        assert_eq!(cmdline_key(&p), "kthreadd");
    }

    #[test]
    fn shortens_path_tokens() {
        assert_eq!(short_token("/usr/bin/gcc"), "gcc");
        assert_eq!(short_token("--config=/etc/x.conf"), "--config=x.conf");
        assert_eq!(short_token("--algo=hnsw"), "--algo=hnsw");
        assert_eq!(short_token("plain"), "plain");
        assert_eq!(
            fmt_cmdline(&["/usr/bin/python3".into(), "/exp/antagonists.py".into(), "--role=cpu".into()]),
            "python3 antagonists.py --role=cpu"
        );
    }
}

fn psi_base(psi: &PsiSet) -> [u64; 3] {
    [
        psi.cpu.some.total,
        full_total(&psi.mem),
        full_total(&psi.io),
    ]
}

pub fn snapshot_text(s: &Snapshot, sys: &SysInfo) -> String {
    use crate::metrics::{fmt_bytes, fmt_pct, fmt_secs};

    let mut out = String::new();
    let status = match s.status {
        TargetStatus::NoTarget => "no target".to_string(),
        TargetStatus::Searching => format!("searching for '{}'", s.target),
        TargetStatus::Active(n) => {
            format!("active: {n} experiment process(es), {} run group(s)", s.runs.len())
        }
        TargetStatus::Exited => "experiment processes exited".to_string(),
    };
    out.push_str(&format!("target: '{}'  status: {status}\n", s.target));
    out.push_str(&format!(
        "scanned: {} procs, {} cores\n",
        s.scanned, sys.cores
    ));
    out.push_str(&format!(
        "mem: total {}  avail {}  used {}\n",
        fmt_bytes(s.mem_total),
        fmt_bytes(s.mem_avail),
        fmt_bytes(s.mem_total.saturating_sub(s.mem_avail))
    ));
    out.push_str(&format!(
        "psi cur:  cpu {}  mem {}  io {}\n",
        fmt_pct(s.psi_pct.cpu_some),
        fmt_pct(s.psi_pct.mem_some),
        fmt_pct(s.psi_pct.io_some)
    ));
    let f = |l: &crate::procfs::PsiLine| format!("{:.1}/{:.1}/{:.1}", l.avg10, l.avg60, l.avg300);
    out.push_str(&format!(
        "psi avg10/60/300: cpu {}  mem {}  io {}\n",
        f(&s.psi.cpu.some),
        f(&s.psi.mem.some),
        f(&s.psi.io.some),
    ));
    out.push_str(&format!(
        "share cpu: target {:.1}%  others {:.1}%  idle {:.1}%\n",
        s.share_cpu[0], s.share_cpu[1], s.share_cpu[2]
    ));
    out.push_str(&format!(
        "share mem: target {:.1}%  others {:.1}%  free {:.1}%\n",
        s.share_mem[0], s.share_mem[1], s.share_mem[2]
    ));
    let sys_wait = match s.sys_wait {
        Some(p) => fmt_pct(p),
        None => "—".to_string(),
    };
    out.push_str(&format!(
        "sys now: sched wait {}  rss total {}  used {}  free {}\n",
        sys_wait,
        fmt_bytes(s.rss_total),
        fmt_bytes(s.mem_total.saturating_sub(s.mem_avail)),
        fmt_bytes(s.mem_avail)
    ));
    out.push_str("runs:\n");
    for r in &s.runs {
        let st = if r.alive { "alive" } else { "done " };
        let wait = match r.wait_pct {
            Some(p) => format!("{p:.1}%"),
            None => "—".to_string(),
        };
        out.push_str(&format!(
            "  {st} wall {}  cpu {}  wait {wait}  cpu% {}  rss {}  psi[c {} m {} i {}]  pids {:?}  {}\n",
            fmt_secs(r.wall),
            fmt_secs(r.cpu_secs),
            fmt_pct(r.cpu_pct),
            fmt_bytes(r.rss),
            fmt_pct(r.psi[0]),
            fmt_pct(r.psi[1]),
            fmt_pct(r.psi[2]),
            r.roots,
            r.params
        ));
    }
    if s.antagonists.is_empty() {
        out.push_str("antagonists: none\n");
    } else {
        let state = if s.collecting { "● collecting" } else { "○ idle (no experiments running)" };
        out.push_str(&format!("antagonists (accumulated during experiments, {state}):\n"));
        for (i, a) in s.antagonists.iter().enumerate() {
            out.push_str(&format!(
                "  {}. {}  {}  cpu {}  wait {}  rss {}  {}\n",
                i + 1,
                a.user,
                a.comm,
                fmt_secs(a.cpu_secs),
                fmt_secs(a.wait_secs),
                fmt_bytes(a.rss),
                a.cmdline
            ));
        }
    }
    if s.users.is_empty() {
        out.push_str("adversarial users (excl. me): none\n");
    } else {
        let total: f64 = s.users.iter().map(|u| u.cpu_secs).sum();
        out.push_str("adversarial users (excl. me, accumulated):\n");
        for (i, u) in s.users.iter().enumerate() {
            let share = if total > 0.0 {
                u.cpu_secs / total * 100.0
            } else {
                0.0
            };
            out.push_str(&format!(
                "  {}. {}  {:.0}% of load  cpu {}  wait {}  rss {}  {} procs\n",
                i + 1,
                u.user,
                share,
                fmt_secs(u.cpu_secs),
                fmt_secs(u.wait_secs),
                fmt_bytes(u.rss),
                u.procs
            ));
        }
    }
    out
}
