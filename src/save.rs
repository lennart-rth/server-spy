use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::collector::{Antag, PsiPct, RunRow, Snapshot, TargetStatus, UserShare};
use crate::procfs::{PsiFile, PsiLine, PsiSet};

fn expand_path(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return format!("{}/{}", home.to_string_lossy(), rest);
    }
    p.to_string()
}

pub fn save_snapshot(s: &Snapshot, path: &str) -> io::Result<()> {
    let path = expand_path(path);
    if let Some(parent) = std::path::Path::new(&path).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = String::new();
    out.push_str(&format!("# server-spy {}\n", env!("CARGO_PKG_VERSION")));
    out.push_str(&format!("# saved {}\n", iso_now()));
    out.push_str(&format!("# target {}\n", csv(&s.target)));
    out.push_str(&format!("# cores {}\n", s.cores));
    out.push_str(&format!("# mem {}\n", s.mem_total));
    out.push_str(&format!("# rss {}\n", s.rss_total));
    out.push_str(&format!(
        "# psi cur {} {} {}\n",
        f(s.psi_pct.cpu_some),
        f(s.psi_pct.mem_some),
        f(s.psi_pct.io_some)
    ));
    out.push_str(&format!(
        "# psi avg10 cpu {} {} {} mem {} {} {} io {} {} {}\n",
        f(s.psi.cpu.some.avg10),
        f(s.psi.cpu.some.avg60),
        f(s.psi.cpu.some.avg300),
        f(s.psi.mem.some.avg10),
        f(s.psi.mem.some.avg60),
        f(s.psi.mem.some.avg300),
        f(s.psi.io.some.avg10),
        f(s.psi.io.some.avg60),
        f(s.psi.io.some.avg300),
    ));
    out.push_str(&format!(
        "# share cpu {} {} {} mem {} {} {}\n",
        f(s.share_cpu[0]),
        f(s.share_cpu[1]),
        f(s.share_cpu[2]),
        f(s.share_mem[0]),
        f(s.share_mem[1]),
        f(s.share_mem[2]),
    ));
    out.push('\n');

    out.push_str("# runs\n");
    out.push_str("params,wall_s,cpu_s,wait_s,wait%,cpu%,rss_b,psi_cpu%,psi_mem%,psi_io%,state\n");
    for r in &s.runs {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{}\n",
            csv(&r.params),
            fs(r.wall),
            fs(r.cpu_secs),
            fs(r.wait_secs),
            opt_f(r.wait_pct),
            f(r.cpu_pct),
            r.rss,
            f(r.psi[0]),
            f(r.psi[1]),
            f(r.psi[2]),
            if r.alive { "alive" } else { "done" },
        ));
    }
    out.push('\n');

    out.push_str("# users\n");
    out.push_str("user,cpu_s,wait_s,rss_b,procs\n");
    for u in &s.users {
        out.push_str(&format!(
            "{},{},{},{},{}\n",
            csv(&u.user),
            fs(u.cpu_secs),
            fs(u.wait_secs),
            u.rss,
            u.procs
        ));
    }
    out.push('\n');

    out.push_str("# antagonists\n");
    out.push_str("user,comm,cpu_s,wait_s,rss_b,cmdline\n");
    for a in &s.antagonists {
        out.push_str(&format!(
            "{},{},{},{},{},{}\n",
            csv(&a.user),
            csv(&a.comm),
            fs(a.cpu_secs),
            fs(a.wait_secs),
            a.rss,
            csv(&a.cmdline)
        ));
    }
    std::fs::write(path, out)
}

