//! Data memory of a PIC18 part: flat byte array addressed as the hardware does (12 or 14 bits), the
//! power-on values and implemented masks from the device description, and a record of the SFRs that
//! have no peripheral model behind them.

use devdesc::Device;
use std::collections::BTreeSet;

/// Addresses of the core registers, found by name in the device description.
#[derive(Debug, Clone, Default)]
pub struct CoreRegs {
    pub status: u32,
    pub wreg: u32,
    pub bsr: u32,
    pub fsr: [u32; 3],         // FSRnL (FSRnH is +1)
    pub indf: [[u32; 5]; 3],   // INDF, POSTINC, POSTDEC, PREINC, PLUSW
    pub prodl: u32,
    pub prodh: u32,
    pub pcl: u32,
    pub pclath: u32,
    pub pclatu: u32,
    pub tosl: u32,
    pub stkptr: u32,
    pub tblptr: u32, // TBLPTRL (H, U follow)
    pub tablat: u32,
}

/// What an address does when accessed, other than behaving as a latch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Special {
    None,
    /// FSRn indirect register: (n, kind) with kind 0 INDF, 1 POSTINC, 2 POSTDEC, 3 PREINC, 4 PLUSW.
    Indirect(u8, u8),
    Pcl,
    Tos(u8),
    /// A register the core models itself (reads and writes are the stored byte).
    Core,
    /// A peripheral register with a model behind it (`periph.rs`); the code says which.
    Hook(u8),
    /// A CRC / scanner register (`periph/crc.rs`); the code is the sub-code there. Reads are the stored byte.
    Crc(u8),
    /// A register that changes what a pin or a gate source reads (LATx, TRISx, ANSELx, RxyPPS, TxGATE, TxGPPS): the
    /// peripherals are brought up to date before a write; reads and the stored byte are plain.
    Sync,
}

pub struct DataMem {
    pub mem: Vec<u8>,
    /// Bits that exist, per address (0xFF for RAM).
    pub implemented: Vec<u8>,
    pub special: Vec<Special>,
    /// Address is an SFR (for the unmodelled-register log).
    pub is_sfr: Vec<bool>,
    pub names: Vec<Option<String>>,
    pub regs: CoreRegs,
    /// SFRs read or written that no model backs.
    pub unmodelled: BTreeSet<String>,
    /// Addresses outside every RAM and SFR range that were accessed.
    pub stray: BTreeSet<u32>,
    pub access_ram: (u32, u32),
    pub access_sfr_start: u32,
    pub size: u32,
}

fn por_bits(s: &str) -> u8 {
    let mut v = 0u8;
    for (i, c) in s.chars().rev().enumerate() {
        if c == '1' && i < 8 {
            v |= 1 << i;
        }
    }
    v
}

impl DataMem {
    pub fn new(dev: &Device) -> Result<DataMem, String> {
        let d = &dev.memory.data;
        let mut top = 0;
        for g in &d.gpr {
            top = top.max(g.end);
        }
        for r in &d.sfr {
            top = top.max(r.end);
        }
        let size = top.next_power_of_two().max(0x1000);
        let n = size as usize;
        let mut m = DataMem {
            mem: vec![0; n],
            implemented: vec![0; n],
            special: vec![Special::None; n],
            is_sfr: vec![false; n],
            names: vec![None; n],
            regs: CoreRegs::default(),
            unmodelled: BTreeSet::new(),
            stray: BTreeSet::new(),
            access_ram: (d.access_ram.start, d.access_ram.end),
            access_sfr_start: d.access_sfr.start,
            size,
        };
        for g in &d.gpr {
            for a in g.start..g.end {
                m.implemented[a as usize] = 0xFF;
            }
        }
        for r in &d.sfr {
            for a in r.start..r.end {
                m.is_sfr[a as usize] = true;
                m.implemented[a as usize] = 0xFF;
            }
        }
        for s in &dev.sfr {
            if s.width != 8 {
                continue;
            }
            let a = s.addr as usize;
            if a >= n {
                continue;
            }
            m.names[a] = Some(s.name.clone());
            m.implemented[a] = s.implemented.map_or(0xFF, |v| v as u8);
            m.mem[a] = por_bits(&s.por);
        }
        let addr = |name: &str| -> Result<u32, String> {
            dev.find_sfr(name).map(|s| s.addr).ok_or_else(|| format!("{}: no SFR {name}", dev.device.name))
        };
        let mut r = CoreRegs {
            status: addr("STATUS")?,
            wreg: addr("WREG")?,
            bsr: addr("BSR")?,
            prodl: addr("PRODL")?,
            prodh: addr("PRODH")?,
            pcl: addr("PCL")?,
            pclath: addr("PCLATH")?,
            pclatu: addr("PCLATU")?,
            tosl: addr("TOSL")?,
            stkptr: addr("STKPTR")?,
            tblptr: addr("TBLPTRL")?,
            tablat: addr("TABLAT")?,
            ..Default::default()
        };
        const KINDS: [&str; 5] = ["INDF", "POSTINC", "POSTDEC", "PREINC", "PLUSW"];
        for f in 0..3 {
            r.fsr[f] = addr(&format!("FSR{f}L"))?;
            for (k, kn) in KINDS.iter().enumerate() {
                let a = addr(&format!("{kn}{f}"))?;
                r.indf[f][k] = a;
                m.special[a as usize] = Special::Indirect(f as u8, k as u8);
            }
        }
        m.special[r.pcl as usize] = Special::Pcl;
        for i in 0..3 {
            m.special[(r.tosl + i) as usize] = Special::Tos(i as u8);
        }
        for a in [
            r.status, r.wreg, r.bsr, r.prodl, r.prodh, r.pclath, r.pclatu, r.stkptr, r.tablat, r.tblptr,
            r.tblptr + 1, r.tblptr + 2, r.fsr[0], r.fsr[0] + 1, r.fsr[1], r.fsr[1] + 1, r.fsr[2], r.fsr[2] + 1,
        ] {
            m.special[a as usize] = Special::Core;
        }
        m.regs = r;
        Ok(m)
    }

    /// Effective address for a file-register operand: `f` is the 8-bit field, `a` the access bit.
    #[inline]
    pub fn ea(&self, f: u32, a: bool) -> u32 {
        if a {
            (u32::from(self.mem[self.regs.bsr as usize] & 0x3F) << 8 | f) & (self.size - 1)
        } else {
            let len = self.access_ram.1 - self.access_ram.0;
            if f < len { self.access_ram.0 + f } else { self.access_sfr_start + (f - len) }
        }
    }

    #[inline]
    pub fn note_access(&mut self, a: u32) {
        let i = a as usize;
        if i >= self.mem.len() || self.implemented[i] == 0 && !self.is_sfr[i] {
            self.stray.insert(a);
        } else if self.is_sfr[i] && self.special[i] == Special::None
            && let Some(n) = &self.names[i]
        {
            self.unmodelled.insert(n.clone());
        }
    }
}
