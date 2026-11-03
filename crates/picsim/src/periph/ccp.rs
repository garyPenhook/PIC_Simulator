//! CCP1/CCP2 of the PIC18F27Q10 (DS40002043F ch.22 p.345-359): capture, compare and standard PWM.
//!
//! Rules relied on:
//! * CCPxCON: EN bit 7, OUT bit 5 (read only, follows the output latch), FMT bit 4, MODE[3:0] (Table 22-6, p.356).
//!   MODE 0001 toggle + clear timer, 0010 toggle, 1000 set, 1001 clear, 1010/1011 pulse (not modelled), 0011 every edge,
//!   0100 falling, 0101 rising, 0110 every 4th rising, 0111 every 16th rising, 11xx PWM.
//! * Timer selection (CCPTMRS, p.359, reset 0x55): Cn = 01 -> Timer1 for capture/compare and Timer2 for PWM. The other
//!   timers (3-6) are not modelled; such a CCP is left idle with a note.
//! * Capture copies TMR1 into CCPRx on the selected edge and sets CCPxIF; only CTS = 0000 (the CCPxPPS pin) is
//!   modelled (p.346-347). The prescaler counter is cleared when the module is written (p.347, 22.2.4).
//! * Compare: the 16-bit CCPRx is compared with TMR1; MODE 0001 and 1011 clear TMR1 without a Timer1 interrupt
//!   (20.11, p.314). Clearing CCPxCON forces the compare output latch low (22.3.1, p.349).
//! * PWM (22.4, p.350-353): at the Timer2 period match (TMR2 = T2PR) the pin is set (not when the duty is 0) and the
//!   duty is latched from CCPRx; the pin clears when the 10-bit time base reaches the duty: pulse width =
//!   CCPR * TOSC * prescale (Eq. 22-2), period = (T2PR + 1) * 4 * TOSC * prescale (Eq. 22-1). A pulse width greater
//!   than or equal to the period leaves the pin unchanged. FMT = 0 right-aligned (CCPRH[1:0]:CCPRL), FMT = 1
//!   left-aligned (CCPRH:CCPRL[7:6]) (p.358).
//! Assumptions `[A#]` (to be measured against MDB):
//! * `[A61]` A compare match is seen on the Timer1 clock that makes TMR1 equal CCPRx; the flag and the pin change
//!   are on that instruction cycle. With TMR1 clearing (MODE 0001/1011) TMR1 goes to 0 on the next clock, like the
//!   Timer2 period match, so CCPRx is the period register with a period of CCPRx + 1 clocks (20.11, p.314).
//! * `[A62]` In PWM mode CCPxIF is set when the pin clears (the "set CCPIF" arrow of Figure 22-4 leaves the duty
//!   comparator); the data sheet does not say more.
//! * `[A63]` Pulse output compare modes (MODE 1010, 1011) only set the flag and clear TMR1; the pulse width is not
//!   documented, so the pin is not driven.

use super::{addr, Flag};
use crate::mem::DataMem;
use devdesc::Device;

/// Hook number: only CCPxCON needs one (the other registers are plain stored bytes).
pub const HOOKS: u8 = 1;

pub struct Ccp {
    pub n: u8,
    pub con: usize,
    pub cap: usize,
    pub rl: usize,
    pub rh: usize,
    pub pps: Option<usize>,
    pub flag: Flag,
    /// PPS output source code (Table 18-2: CCP1 = 0x05, CCP2 = 0x06).
    pub sig: u8,
    /// Cycle up to which input edges have been looked at.
    pub scan: u64,
    /// Rising edges counted by the 1:4 / 1:16 capture prescaler.
    pub edges: u8,
}

impl Ccp {
    pub fn find(dev: &Device, n: u8) -> Option<Ccp> {
        Some(Ccp {
            n,
            con: addr(dev, &format!("CCP{n}CON"))?,
            cap: addr(dev, &format!("CCP{n}CAP"))?,
            rl: addr(dev, &format!("CCPR{n}L"))?,
            rh: addr(dev, &format!("CCPR{n}H"))?,
            pps: addr(dev, &format!("CCP{n}PPS")),
            flag: Flag::find(dev, &format!("CCP{n}IF"))?,
            sig: 4 + n,
            scan: 0,
            edges: 0,
        })
    }

    pub fn en(&self, m: &DataMem) -> bool {
        m.mem[self.con] & 0x80 != 0
    }

    pub fn mode(&self, m: &DataMem) -> u8 {
        m.mem[self.con] & 15
    }

    pub fn ccpr(&self, m: &DataMem) -> u16 {
        u16::from(m.mem[self.rh]) << 8 | u16::from(m.mem[self.rl])
    }

    /// The 10-bit PWM duty value after the FMT alignment.
    pub fn duty(&self, m: &DataMem) -> u32 {
        let (h, l) = (u32::from(m.mem[self.rh]), u32::from(m.mem[self.rl]));
        if m.mem[self.con] & 0x10 != 0 { h << 2 | l >> 6 } else { (h & 3) << 8 | l }
    }
}

/// Timer a CCP uses, from CCPTMRS (two bits per CCP, `pwm` selects the PWM timer): the data sheet number of the
/// timer (1, 3, 5 for capture/compare, 2, 4, 6 for PWM), or 0 for the reserved code.
pub fn timer_of(tmrs: Option<usize>, m: &DataMem, n: u8, pwm: bool) -> u8 {
    let sel = tmrs.map_or(1, |a| m.mem[a] >> (2 * (n - 1)) & 3);
    match sel {
        1 => 1 + u8::from(pwm),
        2 => 3 + u8::from(pwm),
        3 => 5 + u8::from(pwm),
        _ => 0,
    }
}
