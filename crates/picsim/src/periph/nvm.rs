//! Data EEPROM (data Flash memory, DFM) of the PIC18F27/47Q10 (DS40002043F ch.12, p.147-164).
//!
//! Sourced rules:
//! * Read (12.3.1 p.157): address in NVMADRU:H:L, set RD; the data is in NVMDATL on the very next instruction, so
//!   the model completes it inside the writing instruction. A read works with NVMEN clear (NVMCON0 NVMEN, p.162).
//! * Write (12.1.2 p.147, 12.3.2 p.158): NVMCON2 = 55h, NVMCON2 = AAh, then WR = 1. WR cannot be set unless NVMEN is
//!   set. A write erases the byte first, the CPU keeps running, and on completion hardware clears WR and sets NVMIF
//!   (PIR7 bit 5, p.206). A started write is not affected by clearing NVMEN, nor by later changes to the address or data.
//! * A write to an address that is not DFM clears WR without effect and sets NVMERR (12.3.4 p.159).
//! * NVMCON2 always reads zero (12.5.3 p.164). WR and RD cannot be cleared by software (12.5.2 p.163).
//! * Byte erase and write time MEM24: 10 ms typical, 11 ms maximum (39.3.5 p.697).
//!
//! Assumptions `[A#]`, to be measured on hardware:
//! * `[A30]` The write takes the typical 10 ms. The model does not know Fosc, so the time is `write_cycles`
//!   instruction cycles (default 10 000, i.e. Fosc = 4 MHz); set it for another clock.
//! * `[A31]` The unlock sequence is tracked as 55h then AAh written to NVMCON2 and the next NVMCON1 write; instructions
//!   between those writes are not checked. Any other NVMCON2 value, or an NVMCON1 write, restarts it.
//! * `[A32]` Not modelled: program flash reads and writes, sector operations, write protection (WRTD); each one
//!   clears its control bit and adds a note. The EEPROM content does not survive `Machine::reset` (power-on).

use super::{Flag, addr};
use crate::mem::DataMem;
use devdesc::Device;
use std::collections::BTreeSet;

/// Hook numbers within this module: NVMCON1 and NVMCON2.
pub const CON1: u8 = 0;
pub const CON2: u8 = 1;

const NVMEN: u8 = 0x80;
const NVMERR: u8 = 0x10;
const WR: u8 = 0x10;
const RD: u8 = 0x01;

pub struct Nvm {
    adr: [usize; 3],
    datl: usize,
    con0: usize,
    pub con1: usize,
    pub con2: usize,
    flag: Flag,
    start: u32,
    pub eeprom: Vec<u8>,
    /// 0 = locked, 1 = 55h seen, 2 = 55h and AAh seen.
    unlock: u8,
    /// A write in progress: (completion cycle, byte offset, data).
    op: Option<(u64, usize, u8)>,
    pub write_cycles: u64,
}

impl Nvm {
    pub fn find(dev: &Device) -> Option<Nvm> {
        let r = &dev.memory.eeprom;
        Some(Nvm {
            adr: [addr(dev, "NVMADRL")?, addr(dev, "NVMADRH")?, addr(dev, "NVMADRU")?],
            datl: addr(dev, "NVMDATL")?,
            con0: addr(dev, "NVMCON0")?,
            con1: addr(dev, "NVMCON1")?,
            con2: addr(dev, "NVMCON2")?,
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
        let a = self.address(m);
        a.checked_sub(self.start).filter(|o| (*o as usize) < self.eeprom.len()).map(|o| o as usize)
    }

    /// Complete the write that is due by cycle `now`; returns the cycle it finished on.
    pub fn sync(&mut self, m: &mut DataMem, now: u64) -> Option<u64> {
        let (e, off, v) = self.op.filter(|o| o.0 <= now)?;
        self.op = None;
        self.eeprom[off] = v;
        m.mem[self.con1] &= !WR;
        self.flag.set(m);
        Some(e)
    }

    pub fn write(&mut self, k: u8, v: u8, now: u64, m: &mut DataMem, notes: &mut BTreeSet<String>) {
        if k == CON2 {
            self.unlock = match (self.unlock, v) {
                (_, 0x55) => 1,
                (1, 0xAA) => 2,
                _ => 0,
            };
            return; // NVMCON2 reads zero
        }
        let unlocked = std::mem::replace(&mut self.unlock, 0) == 2;
        if v & !(WR | RD) != 0 {
            notes.insert("NVM sector operations are not modelled; their control bits are ignored".into());
        }
        if v & RD != 0 && self.op.is_none() {
            match self.offset(m) {
                Some(o) => m.mem[self.datl] = self.eeprom[o],
                None => {
                    notes.insert("NVM reads outside the data EEPROM (program flash, IDs) are not modelled; NVMDAT is unchanged".into());
                }
            }
        }
        if v & WR != 0 && unlocked && m.mem[self.con0] & NVMEN != 0 && self.op.is_none() {
            match self.offset(m) {
                Some(o) => {
                    self.op = Some((now + self.write_cycles, o, m.mem[self.datl]));
                    m.mem[self.con1] |= WR;
                }
                None => {
                    m.mem[self.con0] |= NVMERR;
                    notes.insert("an NVM write outside the data EEPROM is not modelled; it only sets NVMERR".into());
                }
            }
        }
    }
}
