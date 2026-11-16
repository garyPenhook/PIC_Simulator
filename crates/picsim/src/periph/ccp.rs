//! CCP1/CCP2 of the PIC18F27Q10 (DS40002043F ch.22 p.345-359): capture, compare and standard PWM.
//!
//! Rules relied on:
//! * CCPxCON: EN bit 7, OUT bit 5 (read only, follows the output latch), FMT bit 4, MODE[3:0] (Table 22-6, p.356).
//!   MODE 0001 toggle + clear timer, 0010 toggle, 1000 set, 1001 clear, 1010/1011 pulse (not modelled), 0011 every edge,
//!   0100 falling, 0101 rising, 0110 every 4th rising, 0111 every 16th rising, 11xx PWM.
//! * Timer selection (CCPTMRS, p.359, reset 0x55): Cn = 01 -> Timer1 for capture/compare and Timer2 for PWM. Cn = 10 -> Timer3/Timer4, 11 -> Timer5/Timer6;
//!   00 is reserved and leaves the CCP idle with a note.
//! * Capture copies the capture timer (TMR1/3/5) into CCPRx on the selected edge and sets CCPxIF; only CTS = 0000 (the CCPxPPS pin) is
//!   modelled (p.346-347). The prescaler counter is cleared when the module is written (p.347, 22.2.4).
//! * Compare: the 16-bit CCPRx is compared with TMR1; MODE 0001 and 1011 clear TMR1 without a Timer1 interrupt
//!   (20.11, p.314). Clearing CCPxCON forces the compare output latch low (22.3.1, p.349).
//! * PWM (22.4, p.350-353): at the Timer2 period match (TMR2 = T2PR) the pin is set (not when the duty is 0) and the
//!   duty is latched from CCPRx; the pin clears when the 10-bit time base reaches the duty: pulse width =
//!   CCPR * TOSC * prescale (Eq. 22-2), period = (T2PR + 1) * 4 * TOSC * prescale (Eq. 22-1). A pulse width greater
//!   than or equal to the period leaves the pin unchanged. FMT = 0 right-aligned (CCPRH[1:0]:CCPRL), FMT = 1
//!   left-aligned (CCPRH:CCPRL[7:6]) (p.358).
//! Assumptions `[A#]` (the data sheet gives no timing for them; to be checked on hardware, MDB is not a reference):
//! * `[A61]` A compare match is seen on the Timer1 clock that makes TMR1 equal CCPRx; the flag and the pin change
//!   are on that instruction cycle. With TMR1 clearing (MODE 0001/1011) TMR1 goes to 0 on the next clock, like the
//!   Timer2 period match, so CCPRx is the period register with a period of CCPRx + 1 clocks (20.11, p.314).
//! * `[A62]` In PWM mode CCPxIF is set when the pin clears (the "set CCPIF" arrow of Figure 22-4 leaves the duty
//!   comparator); the data sheet does not say more.
//! * `[A63]` Pulse output compare modes (MODE 1010, 1011) only set the flag and clear TMR1; the pulse width is not
//!   documented, so the pin is not driven.

//! PIC18(L)F2x/4xK22 CCP1..CCP5 in PWM mode (DS40001412H ch.14 p.173-203; `Ccp::find_k22`):
//! * CCPxCON = PxM<1:0> (7:6, ECCP only) | DCxB<1:0> (5:4, the duty's two LSbs) | CCPxM<3:0>; there is no EN bit, CCPxM = 0000 turns
//!   the module off, 11xx is PWM (Registers 14-1, 14-2 p.198-200). The duty is CCPRxL:DCxB (Equation 14-2 p.181), latched at the
//!   period match like the Q10's. On the ECCP modules CCP1..CCP3 CCPxM = 1110/1111 makes PxA active low; PxM other than 00 (half-
//!   and full-bridge) is not modelled (a note), nor capture and compare.
//! * Timer: CCPTMRS0 C1TSEL (1:0), C2TSEL (4:3), C3TSEL (7:6), CCPTMRS1 C4TSEL (1:0), C5TSEL (3:2): 00 = Timer1 / Timer2 (PWM),
//!   01 = Timer3 / Timer4, 10 = Timer5 / Timer6, 11 reserved (Registers 14-3, 14-4 p.201).
//! * Pins are fixed (Tables 2, 3 p.6-8): CCP1 RC2; CCP2 RC1 or RB3 by Config CCP2MX; CCP3 RB5 or (28 pins) RC6 / (40/44 pins) RE0 by
//!   CCP3MX; CCP4 RB0 / RD1; CCP5 RA4 / RE2 (`Periph::route_k22_ccp`).

use super::{addr, Flag};
use crate::mem::DataMem;
use devdesc::Device;

/// Hook number: only CCPxCON needs one (the other registers are plain stored bytes).
pub const HOOKS: u8 = 1;

#[derive(Clone)]
pub struct Ccp {
    pub n: u8,
    md: Option<(usize, u8)>,
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
    /// The PIC18(L)F2x/4xK22 CCP (see the module comment); `ecc` for the ECCP modules CCP1..CCP3.
    pub k22: bool,
    pub ecc: bool,
}

impl Ccp {
    pub fn find(dev: &Device, n: u8) -> Option<Ccp> {
        Some(Ccp {
            n,
            md: super::pmd(dev, &format!("CCP{n}MD")),
            con: addr(dev, &format!("CCP{n}CON"))?,
            cap: addr(dev, &format!("CCP{n}CAP"))?,
            rl: addr(dev, &format!("CCPR{n}L"))?,
            rh: addr(dev, &format!("CCPR{n}H"))?,
            pps: addr(dev, &format!("CCP{n}PPS")),
            flag: Flag::find(dev, &format!("CCP{n}IF"))?,
            sig: 4 + n,
            scan: 0,
            edges: 0,
            k22: false,
            ecc: false,
        })
    }

