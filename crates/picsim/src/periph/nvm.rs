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
//! * `[A30]` The write takes the typical 10 ms, counted in instruction cycles at the board's Fosc when the write starts
//!   (`fosc_mhz`; a clock change during the write does not change its end).
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
//!   `[A37]` the sector read takes no time (the data sheet gives none). Times are converted at Fosc as in A30.

//! PIC18(L)F2x/4xK22 (DS40001412H ch.6 and 7, p.90-103; `Nvm::find_k22`), an older controller with the same interface:
//! * EECON1 = EEPGD (7) | CFGS (6) | FREE (4) | WRERR (3) | WREN (2) | WR (1) | RD (0); WR and RD can only be set (Register 7-1 p.100).
//!   EECON2 is not a register and reads 0; the unlock is 55h, AAh to EECON2, then WR (7.4, Example 7-2 p.101). WR is inhibited
//!   unless WREN was already set: both cannot be set by one instruction (7.4). `[A142]` the unlock is tracked as for the Q10
//!   (`[A31]`): AAh right after 55h, and the next EECON1 write.
//! * Data EEPROM (EEPGD = CFGS = 0): RD loads EEDATA from EEADRH:EEADR at once (7.3 p.101); a write erases and programs the byte
//!   while the CPU runs, then WR clears and EEIF (PIR2 bit 4) is set (7.4). `[A143]` it takes TDEW typical, 3 ms (D175 p.430).
//!   The address wraps at the array size (EEADRH has 2 bits on the 1 KB parts, none on the 256-byte ones).
//! * Program flash (EEPGD = 1, CFGS = 0): TBLPTR<21:6> picks a 64-byte block. FREE = 1 erases it (FFh), FREE then clears; FREE = 0
//!   writes the 64 holding registers into it and puts them back to FFh, a holding byte of FFh leaving its flash byte as it is
//!   (6.5, 6.6 and note p.95-96). The CPU stalls, `[A144]` for TIW typical, 2 ms (D183 p.430; 6.5.1 step 7 "about 2 ms"), and
//!   EEIF is set (6.2.1 note p.91). TBLWT writes the holding register TBLPTR<5:0> (Figure 6-2, 6.3.2 p.93), see `Pic18::tblwt`.
//! * Not modelled: CFGS = 1 (configuration writes, a note), write protection, WRERR after a Reset during a write.

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

#[derive(Clone)]
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
    /// Fosc in MHz, set by `Periph` before each write (operation times are taken at their start).
    pub fosc_mhz: f64,
    /// The PIC18(L)F2x/4xK22 controller: its 64 holding registers and the program memory size (see the module comment).
    k22: Option<K22>,
}

#[derive(Clone)]
struct K22 {
    hold: [u8; 64],
    tblptr: usize,
    prog_size: usize,
}

const K_RD: u8 = 0x01;
const K_WR: u8 = 0x02;
const K_WREN: u8 = 0x04;
const K_FREE: u8 = 0x10;
const K_CFGS: u8 = 0x40;
const K_EEPGD: u8 = 0x80;

impl Nvm {
    /// Something is under way that `sync` must follow cycle by cycle (`Periph::tick` does not skip a sync meanwhile).
    pub fn busy(&self) -> bool {
        self.op.is_some()
    }

    /// The EECON1/EECON2 controller of the PIC18(L)F2x/4xK22, or None.
    pub fn find_k22(dev: &Device) -> Option<Nvm> {
        let r = &dev.memory.eeprom;
        let (con1, con2, adrl, dat) = (addr(dev, "EECON1")?, addr(dev, "EECON2")?, addr(dev, "EEADR")?, addr(dev, "EEDATA")?);
        let adrh = addr(dev, "EEADRH").unwrap_or(usize::MAX);
        let tblptr = addr(dev, "TBLPTRL")?;
        Some(Nvm {
            adr: [adrl, adrh, usize::MAX],
            datl: dat,
            con0: con1,
            con1,
            con2,
            flag: Flag::find(dev, "EEIF")?,
            start: r.start,
            eeprom: vec![0xFF; r.size as usize],
            first: None,
            armed: None,
            pfm: None,
            datah: dat,
            op: None,
            fosc_mhz: 64.0,
            k22: Some(K22 { hold: [0xFF; 64], tblptr, prog_size: dev.memory.program.size as usize }),
        })
    }

