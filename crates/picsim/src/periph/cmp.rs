//! Comparators C1 and C2 of the PIC18F27Q10 (DS40002043F ch.34 p.589-600).
//!
//! Rules relied on:
//! * CMxCON0 (p.597): EN bit 7, OUT bit 6 (read only), POL bit 4, HYS bit 1, SYNC bit 0. CMxCON1 (p.598): INTP bit 1, INTN bit 0.
//!   CMOUT (p.596): MC2OUT bit 1, MC1OUT bit 0 (read only).
//! * Output (Table 34-1 p.591): CxVP > CxVN gives 1 with POL = 0, inverted with POL = 1. With EN = 0 the comparator produces a 0
//!   (Figure 34-2 note, p.590); POL is applied after it, so changing POL or EN while disabled can raise an interrupt (34.5 p.592).
//! * CxIF (PIR2) is set on a rising edge of the output when INTP = 1 and a falling edge when INTN = 1 (34.5).
//! * Inputs (28-pin, Table 3 p.5; selects p.599-600). Negative: NCH 000..011 = CxIN0-..CxIN3- = RA0, RA1, RB3, RB1; 110 FVR_Buffer2;
//!   111 AVSS; 100/101 not connected. Positive: PCH 000 CxIN0+ = RA2, 001 CxIN1+ = RA3 (C1) or RB0 (C2); 101 DAC output; 110 FVR_Buffer2;
//!   111 AVSS; 010..100 not connected.
//! * FVR_Buffer2 is 1.024 / 2.048 / 4.096 V for CDAFVR = 01 / 10 / 11 and off (0 V) for 00 (FVRCON p.536; needs FVREN).
//! * The output goes to PPS code 0x0D (CMP1) / 0x0E (CMP2) (Table 18-2 p.287) and is the CWG data input ISM 0101 / 0110 and
//!   shutdown source AS4E / AS5E (low shuts down).
//! Assumptions `[A#]`:
//! * `[A79]` Pin voltages come from `[[analog]]` stimulus entries (volts, 0 V until one says otherwise). VDD is not modelled, so the FVR is
//!   never limited by it. ANSELx and TRISx are not looked at.
//! * `[A80]` No input offset, no hysteresis (HYS is stored; the data sheet gives the amount only in 39.4.9) and no response time: the
//!   output changes in the cycle the input changes. SYNC (Timer1 clock latch) is not modelled; the output stays asynchronous (note).
//! * `[A81]` The DAC output (PCH = 101) is not modelled and reads 0 V (note); not-connected selections read 0 V.
//! * `[A82]` Only the pins of the 28-pin table are mapped.

use super::Flag;
use devdesc::Device;

/// Hook codes (CMP0 + 3 * (n - 1) + sub) and the CMOUT hook.
pub const CON0: u8 = 0;
pub const NCH: u8 = 1;
pub const PCH: u8 = 2;
pub const HOOKS: u8 = 3;

pub struct Cmp {
    pub n: u8,
    pub con0: usize,
    pub con1: usize,
    pub nch: usize,
    pub pch: usize,
    pub flag: Flag,
    /// PPS output code: CMP1 = 0x0D, CMP2 = 0x0E.
    pub sig: u8,
    /// The output level now (after POL).
    pub out: bool,
    /// Cycle up to which stimulus changes have been looked at.
    pub scan: u64,
}

impl Cmp {
    pub fn find(dev: &Device, n: u8) -> Option<Cmp> {
        let a = |s: &str| dev.find_sfr(&format!("CM{n}{s}")).map(|x| x.addr as usize);
        Some(Cmp {
            n,
            con0: a("CON0")?,
            con1: a("CON1")?,
            nch: a("NCH")?,
            pch: a("PCH")?,
            flag: Flag::find(dev, &format!("C{n}IF"))?,
            sig: 12 + n,
            out: false,
            scan: 0,
        })
    }
}

/// Where a channel select leads.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Src {
    Pin(u8, u8),
    Fvr,
    Gnd,
    Dac,
    Open,
}

/// Inverting input for NCH.
pub fn neg(nch: u8) -> Src {
    match nch & 7 {
        0 => Src::Pin(b'A', 0),
        1 => Src::Pin(b'A', 1),
        2 => Src::Pin(b'B', 3),
        3 => Src::Pin(b'B', 1),
        6 => Src::Fvr,
        7 => Src::Gnd,
        _ => Src::Open,
    }
}

/// Non-inverting input of comparator `n` for PCH.
pub fn pos(n: u8, pch: u8) -> Src {
    match pch & 7 {
        0 => Src::Pin(b'A', 2),
        1 if n == 1 => Src::Pin(b'A', 3),
        1 => Src::Pin(b'B', 0),
        5 => Src::Dac,
        6 => Src::Fvr,
        7 => Src::Gnd,
        _ => Src::Open,
    }
}

/// FVR_Buffer2 voltage for FVRCON (0 when the FVR or the buffer is off).
pub fn fvr_volts(fvrcon: u8) -> f64 {
    if fvrcon & 0x80 == 0 {
        return 0.0;
    }
    match fvrcon >> 2 & 3 {
        1 => 1.024,
        2 => 2.048,
        3 => 4.096,
        _ => 0.0,
    }
}
