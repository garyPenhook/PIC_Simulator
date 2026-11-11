//! Data EEPROM (DFM) of the PIC18F0x/1xQ41 (DS40002214G ch.10, p.87-108).
//!
//! Sourced rules:
//! * NVMCON1.NVMCMD selects the operation (Table 10-1, p.87-88); GO (NVMCON0.0) starts it. Read (000) and read with
//!   post increment (001) need no unlock and no NVMIF; the byte is in NVMDATL on the next instruction (10.4.1 p.101).
//! * Write (011) and write with post increment (100) need NVMLOCK = 55h, AAh immediately before GO (10.2 p.88, 10.4.2
//!   p.102-103); a byte write erases first, the CPU keeps running, and on completion hardware clears GO and sets NVMIF
//!   (PIR0.4, p.144).
//! * While GO is set, writes to the NVM registers are blocked and GO cannot be cleared by software (10.1 p.88).
//! * WRERR (NVMCON1.7) is set when a write is attempted on an unimplemented area, while locked, or as a page
//!   operation on the DFM (10.5.2 p.106); the GO attempt then fails (10.4.5 p.104). WRERR is cleared by software and
//!   cannot be set by it (R/C/HS).
//! * NVMLOCK always reads zero (10.5.3 p.107).
//! * Byte erase and write time MEM24: 11 ms maximum, no typical value (48.3.5 p.895).
//!
//! Assumptions `[A#]`, to be measured on hardware:
//! * `[A33]` The write takes `write_cycles` instruction cycles (default 10 000 = 10 ms at Fosc 4 MHz; the data sheet
//!   gives only the 11 ms maximum, and the models do not know Fosc).
//! * `[A34]` Post increment adds 1 to NVMADR on the DFM, when the operation completes (reads at once). The data sheet
//!   does not say when a write increments (its Example 10-11 sets GO n times without polling).
//! * `[A35]` The unlock is tracked as 55h then AAh written to NVMLOCK and the next NVMCON0 write; instructions between
//!   are not checked. A GO write consumes the unlock whether or not it starts anything.
//! * `[A36]` Not modelled: write protection (WRTn), User ID and configuration space, WRERR after a Reset during a
//!   write, blocking of NVMADR/NVMDAT writes while GO is set.
//!
//! Program flash (10.3, p.88-98; array rules in `flash.rs`):
//! * Word read (000) fills NVMDAT, page read (010) fills the buffer RAM (the 256-byte GPR bank after the last one,
//!   Table 10-2 p.92). Word write (011) needs an erased word; page erase (110) and page write (101) work on the page
//!   NVMADR[21:8] selects (10.3.1 p.89, 10.3.4 p.92, 10.3.5 p.94). All need the unlock except the reads.
//! * The CPU stalls during an erase or write and GO is clear when it resumes; NVMIF is set (10.3.1 p.89, 10.3.4 p.92).
//!   Reads set no NVMIF (Table 10-1).
//! * A write or erase at an invalid address sets WRERR and the GO attempt fails (10.3 p.91, 10.3.4 p.92).
//! * Times (48.3.6 p.896) are maxima only: page write 10 ms, page erase 11 ms, word write 75 us. `[A38]` the model
//!   uses them as the durations, scaled with `write_cycles` (A33). `[A39]` a page read takes no time, and post
//!   increment adds 2 to NVMADR on the PFM (a word) when the operation ends.

use super::flash::Flash as Pfm;
use super::{Flag, addr};
use crate::mem::DataMem;
use devdesc::Device;
use std::collections::BTreeSet;

pub const CON0: u8 = 0;
pub const CON1: u8 = 1;
pub const LOCK: u8 = 2;

const GO: u8 = 1;
const WRERR: u8 = 0x80;

pub struct NvmQ {
    pub con0: usize,
    pub con1: usize,
    pub lock: usize,
    adr: [usize; 3],
    datl: usize,
    flag: Flag,
    pub(crate) start: u32,
    pub eeprom: Vec<u8>,
    /// 0 = locked, 1 = 55h seen, 2 = 55h and AAh seen.
    unlock: u8,
    /// A write in progress: (completion cycle, byte offset, data, post increment).
    op: Option<(u64, usize, u8, bool)>,
    pub write_cycles: u64,
    pfm: Option<Pfm>,
    datah: usize,
}

impl NvmQ {
    pub fn find(dev: &Device) -> Option<NvmQ> {
        let r = &dev.memory.eeprom;
        let con0 = addr(dev, "NVMCON0")?;
        Some(NvmQ {
            con0,
            con1: addr(dev, "NVMCON1")?,
            lock: addr(dev, "NVMLOCK")?,
            adr: [addr(dev, "NVMADRL")?, addr(dev, "NVMADRH")?, addr(dev, "NVMADRU")?],
            datl: addr(dev, "NVMDATL")?,
            flag: Flag::find(dev, "NVMIF")?,
            start: r.start,
            eeprom: vec![0xFF; r.size as usize],
            unlock: 0,
            op: None,
            write_cycles: 10_000,
            pfm: Pfm::find(dev),
            datah: addr(dev, "NVMDATH")?,
        })
    }

    fn address(&self, m: &DataMem) -> u32 {
        u32::from(m.mem[self.adr[0]]) | u32::from(m.mem[self.adr[1]]) << 8 | u32::from(m.mem[self.adr[2]]) << 16
    }

