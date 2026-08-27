use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::collector::RunRow;
use crate::metrics::system_congestion_index;

/// Distribution statistics of one per-run metric. Robust statistics
/// (median / MAD / IQR) are the headline because scheduler noise is
/// heavy-tailed; mean / SD / variance are included for conventional
/// reporting.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Dist {
    pub n: usize,
    pub min: f64,
    pub p25: f64,
    pub median: f64,
    pub p75: f64,
    /// The 90th percentile: the level reached by the worst 10% of runs, so
    /// short congestion spikes are not buried in the median.
    pub p90: f64,
    pub max: f64,
    pub mean: f64,
    pub sd: f64,
    pub var: f64,
    pub mad: f64,
    /// Median absolute deviation as a percentage of the median
    /// ("typical run-to-run deviation").
    pub mad_rel: f64,
    pub iqr: f64,
}

/// Summary of how consistent the server conditions were across all
/// completed experiment runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CondSummary {
    pub cores: u64,
    pub n: usize,
    pub ci: Option<Dist>,
    pub cl: Option<Dist>,
    pub wait: Option<Dist>,
    /// Per-workload wall-time repeat statistics (grouped by cmdline).
    pub workloads: Vec<WorkloadCond>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadCond {
    pub params: String,
    pub n: usize,
    pub wall_median: f64,
    pub wall_mad_rel: f64,
    pub wall_mean: f64,
    pub wall_sd: f64,
}

pub fn build_conditions(runs: &[RunRow], cores: u64) -> CondSummary {
    let done: Vec<&RunRow> = runs.iter().filter(|r| !r.alive).collect();
    let n = done.len();
    let ci: Vec<f64> = done
        .iter()
        .map(|r| {
            system_congestion_index(r.psi[0], r.psi[1], r.psi[2], r.wait_pct.unwrap_or(0.0))
        })
        .collect();
    let cl: Vec<f64> = done.iter().filter_map(|r| r.cl).collect();
    let wait: Vec<f64> = done.iter().filter_map(|r| r.wait_pct).collect();
    let mut groups: HashMap<&str, Vec<&RunRow>> = HashMap::new();
    for r in &done {
        groups.entry(r.params.as_str()).or_default().push(r);
    }
    let mut workloads: Vec<WorkloadCond> = groups
        .into_iter()
        .map(|(params, rs)| {
            let walls: Vec<f64> = rs.iter().map(|r| r.wall).collect();
            match summarize(&walls) {
                Some(d) => WorkloadCond {
                    params: params.to_string(),
                    n: d.n,
                    wall_median: d.median,
                    wall_mad_rel: d.mad_rel,
                    wall_mean: d.mean,
                    wall_sd: d.sd,
                },
                None => WorkloadCond {
                    params: params.to_string(),
                    n: 0,
                    wall_median: 0.0,
                    wall_mad_rel: 0.0,
                    wall_mean: 0.0,
                    wall_sd: 0.0,
                },
            }
        })
        .collect();
    workloads.sort_by_key(|w| std::cmp::Reverse(w.n));
    CondSummary {
        cores,
        n,
        ci: summarize(&ci),
        cl: summarize(&cl),
        wait: summarize(&wait),
        workloads,
    }
}

pub fn summarize(values: &[f64]) -> Option<Dist> {
    if values.is_empty() {
        return None;
    }
    let mut v: Vec<f64> = values.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    let median = percentile(&v, 0.5);
    let p25 = percentile(&v, 0.25);
    let p75 = percentile(&v, 0.75);
    let p90 = percentile(&v, 0.90);
    let mean = v.iter().sum::<f64>() / n as f64;
    let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
    let sd = var.sqrt();
    let mut abs_devs: Vec<f64> = v.iter().map(|x| (x - median).abs()).collect();
    abs_devs.sort_by(f64::total_cmp);
    let mad = percentile(&abs_devs, 0.5);
    let mad_rel = if median == 0.0 {
        f64::NAN
    } else {
        mad / median.abs() * 100.0
    };
    Some(Dist {
        n,
        min: v[0],
        p25,
        median,
        p75,
        p90,
        max: v[n - 1],
        mean,
        sd,
        var,
        mad,
        mad_rel,
        iqr: p75 - p25,
    })
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let idx = q * (sorted.len() - 1) as f64;
    let lo = idx.floor() as usize;
    let hi = idx.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        sorted[lo] + (sorted[hi] - sorted[lo]) * (idx - lo as f64)
    }
}

