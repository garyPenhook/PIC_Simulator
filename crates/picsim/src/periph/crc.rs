//! CRC with memory scanner of the PIC18F27Q10 (DS40002043F ch.14 p.174-192).
//!
//! Rules relied on:
//! * Registers (p.181-192): CRCCON0 EN bit 7, GO bit 6, BUSY bit 5 (RO), ACCM bit 4, SHIFTM bit 1, FULL bit 0 (RO); CRCCON1 DLEN[7:4] (data
//!   length - 1), PLEN[3:0] (accumulator length - 1: an n-bit accumulator has PLEN = n-1, Figure 14-1 shows 0b1111 for CRC-16); CRCDAT,
//!   CRCACC, CRCSHIFT (RO), CRCXOR (bit 0 reads 1 and the LSb of the polynomial is always 1, 14.3 p.175). SCANCON0 SCANEN bit 7, SCANGO bit 6
//!   (cleared by hardware), BUSY bit 5 (RO), INVALID bit 4 (RO, reset 1), INTM bit 3, MODE[1:0]; SCANLADR/SCANHADR 22 bits (writes ignored
//!   while SCANGO = 1); SCANTRIG TSEL[4:0].
//! * Data enter MSb first (SHIFTM = 0, DLEN counts down) or LSb first (SHIFTM = 1); only DLEN+1 bits of CRCDAT are used (14.4 p.176). The shifter
//!   starts on a write to CRCDATL (14.4.1). FULL = the CRCDAT registers wait for the shifter (14.7 step 9). BUSY = shifting in progress or pending.
//!   CRCIF is set when BUSY goes 1 -> 0 (14.6 p.177).
//! * ACCM = 1 adds PLEN+1 zeros after the data; ACCM = 0 stops at the end of the data and entering the zeros gives the same value (14.5 p.176).
//!   Figure 14-1 (data 55 66 77 88, x16+x15+x2+1, seed 0, DLEN 7, PLEN 15) gives 0x32D6 (SHIFTM = 0) and 0x6BA2 (SHIFTM = 1) for ACCM = 1. These
//!   are what the direct CRC algorithm gives, which is the same as the augmented shifter followed by PLEN+1 zeros; ACCM = 0 is therefore the
//!   augmented shifter (data enter at bit 0) without the zeros (checked by the unit tests below).
//! * Scanner (14.8-14.10 p.178-179): SCANGO may be set only with the CRC EN and GO set, otherwise INVALID is set and the scan does not start.
//!   The scan ends when SCANLADR passes SCANHADR or reaches an unimplemented address (SCANGO clears, SCANIF set, 14.9); SCANGO clears when
//!   LADR > HADR. MODE 01 Burst: the CPU stalls until the scan ends; 00 Concurrent: the scanner takes memory cycles, the CPU runs between;
//!   10 Peek: only dead cycles, timing unaffected; 11 Triggered: Concurrent after a rising edge of the SCANTRIG source. INTM = 1: Concurrent and
//!   Triggered pause while an interrupt is served (SCANGO overridden), Burst is not interruptible here; Peek ignores it. SCANMD/CRCMD in PMD0 turn
//!   the modules off (14.10.8), the scanner also needs CONFIG4.SCANE.
//!
//! Assumptions `[A#]` (FACTS-SIM.md) cover what the data sheet leaves open: the shifter speed, the fetch width, the scanner's cycle use.

use super::Flag;
use crate::mem::DataMem;
use devdesc::Device;

/// Hook sub-codes (`Special::Crc`).
pub const DATL: u8 = 0;
pub const CON0: u8 = 1;
pub const SCON: u8 = 2;
pub const SLADR: u8 = 3; // L, H, U = 3, 4, 5
pub const SHADR: u8 = 6; // L, H, U = 6, 7, 8

/// A word in the shifter.
#[derive(Clone, Copy)]
struct Cur {
    word: u16,
    start: u64,
    /// Bits in this word (DLEN + 1 when it started).
    bits: u32,
}

pub struct Crc {
    datl: usize,
    dath: usize,
    acc: [usize; 2],
    shift: [usize; 2],
    xor: [usize; 2],
    con0: usize,
    con1: usize,
    ladr: [usize; 3],
    hadr: [usize; 3],
    scon: usize,
    strig: usize,
    crcif: Flag,
    scanif: Flag,
    /// PMD0 bits that switch the modules off.
    crcmd: Option<(usize, u8)>,
    scanmd: Option<(usize, u8)>,
    /// CONFIG4.SCANE.
    pub scane: bool,
    /// Size of program memory in bytes (scanner address limit).
    pub prog_len: usize,
    cur: Option<Cur>,
    held: Option<u16>,
    /// A trigger edge has been seen (Triggered mode runs as Concurrent from then on).
    triggered: bool,
    trig_prev: Option<bool>,
    /// Burst scan finishes at this cycle.
    burst_end: Option<u64>,
    /// BUSY at the last look, for the 1 -> 0 interrupt.
    was_busy: bool,
}

