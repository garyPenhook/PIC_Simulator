//! ADC with computation (ADCC) in basic mode, shared by the PIC18 `Periph` and the PIC16 core (`pic16.rs`):
//! the PIC18F27/47Q10 (DS40002043F ch.33 p.546-553, 566-576), the PIC18F06/16Q41 (DS40002214G ch.40) and the PIC16
//! ADCC parts (PIC16F17126/46 DS40002343F ch.33 p.485-536, PIC16F13145 DS40002519D ch.32). Every function works on the
//! data memory as a byte slice, so the owner decides how the flag reaches the interrupt logic.
//!
//! * ADGO = 1 with ADON = 1 starts a conversion. Q10: ADPRE + ADACQ ADC clocks (0 = not included) and then 11.5 TAD
//!   (33.1.4 p.549); TAD = 2(ADCLK+1) Fosc periods (Table 33-2). Q41 and PIC16: ADPRE and ADACQ are 13 bits counted in
//!   Fosc clocks (ADACQ = 0 with ADPRE != 0 is 8192, Table 33-9 p.520) and the conversion is 14 TAD + 2 TCY (AD21,
//!   DS40002343F p.635, DS40002519D p.557).
//! * The conversion loads ADRES (right or left justified by FM), clears ADGO unless ADCONT = 1, and sets ADIF
//!   (33.2.2 p.494). Clearing ADGO or ADON terminates it, ADRES keeps its value.
//! * The input is the stimulus value of the channel in ADPCH at the end of acquisition (0 without one).
//!   PIC16F17126/46 only: with IC = 1 (differential mode, its reset value) the result is the code of the ADPCH channel
//!   minus that of the ADNCH channel, in the format FM[1:0] selects (33.1.7 p.491-492).
//! * The simple 10-bit ADC of the PIC16F152xx (DS40002195E ch.27 p.318-335) and the PIC16F183xx (DS40001839E ch.22
//!   p.236-250) has ADCON0 = CHS<5:0>, GO, ON and ADCON1 = FM, CS<2:0>, no ADCLK, no precharge or acquisition registers
//!   (the program waits the acquisition time) and no continuous mode. The conversion starts at GO and takes 11.5 TAD
//!   `[A113]`: the PIC16F183xx data sheet says 12 TAD in 22.1.4 p.237 and 11 TAD (typ.) in AD22 p.430, the PIC16F152xx
//!   11 TAD (typ.) in AD22 p.378, and MDB clears GO at 11.5 TAD on the PIC16F18346 at every clock (`tests/probe/
//!   adc_simple_n.S`); TAD is 2, 4, 8, 16, 32 or 64 Fosc periods for CS = 000, 100, 001, 101, 010, 110 (Table 27-1,
//!   Table 22-1). A SLEEP aborts a running conversion unless ADCRC is the clock (27.2.3 p.323, 22.2.3 p.240)
//!   `[A114]`: GO is cleared, ADRES and ADIF are left alone, and the converter works again afterwards (the data
//!   sheets say only "turned off, ON remains set").
//! * Not modelled, with a note: the ADCRC clock (CS = 1; simple ADC CS = x11), computation (filter, threshold,
//!   ADPREV/ADACC), triggers (ADACT) and double sampling.
//! * `[A12]` The conversion starts in the cycle of the instruction that sets ADGO. `[A13]` ADGO written 1 while
//!   ADON = 0 reads back 0 and starts nothing. `[A14]` With ADCONT = 1 the next conversion starts in the cycle the
//!   previous one ended.

use super::{addr, Flag};
use devdesc::Device;
use std::collections::BTreeSet;

