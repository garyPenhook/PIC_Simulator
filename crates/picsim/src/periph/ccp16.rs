//! CCP1-CCP4 of the PIC16 parts (capture, compare and standard PWM), the module of the Q10 (`periph/ccp.rs`) on a plain
//! register array. The register layout is the same on every PIC16 family that has it: CCPxCON (EN bit 7, OUT bit 5 read only,
//! FMT bit 4, MODE<3:0>), CCPxCAP (CTS<3:0>), CCPRxL/H and, where the part has it, CCPTMRS(0) with two bits per CCP
//! (DS40002343F 27.5.1-27.5.3 p.335-337, 28.1.1 p.340; DS40002195E 22.5.1 p.208; DS40002519D 24.5.1 p.279; DS40001839E
//! Register 29-1 p.309). The PPS output codes of the CCP pins are the families' Table 18-2 / Register 13-2 values (`code_for`).
//!
//! The rules are the Q10's (`[A61]`-`[A63]`, DS40002043F ch.22) and the PIC16 core's timers do the counting:
//! * Capture copies TMR1 into CCPRx on the selected edge of the CCPxPPS pin (CTS = 0000 only) and sets CCPxIF.
//! * Compare matches on the Timer1 clock after TMR1 reached CCPRx; MODE 0001 and 1011 then restart it at 1, so the period is
//!   CCPRx clocks (MDB, `[A61]`). 1000 sets the output, 1001 clears it, 0001 and 0010 toggle it; 1010 and 1011 only set the
//!   flag (`[A63]`, `[A128]`).
//! * PWM sets the pin at the Timer2 period match (not when the duty is 0), takes the duty from CCPRx then, and clears the pin
//!   when the 10-bit time base reaches it: pulse width = duty * Tosc * prescale (Equation 27-2), period = (T2PR + 1) * 4 *
//!   Tosc * prescale (Equation 27-1). A pulse at least as long as the period leaves the pin set (27.4.6). CCPxIF is set when
//!   the pin clears (`[A62]`).

use devdesc::Device;

pub struct Ccp16 {
    pub n: u8,
    pub con: usize,
    pub cap: usize,
    pub rl: usize,
    pub rh: usize,
    /// CCPxPPS, the capture input pin.
    pub pps: Option<usize>,
    /// CCPxIF: register address and mask.
    pub flag: (usize, u8),
    /// The RxyPPS output code of this module.
    pub code: u8,
    /// The compare / PWM output latch (the OUT bit of CCPxCON).
    pub out: bool,
    /// PWM: the cycle in which the pulse that is running ends (the pin clears), if it ends.
    pub clear_at: Option<u64>,
    /// Rising edges counted by the 1:4 and 1:16 capture prescaler.
    pub edges: u8,
    /// Cycle up to which the input pin has been looked at.
    pub scan: u64,
}

/// The RxyPPS output code of CCP `n` on the family of `name` (DS40002343F Table 18-2 p.202: CCP1 09h, CCP2 0Ah, and the same
/// on DS40002519D p.188 and DS40002686B Table 18-2 p.207; DS40002195E p.148: 01h and 02h; DS40001839E Register 13-2 p.163: 01100 to 01111). `None` where the
/// simulator has no data sheet (PIC16F180x6).
pub fn code_for(name: &str, n: u8) -> Option<u8> {
    if name.starts_with("PIC16F171") || name.starts_with("PIC16F131") || name.starts_with("PIC16F132") {
        Some(8 + n)
    } else if name.starts_with("PIC16F152") {
        Some(n)
    } else if name.starts_with("PIC16F183") {
        Some(11 + n)
    } else {
        None
    }
}

impl Ccp16 {
    pub fn find(dev: &Device, n: u8, flag: (usize, u8)) -> Option<Ccp16> {
        let addr = |name: String| dev.find_sfr(&name).map(|s| s.addr as usize);
        Some(Ccp16 {
            n,
            con: addr(format!("CCP{n}CON"))?,
            cap: addr(format!("CCP{n}CAP"))?,
            rl: addr(format!("CCPR{n}L"))?,
            rh: addr(format!("CCPR{n}H"))?,
            pps: addr(format!("CCP{n}PPS")),
            flag,
            code: code_for(&dev.device.name, n)?,
            out: false,
            clear_at: None,
            edges: 0,
            scan: 0,
        })
    }

    pub fn en(&self, m: &[u8]) -> bool {
        m[self.con] & 0x80 != 0
    }

    pub fn mode(&self, m: &[u8]) -> u8 {
        m[self.con] & 15
    }

    pub fn ccpr(&self, m: &[u8]) -> u16 {
        u16::from(m[self.rh]) << 8 | u16::from(m[self.rl])
    }