/// One data word through the shifter: the accumulator after `word`.
pub fn step_word(mut acc: u32, word: u16, dlen: u32, plen: u32, xor: u16, accm: bool, shiftm: bool) -> u32 {
    let n = plen + 1;
    let mask: u32 = if n >= 32 { u32::MAX } else { (1u32 << n) - 1 };
    let poly = (u32::from(xor) | 1) & mask;
    for i in 0..=dlen {
        let bit = if shiftm { u32::from(word >> i & 1) } else { u32::from(word >> (dlen - i) & 1) };
        let msb = acc >> (n - 1) & 1;
        if accm {
            // The same result as the augmented shifter followed by n zeros.
            acc = (acc << 1) & mask;
            if msb ^ bit != 0 {
                acc ^= poly;
            }
        } else {
            acc = (acc << 1 | bit) & mask;
            if msb != 0 {
                acc ^= poly;
            }
        }
    }
    acc & mask
}

impl Crc {
    pub fn find(dev: &Device) -> Option<Crc> {
        let a = |s: &str| dev.find_sfr(s).map(|x| x.addr as usize);
        let md = |bit: &str| {
            let s = dev.find_sfr("PMD0")?;
            let f = s.fields.iter().find(|f| f.name == bit)?;
            Some((s.addr as usize, 1u8 << f.pos))
        };
        Some(Crc {
            datl: a("CRCDATL")?,
            dath: a("CRCDATH")?,
            acc: [a("CRCACCL")?, a("CRCACCH")?],
            shift: [a("CRCSHIFTL")?, a("CRCSHIFTH")?],
            xor: [a("CRCXORL")?, a("CRCXORH")?],
            con0: a("CRCCON0")?,
            con1: a("CRCCON1")?,
            ladr: [a("SCANLADRL")?, a("SCANLADRH")?, a("SCANLADRU")?],
            hadr: [a("SCANHADRL")?, a("SCANHADRH")?, a("SCANHADRU")?],
            scon: a("SCANCON0")?,
            strig: a("SCANTRIG")?,
            crcif: Flag::find(dev, "CRCIF")?,
            scanif: Flag::find(dev, "SCANIF")?,
            crcmd: md("CRCMD"),
            scanmd: md("SCANMD"),
            scane: true,
            prog_len: dev.memory.program.size as usize,
            cur: None,
            held: None,
            triggered: false,
            trig_prev: None,
            burst_end: None,
            was_busy: false,
        })
    }

    /// Register address of a hook sub-code.
    pub fn regs(&self) -> [(usize, u8); 9] {
        [
            (self.datl, DATL),
            (self.con0, CON0),
            (self.scon, SCON),
            (self.ladr[0], SLADR),
            (self.ladr[1], SLADR + 1),
            (self.ladr[2], SLADR + 2),
            (self.hadr[0], SHADR),
            (self.hadr[1], SHADR + 1),
            (self.hadr[2], SHADR + 2),
        ]
    }

    /// Reset values the register table does not carry: CRCXOR bit 0 reads 1, INVALID resets to 1, SCANHADR to all ones.
    pub fn init(&self, m: &mut DataMem) {
        m.mem[self.xor[0]] |= 1;
        m.mem[self.scon] |= 0x10;
        m.mem[self.hadr[0]] = 0xFF;
        m.mem[self.hadr[1]] = 0xFF;
        m.mem[self.hadr[2]] = 0x3F;
    }

    fn off(&self, m: &DataMem, md: Option<(usize, u8)>) -> bool {
        md.is_some_and(|(a, b)| m.mem[a] & b != 0)
    }

    fn crc_on(&self, m: &DataMem) -> bool {
        m.mem[self.con0] & 0x80 != 0 && !self.off(m, self.crcmd)
    }

    fn crc_go(&self, m: &DataMem) -> bool {
        self.crc_on(m) && m.mem[self.con0] & 0x40 != 0
    }

    fn scan_on(&self, m: &DataMem) -> bool {
        self.scane && m.mem[self.scon] & 0x80 != 0 && !self.off(m, self.scanmd)
    }

    fn scan_go(&self, m: &DataMem) -> bool {
        m.mem[self.scon] & 0x40 != 0
    }

