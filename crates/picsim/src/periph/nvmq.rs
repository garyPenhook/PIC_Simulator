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
//! * `[A36]` Not modelled: program flash and page operations, write protection (WRTn), WRERR after a Reset during a
//!   write, blocking of NVMADR/NVMDAT writes while GO is set. Each page or PFM request clears GO and adds a note.

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
    start: u32,
    pub eeprom: Vec<u8>,
    /// 0 = locked, 1 = 55h seen, 2 = 55h and AAh seen.
    unlock: u8,
    /// A write in progress: (completion cycle, byte offset, data, post increment).
    op: Option<(u64, usize, u8, bool)>,
    pub write_cycles: u64,
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
        })
    }

    fn address(&self, m: &DataMem) -> u32 {
        u32::from(m.mem[self.adr[0]]) | u32::from(m.mem[self.adr[1]]) << 8 | u32::from(m.mem[self.adr[2]]) << 16
    }

    fn offset(&self, m: &DataMem) -> Option<usize> {
        self.address(m).checked_sub(self.start).filter(|o| (*o as usize) < self.eeprom.len()).map(|o| o as usize)
    }

    fn post_increment(&self, m: &mut DataMem) {
        let a = self.address(m).wrapping_add(1);
        for (i, r) in self.adr.iter().enumerate() {
            m.mem[*r] = (a >> (8 * i)) as u8;
        }
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

    pub fn write(&mut self, k: u8, v: u8, now: u64, m: &mut DataMem, notes: &mut BTreeSet<String>) {
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
                    return; // blocked while GO is set
                }
                // WRERR can be cleared but not set by software; NVMCMD is read/write.
                m.mem[self.con1] = (m.mem[self.con1] & v & WRERR) | (v & 7);
            }
            _ => {
                if self.busy() {
                    return; // GO cannot be cleared by software, and the register is blocked
                }
                let unlocked = std::mem::replace(&mut self.unlock, 0) == 2;
                if v & GO != 0 {
                    self.go(unlocked, now, m, notes);
                }
            }
        }
    }

    fn go(&mut self, unlocked: bool, now: u64, m: &mut DataMem, notes: &mut BTreeSet<String>) {
        let cmd = m.mem[self.con1] & 7;
        match cmd {
            0 | 1 => match self.offset(m) {
                Some(o) => {
                    m.mem[self.datl] = self.eeprom[o];
                    if cmd == 1 {
                        self.post_increment(m);
                    }
                }
                None => {
                    notes.insert("NVM reads outside the data EEPROM (program flash, IDs) are not modelled; NVMDAT is unchanged".into());
                }
            },
            3 | 4 => {
                if !unlocked {
                    m.mem[self.con1] |= WRERR; // a write attempted while locked
                } else if let Some(o) = self.offset(m) {
                    self.op = Some((now + self.write_cycles, o, m.mem[self.datl], cmd == 4));
                    m.mem[self.con0] |= GO;
                } else {
                    m.mem[self.con1] |= WRERR; // unimplemented area
                    notes.insert("an NVM write outside the data EEPROM is not modelled; it only sets WRERR".into());
                }
            }
            2 | 5 | 6 => {
                if self.offset(m).is_some() {
                    m.mem[self.con1] |= WRERR; // a page operation directed to a DFM area
                } else {
                    notes.insert("NVM page operations on program flash are not modelled; GO is cleared".into());
                }
            }
            _ => {} // 111 reserved: no operation
        }
    }
}
