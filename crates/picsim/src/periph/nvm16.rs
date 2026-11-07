//! Data EEPROM (data flash memory, DFM) of the PIC16 parts that have one through the NVM registers: the PIC16F183xx
//! (DS40001839E ch.11, p.123-139) and the PIC16F171xx / PIC16F180x6 (PIC16F17126/46 DS40002343F ch.15, p.154-178).
//! The PIC16F13145 and PIC16F152xx families have none. Program flash self-write is not modelled (note on use).
//!
//! Sourced rules:
//! * The array is 256 bytes at NVMADR = 7000h-70FFh with NVMREGS = 1 (11.4.3 / 15.2.2.4; the 183xx reads NVMADRH back as
//!   F0h because bit 7 is unimplemented and reads 1, Register 11-4 p.137).
//! * Read: NVMREGS = 1, address in NVMADRH:L, RD = 1. The data is in NVMDATL on the very next instruction, RD is cleared
//!   by hardware, only NVMDATL is loaded (Register 11-5 note 7 p.138; 11.4.1 p.124; 15.2.2.1 p.168). A read works with
//!   WREN clear. On the 183xx a read outside Table 11-3 clears NVMDATH:NVMDATL (11.4.7 p.134).
//! * Write: NVMREGS = 1, WREN = 1, address, NVMDATL, then NVMCON2 = 55h, AAh and WR = 1; the byte is erased first
//!   (11.4.3 p.127, 15.2.2.4 p.172). On completion hardware clears WR and sets NVMIF (PIR2.4 on the 183xx, PIR1.0 on
//!   the 180x6, see the device file). Clearing WR (or NVMREGS and so on) during a write has no effect. WR can only be set by
//!   the unlock sequence (NVMCON1 note 4). NVMCON2 is write only.
//! * 183xx: CPU execution continues in parallel while the byte is written (11.4.3 p.127); typical 4.0 ms (MEM24 p.419).
//! * 171xx / 180x6: with WR set "the processor will stall internal operations until the operation is complete and then
//!   resume with the next instruction" (15.2.2.2 p.169); MEM24 gives 11 ms maximum and no typical value (44.3.5 p.627).
//! * WRERR (NVMCON1.3): set when WR is set for a write-protected address (WRTD = 0 on the 171xx, 15.2.2.4 p.172), or when a
//!   Reset occurs during the write (15.2.2.5 p.172, 11.5 Register 11-5 p.138); software clears it.
//!
//! Assumptions `[A#]`, to be measured on hardware:
//! * `[A115]` Times in instruction cycles at 1 000 cycles per millisecond (Fosc = 4 MHz, as `nvm.rs` `[A30]`; the models do not
//!   know Fosc): 4 000 for the 183xx (the typical value), 11 000 for the 171xx (the maximum). `write_cycles` changes it.
//!   The 171xx stall applies the general sentence of 15.2.2.2 to the DFM; the DFM text itself does not say.
//! * `[A116]` The unlock is tracked as 55h then AAh written to NVMCON2 and the next NVMCON1 write; instructions between those
//!   writes are not checked, any other NVMCON2 value or an NVMCON1 write restarts it. A WR written without it, or with
//!   WREN clear, is ignored (WR stays 0) and WRERR is left alone.
//! * `[A117]` The write keeps running in Sleep and its end sets NVMIF (which wakes the part when enabled). A Reset during
//!   the write drops it, leaves the byte as it was, and sets WRERR. The data and address are those at the start of the write.
//! * `[A118]` Not modelled: program flash, User ID, configuration and device ID access (a read of them leaves NVMDAT alone
//!   on the 171xx, a write is ignored; both with a note), NVMIF for a write blocked by WRTD (the data sheet says only that WR is cleared and the write does not happen, 15.2.2.4 p.172; the model sets WRERR as 15.2.2.5 says and no flag), write protection other than WRTD, NVMERR-like errata behaviour
//!   (WRERR set again by every later Reset until a write succeeds: DS80000666H 4.2).

use super::{addr, Flag};
use devdesc::Device;
use std::collections::BTreeSet;

/// Hook sub-codes (the order of `regs`): 0 NVMCON1, 1 NVMCON2.
const CON2: u8 = 1;

const NVMREGS: u8 = 0x40;
const WRERR: u8 = 0x08;
const WREN: u8 = 0x04;
const WR: u8 = 0x02;
const RD: u8 = 0x01;

pub(crate) struct Nvm16 {
    adr: [usize; 2],
    datl: usize,
    pub con1: usize,
    pub con2: usize,
    pub flag: Flag,
    pub eeprom: Vec<u8>,
    /// 55h was written to NVMCON2 and nothing since.
    first: bool,
    /// 55h, AAh were written to NVMCON2 and no NVMCON1 write since.
    armed: bool,
    /// The 183xx: a read outside the array clears NVMDAT, and the CPU runs on during a write.
    f183: bool,
    datah: Option<usize>,
    /// A write in progress: (completion cycle, byte offset, data).
    op: Option<(u64, usize, u8)>,
    pub write_cycles: u64,
}