pub fn load_snapshot(path: &str) -> io::Result<Snapshot> {
    let text = std::fs::read_to_string(expand_path(path))?;
    let mut target = String::new();
    let mut cores = 0u64;
    let mut mem = 0u64;
    let mut rss = 0u64;
    let mut psi_cur = [0.0; 3];
    let mut psi_avg = [[0.0; 3]; 3];
    let mut share_cpu = [0.0; 3];
    let mut share_mem = [0.0; 3];
    let mut runs = Vec::new();
    let mut users = Vec::new();
    let mut ants = Vec::new();
    let mut section = "";
    let mut order = 0u64;

    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if let Some(c) = line.strip_prefix('#') {
            let mut it = c.trim_start().splitn(2, ' ');
            let key = it.next().unwrap_or("");
            let val = it.next().unwrap_or("");
            match key {
                "target" => target = unquote(val),
                "cores" => cores = val.parse().ok().unwrap_or(0),
                "mem" => mem = val.parse().ok().unwrap_or(0),
                "rss" => rss = val.parse().ok().unwrap_or(0),
                "psi" => {
                    let mut v = val.split_whitespace();
                    match v.next() {
                        Some("cur") => psi_cur = parse3(v),
                        Some("avg10") => psi_avg = parse3x3(v),
                        _ => {}
                    }
                }
                "share" => {
                    let mut v = val.split_whitespace();
                    while let Some(k) = v.next() {
                        match k {
                            "cpu" => share_cpu = parse3(&mut v),
                            "mem" => share_mem = parse3(&mut v),
                            _ => break,
                        }
                    }
                }
                "runs" | "users" | "antagonists" => section = key,
                _ => {}
            }
            continue;
        }
        if line.is_empty() {
            section = "";
            continue;
        }
        if section.is_empty() {
            continue;
        }
        let cells = parse_row(line);
        match section {
            "runs"
                if cells.len() >= 11 && cells[1].parse::<f64>().is_ok() => {
                    order += 1;
                    runs.push(RunRow {
                        params: cells[0].clone(),
                        roots: Vec::new(),
                        wall: cell_f(&cells, 1),
                        cpu_secs: cell_f(&cells, 2),
                        wait_secs: cell_f(&cells, 3),
                        wait_pct: opt_f_inv(cells[4].as_str()),
                        cpu_pct: cell_f(&cells, 5),
                        rss: cell_u(&cells, 6),
                        psi: [
                            cell_f(&cells, 7),
                            cell_f(&cells, 8),
                            cell_f(&cells, 9),
                        ],
                        alive: cells[10] == "alive",
                        order,
                    });
                }
            "users"
                if cells.len() >= 5 && cells[1].parse::<f64>().is_ok() => {
                    users.push(UserShare {
                        user: cells[0].clone(),
                        cpu_secs: cell_f(&cells, 1),
                        wait_secs: cell_f(&cells, 2),
                        rss: cell_u(&cells, 3),
                        procs: cells[4].parse().unwrap_or(0),
                    });
                }
            "antagonists"
                if cells.len() >= 6 && cells[2].parse::<f64>().is_ok() => {
                    ants.push(Antag {
                        pid: -1,
                        user: cells[0].clone(),
                        comm: cells[1].clone(),
                        cpu_secs: cell_f(&cells, 2),
                        wait_secs: cell_f(&cells, 3),
                        rss: cell_u(&cells, 4),
                        cmdline: cells[5].clone(),
                    });
                }
            _ => {}
        }
    }

    let alive = runs.iter().any(|r| r.alive);
    let status = if runs.is_empty() {
        if target.is_empty() {
            TargetStatus::NoTarget
        } else {
            TargetStatus::Searching
        }
    } else if alive {
        TargetStatus::Active(runs.iter().filter(|r| r.alive).count())
    } else {
        TargetStatus::Exited
    };
    let psi = PsiSet {
        cpu: file(psi_avg[0]),
        mem: file(psi_avg[1]),
        io: file(psi_avg[2]),
    };
    Ok(Snapshot {
        seq: 0,
        history: Vec::new(),
        target,
        status,
        psi,
        psi_pct: PsiPct {
            cpu_some: psi_cur[0],
            mem_some: psi_cur[1],
            io_some: psi_cur[2],
        },
        sys_wait: None,
        rss_total: rss,
        mem_total: mem,
        mem_avail: 0,
        runs,
        share_cpu,
        share_mem,
        antagonists: ants,
        users,
        collecting: alive,
        cores,
        collecting_secs: 0.0,
        rec_secs: 0.0,
        scanned: 0,
    })
}

fn file(a: [f64; 3]) -> PsiFile {
    PsiFile {
        some: PsiLine {
            avg10: a[0],
            avg60: a[1],
            avg300: a[2],
            total: 0,
        },
        full: None,
    }
}

fn parse3<'a>(v: impl Iterator<Item = &'a str>) -> [f64; 3] {
    let mut out = [0.0; 3];
    for (i, x) in v.take(3).enumerate() {
        out[i] = x.parse().unwrap_or(0.0);
    }
    out
}

fn parse3x3<'a>(v: impl Iterator<Item = &'a str>) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    let vals: Vec<f64> = v.take(9).filter_map(|x| x.parse().ok()).collect();
    for (i, x) in vals.iter().take(9).enumerate() {
        out[i / 3][i % 3] = *x;
    }
    out
}

fn parse_row(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_q => {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_q = false;
                }
            }
            '"' => in_q = true,
            ',' if !in_q => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn unquote(s: &str) -> String {
    if let Some(inner) = s.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
        inner.replace("\"\"", "\"")
    } else {
        s.to_string()
    }
}