    fn offset(&self, m: &DataMem) -> Option<usize> {
        self.address(m).checked_sub(self.start).filter(|o| (*o as usize) < self.eeprom.len()).map(|o| o as usize)
    }

    fn post_increment(&self, m: &mut DataMem) {
        self.post_increment_by(m, 1);
    }

    fn busy(&self) -> bool {
        self.op.is_some()
    }

    /// Complete the write that is due by cycle `now`; returns the cycle it finished on.
    pub fn sync(&mut self, m: &mut DataMem, now: u64) -> Option<u64> {
        let (e, off, v, inc) = self.op.filter(|o| o.0 <= now)?;
        self.op = None;
        self.eeprom[off] = v;
        if inc {
            self.post_increment(m);
        }
        m.mem[self.con0] &= !GO;
        self.flag.set(m);
        Some(e)
    }

    /// Cycle the running write ends.
    pub fn end(&self) -> Option<u64> {
        self.op.map(|o| o.0)
    }

    pub fn hold(&self) -> Option<usize> {
        self.pfm.as_ref().map(|f| f.hold)
    }

    /// Scale a time in microseconds to instruction cycles (`write_cycles` is the 10 ms of A33).
    fn us(&self, t: u64) -> u64 {
        t * self.write_cycles / 10_000
    }

    fn post_increment_by(&self, m: &mut DataMem, n: u32) {
        let a = self.address(m).wrapping_add(n);
        for (i, r) in self.adr.iter().enumerate() {
            m.mem[*r] = (a >> (8 * i)) as u8;
        }
    }

    /// A write to NVMCON0, NVMCON1 or NVMLOCK. Returns the cycles the CPU stalls (a program flash operation).
    pub fn write(&mut self, k: u8, v: u8, now: u64, m: &mut DataMem, prog: &mut [u8], notes: &mut BTreeSet<String>) -> u64 {
        match k {
            LOCK => {
                self.unlock = match (self.unlock, v) {
                    (_, 0x55) => 1,
                    (1, 0xAA) => 2,
                    _ => 0,
                };
            }
            CON1 => {
                if self.busy() {
                    return 0; // blocked while GO is set
                }
                // WRERR can be cleared but not set by software; NVMCMD is read/write.
                m.mem[self.con1] = (m.mem[self.con1] & v & WRERR) | (v & 7);
            }
            _ => {
                if self.busy() {
                    return 0; // GO cannot be cleared by software, and the register is blocked
                }
                let unlocked = std::mem::replace(&mut self.unlock, 0) == 2;
                if v & GO != 0 {
                    return self.go(unlocked, now, m, prog, notes);
                }
            }
        }
        0
    }

    fn go(&mut self, unlocked: bool, now: u64, m: &mut DataMem, prog: &mut [u8], notes: &mut BTreeSet<String>) -> u64 {
        let cmd = m.mem[self.con1] & 7;
        let a = self.address(m);
        let dfm = self.offset(m);
        match cmd {
            0 | 1 => {
                if let Some(o) = dfm {
                    m.mem[self.datl] = self.eeprom[o];
                    if cmd == 1 {
                        self.post_increment(m);
                    }
                } else if let Some(f) = self.pfm.as_ref().filter(|f| f.valid(a)) {
                    let w = f.word_read(prog, a);
                    m.mem[self.datl] = w as u8;
                    m.mem[self.datah] = (w >> 8) as u8;
                    if cmd == 1 {
                        self.post_increment_by(m, 2);
                    }
                } else {
                    notes.insert("NVM reads of User ID, configuration and device ID space are not modelled; NVMDAT is unchanged".into());
                }
                0
            }
            2 => {
                // Page read: PFM into the buffer RAM; not supported for the DFM (10.4.1 p.102).
                match self.pfm.as_ref().filter(|f| f.valid(a)) {
                    Some(f) => f.sector_read(prog, m, a),
                    None => {
                        notes.insert("a NVM page read outside program flash does nothing in the model".into());
                    }
                }
                0
            }
            3 | 4 | 5 | 6 => {
                if !unlocked {
                    m.mem[self.con1] |= WRERR; // a write attempted while locked
                    return 0;
                }
                if let Some(o) = dfm {
                    if cmd == 3 || cmd == 4 {
                        self.op = Some((now + self.write_cycles, o, m.mem[self.datl], cmd == 4));
                        m.mem[self.con0] |= GO;
                    } else {
                        m.mem[self.con1] |= WRERR; // a page operation directed to a DFM area
                    }
                    return 0;
                }
                let Some(f) = self.pfm.as_ref() else { return 0 };
                if !f.valid(a) {
                    if f.is_aux(a) {
                        notes.insert("NVM writes to User ID and configuration space are not modelled; the operation is ignored".into());
                    } else {
                        m.mem[self.con1] |= WRERR; // unimplemented area
                    }
                    return 0;
                }
                let stall = match cmd {
                    3 | 4 => {
                        let w = u16::from(m.mem[self.datl]) | u16::from(m.mem[self.datah]) << 8;
                        f.word_write(prog, a, w, notes);
                        if cmd == 4 {
                            self.post_increment_by(m, 2);
                        }
                        self.us(75)
                    }
                    5 => {
                        f.sector_write(prog, m, a, notes);
                        self.us(10_000)
                    }
                    _ => {
                        f.sector_erase(prog, a);
                        self.us(11_000)
                    }
                };
                self.flag.set(m);
                stall
            }
            _ => 0, // 111 reserved: no operation
        }
    }
}