pub fn fmt_num(x: f64) -> String {
    if x.is_nan() {
        return "–".to_string();
    }
    let s = if x.abs() < 1.0 {
        format!("{x:.2}")
    } else {
        format!("{x:.1}")
    };
    let t = s.trim_end_matches('0').trim_end_matches('.');
    if t.is_empty() {
        "0".to_string()
    } else {
        t.to_string()
    }
}


/// A paper-ready LaTeX sentence embedding the summary numbers, written so
/// that readers unfamiliar with the tool's indices understand each metric.
pub fn latex_sentence(c: &CondSummary) -> String {
    let mut s = format!(
        "Across {} completed runs on {} cores, ",
        c.n, c.cores
    );
    let mut parts: Vec<String> = Vec::new();
    if let Some(d) = &c.ci {
        parts.push(format!(
            "the median composite score of CPU, memory and I/O pressure was {} (typical run-to-run deviation {} \\%)",
            fmt_num(d.median),
            fmt_num(d.mad_rel)
        ));
    }
    if let Some(d) = &c.cl {
        parts.push(format!(
            "the median share of wall time lost to congestion was {}\\,\\% (typical deviation {} \\%)",
            fmt_num(d.median),
            fmt_num(d.mad_rel)
        ));
    }
    if let Some(d) = &c.wait {
        parts.push(format!(
            "the median scheduler wait was {}\\,\\% (typical deviation {} \\%)",
            fmt_num(d.median),
            fmt_num(d.mad_rel)
        ));
    }
    if let Some(last) = parts.pop() {
        s.push_str(&parts.join(", "));
        if !parts.is_empty() {
            s.push_str(", and ");
        }
        s.push_str(&last);
        s.push('.');
    }
    s
}