pub(crate) struct Adc {
    pub con: usize,
    pub clk: usize,
    pub pre: usize,
    pub acq: usize,
    /// High bytes of ADPRE and ADACQ (Q41, PIC16: 13-bit values counted in Fosc clocks).
    pub wide: Option<(usize, usize)>,
    /// The Q41 does not start a conversion with the instruction that turns the ADC on (DS40002214G 40.2.1 p.720, `[A20]`);
    /// the PIC16 says the same (DS40002343F 33.2.1 p.494).
    pub go_needs_on: bool,
    pub pch: usize,
    /// The simple ADC: its ADCON1. Its ADCON0 has GO at bit 1 and ON at bit 0.
    pub simple: Option<usize>,
    /// ADNCH, the negative channel of the differential mode (PIC16F17126/46).
    pub nch: Option<usize>,
    pub resl: usize,
    pub resh: usize,
    pub flag: Flag,
    pub bits: u32,
    /// Cycle the running conversion ends, the value it will load (negative only in differential mode) and whether
    /// that mode was selected when it started.
    pub end: Option<(u64, i32, bool)>,
    /// Stimulus: (channel, from cycle, code), in the order given.
    pub feed: Vec<(u8, u64, u16)>,
}

impl Adc {
    /// The ADC of the device, or None when it has none of the sourced ones. The resolution is not in the device
    /// description (the 10-bit PIC16F13145 and the 12-bit PIC16F17146 use the same register layout), so it comes from
    /// the part: Q10 10 bits (DS40002043F ch.33), Q41 12 (DS40002214G ch.40), PIC16F131xx 10 (DS40002519D Figure 32-3
    /// p.443), PIC16F171xx 12 (DS40002343F Figure 33-3 p.491). Other PIC16 ADCCs (PIC16F18056/76: DS40002325C is not in
    /// the document set) are left alone.
    pub fn find(dev: &Device) -> Option<Adc> {
        if addr(dev, "ADCLK").is_none() {
            return Self::find_simple(dev);
        }
        let q41 = addr(dev, "ADCPCON").is_some();
        let pic18 = addr(dev, "PRODL").is_some();
        let name = dev.device.name.as_str();
        let bits = if q41 {
            12
        } else if pic18 {
            10
        } else if name.starts_with("PIC16F131") {
            10
        } else if name.starts_with("PIC16F171") {
            12
        } else {
            return None;
        };
        let wide = q41 || !pic18;
        let (con, clk, pch, resl, resh) = (addr(dev, "ADCON0")?, addr(dev, "ADCLK")?, addr(dev, "ADPCH")?, addr(dev, "ADRESL")?, addr(dev, "ADRESH")?);
        let flag = Flag::find(dev, "ADIF")?;
        if wide {
            let (pre, acq) = (addr(dev, "ADPREL")?, addr(dev, "ADACQL")?);
            let hi = (addr(dev, "ADPREH")?, addr(dev, "ADACQH")?);
            Some(Adc { con, clk, pre, acq, wide: Some(hi), go_needs_on: true, simple: None, pch, nch: addr(dev, "ADNCH"), resl, resh, flag, bits, end: None, feed: Vec::new() })
        } else {
            let (pre, acq) = (addr(dev, "ADPRE")?, addr(dev, "ADACQ")?);
            Some(Adc { con, clk, pre, acq, wide: None, go_needs_on: false, simple: None, pch, nch: None, resl, resh, flag, bits, end: None, feed: Vec::new() })
        }
    }

    /// The simple ADC of the PIC16F152xx and PIC16F183xx; the register layouts of the parts of one family are the same
    /// in the device descriptions, DS40001839E covers the PIC16(L)F18326/18346 and the others are taken to match.
    fn find_simple(dev: &Device) -> Option<Adc> {
        let name = dev.device.name.as_str();
        if !(name.starts_with("PIC16F152") || name.starts_with("PIC16F183") || name.starts_with("PIC16LF183")) {
            return None;
        }
        let (con, con1, resl, resh) = (addr(dev, "ADCON0")?, addr(dev, "ADCON1")?, addr(dev, "ADRESL")?, addr(dev, "ADRESH")?);
        let flag = Flag::find(dev, "ADIF")?;
        Some(Adc { con, clk: 0, pre: 0, acq: 0, wide: None, go_needs_on: true, simple: Some(con1), pch: 0, nch: None, resl, resh, flag, bits: 10, end: None, feed: Vec::new() })
    }