    fn params(&self, m: &DataMem) -> (u32, u32, u16, bool, bool) {
        let c1 = m.mem[self.con1];
        let c0 = m.mem[self.con0];
        let xor = u16::from(m.mem[self.xor[1]]) << 8 | u16::from(m.mem[self.xor[0]]);
        (u32::from(c1 >> 4), u32::from(c1 & 15), xor, c0 & 0x10 != 0, c0 & 2 != 0)
    }

    fn acc(&self, m: &DataMem) -> u32 {
        u32::from(m.mem[self.acc[1]]) << 8 | u32::from(m.mem[self.acc[0]])
    }

    fn set_acc(&self, m: &mut DataMem, v: u32) {
        m.mem[self.acc[0]] = v as u8;
        m.mem[self.acc[1]] = (v >> 8) as u8;
    }

    fn address(&self, m: &DataMem, r: &[usize; 3]) -> usize {
        usize::from(m.mem[r[0]]) | usize::from(m.mem[r[1]]) << 8 | usize::from(m.mem[r[2]] & 0x3F) << 16
    }

    fn set_ladr(&self, m: &mut DataMem, a: usize) {
        m.mem[self.ladr[0]] = a as u8;
        m.mem[self.ladr[1]] = (a >> 8) as u8;
        m.mem[self.ladr[2]] = (a >> 16) as u8 & 0x3F;
    }

    fn start(&mut self, word: u16, now: u64, m: &DataMem) {
        let (dlen, ..) = self.params(m);
        self.cur = Some(Cur { word, start: now, bits: dlen + 1 });
    }

    /// Refresh BUSY, FULL, CRCSHIFT and the CRCIF edge at cycle `now`.
    fn status(&mut self, m: &mut DataMem, now: u64) {
        let busy = self.cur.is_some() || self.held.is_some();
        let con0 = &mut m.mem[self.con0];
        *con0 = *con0 & !0x21 | if busy { 0x20 } else { 0 } | u8::from(self.held.is_some());
        let sh = match self.cur {
            Some(c) => {
                let done = (now.saturating_sub(c.start)).min(u64::from(c.bits)) as u32;
                let (dlen, _, _, _, shiftm) = self.params(m);
                let mask = if dlen >= 15 { 0xFFFFu32 } else { (1u32 << (dlen + 1)) - 1 };
                let w = u32::from(c.word) & mask;
                (if shiftm { w >> done } else { w << done & mask }) as u16
            }
            None => 0,
        };
        m.mem[self.shift[0]] = sh as u8;
        m.mem[self.shift[1]] = (sh >> 8) as u8;
        m.mem[self.xor[0]] |= 1;
        if self.was_busy && !busy {
            self.crcif.set(m);
        }
        self.was_busy = busy;
    }

    /// Bring the shifter to cycle `now`: words that ended go into the accumulator, the held word starts when the shifter is free.
    pub fn sync(&mut self, m: &mut DataMem, now: u64) {
        if !self.crc_on(m) {
            if self.cur.is_some() || self.held.is_some() {
                self.cur = None;
                self.held = None;
            }
            self.status(m, now);
            return;
        }
        let (dlen, plen, xor, accm, shiftm) = self.params(m);
        while let Some(c) = self.cur {
            let end = c.start + u64::from(c.bits);
            if end > now {
                break;
            }
            let acc = step_word(self.acc(m), c.word, dlen.min(c.bits - 1), plen, xor, accm, shiftm);
            self.set_acc(m, acc);
            self.cur = None;
            if self.crc_go(m)
                && let Some(w) = self.held.take()
            {
                self.cur = Some(Cur { word: w, start: end, bits: dlen + 1 });
            }
        }
        if self.cur.is_none() && self.crc_go(m) && self.held.is_some() {
            let w = self.held.take().unwrap();
            self.start(w, now, m);
        }
        self.status(m, now);
    }

    /// Latch a data word (CRCDATL write or the scanner): into the shifter when it is idle and GO is set, else held.
    fn push(&mut self, m: &mut DataMem, word: u16, now: u64) {
        if self.cur.is_none() && self.crc_go(m) {
            self.start(word, now, m);
        } else {
            self.held = Some(word); // `[A96]` a word written while the held one has not moved replaces it
        }
        self.status(m, now);
    }

    /// The CRC can take another word.
    fn ready(&self) -> bool {
        self.held.is_none()
    }

