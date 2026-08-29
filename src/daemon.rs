use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::collector::{Collector, Control, MatchInfo, Rule, Snapshot};
use crate::procfs::Process;

pub use crate::collector::exe_name;

pub const PROTOCOL_VERSION: u8 = 14;

/// Renames the current process (comm + cmdline) so tools like ps/top/htop
/// show it under a different name. Used for stealth mode: prctl sets the
/// comm, and the original argv block is zeroed and replaced on the initial
/// stack, so /proc/<pid>/cmdline stops leaking the old command line.
pub fn rename_self(name: &str) {
    let name = &name[..name.len().min(15)];
    if let Ok(c) = std::ffi::CString::new(name) {
        unsafe {
            libc::prctl(libc::PR_SET_NAME, c.as_ptr() as usize);
        }
    }
    clobber_argv(name);
}

/// Overwrites the original argv memory with the new name. The argv strings
/// live on the initial stack of the process, so we scan the stack mapping
/// (found via /proc/self/maps) for the exact byte blob /proc/self/cmdline
/// reports, zero it entirely and write the new name in its place. This works
/// on glibc and musl alike (no reliance on glibc's `program_invocation_name`).
fn clobber_argv(name: &str) {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.is_empty() {
        return;
    }
    let mut blob = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            blob.push(0);
        }
        blob.extend_from_slice(a.as_encoded_bytes());
    }
    // A stack variable gives us an address inside the stack mapping; the
    // argv block sits above it (closer to the initial stack top).
    let mut probe = 0u8;
    let sp = &mut probe as *mut u8 as usize;
    let Some((lo, hi)) = stack_mapping(sp) else {
        return;
    };
    let Some(found) = find_bytes(lo, hi.min(sp.saturating_add(64 << 20)), &blob) else {
        return;
    };
    unsafe {
        std::ptr::write_bytes(found as *mut u8, 0, blob.len());
        let n = name.len().min(blob.len());
        std::ptr::copy_nonoverlapping(name.as_ptr(), found as *mut u8, n);
    }
}

/// The readable mapping containing the given stack address (i.e. our own
/// stack, whose top holds the original argv block).
fn stack_mapping(sp: usize) -> Option<(usize, usize)> {
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    for line in maps.lines() {
        let mut it = line.split_whitespace();
        let range = it.next()?;
        let perms = it.next()?;
        let (lo_s, hi_s) = range.split_once('-')?;
        let lo = usize::from_str_radix(lo_s, 16).ok()?;
        let hi = usize::from_str_radix(hi_s, 16).ok()?;
        if !perms.contains('r') {
            continue;
        }
        if lo <= sp && sp < hi {
            return Some((lo, hi));
        }
    }
    None
}

/// Scans for a byte pattern inside a mapped range (reads are safe: the
/// range comes from /proc/self/maps and belongs to our own process).
fn find_bytes(lo: usize, hi: usize, blob: &[u8]) -> Option<usize> {
    if blob.is_empty() || hi <= lo {
        return None;
    }
    let start = lo as *const u8;
    let mut i = 0usize;
    let first = blob[0];
    while i + blob.len() <= hi - lo {
        unsafe {
            if *start.add(i) == first
                && (blob.len() == 1 || {
                    let a = std::slice::from_raw_parts(start.add(i), blob.len());
                    a == blob
                })
            {
                return Some(lo + i);
            }
        }
        i += 1;
    }
    None
}

/// Verifies the daemon speaks the current protocol. Returns the connect error
/// when offline, or a clear error when a stale daemon is running.
pub fn ensure_compatible() -> io::Result<()> {
    let mut stream = connect()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
    stream.write_all(b"V")?;
    stream.flush()?;
    let mut v = [0u8; 1];
    match stream.read_exact(&mut v) {
        Ok(_) if v[0] == PROTOCOL_VERSION => Ok(()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "daemon is outdated — confirm will restart it",
        )),
    }
}

