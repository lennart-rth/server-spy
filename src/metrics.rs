pub fn wait_overhead_pct(wait_ns: u64, cpu_ns: u64) -> Option<f64> {
    if cpu_ns == 0 {
        return None;
    }
    Some(wait_ns as f64 / cpu_ns as f64 * 100.0)
}

/// Scheduler wait as a percentage of CPU time, for second-based inputs.
/// `None` when there is no CPU time to compare against.
pub fn wait_ratio_pct(wait_secs: f64, cpu_secs: f64) -> Option<f64> {
    if cpu_secs <= 0.0 {
        return None;
    }
    Some(wait_secs / cpu_secs * 100.0)
}

pub fn psi_penalty_pct(delta_total_us: u64, wall_secs: f64) -> f64 {
    if wall_secs <= 0.0 {
        return 0.0;
    }
    delta_total_us as f64 / (wall_secs * 1_000_000.0) * 100.0
}

pub fn cpu_pct(ticks: u64, wall_secs: f64, clk_tck: u64, cores: u64) -> f64 {
    let denom = wall_secs * clk_tck as f64 * cores as f64;
    if denom <= 0.0 {
        return 0.0;
    }
    ticks as f64 / denom * 100.0
}

/// Stall seconds represented by a PSI penalty pct measured over a wall time.
pub fn stall_secs(psi_pct: f64, wall_secs: f64) -> f64 {
    (psi_pct / 100.0 * wall_secs).max(0.0)
}

/// Congestion Factor: how many times longer the run took than it would have
/// on an empty server. `1.0` = clean, `1.5` = 50% of the time stolen.
/// `None` when the run got no CPU time at all (ratio undefined).
pub fn congestion_factor(
    cpu_secs: f64,
    wait_secs: f64,
    mem_stall_secs: f64,
    io_stall_secs: f64,
) -> Option<f64> {
    if cpu_secs <= 0.0 {
        return None;
    }
    Some(1.0 + (wait_secs + mem_stall_secs + io_stall_secs) / cpu_secs)
}

/// Congestion Loss %: the share of the run's wall time stolen by congestion,
/// on a bounded 0-100 scale. Clamped because schedstat wait is summed across
/// threads and can exceed wall time. `None` when wall time is zero.
pub fn congestion_loss_pct(
    wall_secs: f64,
    wait_secs: f64,
    mem_stall_secs: f64,
    io_stall_secs: f64,
) -> Option<f64> {
    if wall_secs <= 0.0 {
        return None;
    }
    Some(
        (100.0 * (wait_secs + mem_stall_secs + io_stall_secs) / wall_secs).clamp(0.0, 100.0),
    )
}

/// Interference attribution: normalized shares (in %) of the run's congestion
/// by resource. `None` when there is no congestion to attribute.
pub fn attribution(wait_secs: f64, mem_stall_secs: f64, io_stall_secs: f64) -> Option<(f64, f64, f64)> {
    let total = wait_secs + mem_stall_secs + io_stall_secs;
    if total <= 0.0 {
        return None;
    }
    Some((
        wait_secs / total * 100.0,
        mem_stall_secs / total * 100.0,
        io_stall_secs / total * 100.0,
    ))
}

// Weights of the CI components. sched_wait overlaps with PSI-CPU (both
// measure runqueue contention), so it is weighted down to avoid double
// counting. Tune here; the formula saturates so relative weights matter.
const CI_W_CPU: f64 = 1.0;
const CI_W_MEM: f64 = 1.0;
const CI_W_IO: f64 = 1.0;
const CI_W_SCHED: f64 = 0.5;

/// System Congestion Index: saturating 0-100 over the live system-wide
/// gauges. Unlike raw percentages it does not blow up under extreme load.
pub fn system_congestion_index(cpu_some: f64, mem_some: f64, io_some: f64, sched_wait: f64) -> f64 {
    let raw = CI_W_CPU * cpu_some + CI_W_MEM * mem_some + CI_W_IO * io_some
        + CI_W_SCHED * sched_wait;
    100.0 * (1.0 - (-raw / 100.0).exp())
}

pub fn fmt_bytes(v: u64) -> String {
    if v >= 1024 * 1024 * 1024 {
        format!("{:.1}G", v as f64 / 1073741824.0)
    } else if v >= 1024 * 1024 {
        format!("{:.1}M", v as f64 / 1048576.0)
    } else if v >= 1024 {
        format!("{:.0}K", v as f64 / 1024.0)
    } else {
        format!("{v}B")
    }
}

