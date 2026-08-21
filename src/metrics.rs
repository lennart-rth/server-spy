pub fn wait_overhead_pct(wait_ns: u64, cpu_ns: u64) -> Option<f64> {
    if cpu_ns == 0 {
        return None;
    }
    Some(wait_ns as f64 / cpu_ns as f64 * 100.0)
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
}