pub fn socket_path() -> PathBuf {
    // Prefer the private per-user runtime directory (0700, owned by us):
    // no other user can create or even reach sockets there. Fall back to
    // /tmp keyed by uid (still protected by the per-connection uid check).
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR")
        && !runtime.is_empty()
    {
        return PathBuf::from(runtime).join("server-spy.sock");
    }
    let uid = unsafe { libc::geteuid() };
    std::env::temp_dir().join(format!("server-spy-{uid}.sock"))
}

fn connect() -> io::Result<UnixStream> {
    let path = socket_path();
    match UnixStream::connect(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
            let _ = fs::remove_file(&path);
            Err(e)
        }
        Err(e) => Err(e),
    }
}

pub fn is_running() -> bool {
    connect().is_ok()
}

pub fn ensure(target: &str, interval: Duration, history_len: usize) -> io::Result<()> {
    if is_running() {
        return Ok(());
    }
    start(target, interval, history_len)
}

pub fn start(target: &str, interval: Duration, history_len: usize) -> io::Result<()> {
    if is_running() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "daemon already running",
        ));
    }
    let exe = std::env::current_exe()?;
    let mut child = Command::new(&exe)
        .arg("daemon")
        .arg("--detach")
        .arg("--target")
        .arg(target)
        .arg("--interval")
        .arg(interval.as_secs_f64().to_string())
        .arg("--history")
        .arg(history_len.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let _ = child.wait();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if is_running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "daemon did not start",
    ))
}

pub fn stop() -> io::Result<()> {
    let mut stream = connect()?;
    stream.write_all(b"X")?;
    stream.flush()?;
    let mut ack = [0u8; 1];
    let _ = stream.read_exact(&mut ack);
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && socket_path().exists() {
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

pub fn set_rules(rules: &[Rule]) -> io::Result<()> {
    ensure_compatible()?;
    let mut stream = connect()?;
    stream.write_all(b"T")?;
    write_rules(&mut stream, rules)?;
    stream.flush()?;
    let mut ack = [0u8; 1];
    stream.read_exact(&mut ack)?;
    Ok(())
}

fn write_rules(stream: &mut impl Write, rules: &[Rule]) -> io::Result<()> {
    stream.write_all(&(rules.len().min(64) as u16).to_le_bytes())?;
    for r in rules.iter().take(64) {
        let mut flags = 0u8;
        if r.regex {
            flags |= 1;
        }
        if r.exclude {
            flags |= 2;
        }
        stream.write_all(&[flags])?;
        let n = r.pattern.len().min(65535) as u16;
        stream.write_all(&n.to_le_bytes())?;
        stream.write_all(&r.pattern.as_bytes()[..n as usize])?;
    }
    Ok(())
}

fn read_rules(stream: &mut impl Read) -> io::Result<Vec<Rule>> {
    let mut buf = [0u8; 2];
    stream.read_exact(&mut buf)?;
    let n = u16::from_le_bytes(buf) as usize;
    if n > 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "too many filter rules",
        ));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let mut flags = [0u8; 1];
        stream.read_exact(&mut flags)?;
        stream.read_exact(&mut buf)?;
        let len = u16::from_le_bytes(buf) as usize;
        let mut pat = vec![0u8; len];
        stream.read_exact(&mut pat)?;
        out.push(Rule {
            pattern: String::from_utf8_lossy(&pat).into_owned(),
            regex: flags[0] & 1 != 0,
            exclude: flags[0] & 2 != 0,
        });
    }
    Ok(out)
}

pub fn set_stealth(name: &str) -> io::Result<()> {
    ensure_compatible()?;
    let mut stream = connect()?;
    stream.write_all(b"E")?;
    let n = name.len().min(255);
    stream.write_all(&(n as u16).to_le_bytes())?;
    stream.write_all(&name.as_bytes()[..n])?;
    stream.flush()?;
    let mut ack = [0u8; 1];
    stream.read_exact(&mut ack)?;
    Ok(())
}