    /// The stored value of a register after a write (`v` already masked) and the side effects. Returns stall cycles (Burst scan).
    pub fn write(&mut self, sub: u8, v: u8, now: u64, m: &mut DataMem, prog: &[u8], notes: &mut std::collections::BTreeSet<String>) -> u64 {
        self.sync(m, now);
        match sub {
            DATL => {
                m.mem[self.datl] = v;
                if self.crc_on(m) {
                    let w = u16::from(m.mem[self.dath]) << 8 | u16::from(v);
                    self.push(m, w, now);
                }
            }
            CON0 => {
                let old = m.mem[self.con0];
                m.mem[self.con0] = old & 0x21 | v & !0x21;
                if old & 0x80 != 0 && v & 0x80 == 0 {
                    // EN cleared: the shifter and the scanner start over, the registers stay (14.8 step 1).
                    self.cur = None;
                    self.held = None;
                    self.was_busy = false;
                    self.abort_scan(m, true);
                } else if old & 0x40 != 0 && v & 0x40 == 0 {
                    self.abort_scan(m, true);
                }
                self.sync(m, now);
            }
            SCON => return self.write_scon(v, now, m, prog, notes),
            SLADR..=5 => {
                if !self.scan_go(m) {
                    m.mem[self.ladr[usize::from(sub - SLADR)]] = v;
                }
            }
            _ => {
                if !self.scan_go(m) {
                    m.mem[self.hadr[usize::from(sub - SHADR)]] = v;
                }
            }
        }
        0
    }

    /// The scan stops because the CRC went away: INVALID set (14.8 step 5). `flag`: SCANIF is set too when it was running.
    fn abort_scan(&mut self, m: &mut DataMem, flag: bool) {
        if self.scan_go(m) {
            m.mem[self.scon] = m.mem[self.scon] & !0x60 | 0x10;
            self.burst_end = None;
            if flag {
                self.scanif.set(m);
            }
        }
    }

    fn write_scon(&mut self, v: u8, now: u64, m: &mut DataMem, prog: &[u8], notes: &mut std::collections::BTreeSet<String>) -> u64 {
        let old = m.mem[self.scon];
        // BUSY and INVALID are read only, SCANGO is set here.
        let mut n = old & 0x30 | v & 0xCF;
        n &= !0x40;
        if v & 0x80 == 0 {
            // SCANEN cleared: the internal state of the scanner is reset, the registers stay.
            n &= !0x40;
            self.burst_end = None;
            self.triggered = false;
        }
        let keep = old & 0x40 != 0 && v & 0xC0 == 0xC0;
        m.mem[self.scon] = n | if keep { 0x40 } else { 0 };
        if !keep && old & 0x40 != 0 && v & 0x80 == 0 {
            self.scanif.set(m); // SCANEN cleared during a scan: SCANGO went 1 -> 0
            return 0;
        }
        if v & 0x40 == 0 {
            if old & 0x40 != 0 {
                // Cleared by software: the scan ends early; SCANGO 1 -> 0 sets SCANIF (14.9).
                if self.burst_end.is_none() {
                    self.scanif.set(m);
                }
                m.mem[self.scon] &= !0x40;
            }
            return 0;
        }
        if old & 0x40 != 0 {
            return 0; // already running
        }
        // SCANGO 0 -> 1.
        if !self.scan_on(m) || !self.crc_go(m) {
            m.mem[self.scon] = m.mem[self.scon] & !0x40 | 0x10;
            return 0;
        }
        let (l, h) = (self.address(m, &self.ladr), self.address(m, &self.hadr));
        if l > h {
            m.mem[self.scon] &= !0x40; // `LADR > HADR`: SCANGO is cleared
            return 0;
        }
        if l >= prog.len() {
            m.mem[self.scon] = m.mem[self.scon] & !0x40 | 0x10;
            self.scanif.set(m);
            return 0;
        }
        m.mem[self.scon] = m.mem[self.scon] & !0x10 | 0x40;
        self.triggered = false;
        self.trig_prev = None;
        let mode = m.mem[self.scon] & 3;
        if mode == 3 {
            return 0;
        }
        if mode == 1 {
            return self.burst(now, m, prog, notes);
        }
        0
    }

    /// Burst scan: the whole range now; the CPU stalls for as long as it takes. Returns the stall in cycles.
    fn burst(&mut self, now: u64, m: &mut DataMem, prog: &[u8], notes: &mut std::collections::BTreeSet<String>) -> u64 {
        let (dlen, ..) = self.params(m);
        if m.mem[self.scon] & 8 != 0 {
            notes.insert("scanner Burst mode with INTM = 1: interrupts do not pause the burst".into());
        }
        // `[A98]` One byte per word, one cycle to fetch; a fetch waits until the held slot is free. The CPU does nothing meanwhile,
        // so the shifter runs on against the same clock.
        let mut t = now;
        loop {
            self.sync(m, t);
            if !self.ready() {
                // wait for the shifter to finish its word
                if let Some(c) = self.cur {
                    t = c.start + u64::from(c.bits);
                    continue;
                }
                break; // GO off: nothing will ever move
            }
            let l = self.address(m, &self.ladr);
            let h = self.address(m, &self.hadr);
            if l > h || l >= prog.len() {
                break;
            }
            t += 1;
            self.push(m, u16::from(prog[l]), t);
            self.set_ladr(m, l + 1);
            if l >= h {
                break;
            }
        }
        let _ = dlen;
        self.finish_scan(m, true);
        let stall = t - now;
        self.burst_end = None;
        stall
    }