    /// The 10-bit PWM duty after the FMT alignment (Figure 27-5 p.333).
    pub fn duty(&self, m: &[u8]) -> u64 {
        let (h, l) = (u64::from(m[self.rh]), u64::from(m[self.rl]));
        if m[self.con] & 0x10 != 0 { h << 2 | l >> 6 } else { (h & 3) << 8 | l }
    }
}

/// The timer a CCP uses, from CCPTMRS (two bits per CCP; `pwm` selects the PWM timer): 1 -> Timer1/Timer2, 2 -> Timer3/Timer4,
/// 3 -> Timer5/Timer6; 0 is reserved and gives 0. Without a CCPTMRS register it is Timer1/Timer2 (DS40002195E).
pub fn timer_of(tmrs: Option<usize>, m: &[u8], n: u8, pwm: bool) -> u8 {
    let sel = tmrs.map_or(1, |a| m[a] >> (2 * (n - 1)) & 3);
    match sel {
        1 => 1 + u8::from(pwm),
        2 => 3 + u8::from(pwm),
        3 => 5 + u8::from(pwm),
        _ => 0,
    }
}

/// A compare module as the counting loop sees it.
pub struct Cmp {
    pub ccpr: u16,
    pub mode: u8,
}

pub struct Advance {
    pub cnt: u16,
    /// TMR1 rolled over from FFFFh to 0000h on a clock.
    pub overflow: bool,
    /// Matches of each module of the list.
    pub matches: Vec<u32>,
}

/// Timer1 counts `ticks` clocks from `cnt`. A module matches on the clock after TMR1 has reached its CCPR: the clock for which
/// the count before it equals CCPR (MDB, `tests/probe/ccp_cmp.S`: the flag comes one clock later than a comparison of the new
/// count would give, and a CCPR of 0 matches on the first clock); on a match of MODE 0001 or 1011 that clock clears TMR1 and
/// then counts, so TMR1 is 1 after it (MDB: the readings after a match run 1, 2, ... CCPR) and the period is CCPR clocks, the
/// first one from 0 CCPR + 1; there is no Timer1 interrupt `[A61]`.
pub fn advance(mut cnt: u16, mut ticks: u64, cmps: &[Cmp]) -> Advance {
    let mut matches = vec![0u32; cmps.len()];
    let mut overflow = false;
    while ticks > 0 {
        // clocks that only count: up to the roll-over or the clock before a match
        let mut run = 0x1_0000 - u64::from(cnt);
        for c in cmps {
            run = run.min((u64::from(c.ccpr) + 0x1_0000 - u64::from(cnt)) & 0xFFFF);
        }
        let run = run.min(ticks);
        if run > 0 {
            let new = u64::from(cnt) + run;
            overflow |= new >= 0x1_0000;
            (cnt, ticks) = (new as u16, ticks - run);
            continue;
        }
        // this clock is a match for the modules whose CCPR is the count now
        let mut clear = false;
        for (k, c) in cmps.iter().enumerate() {
            if c.ccpr == cnt {
                matches[k] += 1;
                clear |= matches!(c.mode, 1 | 11);
            }
        }
        if clear {
            cnt = 1;
        } else {
            overflow |= cnt == 0xFFFF;
            cnt = cnt.wrapping_add(1);
        }
        ticks -= 1;
    }
    Advance { cnt, overflow, matches }
}

