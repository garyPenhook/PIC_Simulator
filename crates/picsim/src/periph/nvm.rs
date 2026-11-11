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
//! * `[A32]` Not modelled: write protection (WRTD/WRTn), User ID and Configuration space, NVMERR after a Reset during
//!   a write, errata 1.9.1. The EEPROM and flash content do not survive `Machine::reset` (power-on).
//!
//! Program flash (12.1, p.142-156; array rules in `flash.rs`):
//! * RD at a program address reads the word into NVMDATH:L, with NVMEN clear too (12.5.1 p.162, 12.5.2 p.163).
//! * Unlock pairs and control bits (Table 12-4 p.147): word write 55h AAh WR, sector write DDh 22h SECWR, sector
//!   erase CCh 33h SECER, sector read BBh 44h SECRD. These bits are gated by NVMEN (12.1.4.5 p.156); the unlock
//!   for one operation does not enable another.
//! * The CPU stalls until a PFM operation ends, then resumes with the next instruction; the control bit is clear
//!   after it and NVMIF is set (12.1.3.1 p.149, 12.1.4 p.151). NVMIF is not set by the sector read or table writes.
//! * Invalid address (beyond program memory) for SECER, SECWR or WR: the bit is cleared without effect and NVMERR is
//!   set (12.1.3.1 p.149, 12.1.4 p.151).
//! * Times, typical (39.3.5 p.697): word write 50 us (MEM37), sector write 6 ms (MEM35), sector erase 10 ms (MEM36).
//!   `[A37]` the sector read takes no time (the data sheet gives none). Times scale with `write_cycles` (A30).

use super::flash::Flash as Pfm;
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
const SECRD: u8 = 0x02;
const SECWR: u8 = 0x20;
const SECER: u8 = 0x40;

pub struct Nvm {
    adr: [usize; 3],
    datl: usize,
    con0: usize,
    pub con1: usize,
    pub con2: usize,
    flag: Flag,
    pub(crate) start: u32,
    pub eeprom: Vec<u8>,
    /// First unlock byte seen (55h, CCh, DDh or BBh).
    first: Option<u8>,
    /// The NVMCON1 control bit the completed unlock pair enables.
    armed: Option<u8>,
    pfm: Option<Pfm>,
    datah: usize,
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
            first: None,
            armed: None,
            pfm: Pfm::find(dev),
            datah: addr(dev, "NVMDATH")?,
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

    /// Cycle the running write ends.
    pub fn end(&self) -> Option<u64> {
        self.op.map(|o| o.0)
    }

    pub fn hold(&self) -> Option<usize> {
        self.pfm.as_ref().map(|f| f.hold)
    }

    /// Scale a time in microseconds to instruction cycles (`write_cycles` is the 10 ms of A30).
    fn us(&self, t: u64) -> u64 {
        t * self.write_cycles / 10_000
    }

    /// A write to NVMCON1 or NVMCON2. Returns the cycles the CPU stalls (a program flash operation).
    pub fn write(&mut self, k: u8, v: u8, now: u64, m: &mut DataMem, prog: &mut [u8], notes: &mut BTreeSet<String>) -> u64 {
        if k == CON2 {
            let (first, armed) = match (self.first, v) {
                (_, 0x55 | 0xCC | 0xDD | 0xBB) => (Some(v), None),
                (Some(0x55), 0xAA) => (None, Some(WR)),
                (Some(0xCC), 0x33) => (None, Some(SECER)),
                (Some(0xDD), 0x22) => (None, Some(SECWR)),
                (Some(0xBB), 0x44) => (None, Some(SECRD)),
                _ => (None, None),
            };
            self.first = first;
            self.armed = armed;
            return 0; // NVMCON2 reads zero
        }
        let armed = self.armed.take();
        self.first = None;
        let enabled = m.mem[self.con0] & NVMEN != 0;
        let a = self.address(m);
        let in_dfm = self.offset(m).is_some();
        if v & RD != 0 && self.op.is_none() {
            match (self.offset(m), &self.pfm) {
                (Some(o), _) => m.mem[self.datl] = self.eeprom[o],
                (None, Some(f)) if f.valid(a) => {
                    let w = f.word_read(prog, a);
                    m.mem[self.datl] = w as u8;
                    m.mem[self.datah] = (w >> 8) as u8;
                }
                _ => {
                    notes.insert("NVM reads of User ID, configuration and device ID space are not modelled; NVMDAT is unchanged".into());
                }
            }
        }
        // The operation this write asks for: its control bit must match the unlock and NVMEN must be set.
        let bit = [WR, SECWR, SECER, SECRD].into_iter().find(|b| v & b != 0 && armed == Some(*b));
        if bit == Some(WR) && in_dfm {
            if enabled && self.op.is_none() {
                self.op = Some((now + self.write_cycles, self.offset(m).unwrap(), m.mem[self.datl]));
                m.mem[self.con1] |= WR;
            }
            return 0;
        }
        let Some(bit) = bit.filter(|_| enabled && self.op.is_none()) else { return 0 };
        let Some(f) = &self.pfm else { return 0 };
        if !f.valid(a) {
            if f.is_aux(a) {
                notes.insert("NVM writes to User ID and configuration space are not modelled; the operation is ignored".into());
            } else {
                m.mem[self.con0] |= NVMERR;
            }
            return 0;
        }
        let stall = match bit {
            SECRD => {
                f.sector_read(prog, m, a);
                return 0;
            }
            SECER => {
                f.sector_erase(prog, a);
                self.us(10_000)
            }
            SECWR => {
                f.sector_write(prog, m, a, notes);
                self.us(6_000)
            }
            _ => {
                f.word_write(prog, a, u16::from(m.mem[self.datl]) | u16::from(m.mem[self.datah]) << 8, notes);
                self.us(50)
            }
        };
        self.flag.set(m);
        stall
    }
}