pub fn preview_rules(rules: &[Rule]) -> io::Result<Vec<MatchInfo>> {
    ensure_compatible()?;
    let mut stream = connect()?;
    stream.write_all(b"P")?;
    write_rules(&mut stream, rules)?;
    stream.flush()?;
    let mut len = [0u8; 8];
    stream.read_exact(&mut len)?;
    let n = u64::from_le_bytes(len) as usize;
    if n > 64 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "preview too large",
        ));
    }
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Requests the current snapshot. Pass the seq of the last snapshot you have;
/// returns Ok(None) when nothing changed since, avoiding a full re-transfer.
pub fn request_snapshot(last_seq: u64) -> io::Result<Option<Snapshot>> {
    let mut stream = connect()?;
    stream.write_all(b"S")?;
    stream.write_all(&last_seq.to_le_bytes())?;
    stream.flush()?;
    let mut status = [0u8; 1];
    stream.read_exact(&mut status)?;
    if status[0] == b'N' {
        return Ok(None);
    }
    let mut len = [0u8; 8];
    stream.read_exact(&mut len)?;
    let n = u64::from_le_bytes(len) as usize;
    if n > 64 * 1024 * 1024 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "snapshot too large"));
    }
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf)?;
    bincode::serde::decode_from_slice(&buf, bincode::config::standard())
        .map(|(s, _)| Some(s))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    SHUTDOWN.store(true, AtomicOrdering::SeqCst);
}

pub fn run_detached(target: String, interval: Duration, history_len: usize) -> ! {
    let control = Arc::new(Control::new(target.clone()));
    let chain = crate::collector::self_chain(std::process::id() as i32);
    control.set_orig_chain(chain);
    unsafe {
        let pid = libc::fork();
        if pid > 0 {
            libc::_exit(0);
        }
        let pid = libc::fork();
        if pid > 0 {
            libc::_exit(0);
        }
        libc::setsid();
    }
    let _ = run_foreground_with(interval, history_len, control);
    unsafe {
        libc::_exit(0);
    }
}

pub fn run_foreground(
    target: String,
    interval: Duration,
    history_len: usize,
) -> io::Result<()> {
    let control = Arc::new(Control::new(target.clone()));
    let chain = crate::collector::self_chain(std::process::id() as i32);
    control.set_orig_chain(chain);
    run_foreground_with(interval, history_len, control)
}

/// The poll source for the daemon: the real collector, or the completely
/// synthetic scenario when SERVER_SPY_SCENARIO points at a scenario file.
enum Source {
    Real(Box<Collector>),
    Fake(Box<crate::scenario::Scenario>),
}

impl Source {
    fn poll(&mut self) -> Snapshot {
        match self {
            Source::Real(c) => c.poll(),
            Source::Fake(s) => s.poll(),
        }
    }

    fn interval(&self) -> Duration {
        match self {
            Source::Real(c) => c.interval,
            Source::Fake(s) => s.interval(),
        }
    }
}

fn scenario_env() -> Option<String> {
    std::env::var("SERVER_SPY_SCENARIO")
        .ok()
        .filter(|p| !p.is_empty())
}

