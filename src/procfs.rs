use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

pub struct SysInfo {
    pub clk_tck: u64,
    pub page_size: u64,
    pub cores: u64,
}

impl SysInfo {
    pub fn detect() -> Self {
        let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let cores = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
        Self {
            clk_tck: if clk_tck > 0 { clk_tck as u64 } else { 100 },
            page_size: if page > 0 { page as u64 } else { 4096 },
            cores: if cores > 0 { cores as u64 } else { 1 },
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PsiLine {
    pub avg10: f64,
    pub avg60: f64,
    pub avg300: f64,
    pub total: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PsiFile {
    pub some: PsiLine,
    pub full: Option<PsiLine>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PsiSet {
    pub cpu: PsiFile,
    pub mem: PsiFile,
    pub io: PsiFile,
}

pub fn read_psi() -> PsiSet {
    PsiSet {
        cpu: fs::read_to_string("/proc/pressure/cpu")
            .ok()
            .and_then(|d| parse_psi_data(&d))
            .unwrap_or_default(),
        mem: fs::read_to_string("/proc/pressure/memory")
            .ok()
            .and_then(|d| parse_psi_data(&d))
            .unwrap_or_default(),
        io: fs::read_to_string("/proc/pressure/io")
            .ok()
            .and_then(|d| parse_psi_data(&d))
            .unwrap_or_default(),
    }
}

pub fn parse_psi_data(data: &str) -> Option<PsiFile> {
    let mut some = None;
    let mut full = None;
    for line in data.lines() {
        let mut it = line.split_whitespace();
        let kind = it.next()?;
        let mut avg10 = 0.0;
        let mut avg60 = 0.0;
        let mut avg300 = 0.0;
        let mut total = 0u64;
        for kv in it {
            if let Some(v) = kv.strip_prefix("avg10=") {
                avg10 = v.parse().unwrap_or(0.0);
            } else if let Some(v) = kv.strip_prefix("avg60=") {
                avg60 = v.parse().unwrap_or(0.0);
            } else if let Some(v) = kv.strip_prefix("avg300=") {
                avg300 = v.parse().unwrap_or(0.0);
            } else if let Some(v) = kv.strip_prefix("total=") {
                total = v.parse().unwrap_or(0);
            }
        }
        let line = PsiLine {
            avg10,
            avg60,
            avg300,
            total,
        };
        match kind {
            "some" => some = Some(line),
            "full" => full = Some(line),
            _ => {}
        }
    }
    Some(PsiFile { some: some?, full })
}

pub fn meminfo() -> (u64, u64) {
    let data = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total = 0u64;
    let mut avail = 0u64;
    for line in data.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail = parse_kb(rest);
        }
    }
    (total * 1024, avail * 1024)
}

fn parse_kb(s: &str) -> u64 {
    s.split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

pub struct ProcStat {
    pub comm: String,
    pub state: char,
    pub ppid: i32,
    pub utime: u64,
    pub stime: u64,
    pub starttime: u64,
    pub rss: u64,
    pub tty: u64,
}

pub fn parse_stat(line: &str) -> Option<ProcStat> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    let comm = &line[open + 1..close];
    let rest: Vec<&str> = line[close + 2..].split_whitespace().collect();
    if rest.len() < 22 {
        return None;
    }
    Some(ProcStat {
        comm: comm.to_string(),
        state: rest[0].chars().next()?,
        ppid: rest[1].parse().ok()?,
        utime: rest[11].parse().ok()?,
        stime: rest[12].parse().ok()?,
        starttime: rest[19].parse().ok()?,
        rss: rest[21].parse().ok()?,
        tty: rest[4].parse().ok()?,
    })
}

pub fn parse_status_uid_tgid(data: &str) -> (Option<u32>, Option<i32>) {
    let mut uid = None;
    let mut tgid = None;
    for line in data.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            uid = rest.split_whitespace().next().and_then(|v| v.parse().ok());
        } else if let Some(rest) = line.strip_prefix("Tgid:") {
            tgid = rest.split_whitespace().next().and_then(|v| v.parse().ok());
        }
    }
    (uid, tgid)
}

pub fn parse_schedstat(line: &str) -> Option<(u64, u64)> {
    let mut it = line.split_whitespace();
    let cpu = it.next()?.parse().ok()?;
    let wait = it.next()?.parse().ok()?;
    Some((cpu, wait))
}

pub fn read_schedstat(pid: i32) -> Option<(u64, u64)> {
    fs::read_to_string(format!("/proc/{pid}/schedstat"))
        .ok()
        .and_then(|d| parse_schedstat(&d))
}

pub fn read_schedstat_sum(pid: i32) -> Option<(u64, u64)> {
    let mut cpu = 0u64;
    let mut wait = 0u64;
    let mut any = false;
    for entry in fs::read_dir(format!("/proc/{pid}/task")).ok()?.flatten() {
        let tid: i32 = match entry.file_name().to_string_lossy().parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some((c, w)) =
            fs::read_to_string(format!("/proc/{pid}/task/{tid}/schedstat"))
                .ok()
                .and_then(|d| parse_schedstat(&d))
        {
            cpu += c;
            wait += w;
            any = true;
        }
    }
    if any {
        Some((cpu, wait))
    } else {
        None
    }
}

pub fn read_cmdline(pid: i32) -> Vec<String> {
    let path = format!("/proc/{pid}/cmdline");
    fs::read(path)
        .map(|b| {
            b.split(|&c| c == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

pub fn read_comm(pid: i32) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Whether the process carries the demo-agent marker in its environment and,
/// if so, under which fake username it should appear. Used by demo mode to
/// simulate "other users" on a shared server.
pub fn read_demo_user(pid: i32) -> Option<String> {
    fs::read(format!("/proc/{pid}/environ")).ok().and_then(|b| {
        b.split(|&c| c == 0).find_map(|e| {
            let e = String::from_utf8_lossy(e);
            e.strip_prefix("SERVER_SPY_DEMO_USER=").map(|v| v.to_string())
        })
    })
}

pub fn boot_secs() -> f64 {
    let data = fs::read_to_string("/proc/stat").unwrap_or_default();
    data.lines()
        .find(|l| l.starts_with("btime "))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0)
}

fn parse_passwd_map(data: &str) -> HashMap<u32, String> {
    let mut m = HashMap::new();
    for line in data.lines() {
        let mut it = line.split(':');
        if let (Some(name), Some(_), Some(id)) = (it.next(), it.next(), it.next())
            && let Ok(u) = id.parse()
        {
            m.insert(u, name.to_string());
        }
    }
    m
}

#[cfg(test)]
pub fn parse_passwd(data: &str, uid: u32) -> Option<String> {
    parse_passwd_map(data).get(&uid).cloned()
}

pub fn username(uid: u32) -> String {
    static CACHE: OnceLock<HashMap<u32, String>> = OnceLock::new();
    let map = CACHE.get_or_init(|| {
        fs::read_to_string("/etc/passwd")
            .map(|d| parse_passwd_map(&d))
            .unwrap_or_default()
    });
    map.get(&uid).cloned().unwrap_or_else(|| uid.to_string())
}

/// Whether the uid belongs to a human user: system/service users (docker,
/// avahi, gdm, ...) sit below uid 1000 or have no login shell, so both
/// checks are combined.
pub fn is_human_uid(uid: u32) -> bool {
    static CACHE: OnceLock<HashSet<u32>> = OnceLock::new();
    let human = CACHE.get_or_init(|| {
        let mut set = HashSet::new();
        if let Ok(data) = fs::read_to_string("/etc/passwd") {
            for line in data.lines() {
                let fields: Vec<&str> = line.split(':').collect();
                if fields.len() < 7 {
                    continue;
                }
                let id: u32 = match fields[2].parse() {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if id >= 1000 && is_login_shell(fields[6]) {
                    set.insert(id);
                }
            }
        }
        set
    });
    human.contains(&uid)
}

fn is_login_shell(shell: &str) -> bool {
    let base = shell.rsplit('/').next().unwrap_or(shell);
    !matches!(
        base,
        "" | "nologin" | "false" | "true" | "sync" | "halt" | "shutdown"
    )
}

#[derive(Debug, Clone)]
pub struct Process {
    pub pid: i32,
    pub ppid: i32,
    pub comm: String,
    pub cmdline: Vec<String>,
    pub uid: u32,
    pub ticks: u64,
    pub rss: u64,
    pub start_secs: f64,
    pub demo_user: String,
    pub tty: u64,
}

/// Per-pid data that does not change for the lifetime of a process, cached
/// across polls to avoid re-reading /proc files that would return the same
/// bytes. Pid reuse is detected via the monotonic `starttime`.
pub struct ScanCache {
    /// uid, tgid, starttime (in clock ticks), demo-agent flag
    meta: HashMap<i32, (u32, i32, u64, bool)>,
    /// fake username of demo agents (only set for demo processes)
    demo_user: HashMap<i32, String>,
    cmdline: HashMap<i32, (Vec<String>, u64)>,
    poll: u64,
}

/// How many polls pass before a cached cmdline is re-read (staggered, so each
/// poll only refreshes 1/PERIOD of the processes). Exec'd processes are seen
/// within this many polls; freshly spawned pids are always read immediately.
const CMDLINE_REFRESH_PERIOD: u64 = 8;

impl ScanCache {
    pub fn new() -> Self {
        Self {
            meta: HashMap::new(),
            demo_user: HashMap::new(),
            cmdline: HashMap::new(),
            poll: 0,
        }
    }

    /// Whether the given pid was marked as a demo agent in the last scan.
    pub fn is_demo_agent(&self, pid: i32) -> bool {
        self.demo_user.contains_key(&pid)
    }

    /// The fake username of a demo agent, if it is one.
    pub fn demo_user(&self, pid: i32) -> Option<&str> {
        self.demo_user.get(&pid).map(|s| s.as_str())
    }
}

impl Default for ScanCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
pub fn mark_demo_agent(cache: &mut ScanCache, pid: i32, user: &str) {
    cache.meta.insert(pid, (1000, pid, 0, true));
    cache.demo_user.insert(pid, user.to_string());
}

pub fn scan_processes(sys: &SysInfo, boot: f64, cache: &mut ScanCache, demo: bool) -> Vec<Process> {
    let mut out = Vec::new();
    cache.poll = cache.poll.wrapping_add(1);
    let Ok(dir) = fs::read_dir("/proc") else {
        return out;
    };
    for entry in dir.flatten() {
        let path: PathBuf = entry.path();
        let pid: i32 = match path
            .file_name()
            .and_then(|n| n.to_string_lossy().parse().ok())
        {
            Some(p) => p,
            None => continue,
        };
        let Ok(stat_data) = fs::read_to_string(path.join("stat")) else {
            continue;
        };
        let Some(st) = parse_stat(&stat_data) else {
            continue;
        };
        if st.state == 'Z' || st.state == 'X' {
            continue;
        }
        let reused = cache
            .meta
            .get(&pid)
            .map(|(_, _, start, _)| st.starttime < *start)
            .unwrap_or(true);
        let (uid, tgid, _agent) = if reused {
            let (uid, tgid) = fs::read_to_string(path.join("status"))
                .ok()
                .map(|d| parse_status_uid_tgid(&d))
                .unwrap_or((None, None));
            let uid = uid.unwrap_or(0);
            let tgid = tgid.unwrap_or(pid);
            let demo_user = if demo {
                read_demo_user(pid).filter(|n| !n.is_empty())
            } else {
                None
            };
            let agent = demo_user.is_some();
            if let Some(n) = demo_user {
                cache.demo_user.insert(pid, n);
            } else {
                cache.demo_user.remove(&pid);
            }
            cache.meta.insert(pid, (uid, tgid, st.starttime, agent));
            (uid, tgid, agent)
        } else {
            let (uid, tgid, _, agent) = cache.meta[&pid];
            (uid, tgid, agent)
        };
        if tgid != pid {
            continue;
        }
        let comm = if st.comm.contains('(') || st.comm.contains(')') {
            read_comm(pid).unwrap_or(st.comm)
        } else {
            st.comm
        };
        let stale_cmdline = cache
            .cmdline
            .get(&pid)
            .map(|(_, at)| cache.poll.saturating_sub(*at) >= CMDLINE_REFRESH_PERIOD)
            .unwrap_or(true);
        if reused || stale_cmdline {
            cache.cmdline.insert(pid, (read_cmdline(pid), cache.poll));
        }
        let cmdline = cache
            .cmdline
            .get(&pid)
            .map(|(c, _)| c.clone())
            .unwrap_or_default();
        out.push(Process {
            pid,
            ppid: st.ppid,
            comm,
            cmdline,
            uid,
            ticks: st.utime + st.stime,
            rss: st.rss * sys.page_size,
            start_secs: boot + st.starttime as f64 / sys.clk_tck as f64,
            demo_user: cache.demo_user.get(&pid).cloned().unwrap_or_default(),
            tty: st.tty,
        });
    }
    let alive: std::collections::HashSet<i32> = out.iter().map(|p| p.pid).collect();
    cache.meta.retain(|pid, _| alive.contains(pid));
    cache.demo_user.retain(|pid, _| alive.contains(pid));
    cache.cmdline.retain(|pid, _| alive.contains(pid));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stat() {
        let line = "1234 (worker) S 100 100 100 0 -1 4194560 100 0 0 0 12345 6789 0 0 20 0 4 0 987654 0 4096 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let st = parse_stat(line).unwrap();
        assert_eq!(st.comm, "worker");
        assert_eq!(st.state, 'S');
        assert_eq!(st.ppid, 100);
        assert_eq!(st.utime, 12345);
        assert_eq!(st.stime, 6789);
        assert_eq!(st.starttime, 987654);
        assert_eq!(st.rss, 4096);
    }

    #[test]
    fn parses_stat_with_spaces_in_comm() {
        let line = "1 (systemd (main)) S 0 1 1 0 -1 4194560 0 0 0 0 100 200 0 0 20 0 1 0 54321 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let st = parse_stat(line).unwrap();
        assert_eq!(st.comm, "systemd (main)");
        assert_eq!(st.state, 'S');
        assert_eq!(st.utime, 100);
        assert_eq!(st.starttime, 54321);
    }

    #[test]
    fn parses_status() {
        let data = "Name:\tfoo\nState:\tS (sleeping)\nTgid:\t1234\nUid:\t1000\t1000\t1000\t1000\n";
        let (uid, tgid) = parse_status_uid_tgid(data);
        assert_eq!(uid, Some(1000));
        assert_eq!(tgid, Some(1234));
    }

    #[test]
    fn parses_schedstat() {
        let (cpu, wait) = parse_schedstat("123456789 234567 42\n").unwrap();
        assert_eq!(cpu, 123456789);
        assert_eq!(wait, 234567);
    }

    #[test]
    fn parses_psi_lines() {
        let data = "some avg10=1.23 avg60=0.45 avg300=0.10 total=1234567\nfull avg10=0.11 avg60=0.02 avg300=0.00 total=89012\n";
        let f = parse_psi_data(data).unwrap();
        assert_eq!(f.some.avg10, 1.23);
        assert_eq!(f.some.total, 1234567);
        assert_eq!(f.full.unwrap().total, 89012);
    }

    #[test]
    fn parses_cpu_psi_without_full() {
        let data = "some avg10=0.00 avg60=0.00 avg300=0.03 total=187136176\n";
        let f = parse_psi_data(data).unwrap();
        assert_eq!(f.some.total, 187136176);
        assert!(f.full.is_none());
    }

    #[test]
    fn resolves_passwd_username() {
        let data = "root:x:0:0:root:/root:/bin/sh\nalice:x:1000:1000:Alice:/home/alice:/bin/bash\n";
        assert_eq!(parse_passwd(data, 1000), Some("alice".to_string()));
        assert_eq!(parse_passwd(data, 0), Some("root".to_string()));
        assert_eq!(parse_passwd(data, 9999), None);
    }
}
