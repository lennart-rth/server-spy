use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::collector::{Collector, Control, MatchInfo, Snapshot};
use crate::procfs::Process;

pub use crate::collector::exe_name;

pub const PROTOCOL_VERSION: u8 = 3;

#[cfg(target_env = "gnu")]
unsafe extern "C" {
    static program_invocation_name: *const libc::c_char;
}

/// Renames the current process (comm + cmdline) so tools like ps/top show it
/// under a different name. Used for stealth mode.
pub fn rename_self(name: &str) {
    let name = &name[..name.len().min(15)];
    if let Ok(c) = std::ffi::CString::new(name) {
        unsafe {
            libc::prctl(libc::PR_SET_NAME, c.as_ptr() as usize);
        }
    }
    #[cfg(target_env = "gnu")]
    clobber_argv(name);
}

/// Overwrites the real argv memory (glibc `program_invocation_name`) so
/// /proc/<pid>/cmdline shows only the new name. glibc-only: musl provides no
/// way to locate the original argv buffer, so there the comm rename above is
/// all we get.
#[cfg(target_env = "gnu")]
fn clobber_argv(name: &str) {
    let argc = std::env::args_os().count();
    unsafe {
        let start = program_invocation_name as *const u8;
        let mut p = start;
        let mut guard = 0;
        for _ in 0..argc {
            while *p != 0 && guard < 1 << 20 {
                p = p.add(1);
                guard += 1;
            }
            if *p != 0 {
                return;
            }
            p = p.add(1);
        }
        let len = p.offset_from(start) as usize;
        std::ptr::write_bytes(start as *mut u8, 0, len);
        let n = name.len().min(len);
        std::ptr::copy_nonoverlapping(name.as_ptr(), start as *mut u8, n);
    }
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

pub fn set_target_mode(name: &str, regex: bool) -> io::Result<()> {
    ensure_compatible()?;
    let mut stream = connect()?;
    stream.write_all(b"T")?;
    stream.write_all(&[regex as u8])?;
    let n = name.len().min(255);
    stream.write_all(&(n as u16).to_le_bytes())?;
    stream.write_all(&name.as_bytes()[..n])?;
    stream.flush()?;
    let mut ack = [0u8; 1];
    stream.read_exact(&mut ack)?;
    Ok(())
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

pub fn preview_filter(filter: &str, regex: bool) -> io::Result<Vec<MatchInfo>> {
    ensure_compatible()?;
    let mut stream = connect()?;
    stream.write_all(b"P")?;
    stream.write_all(&[regex as u8])?;
    let n = filter.len().min(255);
    stream.write_all(&(n as u16).to_le_bytes())?;
    stream.write_all(&filter.as_bytes()[..n])?;
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

pub fn request_snapshot() -> io::Result<Snapshot> {
    let mut stream = connect()?;
    stream.write_all(b"S")?;
    stream.flush()?;
    let mut len = [0u8; 8];
    stream.read_exact(&mut len)?;
    let n = u64::from_le_bytes(len) as usize;
    if n > 64 * 1024 * 1024 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "snapshot too large"));
    }
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    SHUTDOWN.store(true, AtomicOrdering::SeqCst);
}

pub fn run_detached(target: String, interval: Duration, history_len: usize) -> ! {
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
    let _ = run_foreground(target, interval, history_len);
    unsafe {
        libc::_exit(0);
    }
}

pub fn run_foreground(
    target: String,
    interval: Duration,
    history_len: usize,
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

    let control = Arc::new(Control::new(target));
    let mut col = Collector::new(interval, control.clone(), history_len);
    let procs_shared: Arc<Mutex<Vec<Process>>> = Arc::new(Mutex::new(Vec::new()));
    col.set_shared_procs(procs_shared.clone());
    let latest: Arc<Mutex<Snapshot>> = Arc::new(Mutex::new(col.poll()));

    let latest2 = latest.clone();
    let collector_thread = std::thread::spawn(move || loop {
        if SHUTDOWN.load(AtomicOrdering::SeqCst) {
            break;
        }
        *latest2.lock().unwrap() = col.poll();
        let step = Duration::from_millis(100);
        let mut slept = Duration::ZERO;
        while slept < col.interval {
            if SHUTDOWN.load(AtomicOrdering::SeqCst) {
                break;
            }
            let wait = step.min(col.interval - slept);
            std::thread::sleep(wait);
            slept += wait;
        }
    });

    loop {
        if SHUTDOWN.load(AtomicOrdering::SeqCst) {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => handle_conn(stream, &latest, &control, &procs_shared),
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
    control: &Control,
    procs: &Mutex<Vec<Process>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    if let Some(pid) = peer_pid(&stream) {
        control.set_peer(pid);
    }
    let mut cmd = [0u8; 1];
    if stream.read_exact(&mut cmd).is_err() {
        return;
    }
    match cmd[0] {
        b'V' => {
            let _ = stream.write_all(&[PROTOCOL_VERSION]);
        }
        b'S' => {
            let snap = latest.lock().unwrap().clone();
            let bytes = serde_json::to_vec(&snap).unwrap_or_default();
            let _ = stream.write_all(&(bytes.len() as u64).to_le_bytes());
            let _ = stream.write_all(&bytes);
        }
        b'T' => {
            let mut mode = [0u8; 1];
            if stream.read_exact(&mut mode).is_err() {
                return;
            }
            let mut len = [0u8; 2];
            if stream.read_exact(&mut len).is_ok() {
                let n = u16::from_le_bytes(len) as usize;
                let mut name = vec![0u8; n];
                if stream.read_exact(&mut name).is_ok() {
                    control.set(
                        String::from_utf8_lossy(&name).into_owned(),
                        mode[0] != 0,
                    );
                }
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
            let mut mode = [0u8; 1];
            if stream.read_exact(&mut mode).is_err() {
                return;
            }
            let mut len = [0u8; 2];
            if stream.read_exact(&mut len).is_err() {
                return;
            }
            let n = u16::from_le_bytes(len) as usize;
            let mut buf = vec![0u8; n];
            if stream.read_exact(&mut buf).is_err() {
                return;
            }
            let filter = String::from_utf8_lossy(&buf);
            let matches =
                compute_preview(&filter, mode[0] != 0, &procs.lock().unwrap(), control);
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

fn peer_pid(stream: &UnixStream) -> Option<i32> {
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
        Some(cred.pid as i32)
    } else {
        None
    }
}

fn compute_preview(
    filter: &str,
    regex: bool,
    procs: &[Process],
    control: &Control,
) -> Vec<MatchInfo> {
    if filter.is_empty() {
        return Vec::new();
    }
    let my_exe = exe_name();
    let stealth = control.get_stealth();
    let peer = control.get_peer();
    let re = if regex {
        regex::Regex::new(filter).ok()
    } else {
        None
    };
    let mut out: Vec<MatchInfo> = Vec::new();
    for p in procs {
        if p.pid == peer || crate::collector::is_stealth(p, &stealth) {
            continue;
        }
        let hit = match &re {
            Some(re) => crate::collector::matches_regex(p, re, &my_exe),
            None => crate::collector::matches_name(p, filter, &my_exe),
        };
        if hit {
            out.push(MatchInfo {
                pid: p.pid,
                user: crate::procfs::username(p.uid),
                comm: p.comm.clone(),
                cmdline: p.cmdline.join(" "),
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

    fn preview(filter: &str, regex: bool) -> Vec<MatchInfo> {
        compute_preview(filter, regex, &procs(), &control())
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
        let m = compute_preview("htop", false, &ps, &c);
        assert_eq!(m.len(), 0);
    }
}