/// A LaTeX table version of the server-conditions summary.
pub fn latex_table(c: &CondSummary) -> String {
    let mut out = String::new();
    out.push_str("\\begin{table}[ht]\n\\centering\n");
    out.push_str(&format!(
        "\\caption{{Server conditions across {} completed runs on {} cores.}}\n",
        c.n, c.cores
    ));
    out.push_str("\\label{tab:conditions}\n\\begin{tabular}{lrrrrrrr}\n\\toprule\n");
    out.push_str("Metric & $n$ & Median & MAD & MAD\\% & SD & IQR & Max \\\\\n\\midrule\n");
    for (name, d) in [
        ("Congestion composite index", &c.ci),
        ("Time lost to congestion (\\%)", &c.cl),
        ("Scheduler wait (\\%)", &c.wait),
    ] {
        if let Some(d) = d {
            out.push_str(&format!(
                "{} & {} & {} & {} & {} & {} & {} & {} \\\\\n",
                name,
                d.n,
                fmt_num(d.median),
                fmt_num(d.mad),
                fmt_num(d.mad_rel),
                fmt_num(d.sd),
                fmt_num(d.iqr),
                fmt_num(d.max)
            ));
        }
    }
    out.push_str("\\bottomrule\n\\end{tabular}\n\\end{table}\n");
    out
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::RunRow;

    fn run(wall: f64, wait_pct: Option<f64>, cl: Option<f64>, psi: [f64; 3]) -> RunRow {
        RunRow {
            params: "bench_ann.py --index=hnsw".into(),
            roots: vec![],
            wall,
            cpu_secs: wall * 0.9,
            wait_secs: wait_pct.unwrap_or(0.0) / 100.0 * wall,
            wait_pct,
            cpu_pct: 50.0,
            rss: 1_000_000,
            psi,
            alive: false,
            order: 0,
            users: 2,
            cf: None,
            cl,
            ants: vec![],
            run_users: vec![],
        }
    }

    #[test]
    fn summarize_computes_robust_and_conventional_stats() {
        let vals = [1.0, 2.0, 3.0, 4.0, 100.0];
        let d = summarize(&vals).unwrap();
        assert_eq!(d.n, 5);
        assert_eq!(d.median, 3.0);
        assert_eq!(d.p25, 2.0);
        assert_eq!(d.p75, 4.0);
        assert_eq!(d.iqr, 2.0);
        assert_eq!(d.min, 1.0);
        assert_eq!(d.max, 100.0);
        assert!((d.p90 - 61.6).abs() < 1e-9, "p90 = 4 + (100-4)*0.6");
        assert!((d.mean - 22.0).abs() < 1e-9);
        assert!((d.mad - 1.0).abs() < 1e-9);
        assert!((d.mad_rel - 33.333).abs() < 0.01);
        assert!(d.sd < 40.0, "sd is inflated by the outlier but finite");
        assert!(summarize(&[]).is_none());
    }

    #[test]
    fn build_conditions_groups_workloads_and_skips_alive() {
        let mut a = run(100.0, Some(5.0), Some(2.0), [0.0; 3]);
        a.params = "w1".into();
        let mut b = run(110.0, Some(8.0), Some(3.0), [0.0; 3]);
        b.params = "w1".into();
        let mut c = run(200.0, Some(40.0), Some(20.0), [30.0, 0.0, 0.0]);
        c.params = "w2".into();
        let mut alive = run(50.0, Some(1.0), Some(0.5), [0.0; 3]);
        alive.alive = true;
        let cond = build_conditions(&[a, b, c, alive], 16);
        assert_eq!(cond.n, 3, "alive run excluded");
        assert_eq!(cond.cores, 16);
        let ci = cond.ci.unwrap();
        assert_eq!(ci.n, 3);
        assert!(ci.median > 0.0, "w2's psi pushes ci up");
        assert_eq!(cond.cl.unwrap().n, 3);
        assert_eq!(cond.wait.unwrap().n, 3);
        assert_eq!(cond.workloads.len(), 2);
        assert_eq!(cond.workloads[0].params, "w1");
        assert_eq!(cond.workloads[0].n, 2);
        assert!((cond.workloads[0].wall_median - 105.0).abs() < 1e-9);
        assert_eq!(cond.workloads[1].n, 1);
    }

    #[test]
    fn latex_sentence_embeds_numbers() {
        let mut cond = build_conditions(
            &[
                run(100.0, Some(5.0), Some(2.0), [0.0; 3]),
                run(110.0, Some(8.0), Some(3.0), [0.0; 3]),
                run(200.0, Some(40.0), Some(20.0), [30.0, 0.0, 0.0]),
            ],
            16,
        );
        cond.cores = 16;
        let s = latex_sentence(&cond);
        assert!(s.contains("3 completed runs on 16 cores"), "{s}");
        assert!(s.contains("composite score of CPU, memory and I/O pressure"), "{s}");
        assert!(s.contains("share of wall time lost to congestion"), "{s}");
        assert!(s.contains("scheduler wait"), "{s}");
        assert!(s.contains("\\%"), "{s}");
        assert!(!s.contains("benchmark"), "no workload details: {s}");
        assert!(!s.contains("SCI"), "no custom index names: {s}");
    }

    #[test]
    fn latex_table_contains_conditions_only() {
        let cond = build_conditions(
            &[
                run(100.0, Some(5.0), Some(2.0), [0.0; 3]),
                run(200.0, Some(40.0), Some(20.0), [30.0, 0.0, 0.0]),
            ],
            16,
        );
        let t = latex_table(&cond);
        assert!(t.contains("\\begin{tabular}"), "{t}");
        assert!(t.contains("Congestion composite index"), "{t}");
        assert!(t.contains("\\bottomrule"), "{t}");
        assert!(!t.contains("\\texttt{"), "no per-workload table: {t}");
    }
}