    /// The addresses of the registers this model handles.
    pub fn regs(&self) -> Vec<usize> {
        let mut r = vec![self.con, self.resl, self.resh];
        match self.simple {
            Some(con1) => r.push(con1),
            None => {
                r.extend([self.clk, self.pre, self.acq, self.pch]);
                r.extend(self.wide.iter().flat_map(|&(h, l)| [h, l]));
                r.extend(self.nch);
            }
        }
        r
    }

    /// TAD in Fosc periods for ADCON1's CS<2:0> of the simple ADC; None for ADCRC.
    fn simple_tad(con1: u8) -> Option<u64> {
        [Some(2), Some(8), Some(32), None, Some(4), Some(16), Some(64), None][usize::from(con1 >> 4 & 7)]
    }

    /// A write to ADCON0 (`v` already masked to the implemented bits) at cycle `now`.
    pub fn write_con(&mut self, mem: &mut [u8], v: u8, now: u64, notes: &mut BTreeSet<String>) {
        let (on_bit, go_bit) = if self.simple.is_some() { (1, 2) } else { (0x80, 1) };
        let was_on = mem[self.con] & on_bit != 0;
        let (on, go) = (v & on_bit != 0, v & go_bit != 0 && (!self.go_needs_on || was_on));
        let mut new = v & !go_bit;
        if !(on && go) {
            self.end = None;
        } else {
            new |= go_bit;
            if self.simple.is_some_and(|c1| Self::simple_tad(mem[c1]).is_none()) || (self.simple.is_none() && v & 0x10 != 0) {
                notes.insert("the ADC FRC clock (ADCS = 1) is not modelled; ADGO never clears".into());
            } else if self.end.is_none() {
                mem[self.con] = new;
                self.start(mem, now);
            }
        }
        mem[self.con] = new;
    }

    /// Start a conversion at cycle `t`: end cycle and the code it will load.
    fn start(&mut self, mem: &[u8], t: u64) {
        if let Some(con1) = self.simple {
            // the input is the one at the start; the acquisition time is the program's business
            let tad = Self::simple_tad(mem[con1]).unwrap_or(2);
            let ch = mem[self.con] >> 2;
            let code = i32::from(self.feed.iter().filter(|(c, when, _)| *c == ch && *when <= t).next_back().map_or(0, |f| f.2));
            self.end = Some((t + (23 * tad).div_ceil(8), code.min(1023), false)); // 11.5 TAD of tad Fosc periods, 4 to a cycle
            return;
        }
        let n = u64::from(mem[self.clk] & 0x3F) + 1; // TAD = 2n Fosc periods
        let (sample, end);
        if let Some((ph, ah)) = self.wide {
            // precharge and acquisition are counted in Fosc clocks (ADACQ = 0 with ADPRE != 0 is 8192),
            // the conversion is 14 TAD + 2 TCY. `[A18]`
            let pre = u64::from(mem[ph] & 0x1F) << 8 | u64::from(mem[self.pre]);
            let mut acq = u64::from(mem[ah] & 0x1F) << 8 | u64::from(mem[self.acq]);
            if acq == 0 && pre != 0 {
                acq = 8192;
            }
            sample = t + (pre + acq).div_ceil(4);
            end = sample + (2 * n * 14).div_ceil(4) + 2;
        } else {
            let lead = u64::from(mem[self.pre]) + u64::from(mem[self.acq]);
            sample = t + (2 * n * lead).div_ceil(4);
            end = t + (n * (2 * lead + 23)).div_ceil(4);
        }
        let max = (1i32 << self.bits) - 1;
        let at = |ch: u8| i32::from(self.feed.iter().filter(|(c, when, _)| *c == ch && *when <= sample).next_back().map_or(0, |f| f.2)).min(max);
        let diff = self.nch.is_some() && mem[self.con] & 2 != 0;
        let code = if diff { at(mem[self.pch] & 0x3F) - at(mem[self.nch.unwrap()] & 0x3F) } else { at(if self.wide.is_some() { mem[self.pch] } else { mem[self.pch] & 0x3F }) };
        self.end = Some((end, code, diff));
    }

