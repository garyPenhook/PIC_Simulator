//! Program flash (PFM) array operations shared by the Q10 (DS40002043F 12.1) and Q41 (DS40002214G 10.3) NVM models.
//!
//! * A sector (Q10) or page (Q41) is 128 words = 256 bytes (Table 12-2 p.142; 10.3.4 p.92). Erase sets it to FFh.
//! * The holding registers (Q10) or buffer RAM (Q41) are the 256 bytes of the GPR bank that follows the last GPR bank
//!   (12.1 p.142; Table 10-2 p.92). The pack marks that bank `system`, which is where `hold` comes from. Reading a
//!   sector or page fills them, `TBLWT` writes them (see `Pic18::tblwt`), a sector or page write programs from them.
//! * Program memory is addressed by the byte address held in NVMADR. A word is two bytes, the low byte at the even
//!   address. A sector or page operation ignores NVMADR[7:0] (12.1.3 p.149, 10.3.1 p.89).
//! * `[A40]` Flash programming only clears bits (12.1.4 p.151: a bit cannot change from 0 to 1 without an erase), so a
//!   write ANDs the new data into what is there; a write that would need 0 -> 1 adds a note.

use crate::mem::DataMem;
use devdesc::Device;
use std::collections::BTreeSet;

pub const SECTOR: usize = 256;

#[derive(Clone)]
pub struct Flash {
    /// Data memory address of the first holding register / buffer RAM byte.
    pub hold: usize,
    /// Program memory size in bytes.
    pub size: usize,
    /// User ID and configuration windows: addresses there are valid targets that are not modelled.
    pub aux: Vec<(u32, u32)>,
}

impl Flash {
    pub fn find(dev: &Device) -> Option<Flash> {
        let hold = dev.memory.data.gpr.iter().find(|g| g.system)?.start as usize;
        let r = |r: &devdesc::Region| (r.start, r.start + r.size);
        Some(Flash { hold, size: dev.memory.program.size as usize, aux: vec![r(&dev.memory.user_id), r(&dev.memory.config)] })
    }

    /// Byte address inside program memory.
    pub fn valid(&self, a: u32) -> bool {
        (a as usize) < self.size
    }

    /// An address in the User ID or configuration space (valid on the part, not modelled here).
    pub fn is_aux(&self, a: u32) -> bool {
        self.aux.iter().any(|(s, e)| (*s..*e).contains(&a))
    }

    pub fn word_read(&self, prog: &[u8], a: u32) -> u16 {
        let a = (a & !1) as usize;
        u16::from(prog[a]) | u16::from(prog[a + 1]) << 8
    }

    pub fn word_write(&self, prog: &mut [u8], a: u32, v: u16, notes: &mut BTreeSet<String>) {
        let a = (a & !1) as usize;
        Self::program(&mut prog[a], v as u8, notes);
        Self::program(&mut prog[a + 1], (v >> 8) as u8, notes);
    }

    fn program(cell: &mut u8, v: u8, notes: &mut BTreeSet<String>) {
        if v & !*cell != 0 {
            notes.insert("a flash write needs bits to go from 0 to 1 without an erase; the model keeps them 0 (AND)".into());
        }
        *cell &= v;
    }

    fn base(a: u32) -> usize {
        (a & !0xFF) as usize
    }

    pub fn sector_read(&self, prog: &[u8], m: &mut DataMem, a: u32) {
        let b = Self::base(a);
        m.mem[self.hold..self.hold + SECTOR].copy_from_slice(&prog[b..b + SECTOR]);
    }

    pub fn sector_erase(&self, prog: &mut [u8], a: u32) {
        let b = Self::base(a);
        prog[b..b + SECTOR].fill(0xFF);
    }

    pub fn sector_write(&self, prog: &mut [u8], m: &DataMem, a: u32, notes: &mut BTreeSet<String>) {
        let b = Self::base(a);
        for i in 0..SECTOR {
            Self::program(&mut prog[b + i], m.mem[self.hold + i], notes);
        }
    }
}