    /// TBLWT: the holding register TBLPTR<5:0> on the K22. False on a part whose holding registers are RAM.
    pub fn latch(&mut self, p: u32, v: u8) -> bool {
        match &mut self.k22 {
            Some(k) => {
                k.hold[(p & 63) as usize] = v;
                true
            }
            None => false,
        }
    }

    fn k22_write(&mut self, k: u8, v: u8, now: u64, m: &mut DataMem, prog: &mut [u8], notes: &mut BTreeSet<String>) -> u64 {
        if k == CON2 {
            let (first, armed) = match (self.first, v) {
                (_, 0x55) => (Some(v), None),
                (Some(0x55), 0xAA) => (None, Some(K_WR)),
                _ => (None, None),
            };
            self.first = first;
            self.armed = armed;
            return 0;
        }
        let armed = self.armed.take().is_some();
        self.first = None;
        let old = m.mem[self.con1];
        // WR and RD cannot be cleared by software; RD completes at once.
        m.mem[self.con1] = v & !(K_RD | K_WR) | old & K_WR;
        let ee = v & (K_EEPGD | K_CFGS) == 0;
        let size = self.eeprom.len().max(1);
        let hi = if self.adr[1] == usize::MAX { 0 } else { usize::from(m.mem[self.adr[1]]) };
        let off = (hi << 8 | usize::from(m.mem[self.adr[0]])) % size;
        if v & K_RD != 0 && ee && self.op.is_none() {
            m.mem[self.datl] = self.eeprom[off];
        }
        if v & K_WR == 0 || !armed || old & K_WREN == 0 || self.op.is_some() {
            return 0;
        }
        if v & K_CFGS != 0 {
            notes.insert("EECON1 writes to the configuration registers (CFGS = 1) are not modelled; the operation is ignored".into());
            return 0;
        }
        if ee {
            self.op = Some((now + self.us(3_000), off, m.mem[self.datl]));
            m.mem[self.con1] |= K_WR;
            return 0;
        }
        let k = self.k22.as_mut().unwrap();
        let t = k.tblptr;
        let a = (usize::from(m.mem[t]) | usize::from(m.mem[t + 1]) << 8 | usize::from(m.mem[t + 2] & 0x3F) << 16) & !63;
        if a + 64 > k.prog_size {
            notes.insert("a program flash erase or write outside the program memory (User ID or configuration space) is not modelled; ignored".into());
            return 0;
        }
        if v & K_FREE != 0 {
            prog[a..a + 64].fill(0xFF);
            m.mem[self.con1] &= !K_FREE;
        } else {
            for i in 0..64 {
                let w = k.hold[i];
                if w & !prog[a + i] != 0 {
                    notes.insert("a flash write needs bits to go from 0 to 1 without an erase; the model keeps them 0 (AND)".into());
                }
                prog[a + i] &= w;
            }
            k.hold = [0xFF; 64];
        }
        self.flag.set(m);
        self.us(2_000)
    }

    pub fn find(dev: &Device) -> Option<Nvm> {
        if dev.find_sfr("EECON1").is_some() && dev.find_sfr("NVMCON1").is_none() {
            return Self::find_k22(dev);
        }
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
            fosc_mhz: 64.0,
            k22: None,
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
        m.mem[self.con1] &= if self.k22.is_some() { !K_WR } else { !WR };
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

    /// A time in microseconds as instruction cycles at the current Fosc (Fosc/4 per cycle).
    fn us(&self, t: u64) -> u64 {
        (t as f64 * self.fosc_mhz / 4.0).round() as u64
    }

    /// A write to NVMCON1 or NVMCON2. Returns the cycles the CPU stalls (a program flash operation).
    pub fn write(&mut self, k: u8, v: u8, now: u64, m: &mut DataMem, prog: &mut [u8], notes: &mut BTreeSet<String>) -> u64 {
        if self.k22.is_some() {
            return self.k22_write(k, v, now, m, prog, notes);
        }
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
                self.op = Some((now + self.us(10_000), self.offset(m).unwrap(), m.mem[self.datl]));
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
