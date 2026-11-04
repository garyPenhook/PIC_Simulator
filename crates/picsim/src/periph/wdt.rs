//! Windowed watchdog timer of the PIC18F27Q10 (DS40002043F ch.10 p.96-106, resets 9.5-9.12 p.88-90).
//!
//! Rules relied on:
//! * Modes (Table 10-1 p.98) by the WDTE configuration field: 3 always on, 2 on while awake, 1 on when WDTCON0.SEN = 1, 0 off. Off means the
//!   WWDT is cleared (Table 10-2 p.99).
//! * Time-out (WDTCON0 p.102): WDTPS[4:0] = n gives 2^(n+5) clocks, n = 0..18; 19..31 are reserved and give the minimum, 2^5. The clock is
//!   LFINTOSC 31 kHz (WDTCS = 000) or MFINTOSC 31.25 kHz (001) (WDTCON1 p.103). WDTCPS in CONFIG3 other than 11111 fixes WDTPS, WDTCWS other
//!   than 111 fixes WINDOW and WDTCCS other than 111 fixes WDTCS (read only).
//! * Window (10.4 p.98, WDTTMR p.106): WDTTMR[4:0] is the elapsed time in 32nds of the period; the window is closed while it is below
//!   4, 8, 12, 16, 20, 24, 28 for WINDOW = 110, 101, 100, 011, 010, 001, 000; WINDOW = 111 is not windowed. A CLRWDT in a closed window is a
//!   window violation and resets the part. In windowed mode the WWDT must be armed (read of WDTCON0, 10.5.1 p.99; STATE bit of WDTTMR),
//!   otherwise any CLRWDT is a violation.
//! * Cleared by: any Reset, a valid CLRWDT, being disabled, any write to WDTCON0 or WDTCON1 (10.5 p.99).
//! * WDTPSL, WDTPSH, WDTTMR (p.104-106) show PSCNT[17:0] (low byte, next byte, two upper bits), the armed state and the 5-bit window value.
//! Assumptions `[A88]..[A93]` are in FACTS-SIM.md.

use devdesc::Device;

/// Hook sub-codes (the hook codes themselves are in `periph.rs`, `wdt_hook`).
pub const CON0: u8 = 0;
pub const CON1: u8 = 1;
pub const PSL: u8 = 2;
pub const PSH: u8 = 3;
pub const TMR: u8 = 4;

pub struct Wdt {
    pub con0: usize,
    pub con1: usize,
    pub psl: usize,
    pub psh: usize,
    pub tmr: usize,
    /// WDTE configuration field.
    pub wdte: u8,
    /// WDTPS, WINDOW and WDTCS are read only (configuration words fix them).
    pub ps_fixed: bool,
    pub win_fixed: bool,
    pub cs_fixed: bool,
    /// Cycle of the last clear.
    pub start: u64,
    pub armed: bool,
    pub was_on: bool,
    /// PIC clock for the conversion of the 31 kHz time base to instruction cycles `[A88]`.
    pub fosc_mhz: f64,
}

impl Wdt {
    pub fn find(dev: &Device) -> Option<Wdt> {
        let a = |s: &str| dev.find_sfr(s).map(|x| x.addr as usize);
        Some(Wdt {
            con0: a("WDTCON0")?,
            con1: a("WDTCON1")?,
            psl: a("WDTPSL")?,
            psh: a("WDTPSH")?,
            tmr: a("WDTTMR")?,
            wdte: 0,
            ps_fixed: false,
            win_fixed: false,
            cs_fixed: false,
            start: 0,
            armed: false,
            was_on: false,
            fosc_mhz: 64.0,
        })
    }

    /// n of WDTPS: the period is 2^(n + 5) clocks.
    pub fn n(ps: u8) -> u32 {
        let n = u32::from(ps >> 1 & 31);
        if n > 18 { 0 } else { n }
    }

    /// Instruction cycles per WDT clock.
    pub fn cycles_per_clock(&self, con1: u8) -> f64 {
        let hz = if con1 >> 4 & 7 == 1 { 31_250.0 } else { 31_000.0 };
        self.fosc_mhz * 1e6 / 4.0 / hz
    }

    /// The WDT counts: by WDTE, SEN and whether the device sleeps (it never does while `step` runs).
    pub fn on(&self, con0: u8) -> bool {
        match self.wdte {
            2 | 3 => true,
            1 => con0 & 1 != 0,
            _ => false,
        }
    }

    /// Clocks elapsed since the last clear at cycle `now`.
    pub fn clocks(&self, now: u64, con1: u8) -> u64 {
        (now.saturating_sub(self.start) as f64 / self.cycles_per_clock(con1)) as u64
    }

    /// Cycle of the time-out for the current clear.
    pub fn deadline(&self, con0: u8, con1: u8) -> u64 {
        let period = (1u64 << (Self::n(con0) + 5)) as f64;
        self.start + (period * self.cycles_per_clock(con1)).ceil() as u64
    }

    /// WDTTMR[4:0]: elapsed time in 32nds of the period.
    pub fn window_value(&self, now: u64, con0: u8, con1: u8) -> u8 {
        ((self.clocks(now, con1) >> Self::n(con0)) & 31) as u8
    }

    /// Windowed mode and the window is closed at `now`, or the WWDT is not armed.
    pub fn violation(&self, now: u64, con0: u8, con1: u8) -> bool {
        let w = con1 & 7;
        if w == 7 {
            return false;
        }
        let limit = [28u8, 24, 20, 16, 12, 8, 4][usize::from(w)];
        !self.armed || self.window_value(now, con0, con1) < limit
    }
}