impl Nvm16 {
    /// The model of a part with a data EEPROM and the NVM registers; `f183` for the PIC16F183xx layout.
    pub fn find(dev: &Device, f183: bool) -> Option<Nvm16> {
        let r = &dev.memory.eeprom;
        if r.size == 0 {
            return None;
        }
        Some(Nvm16 {
            adr: [addr(dev, "NVMADRL")?, addr(dev, "NVMADRH")?],
            datl: addr(dev, "NVMDATL")?,
            con1: addr(dev, "NVMCON1")?,
            con2: addr(dev, "NVMCON2")?,
            flag: Flag::find(dev, "NVMIF")?,
            eeprom: vec![0xFF; r.size as usize],
            first: false,
            armed: false,
            f183,
            datah: addr(dev, "NVMDATH"),
            op: None,
            write_cycles: if f183 { 4_000 } else { 11_000 },
        })
    }

    /// Every register address the model indexes (to check against the size of the data memory).
    pub fn addrs(&self) -> Vec<usize> {
        vec![self.adr[0], self.adr[1], self.datl, self.con1, self.con2, self.flag.addr].into_iter().chain(self.datah).collect()
    }

    pub fn adrh(&self) -> usize {
        self.adr[1]
    }

    pub fn regs(&self) -> [usize; 2] {
        [self.con1, self.con2]
    }

    /// Byte offset in the array NVMADR points to, with NVMADR = 7000h + offset (bit 15 is ignored).
    fn offset(&self, mem: &[u8]) -> Option<usize> {
        let a = u32::from(mem[self.adr[0]]) | u32::from(mem[self.adr[1]] & 0x7F) << 8;
        a.checked_sub(0x7000).filter(|o| (*o as usize) < self.eeprom.len()).map(|o| o as usize)
    }

    /// Complete the write that is due by cycle `now`: true when it ended and NVMIF must be raised.
    pub fn sync(&mut self, mem: &mut [u8], now: u64) -> bool {
        let Some((_, off, v)) = self.op.filter(|o| o.0 <= now) else { return false };
        self.op = None;
        self.eeprom[off] = v;
        mem[self.con1] &= !WR;
        true
    }

    /// A Reset other than power-on (call after the registers took their reset values): a running write is lost and WRERR set.
    pub fn reset(&mut self, mem: &mut [u8]) {
        self.first = false;
        self.armed = false;
        if self.op.take().is_some() {
            mem[self.con1] |= WRERR;
        }
    }

    /// A write to NVMCON1 or NVMCON2 (`v` is already masked to the implemented bits); `wrtd` is the data write protect
    /// (171xx WRTD = 0). Returns the cycles the CPU stalls.
    pub fn write(&mut self, k: u8, v: u8, now: u64, mem: &mut [u8], wrtd: bool, notes: &mut BTreeSet<String>) -> u64 {
        if k == CON2 {
            (self.first, self.armed) = match (self.first, v) {
                (_, 0x55) => (true, false),
                (true, 0xAA) => (false, true),
                _ => (false, false),
            };
            mem[self.con2] = 0; // write only
            return 0;
        }
        let armed = std::mem::take(&mut self.armed);
        self.first = false;
        let busy = self.op.is_some();
        // WR and RD are set-only bits; the others are plain read/write (FREE and LWLO have no effect on the array here).
        let mut new = v & !(WR | RD) | if busy { WR } else { 0 };
        let region = v & NVMREGS != 0;
        if !busy && v & RD != 0 {
            match (region.then(|| self.offset(mem)).flatten(), region) {
                (Some(o), _) => mem[self.datl] = self.eeprom[o],
                (None, true) if self.f183 => {
                    mem[self.datl] = 0;
                    if let Some(h) = self.datah {
                        mem[h] = 0;
                    }
                }
                _ => {
                    notes.insert("NVM reads of program flash, User ID, configuration and device ID space are not modelled; NVMDAT is unchanged".into());
                }
            }
        }
        if !busy && v & WR != 0 && armed && v & WREN != 0 {
            match (region.then(|| self.offset(mem)).flatten(), wrtd) {
                (Some(_), true) => new |= WRERR,
                (Some(o), false) => {
                    self.op = Some((now + self.write_cycles, o, mem[self.datl]));
                    new |= WR;
                    mem[self.con1] = new;
                    return if self.f183 { 0 } else { self.write_cycles };
                }
                (None, _) => {
                    notes.insert("NVM writes to program flash, User ID and configuration space are not modelled; the operation is ignored".into());
                }
            }
        }
        mem[self.con1] = new;
        0
    }
}