    /// The 16-bit ADRES word for result `v` in the format ADCON0 selects (single-ended: FM bit; differential:
    /// FM[1:0] = 00 left/two's complement, 01 right/two's complement, 10 left/sign-magnitude, 11 right/sign-magnitude,
    /// DS40002343F 33.1.7 p.491-492). `[A112]` the difference of the two codes is taken as it is (the data sheet gives
    /// no transfer function) and limited to the range of the format: -2047 .. 2046 in two's complement (Equation 33-4
    /// p.493), -2047 .. 2047 in sign/magnitude (Equation 33-3 p.492).
    fn word(&self, con: u8, v: i32, diff: bool) -> u16 {
        if !diff {
            return if con & 4 != 0 { v as u16 } else { (v << (16 - self.bits)) as u16 };
        }
        let (neg, mag) = (v < 0, v.unsigned_abs().min(2047) as u16);
        match con >> 2 & 3 {
            0 => ((v.clamp(-2047, 2046) as u16) & 0xFFF) << 4,
            1 => v.clamp(-2047, 2046) as i16 as u16,
            2 => u16::from(neg) << 15 | mag << 4,
            _ => (if neg { 0xF800 } else { 0 }) | mag,
        }
    }

    /// Finish the conversions that end by cycle `now`: ADRES, ADGO and the next conversion of continuous mode. The
    /// cycles they ended in are pushed to `ev`; true when ADIF has to be raised.
    pub fn sync(&mut self, mem: &mut [u8], now: u64, ev: &mut Vec<u64>) -> bool {
        let mut done = false;
        while let Some((e, v, diff)) = self.end.filter(|(e, ..)| *e <= now) {
            self.end = None;
            let fm = match self.simple {
                Some(con1) => mem[con1] >> 5 & 4, // FM is ADCON1 bit 7
                None => mem[self.con],
            };
            let word = self.word(fm, v, diff);
            mem[self.resl] = word as u8;
            mem[self.resh] = (word >> 8) as u8;
            done = true;
            ev.push(e);
            if self.simple.is_none() && mem[self.con] & 0x40 != 0 {
                self.start(mem, e); // [A14]
            } else {
                mem[self.con] &= if self.simple.is_some() { !2 } else { !1 };
            }
        }
        done
    }

    /// SLEEP: the simple ADC aborts a running conversion, the ADCC's waits for the clock `[A113]`.
    pub fn enter_sleep(&mut self, mem: &mut [u8], notes: &mut BTreeSet<String>) {
        if self.end.is_some() {
            if self.simple.is_some() {
                self.end = None;
                mem[self.con] &= !2;
                notes.insert("SLEEP aborts the running ADC conversion (ADCRC is not modelled)".into());
            } else {
                notes.insert("a conversion running at SLEEP waits for the wake-up (ADCRC is not modelled; the data sheet does not say)".into());
            }
        }
    }

    /// The part slept for `d` cycles with the conversion running: it goes on afterwards.
    pub fn shift(&mut self, d: u64) {
        if let Some((e, ..)) = self.end.as_mut() {
            *e += d;
        }
    }

    /// Stimulus: the input of channel `ch` is code `v` (full scale = the part's resolution) from cycle `at` on.
    pub fn push_feed(&mut self, ch: u8, at: u64, v: u16) {
        self.feed.push((ch, at, v));
        self.feed.sort_by_key(|f| f.1); // stable: same cycle keeps file order
    }
}