fn run_foreground_with(
    interval: Duration,
    history_len: usize,
    control: Arc<Control>,
) -> io::Result<()> {
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as usize);
        libc::signal(libc::SIGINT, on_signal as *const () as usize);
    }
    if is_running() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "daemon already running",
        ));
    }
    let path = socket_path();
    let _ = fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    // Defense in depth: only the owner may connect (the per-connection uid
    // check below is the real gate; this also blocks guessing attempts).
    let _ = fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600));

    let procs_shared: Arc<Mutex<Vec<Process>>> = Arc::new(Mutex::new(Vec::new()));
    let mut source = if let Some(path) = scenario_env() {
        Source::Fake(Box::new(crate::scenario::Scenario::load(
            &path,
            control.clone(),
        )?))
    } else {
        let mut col = Collector::new(interval, control.clone(), history_len);
        col.set_shared_procs(procs_shared.clone());
        Source::Real(Box::new(col))
    };
    if let Source::Fake(s) = &mut source {
        s.set_shared_procs(procs_shared.clone());
    }
    let latest: Arc<Mutex<Snapshot>> = Arc::new(Mutex::new(source.poll()));
    // serialized snapshot cache (seq -> bytes): avoids re-encoding the same
    // snapshot for every client / repeat request
    let snap_cache: Arc<Mutex<(u64, Vec<u8>)>> = Arc::new(Mutex::new((0, Vec::new())));

    let latest2 = latest.clone();
    let control2 = control.clone();
    let mut last_gen = control2.generation();
    let collector_thread = std::thread::spawn(move || loop {
        if SHUTDOWN.load(AtomicOrdering::SeqCst) {
            break;
        }
        *latest2.lock().unwrap() = source.poll();
        let step = Duration::from_millis(100);
        let mut slept = Duration::ZERO;
        while slept < source.interval() {
            if SHUTDOWN.load(AtomicOrdering::SeqCst) {
                break;
            }
            // a filter change must take effect on the next poll, not on the
            // next full interval: wake up as soon as the rules generation
            // moves, so runs appear immediately after confirming a filter
            let now_gen = control2.generation();
            if now_gen != last_gen {
                last_gen = now_gen;
                break;
            }
            let wait = step.min(source.interval() - slept);
            std::thread::sleep(wait);
            slept += wait;
        }
    });

    loop {
        if SHUTDOWN.load(AtomicOrdering::SeqCst) {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                handle_conn(stream, &latest, &snap_cache, &control, &procs_shared)
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    }
    let _ = collector_thread.join();
    let _ = fs::remove_file(&path);
    Ok(())
}

fn handle_conn(
    mut stream: UnixStream,
    latest: &Mutex<Snapshot>,
    snap_cache: &Mutex<(u64, Vec<u8>)>,
    control: &Control,
    procs: &Mutex<Vec<Process>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    // Authenticate: only the owning user may talk to this daemon. The kernel
    // provides SO_PEERCRED, so the identity cannot be spoofed. Without this,
    // any local user could read snapshots, change the filter or stop us.
    let uid = unsafe { libc::geteuid() };
    let Some(cred) = peer_cred(&stream) else {
        return;
    };
    if cred.uid != uid {
        return;
    }
    control.set_peer(cred.pid);
    let mut cmd = [0u8; 1];
    if stream.read_exact(&mut cmd).is_err() {
        return;
    }
    match cmd[0] {
        b'V' => {
            let _ = stream.write_all(&[PROTOCOL_VERSION]);
        }
        b'S' => {
            let mut buf = [0u8; 8];
            let last_seq = stream
                .read_exact(&mut buf)
                .map(|_| u64::from_le_bytes(buf))
                .unwrap_or(0);
            let latest = latest.lock().unwrap();
            if latest.seq == last_seq {
                let _ = stream.write_all(b"N");
            } else {
                let mut cache = snap_cache.lock().unwrap();
                if cache.0 != latest.seq {
                    cache.1 = bincode::serde::encode_to_vec(&*latest, bincode::config::standard())
                        .unwrap_or_default();
                    cache.0 = latest.seq;
                }
                let bytes = &cache.1;
                let _ = stream.write_all(b"C");
                let _ = stream.write_all(&(bytes.len() as u64).to_le_bytes());
                let _ = stream.write_all(bytes);
            }
        }
        b'T' => {
            match read_rules(&mut stream) {
                Ok(rules) => control.set_rules(rules),
                Err(_) => return,
            }
            let _ = stream.write_all(&[0u8]);
        }
        b'E' => {
            let mut len = [0u8; 2];
            if stream.read_exact(&mut len).is_err() {
                return;
            }
            let n = u16::from_le_bytes(len) as usize;
            let mut buf = vec![0u8; n];
            if stream.read_exact(&mut buf).is_err() {
                return;
            }
            let name = String::from_utf8_lossy(&buf).into_owned();
            control.set_stealth(name.clone());
            rename_self(&name);
            let _ = stream.write_all(&[0u8]);
        }
        b'P' => {
            let rules = match read_rules(&mut stream) {
                Ok(r) => r,
                Err(_) => return,
            };
            let matches = compute_preview(&rules, &procs.lock().unwrap(), control);
            let bytes = serde_json::to_vec(&matches).unwrap_or_default();
            let _ = stream.write_all(&(bytes.len() as u64).to_le_bytes());
            let _ = stream.write_all(&bytes);
        }
        b'X' => {
            let _ = stream.write_all(&[0u8]);
            SHUTDOWN.store(true, AtomicOrdering::SeqCst);
        }
        _ => {}
    }
}