    fn finish_scan(&mut self, m: &mut DataMem, ran_past: bool) {
        m.mem[self.scon] &= !0x60;
        let l = self.address(m, &self.ladr);
        let h = self.address(m, &self.hadr);
        if ran_past && (l > h || l >= self.prog_len) && l >= self.prog_len {
            m.mem[self.scon] |= 0x10; // LADR rolled over to an unimplemented address
        }
        self.scanif.set(m);
    }

    /// Concurrent, Triggered and Peek scanning after an instruction that ended at cycle `now`. `dead`: cycles of the instruction in which
    /// the CPU did not use program memory (Peek). `trig`: the level of the trigger source (`None` when it is not modelled). Returns stall
    /// cycles (the scanner took memory cycles).
    pub fn scan(&mut self, m: &mut DataMem, prog: &[u8], now: u64, dead: u64, trig: Option<bool>, in_isr: bool) -> u64 {
        if !self.scan_go(m) {
            return 0;
        }
        let con = m.mem[self.scon];
        if !self.scan_on(m) || !self.crc_go(m) {
            self.abort_scan(m, true);
            return 0;
        }
        let mode = con & 3;
        if mode == 3 && !self.triggered {
            let lv = trig.unwrap_or(false);
            if lv && self.trig_prev == Some(false) {
                self.triggered = true;
            }
            self.trig_prev = Some(lv);
            if !self.triggered {
                return 0;
            }
        }
        if mode != 2 && con & 8 != 0 && in_isr {
            return 0; // INTM = 1: SCANGO is overridden during an interrupt
        }
        let mut stall = 0;
        let mut peeks = dead;
        loop {
            self.sync(m, now + stall);
            if !self.ready() {
                break;
            }
            let l = self.address(m, &self.ladr);
            let h = self.address(m, &self.hadr);
            if l > h || l >= prog.len() {
                self.finish_scan(m, true);
                break;
            }
            if mode == 2 {
                if peeks == 0 {
                    break;
                }
                peeks -= 1;
            } else {
                stall += 1;
            }
            self.push(m, u16::from(prog[l]), now + stall);
            self.set_ladr(m, l + 1);
            if l >= h {
                self.finish_scan(m, false);
                break;
            }
        }
        let c = m.mem[self.scon];
        let _ = self.strig;
        m.mem[self.scon] = c & !0x20 | if c & 0x40 != 0 && self.ready() { 0x20 } else { 0 };
        stall
    }

    /// The part slept for `d` cycles: the shifter has no clock in Sleep and carries on from where it stopped.
    pub fn thaw(&mut self, d: u64) {
        if let Some(c) = &mut self.cur {
            c.start += d;
        }
    }

    /// TSEL value of SCANTRIG.
    pub fn tsel(&self, m: &DataMem) -> u8 {
        m.mem[self.strig] & 31
    }

    /// The scanner is waiting for a trigger.
    pub fn waiting(&self, m: &DataMem) -> bool {
        self.scan_go(m) && m.mem[self.scon] & 3 == 3 && !self.triggered
    }
}

#[cfg(test)]
mod tests {
    use super::step_word;

    fn run(data: &[u8], shiftm: bool, accm: bool) -> u32 {
        data.iter().fold(0, |a, &d| step_word(a, u16::from(d), 7, 15, 0x8004, accm, shiftm))
    }

    #[test]
    fn figure_14_1() {
        let d = [0x55, 0x66, 0x77, 0x88];
        assert_eq!(run(&d, false, true), 0x32D6);
        assert_eq!(run(&d, true, true), 0x6BA2);
    }

    #[test]
    fn no_augmentation_then_zeros_gives_the_same_value() {
        let d = [0x55, 0x66, 0x77, 0x88];
        let a = run(&d, false, false);
        let z = step_word(step_word(a, 0, 7, 15, 0x8004, false, false), 0, 7, 15, 0x8004, false, false);
        assert_eq!(z, 0x32D6);
    }
}