pub fn fmt_secs(s: f64) -> String {
    if s >= 60.0 {
        format!("{:.0}m{:02.0}s", s / 60.0, s % 60.0)
    } else {
        format!("{s:.1}s")
    }
}

pub fn fmt_clock(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

pub fn fmt_pct(p: f64) -> String {
    if p < 1.0 {
        format!("{p:.3}%")
    } else if p < 10.0 {
        format!("{p:.2}%")
    } else {
        format!("{p:.1}%")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overhead_zero_cpu_is_none() {
        assert_eq!(wait_overhead_pct(100, 0), None);
    }

    #[test]
    fn overhead_twenty_percent() {
        let p = wait_overhead_pct(20_000_000_000, 100_000_000_000).unwrap();
        assert!((p - 20.0).abs() < 1e-9);
    }

    #[test]
    fn wait_ratio_secs() {
        assert_eq!(wait_ratio_pct(25.0, 100.0), Some(25.0));
        assert_eq!(wait_ratio_pct(5.0, 0.0), None);
    }

    #[test]
    fn penalty_is_time_fraction() {
        let p = psi_penalty_pct(15_000_000, 100.0);
        assert!((p - 15.0).abs() < 1e-9);
    }

    #[test]
    fn penalty_zero_wall() {
        assert_eq!(psi_penalty_pct(1000, 0.0), 0.0);
    }

    #[test]
    fn cpu_pct_uses_cores() {
        let p = cpu_pct(100, 1.0, 100, 4);
        assert!((p - 25.0).abs() < 1e-9);
    }

    #[test]
    fn bytes_formatting() {
        assert_eq!(fmt_bytes(1073741824), "1.0G");
        assert_eq!(fmt_bytes(1048576), "1.0M");
        assert_eq!(fmt_bytes(512), "512B");
    }

    #[test]
    fn stall_secs_is_pct_of_wall() {
        assert_eq!(stall_secs(25.0, 100.0), 25.0);
        assert_eq!(stall_secs(0.0, 100.0), 0.0);
        assert_eq!(stall_secs(10.0, 0.0), 0.0);
    }

    #[test]
    fn cf_clean_run_is_one() {
        let cf = congestion_factor(9.0, 0.0, 0.0, 0.0).unwrap();
        assert_eq!(cf, 1.0);
    }

    #[test]
    fn cf_fifty_percent_stolen() {
        let cf = congestion_factor(50.0, 50.0, 0.0, 0.0).unwrap();
        assert_eq!(cf, 2.0);
    }

    #[test]
    fn cf_includes_mem_and_io_stalls() {
        let cf = congestion_factor(50.0, 25.0, 10.0, 5.0).unwrap();
        assert!((cf - 1.8).abs() < 1e-9);
    }

    #[test]
    fn cf_zero_cpu_is_none() {
        assert_eq!(congestion_factor(0.0, 1.0, 0.0, 0.0), None);
    }

    #[test]
    fn cl_is_bounded_and_clamped() {
        let cl = congestion_loss_pct(100.0, 20.0, 5.0, 5.0).unwrap();
        assert!((cl - 30.0).abs() < 1e-9);
        let over = congestion_loss_pct(100.0, 200.0, 0.0, 0.0).unwrap();
        assert_eq!(over, 100.0);
        assert_eq!(congestion_loss_pct(0.0, 1.0, 0.0, 0.0), None);
    }

    #[test]
    fn attribution_normalizes_to_100() {
        let (c, m, i) = attribution(60.0, 30.0, 10.0).unwrap();
        assert!((c - 60.0).abs() < 1e-9);
        assert!((m - 30.0).abs() < 1e-9);
        assert!((i - 10.0).abs() < 1e-9);
        assert!((c + m + i - 100.0).abs() < 1e-9);
    }

    #[test]
    fn attribution_none_without_congestion() {
        assert_eq!(attribution(0.0, 0.0, 0.0), None);
    }

    #[test]
    fn ci_zero_input_is_zero() {
        assert_eq!(system_congestion_index(0.0, 0.0, 0.0, 0.0), 0.0);
    }

    #[test]
    fn ci_saturates_below_100() {
        let ci = system_congestion_index(100.0, 100.0, 100.0, 100.0);
        assert!(ci < 100.0);
        assert!(ci > 90.0);
    }

    #[test]
    fn ci_is_monotonic() {
        let low = system_congestion_index(10.0, 0.0, 0.0, 0.0);
        let high = system_congestion_index(50.0, 0.0, 0.0, 0.0);
        assert!(high > low);
    }
}