/// The cycles (of `last+1 ..= now`, at most the last two) in which the pin of a CCP PWM that uses a Timer2 counting Fosc/4 (one
/// prescaler input per cycle) with prescaler 1:2^`sh` is set and the duty taken: the period ends the cycle T2TMR goes from T2PR
/// to 0 (Equation 27-1), but MDB (`tests/probe/ccp_pwm_seq.S`) sets the pin at the first cycle of the last T2TMR count, 2^sh - 1
/// cycles earlier; with the prescaler at 1:1 the two are one. The older Timer2 (PIC16F183xx) is `extra` = 1 cycle earlier still.
/// The timer is at `cnt` and prescaler count `pre` at cycle `last`.
/// A T2TMR above T2PR does not wrap before 256 and gives none `[A128]`.
pub fn period_sets(cnt: u8, pre: u64, pr: u8, sh: u32, extra: u64, last: u64, now: u64) -> Vec<u64> {
    if cnt > pr || now <= last {
        return Vec::new();
    }
    let period = u64::from(pr as u32 + 1) << sh;
    let at = (period - ((1u64 << sh) - 1 + extra).min(period)).max(1);
    let abs0 = u64::from(cnt) << sh | pre;
    let first = if at > abs0 { at - abs0 } else { at + period - abs0 };
    let dt = now - last;
    if first > dt {
        return Vec::new();
    }
    let n = (dt - first) / period;
    (n.saturating_sub(1)..=n).map(|k| last + first + k * period).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(cnt: u16, ticks: u64) -> (u16, bool) {
        let s = u64::from(cnt) + ticks;
        (s as u16, s >= 0x1_0000)
    }

    #[test]
    fn no_compare_counts_like_a_timer() {
        for (c, t) in [(0u16, 5u64), (0xFFFE, 1), (0xFFFE, 2), (0xFFFF, 70_000), (3, 0)] {
            let a = advance(c, t, &[]);
            assert_eq!((a.cnt, a.overflow), plain(c, t), "{c} {t}");
        }
    }

    #[test]
    fn a_match_is_the_clock_after_the_count_reached_ccpr() {
        let cmp = [Cmp { ccpr: 10, mode: 2 }];
        assert_eq!(advance(7, 3, &cmp).matches, [0], "TMR1 is 10 now, the match is next clock");
        assert_eq!(advance(7, 4, &cmp).matches, [1]);
        assert_eq!(advance(10, 1, &cmp).matches, [1], "already equal: the next clock matches");
        assert_eq!(advance(11, 0xFFFF, &cmp).matches, [0], "TMR1 is 10 again after 65535 clocks");
        assert_eq!(advance(11, 0x1_0000, &cmp).matches, [1]);
        assert_eq!(advance(0, 1, &[Cmp { ccpr: 0, mode: 8 }]).matches, [1], "CCPR = 0 matches on the first clock");
        let a = advance(0xFFFF, 1, &[Cmp { ccpr: 0xFFFF, mode: 2 }]);
        assert_eq!((a.cnt, a.overflow, a.matches[0]), (0, true, 1));
    }

    #[test]
    fn clear_mode_counts_one_to_ccpr() {
        let cmp = [Cmp { ccpr: 4, mode: 1 }];
        let a = advance(0, 4, &cmp);
        assert_eq!((a.cnt, a.matches[0], a.overflow), (4, 0, false));
        let a = advance(0, 5, &cmp);
        assert_eq!((a.cnt, a.matches[0]), (1, 1), "the clock after 4 matches and restarts at 1");
        let a = advance(0, 5 + 4 * 6 + 2, &cmp);
        assert_eq!((a.cnt, a.matches[0]), (3, 7), "then a period of CCPR = 4 clocks");
        let a = advance(0xFFF0, 1, &[Cmp { ccpr: 0xFFF0, mode: 11 }]);
        assert_eq!((a.cnt, a.matches[0], a.overflow), (1, 1, false));
        let a = advance(5, 4, &[Cmp { ccpr: 0, mode: 1 }]);
        assert_eq!((a.cnt, a.matches[0]), (9, 0), "CCPR = 0 is behind the count: it comes round at the roll-over");
        let a = advance(0, 4, &[Cmp { ccpr: 0, mode: 1 }]);
        assert_eq!((a.cnt, a.matches[0]), (4, 1), "CCPR = 0 matches once, then TMR1 runs on from 1");
        let a = advance(0, 5, &[Cmp { ccpr: 1, mode: 1 }]);
        assert_eq!((a.cnt, a.matches[0]), (1, 4), "CCPR = 1: every clock but the first");
    }

    #[test]
    fn two_modules_on_one_timer() {
        let cmp = [Cmp { ccpr: 3, mode: 2 }, Cmp { ccpr: 5, mode: 8 }];
        assert_eq!(advance(0, 7, &cmp).matches, [1, 1]);
    }

    #[test]
    fn period_sets_are_the_wraps_at_1_to_1() {
        // T2PR = 3, 1:1: cnt 0 .. 3 then 0; from cnt 2 at cycle 10 the wrap is two clocks later
        assert_eq!(period_sets(2, 0, 3, 0, 0, 10, 13), [12]);
        assert_eq!(period_sets(2, 0, 3, 0, 0, 10, 11), Vec::<u64>::new());
        assert_eq!(period_sets(2, 0, 3, 0, 0, 10, 20), [16, 20]);
        assert_eq!(period_sets(4, 0, 3, 0, 0, 0, 100), Vec::<u64>::new());
    }

    #[test]
    fn period_sets_come_early_with_a_prescaler() {
        // 1:4 (sh = 2), T2PR = 3: 16 cycles a period, the pin is set at cycle 13 of it
        assert_eq!(period_sets(0, 0, 3, 2, 0, 0, 20), [13]);
        assert_eq!(period_sets(0, 0, 3, 2, 0, 0, 12), Vec::<u64>::new());
        assert_eq!(period_sets(3, 1, 3, 2, 0, 100, 102), Vec::<u64>::new(), "cnt 3, pre 1: abs 13 is where the set is, already past");
        assert_eq!(period_sets(3, 1, 3, 2, 0, 100, 116), [116], "the next set is a period on: 13 + 16 - 13 = 16 cycles later");
        assert_eq!(period_sets(3, 0, 3, 2, 0, 100, 101), [101], "one cycle before the set point");
        assert_eq!(period_sets(0, 0, 3, 2, 1, 0, 20), [12], "the older Timer2 is a cycle earlier");
    }
}