    /// CCPn of the PIC18(L)F2x/4xK22 (no CCPxCAP, no PPS).
    pub fn find_k22(dev: &Device, n: u8) -> Option<Ccp> {
        let con = addr(dev, &format!("CCP{n}CON"))?;
        Some(Ccp {
            n,
            md: super::pmd(dev, &format!("CCP{n}MD")),
            con,
            cap: con,
            rl: addr(dev, &format!("CCPR{n}L"))?,
            rh: addr(dev, &format!("CCPR{n}H"))?,
            pps: None,
            flag: Flag::find(dev, &format!("CCP{n}IF"))?,
            sig: 4 + n,
            scan: 0,
            edges: 0,
            k22: true,
            ecc: n <= 3,
        })
    }

    pub fn en(&self, m: &DataMem) -> bool {
        if super::pmd_off(m, self.md) { return false; }
        if self.k22 { m.mem[self.con] & 15 != 0 } else { m.mem[self.con] & 0x80 != 0 }
    }

    /// The PWM output is active low (an ECCP of the K22 with CCPxM = 111x, PxA active low).
    pub fn inverted(&self, m: &DataMem) -> bool {
        self.k22 && self.ecc && m.mem[self.con] & 14 == 14
    }

    pub fn mode(&self, m: &DataMem) -> u8 {
        m.mem[self.con] & 15
    }

    pub fn ccpr(&self, m: &DataMem) -> u16 {
        u16::from(m.mem[self.rh]) << 8 | u16::from(m.mem[self.rl])
    }

    /// The 10-bit PWM duty value after the FMT alignment.
    pub fn duty(&self, m: &DataMem) -> u32 {
        if self.k22 {
            return u32::from(m.mem[self.rl]) << 2 | u32::from(m.mem[self.con] >> 4 & 3);
        }
        let (h, l) = (u32::from(m.mem[self.rh]), u32::from(m.mem[self.rl]));
        if m.mem[self.con] & 0x10 != 0 { h << 2 | l >> 6 } else { (h & 3) << 8 | l }
    }
}

/// Where the timer selection of the CCPs is: the Q10's CCPTMRS (two bits per CCP, 01 = Timer1/Timer2), or the K22's CCPTMRS0 and
/// CCPTMRS1 (see the module comment).
#[derive(Clone, Copy)]
pub enum TmrSel {
    Q10(Option<usize>),
    K22(usize, usize),
}

/// Timer a CCP uses, from CCPTMRS (two bits per CCP, `pwm` selects the PWM timer): the data sheet number of the
/// timer (1, 3, 5 for capture/compare, 2, 4, 6 for PWM), or 0 for the reserved code.
pub fn timer_of(tmrs: TmrSel, m: &DataMem, n: u8, pwm: bool) -> u8 {
    let sel = match tmrs {
        TmrSel::Q10(t) => t.map_or(1, |a| m.mem[a] >> (2 * (n - 1)) & 3),
        TmrSel::K22(t0, t1) => {
            // 00 = Timer1/2 here; shifted to the Q10 codes (01 = Timer1/2) for the table below.
            let v = match n {
                1 => m.mem[t0] & 3,
                2 => m.mem[t0] >> 3 & 3,
                3 => m.mem[t0] >> 6 & 3,
                4 => m.mem[t1] & 3,
                _ => m.mem[t1] >> 2 & 3,
            };
            if v == 3 { 0 } else { v + 1 }
        }
    };
    match sel {
        1 => 1 + u8::from(pwm),
        2 => 3 + u8::from(pwm),
        3 => 5 + u8::from(pwm),
        _ => 0,
    }
}

/// PWM3/PWM4 of the PIC18F27Q10 (DS40002043F ch.23 p.360-368): 10-bit PWM on Timer2/4/6 chosen by CCPTMRS (P3TSEL, P4TSEL).
/// Same time base as the CCP PWM: set at the period match (not when the duty is 0), cleared when the 10-bit base reaches
/// DCH:DCL[7:6]; a duty at or above the period leaves the output set (23.1, 23.4, 23.5). PPS output codes 0x07 (PWM3), 0x08 (PWM4).
/// Assumptions: `[A64]` POL inverts the pin and the OUT bit alike (Figure 23-1 shows the polarity mux before the pin; the
/// "to other peripherals" tap is not placed). `[A65]` a disabled module (EN = 0) drives its output low, whatever POL is.
/// `[A66]` the double buffering of the duty changes nothing here: the duty is read when the period match is scheduled.
#[derive(Clone)]
pub struct Pwm {
    pub n: u8,
    pub con: usize,
    pub dcl: usize,
    pub dch: usize,
    pub sig: u8,
}

impl Pwm {
    pub fn find(dev: &Device, n: u8) -> Option<Pwm> {
        Some(Pwm {
            n,
            con: addr(dev, &format!("PWM{n}CON"))?,
            dcl: addr(dev, &format!("PWM{n}DCL"))?,
            dch: addr(dev, &format!("PWM{n}DCH"))?,
            sig: 4 + n,
        })
    }

    pub fn en(&self, m: &DataMem) -> bool {
        m.mem[self.con] & 0x80 != 0
    }

    pub fn pol(&self, m: &DataMem) -> bool {
        m.mem[self.con] & 0x10 != 0
    }

    pub fn duty(&self, m: &DataMem) -> u32 {
        u32::from(m.mem[self.dch]) << 2 | u32::from(m.mem[self.dcl] >> 6)
    }
}