fn peer_cred(stream: &UnixStream) -> Option<libc::ucred> {
    use std::os::unix::io::AsRawFd;
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc == 0 {
        Some(cred)
    } else {
        None
    }
}

fn compute_preview(
    rules: &[Rule],
    procs: &[Process],
    control: &Control,
) -> Vec<MatchInfo> {
    if rules.is_empty() || !rules.iter().any(|r| !r.exclude) {
        return Vec::new();
    }
    let my_exe = exe_name();
    let stealth = control.get_stealth();
    let peer = control.get_peer();
    let demo = control.get_demo();
    let mut chain = crate::collector::self_chain(std::process::id() as i32);
    chain.extend(control.get_orig_chain());
    if peer > 0 {
        chain.extend(crate::collector::self_chain(peer));
    }
    let mut out: Vec<MatchInfo> = Vec::new();
    for p in procs {
        if p.pid == peer || chain.contains(&p.pid) || crate::collector::is_stealth(p, &stealth) {
            continue;
        }
        if demo && p.demo_user.is_empty() {
            continue;
        }
        if crate::collector::matches_rules(p, rules, &my_exe) {
            out.push(MatchInfo {
                pid: p.pid,
                user: if demo {
                    p.demo_user.clone()
                } else {
                    crate::procfs::username(p.uid)
                },
                comm: p.comm.clone(),
                cmdline: crate::collector::fmt_cmdline(&p.cmdline),
            });
        }
    }
    out.sort_by_key(|m| m.pid);
    out.truncate(100);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procfs::Process;

    fn proc(pid: i32, comm: &str, cmdline: Vec<&str>) -> Process {
        Process {
            pid,
            ppid: 1,
            comm: comm.into(),
            cmdline: cmdline.iter().map(|s| s.to_string()).collect(),
            uid: 1000,
            ticks: 0,
            rss: 0,
            start_secs: 0.0,
            demo_user: String::new(),
            tty: 0,
            last_cpu: 0,
        }
    }

    fn procs() -> Vec<Process> {
        let my = exe_name();
        vec![
            proc(10, "worker", vec!["/usr/bin/python3", "/exp/worker.py", "--algo=hnsw", "--M=16"]),
            proc(11, "worker", vec!["/usr/bin/python3", "/exp/worker.py", "--algo=annoy", "--M=32"]),
            proc(12, "bash", vec!["/usr/bin/bash", "-c", "worker --algo brute"]),
            proc(13, "sleep", vec!["/usr/bin/sleep", "100"]),
            proc(14, "bash", vec![&format!("/usr/bin/{my}"), "tui"]),
        ]
    }

    fn control() -> Control {
        Control::new(String::new())
    }

    fn rule(pattern: &str, regex: bool, exclude: bool) -> Rule {
        Rule {
            pattern: pattern.into(),
            regex,
            exclude,
        }
    }

    fn preview(filter: &str, regex: bool) -> Vec<MatchInfo> {
        compute_preview(&[rule(filter, regex, false)], &procs(), &control())
    }

    #[test]
    fn simple_filter_matches_comm_and_cmdline() {
        let m = preview("worker", false);
        let pids: Vec<i32> = m.iter().map(|x| x.pid).collect();
        assert_eq!(pids, vec![10, 11, 12]);
    }

    #[test]
    fn simple_filter_with_phrase() {
        let m = preview("algo=hnsw", false);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].pid, 10);
    }

    #[test]
    fn regex_filter_matches() {
        let m = preview(r"worker\.py.*M=3\d", true);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].pid, 11);
    }

    #[test]
    fn regex_filter_comm_alternation() {
        let m = preview("^(worker|sleep)$", true);
        let pids: Vec<i32> = m.iter().map(|x| x.pid).collect();
        assert_eq!(pids, vec![10, 11, 13]);
    }

    #[test]
    fn excludes_own_binary() {
        let m = preview("server-spy", false);
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn invalid_regex_matches_nothing() {
        let m = preview("([", true);
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn empty_filter_matches_nothing() {
        let m = preview("", false);
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn stealth_and_peer_are_excluded() {
        let c = Control::new(String::new());
        c.set_stealth("htop".into());
        c.set_peer(13);
        let mut ps = procs();
        ps.push(proc(20, "htop", vec!["/usr/bin/htop"]));
        ps.push(proc(21, "htop", vec!["/usr/bin/htop"]));
        let m = compute_preview(&[rule("htop", false, false)], &ps, &c);
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn rules_and_and_exclude_combine() {
        let rules = vec![
            rule("worker", false, false),
            rule("algo=hnsw", false, false),
            rule("sleep", false, true),
        ];
        let m = compute_preview(&rules, &procs(), &control());
        let pids: Vec<i32> = m.iter().map(|x| x.pid).collect();
        assert_eq!(pids, vec![10]);
    }

    #[test]
    fn exclude_rule_vetoes_even_matching_includes() {
        let rules = vec![rule("worker", false, false), rule("brute", false, true)];
        let m = compute_preview(&rules, &procs(), &control());
        let pids: Vec<i32> = m.iter().map(|x| x.pid).collect();
        assert_eq!(pids, vec![10, 11]);
    }

    #[test]
    fn rules_without_includes_match_nothing() {
        let rules = vec![rule("worker", false, true)];
        assert_eq!(compute_preview(&rules, &procs(), &control()).len(), 0);
    }

    #[test]
    fn snapshot_bincode_roundtrip() {
        let mut s = crate::collector::Snapshot {
            seq: 7,
            history: vec![[1.0, 2.0, 3.0, 4.0]],
            target: "worker".into(),
            rules: vec![rule("worker", false, false)],
            status: crate::collector::TargetStatus::Active(2),
            psi: crate::procfs::PsiSet::default(),
            psi_pct: crate::collector::PsiPct::default(),
            sys_wait: Some(1.5),
            rss_total: 0,
            mem_total: 0,
            mem_avail: 0,
            runs: Vec::new(),
            share_cpu: [0.0; 3],
            share_mem: [0.0; 3],
            antagonists: Vec::new(),
            users: Vec::new(),
            live_ants: Vec::new(),
            live_users: Vec::new(),
            live_dt: 1.0,
            conditions: crate::conditions::CondSummary::default(),
            collecting: false,
            cores: 16,
            our_cores: 0,
            collecting_secs: 10.0,
            rec_secs: 12.0,
            scanned: 42,
        };
        s.runs.push(crate::collector::RunRow {
            params: "bench.py".into(),
            comm: "bench.py".into(),
            roots: vec![1],
            wall: 10.0,
            cpu_secs: 5.0,
            wait_secs: 1.0,
            wait_pct: Some(20.0),
            cpu_pct: 50.0,
            rss: 1000,
            psi: [1.0, 2.0, 3.0],
            alive: false,
            order: 1,
            users: 2,
            cf: Some(1.2),
            cl: Some(10.0),
            ants: vec![],
            run_users: vec![],
        });
        let bytes =
            bincode::serde::encode_to_vec(&s, bincode::config::standard()).unwrap();
        let (out, used) =
            bincode::serde::decode_from_slice::<Snapshot, _>(&bytes, bincode::config::standard())
                .unwrap();
        assert_eq!(used, bytes.len());
        assert_eq!(out.seq, 7);
        assert_eq!(out.target, "worker");
        assert_eq!(out.runs.len(), 1);
        assert_eq!(out.runs[0].cf, Some(1.2));
        assert_eq!(out.runs[0].wait_pct, Some(20.0));
        assert_eq!(out.history, [[1.0, 2.0, 3.0, 4.0]]);
    }
}
