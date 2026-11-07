//! Run-speed control: how many instruction cycles the simulated part may run by now, so a program can run at real time or at a
//! fraction of it. One instruction cycle is 4 Fosc clocks, so real time is `fosc_mhz * 1e6 / 4` cycles per second.

use std::time::{Duration, Instant};

/// Never catch up more than this much real time after a stall (a slow host, a paused window): the part would otherwise
/// run in one fast burst to make up for it.
const MAX_LAG_S: f64 = 0.25;

/// The speed choices offered by the GUI and `basil-sim --speed`: label and the fraction of real time (0 = as fast as possible).
pub const SPEEDS: [(&str, f64); 7] = [("Max", 0.0), ("1x (real time)", 1.0), ("1/2", 0.5), ("1/4", 0.25), ("1/10", 0.1), ("1/100", 0.01), ("1/1000", 0.001)];

/// Parses a `--speed` value: `max`, or a fraction of real time such as `1`, `0.5`, `1/10`, `0.001`.
pub fn parse_ratio(s: &str) -> Result<f64, String> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("max") {
        return Ok(0.0);
    }
    let bad = || format!("bad speed `{s}` (max, or a fraction of real time such as 1, 0.5 or 1/10)");
    let r = match s.split_once('/') {
        Some((a, b)) => a.trim().parse::<f64>().map_err(|_| bad())? / b.trim().parse::<f64>().map_err(|_| bad())?,
        None => s.parse::<f64>().map_err(|_| bad())?,
    };
    if r.is_finite() && r > 0.0 {
        Ok(r)
    } else {
        Err(bad())
    }
}

#[derive(Clone, Debug)]
pub struct Pace {
    /// Fraction of real time; 0 = no limit.
    pub ratio: f64,
    pub fosc_mhz: f64,
    t0: Instant,
    c0: u64,
}

impl Pace {
    pub fn new(ratio: f64, fosc_mhz: f64) -> Pace {
        Pace { ratio, fosc_mhz, t0: Instant::now(), c0: 0 }
    }

    /// True when there is no limit.
    pub fn is_max(&self) -> bool {
        self.ratio <= 0.0
    }

    /// Simulated instruction cycles per second of real time at this speed.
    pub fn cycles_per_sec(&self) -> f64 {
        self.fosc_mhz * 1e6 / 4.0 * self.ratio
    }

    /// Starts timing from now, with the part at `cycles` (call when Run starts or the speed changes).
    pub fn restart(&mut self, cycles: u64) {
        self.t0 = Instant::now();
        self.c0 = cycles;
    }

    /// Cycles the part may run now, given that it is at `cycles`. `u64::MAX` with no limit.
    pub fn due(&mut self, cycles: u64) -> u64 {
        if self.is_max() {
            return u64::MAX;
        }
        let e = self.t0.elapsed().as_secs_f64();
        self.due_at(e, cycles)
    }

    /// `due` for `elapsed` seconds since the last restart (separate so the arithmetic can be tested).
    pub fn due_at(&mut self, elapsed: f64, cycles: u64) -> u64 {
        let cps = self.cycles_per_sec();
        let want = self.c0 as f64 + elapsed * cps;
        let lag = want - cycles as f64;
        let cap = MAX_LAG_S * cps;
        if lag > cap {
            // Behind by more than the cap: forget the excess by moving the start forward, so the speed stays right from here on.
            self.t0 += Duration::from_secs_f64((lag - cap) / cps);
            return cap as u64;
        }
        if lag > 0.0 {
            lag as u64
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_time_is_a_quarter_of_fosc() {
        let p = Pace::new(1.0, 64.0);
        assert_eq!(p.cycles_per_sec(), 16e6);
        assert_eq!(Pace::new(0.1, 4.0).cycles_per_sec(), 100_000.0);
    }

    #[test]
    fn due_follows_elapsed_time_and_what_has_run() {
        let mut p = Pace::new(0.5, 4.0); // 500 000 cycles per second
        assert_eq!(p.due_at(0.1, 0), 50_000);
        assert_eq!(p.due_at(0.1, 50_000), 0);
        assert_eq!(p.due_at(0.1, 60_000), 0, "ahead of time: nothing due");
        assert_eq!(p.due_at(0.2, 50_000), 50_000);
    }

    #[test]
    fn a_stall_does_not_cause_a_burst() {
        let mut p = Pace::new(1.0, 4.0); // 1 000 000 cycles per second
        assert_eq!(p.due_at(10.0, 0), 250_000, "capped at 0.25 s");
    }

    #[test]
    fn no_limit_is_unbounded() {
        let mut p = Pace::new(0.0, 4.0);
        assert!(p.is_max());
        assert_eq!(p.due(123), u64::MAX);
    }

    #[test]
    fn restart_rebases_on_the_cycle_count() {
        let mut p = Pace::new(1.0, 4.0);
        p.restart(5_000);
        assert_eq!(p.due_at(0.0, 5_000), 0);
        assert_eq!(p.due_at(0.01, 5_000), 10_000);
    }

    #[test]
    fn speed_text() {
        assert_eq!(parse_ratio("max"), Ok(0.0));
        assert_eq!(parse_ratio("1"), Ok(1.0));
        assert_eq!(parse_ratio("1/10"), Ok(0.1));
        assert_eq!(parse_ratio("0.001"), Ok(0.001));
        assert!(parse_ratio("0").is_err());
        assert!(parse_ratio("-1").is_err());
        assert!(parse_ratio("fast").is_err());
        assert!(parse_ratio("1/0").is_err());
    }
}