fn csv(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn trim0(s: String) -> String {
    let t = s.trim_end_matches('0').trim_end_matches('.');
    if t.is_empty() {
        "0".to_string()
    } else {
        t.to_string()
    }
}

fn f(x: f64) -> String {
    let s = if x < 1.0 {
        format!("{x:.3}")
    } else if x < 10.0 {
        format!("{x:.2}")
    } else {
        format!("{x:.1}")
    };
    trim0(s)
}

fn fs(x: f64) -> String {
    trim0(format!("{x:.2}"))
}

fn opt_f(x: Option<f64>) -> String {
    x.map(f).unwrap_or_default()
}

fn opt_f_inv(s: &str) -> Option<f64> {
    if s.is_empty() {
        None
    } else {
        s.parse().ok()
    }
}

fn cell_f(cells: &[String], i: usize) -> f64 {
    cells.get(i).and_then(|c| c.parse().ok()).unwrap_or(0.0)
}

fn cell_u(cells: &[String], i: usize) -> u64 {
    cells.get(i).and_then(|c| c.parse().ok()).unwrap_or(0)
}

pub fn default_save_path(target: &str) -> String {
    let clean: String = target
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let clean: String = clean.chars().take(40).collect();
    let ts: String = iso_now().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if clean.is_empty() {
        format!("server-spy-{ts}.csv")
    } else {
        format!("server-spy-{clean}-{ts}.csv")
    }
}

pub fn latest_save() -> Option<String> {
    let mut best: Option<(SystemTime, String)> = None;
    if let Ok(rd) = std::fs::read_dir(".") {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("server-spy-") && name.ends_with(".csv")
                && let Ok(meta) = e.metadata()
                    && let Ok(mt) = meta.modified()
                    && best.as_ref().map(|(t, _)| mt > *t).unwrap_or(true) {
                        best = Some((mt, name));
                    }
        }
    }
    best.map(|(_, n)| n)
}

fn iso_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mth <= 2 { y + 1 } else { y };
    format!("{y:04}-{mth:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        let mut s = crate::collector::Snapshot {
            seq: 1,
            history: Vec::new(),
            target: "worker.py --algo=hnsw, v2".into(),
            status: TargetStatus::Active(1),
            psi: PsiSet::default(),
            psi_pct: PsiPct {
                cpu_some: 1.23,
                mem_some: 0.045,
                io_some: 9.88,
            },
            sys_wait: None,
            rss_total: 1000,
            mem_total: 1024,
            mem_avail: 0,
            runs: Vec::new(),
            share_cpu: [10.0, 50.0, 40.0],
            share_mem: [1.0, 2.0, 97.0],
            antagonists: Vec::new(),
            users: Vec::new(),
            collecting: true,
            cores: 16,
            collecting_secs: 0.0,
            rec_secs: 0.0,
            scanned: 0,
        };
        s.runs.push(RunRow {
            params: "worker.py --algo=hnsw, v2".into(),
            roots: vec![1, 2],
            wall: 120.5,
            cpu_secs: 95.25,
            wait_secs: 3.125,
            wait_pct: Some(3.28),
            cpu_pct: 79.3,
            rss: 123456789,
            psi: [1.23, 0.45, 2.1],
            alive: true,
            order: 1,
        });
        s.users.push(UserShare {
            user: "lennart".into(),
            cpu_secs: 30.0,
            wait_secs: 1.2,
            rss: 1048576,
            procs: 2,
        });
        s.antagonists.push(Antag {
            pid: 42,
            user: "root".into(),
            comm: "bash".into(),
            cpu_secs: 5.0,
            wait_secs: 0.5,
            rss: 2048,
            cmdline: "/usr/bin/bash -c \"echo hi\", ok".into(),
        });
        s
    }

    #[test]
    fn roundtrip_preserves_data() {
        let s = snap();
        let path = std::env::temp_dir().join("server-spy-test.csv");
        save_snapshot(&s, path.to_str().unwrap()).unwrap();
        let l = load_snapshot(path.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(l.target, s.target);
        assert_eq!(l.cores, 16);
        assert_eq!(l.mem_total, 1024);
        assert_eq!(l.rss_total, 1000);
        assert_eq!(l.psi_pct.cpu_some, 1.23);
        assert_eq!(l.psi_pct.mem_some, 0.045);
        assert_eq!(l.psi_pct.io_some, 9.88);
        assert_eq!(l.share_cpu, [10.0, 50.0, 40.0]);
        assert_eq!(l.share_mem, [1.0, 2.0, 97.0]);
        assert_eq!(l.runs.len(), 1);
        let r = &l.runs[0];
        assert_eq!(r.params, s.runs[0].params);
        assert!((r.wall - 120.5).abs() < 0.01);
        assert!((r.cpu_secs - 95.25).abs() < 0.01);
        assert_eq!(r.wait_pct, Some(3.28));
        assert!(r.alive);
        assert_eq!(r.rss, 123456789);
        assert_eq!(l.users.len(), 1);
        assert_eq!(l.users[0].user, "lennart");
        assert_eq!(l.users[0].procs, 2);
        assert_eq!(l.antagonists.len(), 1);
        assert_eq!(l.antagonists[0].cmdline, "/usr/bin/bash -c \"echo hi\", ok");
        assert_eq!(l.status, TargetStatus::Active(1));
    }

    #[test]
    fn csv_escaping() {
        assert_eq!(csv("plain"), "plain");
        assert_eq!(csv("a,b"), "\"a,b\"");
        assert_eq!(csv("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn row_parser_handles_quotes() {
        assert_eq!(
            parse_row("a,\"b,c\",\"say \"\"hi\"\"\",d"),
            vec!["a", "b,c", "say \"hi\"", "d"]
        );
    }
}
