//! The dsPIC33 (CK/CH/CD/EP/EV) core: 16-bit data path, 24-bit instructions, DSP engine, exceptions.
//!
//! Instruction encodings are decoded by `dspic_isa`; the peripherals are in `dspic_periph`. Timing follows the 16-Bit MCU
//! and DSC Programmer's Reference Manual (DS70000157G, 3.2.1) for the dsPIC33C and was checked against MPLAB X's MDB
//! simulator (`scripts/dspic_diff.py`).
//!
//! Addresses: the PC and every program address is the value the hardware shows (two per instruction word); the data
//! space is 16 bits of byte addresses.

use crate::dspic_isa::*;
use crate::dspic_periph::Periph;
use devdesc::Device;
use std::collections::BTreeSet;

// SR bits
pub const C: u16 = 1;
pub const Z: u16 = 2;
pub const OV: u16 = 4;
pub const N: u16 = 8;
pub const RA: u16 = 0x10;
pub const DC: u16 = 0x100;
pub const DA: u16 = 0x200;
pub const SAB: u16 = 0x400;
pub const OAB: u16 = 0x800;
pub const SB: u16 = 0x1000;
pub const SA: u16 = 0x2000;
pub const OB: u16 = 0x4000;
pub const OA: u16 = 0x8000;

// CORCON bits
pub const CC_IF: u16 = 1;
pub const CC_RND: u16 = 2;
pub const CC_SFA: u16 = 4;
pub const CC_IPL3: u16 = 8;
pub const CC_ACCSAT: u16 = 0x10;
pub const CC_SATDW: u16 = 0x20;
pub const CC_SATB: u16 = 0x40;
pub const CC_SATA: u16 = 0x80;

/// Addresses of the CPU registers in the SFR space.
pub mod reg {
    pub const SPLIM: usize = 0x20;
    pub const ACCAL: usize = 0x22;
    pub const ACCAH: usize = 0x24;
    pub const ACCAU: usize = 0x26;
    pub const ACCBL: usize = 0x28;
    pub const ACCBH: usize = 0x2A;
    pub const ACCBU: usize = 0x2C;
    pub const PCL: usize = 0x2E;
    pub const PCH: usize = 0x30;
    pub const DSRPAG: usize = 0x32;
    pub const DSWPAG: usize = 0x34;
    pub const RCOUNT: usize = 0x36;
    pub const DCOUNT: usize = 0x38;
    pub const DOSTARTL: usize = 0x3A;
    pub const DOSTARTH: usize = 0x3C;
    pub const DOENDL: usize = 0x3E;
    pub const DOENDH: usize = 0x40;
    pub const SR: usize = 0x42;
    pub const CORCON: usize = 0x44;
    pub const MODCON: usize = 0x46;
    pub const XMODSRT: usize = 0x48;
    pub const XMODEND: usize = 0x4A;
    pub const YMODSRT: usize = 0x4C;
    pub const YMODEND: usize = 0x4E;
    pub const XBREV: usize = 0x50;
    pub const DISICNT: usize = 0x52;
    pub const TBLPAG: usize = 0x54;
    pub const YPAG: usize = 0x56;
    pub const MSTRPR: usize = 0x58;
    pub const CTXTSTAT: usize = 0x5A;
    /// first address that is not a CPU register
    pub const CPU_END: usize = 0x5C;
}

/// A trap, with its vector number as `INTTREG` shows it and its IVT address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trap {
    OscFail,
    Address,
    Hard,
    Stack,
    Math,
    Soft,
}

impl Trap {
    pub fn vector(self) -> u32 {
        match self {
            Trap::OscFail => 0x04,
            Trap::Address => 0x06,
            Trap::Hard => 0x08,
            Trap::Stack => 0x0A,
            Trap::Math => 0x0C,
            Trap::Soft => 0x10,
        }
    }
    pub fn number(self) -> u8 {
        match self {
            Trap::OscFail => 0,
            Trap::Address => 1,
            Trap::Hard => 2,
            Trap::Stack => 3,
            Trap::Math => 4,
            Trap::Soft => 6,
        }
    }
}

/// Why `step` stopped before finishing an instruction in a way the caller should know about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    None,
    /// `RESET` instruction
    Reset,
}

pub struct Dspic {
    pub pc: u32,
    pub cycles: u64,
    pub icount: u64,
    pub w: [u16; 16],
    /// alternate W register arrays (CTXTSWP, 1..=4); index 0 is unused
    pub alt: [[u16; 16]; 5],
    pub ctx: usize,
    pub sr: u16,
    pub corcon: u16,
    pub acc: [i64; 2],
    /// data space (the SFRs 0x0000..0x1000 included; the CPU registers are kept in the fields above and below)
    pub mem: Vec<u8>,
    /// program memory, one 24-bit word per instruction (index = address / 2)
    pub prog: Vec<u32>,
    pub prog_words: usize,
    /// the program memory as bytes, four per instruction word (the layout of the hex file; the fourth byte is 0)
    pub prog_b: Vec<u8>,
    pub ram: (u32, u32),
    pub splim: u16,
    pub splim_set: bool,
    pub rcount: u16,
    pub ra: bool,
    /// calls and interrupt entries not yet returned from (for the debugger's stepping)
    pub call_depth: i32,
    /// passes of the current `REPEAT` already done (the divide instructions work in the first)
    pub repeat_iter: u32,
    pub dcount: u16,
    pub dostart: u32,
    pub doend: u32,
    pub do_stack: Vec<(u32, u32, u16)>,
    /// DO nesting level 0..=4 (`CORCON.DL`)
    pub dl_level: u8,
    /// stack frame active (`CORCON.SFA`)
    pub sfa: bool,
    /// early DO loop termination requested
    pub edt: bool,
    pub dsrpag: u16,
    pub dswpag: u16,
    pub tblpag: u16,
    pub modcon: u16,
    pub xmodsrt: u16,
    pub xmodend: u16,
    pub ymodsrt: u16,
    pub ymodend: u16,
    pub xbrev: u16,
    pub disicnt: u16,
    pub shadow: Option<([u16; 4], u16)>,
    /// extra cycles of the instruction under way (SFR reads, PSV)
    pub extra: u32,
    /// W registers the instruction under way wrote (data results, or a modified destination pointer) and the ones it used
    /// as source address registers; with the previous instruction's they decide the read-after-write stall (`step`)
    pub wmask: u16,
    pub w15_direct: bool,
    pub src_used: u16,
    pub prev_wmask: u16,
    pub sleeping: Option<u8>,
    pub event: Event,
    pub trap_pending: Option<Trap>,
    pub unmodelled: BTreeSet<String>,
    pub notes: BTreeSet<String>,
    pub p: Periph,
    pub dev_id: u32,
    pub clock: crate::dspic_clock::Clock,
    /// power-on values of the SFRs: (address, value, bytes)
    pub por: Vec<(usize, u16, u8)>,
    /// configuration fields: (name, word address, bit position, width)
    pub cfg: Vec<(String, u32, u8, u8)>,
    pub stopped_on_error: Option<String>,
    /// last address where a data read or write of a watched kind happened (debugger support)
    pub last_write: Option<(u16, u8)>,
}

pub(crate) fn sext8(v: u16) -> u16 {
    v as u8 as i8 as i16 as u16
}

impl Dspic {
    pub fn new(dev: &Device) -> Result<Dspic, String> {
        if dev.device.family != "dspic33" {
            return Err(format!("{}: family {} is not a dsPIC33", dev.device.name, dev.device.family));
        }
        let prog_end = dev.memory.config.start + dev.memory.config.size;
        let prog_words = (prog_end / 2) as usize;
        let gpr = dev.memory.data.gpr.first().ok_or("device has no RAM")?;
        let ram = (gpr.start, dev.memory.data.gpr.last().map_or(gpr.end, |g| g.end));
        let mut c = Dspic {
            pc: 0,
            cycles: 0,
            icount: 0,
            w: [0; 16],
            alt: [[0; 16]; 5],
            ctx: 0,
            sr: 0,
            corcon: 0x0020,
            acc: [0; 2],
            mem: vec![0; 0x10000],
            prog: vec![0xFF_FFFF; prog_words],
            prog_words,
            prog_b: vec![0xFF; prog_words * 4],
            ram,
            splim: 0,
            splim_set: false,
            rcount: 0,
            ra: false,
            call_depth: 0,
            repeat_iter: 0,
            dcount: 0,
            dostart: 0,
            doend: 0,
            do_stack: Vec::new(),
            dl_level: 0,
            sfa: false,
            edt: false,
            dsrpag: 1,
            dswpag: 1,
            tblpag: 0,
            modcon: 0,
            xmodsrt: 0,
            xmodend: 1,
            ymodsrt: 0,
            ymodend: 1,
            xbrev: 0,
            disicnt: 0,
            shadow: None,
            extra: 0,
            wmask: 0,
            w15_direct: false,
            src_used: 0,
            prev_wmask: 0,
            sleeping: None,
            event: Event::None,
            trap_pending: None,
            unmodelled: BTreeSet::new(),
            notes: BTreeSet::new(),
            p: Periph::new(dev)?,
            dev_id: dev.memory.device_id.value,
            clock: Default::default(),
            por: Vec::new(),
            cfg: dev.config.iter().flat_map(|c| c.fields.iter().map(move |f| (f.name.clone(), c.addr, f.pos, f.width))).collect(),
            stopped_on_error: None,
            last_write: None,
        };
        c.reset_sfrs(dev);
        Ok(c)
    }

    /// Power-on values of the SFRs from the device description (`0`/`1` bits as given; unknown bits 0).
    fn reset_sfrs(&mut self, dev: &Device) {
        for s in &dev.sfr {
            if s.addr as usize >= 0x1000 || s.por.is_empty() {
                continue;
            }
            let mut v: u32 = 0;
            for ch in s.por.chars() {
                v = v << 1 | u32::from(ch == '1');
            }
            self.por.push((s.addr as usize, v as u16, if s.width > 8 { 2 } else { 1 }));
        }
        self.apply_por();
    }

    /// Puts every SFR and the core back to its reset state (program and RAM are kept).
    pub fn apply_por(&mut self) {
        for &(a, v, n) in &self.por.clone() {
            self.mem[a] = v as u8;
            if n > 1 {
                self.mem[a + 1] = (v >> 8) as u8;
            }
        }
        let sp = rd16m(&self.mem, 0x1E);
        self.w = [0; 16];
        self.w[15] = sp;
        self.alt = [[0; 16]; 5];
        self.ctx = 0;
        self.sr = 0;
        self.corcon = 0x0020;
        self.acc = [0; 2];
        self.splim = 0;
        self.splim_set = false;
        self.rcount = 0;
        self.ra = false;
        self.repeat_iter = 0;
        self.dcount = 0;
        self.dostart = 0;
        self.doend = 0;
        self.do_stack.clear();
        self.dl_level = 0;
        self.sfa = false;
        self.edt = false;
        self.dsrpag = 1;
        self.dswpag = 1;
        self.tblpag = 0;
        self.modcon = 0;
        self.xmodsrt = 0;
        self.xmodend = 1;
        self.ymodsrt = 0;
        self.ymodend = 1;
        self.xbrev = 0;
        self.disicnt = 0;
        self.shadow = None;
        self.sleeping = None;
        self.trap_pending = None;
        self.call_depth = 0;
        self.pc = 0;
        self.prev_wmask = 0;
        for a in 0..0x20 {
            self.mem[a] = 0;
        }
        self.clock_reset();
    }

    /// The value of a configuration field of the hex file (`FNOSC`, `FWDTEN`...), or its erased value.
    pub fn config_field(&self, name: &str) -> u32 {
        let Some((_, addr, pos, w)) = self.cfg.iter().find(|c| c.0 == name) else { return 0 };
        let word = self.prog.get((*addr / 2) as usize).copied().unwrap_or(0xFF_FFFF);
        (word >> pos) & ((1 << w) - 1)
    }

    /// Loads hex image bytes (hex byte address -> byte) into program memory: each instruction is four hex bytes
    /// (three of the word and a zero), at twice the PC value.
    pub fn load(&mut self, img: &crate::hex::Image) {
        let mut words: std::collections::BTreeMap<u32, [u8; 4]> = std::collections::BTreeMap::new();
        for (&a, &b) in &img.bytes {
            let e = words.entry(a & !3).or_insert([0xFF, 0xFF, 0xFF, 0]);
            e[(a & 3) as usize] = b;
        }
        for (a, b) in words {
            let pc = a / 2;
            let idx = (pc / 2) as usize;
            if idx < self.prog.len() {
                self.set_prog(idx, u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16);
            }
        }
        self.clock_reset();
    }

    /// Stores an instruction word in both views of program memory.
    pub fn set_prog(&mut self, idx: usize, w: u32) {
        if idx < self.prog.len() {
            self.prog[idx] = w & 0xFF_FFFF;
            self.prog_b[idx * 4..idx * 4 + 4].copy_from_slice(&[w as u8, (w >> 8) as u8, (w >> 16) as u8, 0]);
        }
    }

    /// A debugger write of a data byte (no side effects of the instruction kind, but the peripherals see it).
    pub fn write(&mut self, a: u32, v: u8) {
        self.wr8(a as u16, v);
    }

    pub fn settle(&mut self) {}

    pub fn fetch(&self, pc: u32) -> u32 {
        self.prog.get((pc >> 1) as usize).copied().unwrap_or(0xFF_FFFF)
    }

    // ------------------------------------------------------------------------------------------------
    // Registers and flags
    // ------------------------------------------------------------------------------------------------

    pub fn ipl(&self) -> u8 {
        (((self.sr >> 5) & 7) as u8) | if self.corcon & CC_IPL3 != 0 { 8 } else { 0 }
    }

    pub fn set_ipl(&mut self, v: u8) {
        self.sr = (self.sr & !0xE0) | (u16::from(v & 7) << 5);
        self.corcon = (self.corcon & !CC_IPL3) | if v & 8 != 0 { CC_IPL3 } else { 0 };
    }

    pub fn flag(&self, f: u16) -> bool {
        self.sr & f != 0
    }

    pub(crate) fn set_flag(&mut self, f: u16, v: bool) {
        if v {
            self.sr |= f
        } else {
            self.sr &= !f
        }
    }

    /// SR as software reads it: RA and DA reflect the loops.
    pub fn sr_read(&self) -> u16 {
        let mut v = self.sr & !(RA | DA | OAB | SAB);
        if self.ra {
            v |= RA;
        }
        if self.da() {
            v |= DA;
        }
        if v & (OA | OB) != 0 {
            v |= OAB;
        }
        if v & (SA | SB) != 0 {
            v |= SAB;
        }
        v
    }

    pub fn da(&self) -> bool {
        self.dl() > 0
    }

    pub fn dl(&self) -> u8 {
        self.dl_level
    }

    fn sr_write(&mut self, v: u16) {
        // RA and DA are read only; clearing OAB / SAB clears OA,OB / SA,SB
        let mut n = v & !(RA | DA | OAB | SAB);
        if v & OAB == 0 {
            n &= !(OA | OB);
        } else {
            n = (n & !(OA | OB)) | (self.sr & (OA | OB));
            n = (n & !(OA | OB)) | (v & (OA | OB));
        }
        if v & SAB == 0 {
            n &= !(SA | SB);
        }
        if self.nstdis() {
            n = (n & !0xE0) | (self.sr & 0xE0);
        }
        self.sr = n;
    }

    fn nstdis(&self) -> bool {
        self.mem[0x8C1] & 0x80 != 0
    }

    fn corcon_write(&mut self, v: u16) {
        // DL, SFA, IPL3 are read only (IPL3 is cleared, not set, by software); EDT reads 0
        let keep = CC_SFA | 0x0700 | CC_IPL3;
        let mut n = (v & !keep) | (self.corcon & keep);
        if v & CC_IPL3 == 0 {
            n &= !CC_IPL3;
        }
        if v & 0x0800 != 0 {
            self.edt = true;
        }
        n &= !0x0800;
        self.corcon = n;
    }

    // ------------------------------------------------------------------------------------------------
    // Data space
    // ------------------------------------------------------------------------------------------------

    fn core_read(&self, a: usize) -> Option<u16> {
        Some(match a {
            0x00..=0x1F => {
                if a & 1 != 0 {
                    return None;
                }
                self.w[a >> 1]
            }
            reg::SPLIM => self.splim,
            reg::ACCAL => self.acc[0] as u16,
            reg::ACCAH => (self.acc[0] >> 16) as u16,
            reg::ACCAU => ((self.acc[0] >> 32) as i8) as i16 as u16,
            reg::ACCBL => self.acc[1] as u16,
            reg::ACCBH => (self.acc[1] >> 16) as u16,
            reg::ACCBU => ((self.acc[1] >> 32) as i8) as i16 as u16,
            reg::PCL => self.pc as u16,
            reg::PCH => ((self.pc >> 16) & 0x7F) as u16,
            reg::DSRPAG => self.dsrpag,
            reg::DSWPAG => self.dswpag,
            reg::RCOUNT => self.rcount,
            reg::DCOUNT => self.dcount,
            reg::DOSTARTL => self.dostart as u16,
            reg::DOSTARTH => ((self.dostart >> 16) & 0x7F) as u16,
            reg::DOENDL => self.doend as u16,
            reg::DOENDH => ((self.doend >> 16) & 0x7F) as u16,
            reg::SR => self.sr_read(),
            reg::CORCON => self.corcon_read(),
            reg::MODCON => self.modcon,
            reg::XMODSRT => self.xmodsrt,
            reg::XMODEND => self.xmodend,
            reg::YMODSRT => self.ymodsrt,
            reg::YMODEND => self.ymodend,
            reg::XBREV => self.xbrev,
            reg::DISICNT => self.disicnt,
            reg::TBLPAG => self.tblpag,
            reg::YPAG => rd16m(&self.mem, a),
            _ => return None,
        })
    }

    fn corcon_read(&self) -> u16 {
        let mut v = self.corcon & !0x0F00 & !CC_SFA;
        v |= (u16::from(self.dl_level & 7)) << 8;
        if self.sfa {
            v |= CC_SFA;
        }
        v
    }

    fn core_write(&mut self, a: usize, v: u16, mask: u16) -> bool {
        let merge = |old: u16| (old & !mask) | (v & mask);
        match a {
            0x00..=0x1F if a & 1 == 0 => {
                let n = a >> 1;
                let nv = merge(self.w[n]);
                self.w[n] = if n == 15 { nv & !1 } else { nv };
            }
            reg::SPLIM => {
                self.splim = merge(self.splim) & !1;
                self.splim_set = true;
            }
            reg::ACCAL => self.acc[0] = (self.acc[0] & !0xFFFF) | i64::from(merge(self.acc[0] as u16)),
            reg::ACCAH => self.acc[0] = (self.acc[0] & !0xFFFF_0000) | i64::from(merge((self.acc[0] >> 16) as u16)) << 16,
            reg::ACCAU => self.acc[0] = (self.acc[0] & 0xFF_FFFF_FFFF) & !(0xFFi64 << 32) | i64::from(merge((self.acc[0] >> 32) as u16) as u8) << 32,
            reg::ACCBL => self.acc[1] = (self.acc[1] & !0xFFFF) | i64::from(merge(self.acc[1] as u16)),
            reg::ACCBH => self.acc[1] = (self.acc[1] & !0xFFFF_0000) | i64::from(merge((self.acc[1] >> 16) as u16)) << 16,
            reg::ACCBU => self.acc[1] = (self.acc[1] & 0xFF_FFFF_FFFF) & !(0xFFi64 << 32) | i64::from(merge((self.acc[1] >> 32) as u16) as u8) << 32,
            reg::PCL | reg::PCH => {}
            reg::DSRPAG => self.dsrpag = merge(self.dsrpag) & 0x3FF,
            reg::DSWPAG => self.dswpag = merge(self.dswpag) & 0x1FF,
            reg::RCOUNT => self.rcount = merge(self.rcount),
            reg::DCOUNT => self.dcount = merge(self.dcount),
            reg::DOSTARTL => self.dostart = (self.dostart & 0xFF_0000) | u32::from(merge(self.dostart as u16) & !1),
            reg::DOSTARTH => self.dostart = (self.dostart & 0xFFFF) | u32::from(merge((self.dostart >> 16) as u16) & 0x7F) << 16,
            reg::DOENDL => self.doend = (self.doend & 0xFF_0000) | u32::from(merge(self.doend as u16) & !1),
            reg::DOENDH => self.doend = (self.doend & 0xFFFF) | u32::from(merge((self.doend >> 16) as u16) & 0x7F) << 16,
            reg::SR => {
                let n = merge(self.sr_read());
                self.sr_write(n);
            }
            reg::CORCON => {
                let n = merge(self.corcon_read());
                self.corcon_write(n);
            }
            reg::MODCON => self.modcon = merge(self.modcon),
            reg::XMODSRT => self.xmodsrt = merge(self.xmodsrt) & !1,
            reg::XMODEND => self.xmodend = merge(self.xmodend) | 1,
            reg::YMODSRT => self.ymodsrt = merge(self.ymodsrt) & !1,
            reg::YMODEND => self.ymodend = merge(self.ymodend) | 1,
            reg::XBREV => self.xbrev = merge(self.xbrev),
            reg::DISICNT => {
                self.disicnt = merge(self.disicnt) & 0x3FFF;
            }
            reg::TBLPAG => self.tblpag = merge(self.tblpag) & 0xFF,
            reg::YPAG => {
                let o = rd16m(&self.mem, a);
                wr16m(&mut self.mem, a, (o & !mask) | (v & mask));
            }
            0x58..=0x5B => {
                let o = rd16m(&self.mem, a);
                wr16m(&mut self.mem, a, (o & !mask) | (v & mask));
            }
            _ => return false,
        }
        true
    }

    fn implemented(&self, a: u16) -> bool {
        let a = u32::from(a);
        a < 0x1000 || (a >= self.ram.0 && a < self.ram.1)
    }

    fn addr_error(&mut self) {
        self.mem[0x8C0] |= 0x08; // ADDRERR (INTCON1 bit 3)
        self.trap_pending = Some(Trap::Address);
    }

    /// Program-space visibility: the byte at the PSV address `{DSRPAG[8:0], EA[14:0]}`.
    fn psv_byte(&self, ea: u16) -> u8 {
        let addr = (u32::from(self.dsrpag & 0x1FF) << 15) | u32::from(ea & 0x7FFF);
        let w = self.fetch(addr & !1);
        if self.dsrpag & 0x100 != 0 {
            // pages 0x300..: the upper byte of the instruction word
            if addr & 1 == 0 { (w >> 16) as u8 } else { 0 }
        } else if addr & 1 == 0 {
            w as u8
        } else {
            (w >> 8) as u8
        }
    }

    /// Reads a byte; `ea` of a PSV access at 0x8000 and up.
    pub fn rd8(&mut self, ea: u16) -> u8 {
        let a = ea as usize;
        if a < 0x1000 {
            if a < reg::CPU_END {
                if let Some(w) = self.core_read(a & !1) {
                    return (w >> (8 * (a & 1))) as u8;
                }
            } else {
                self.extra = self.extra.max(1);
                let v = self.sfr_read(a & !1);
                return (v >> (8 * (a & 1))) as u8;
            }
            return self.mem[a];
        }
        if ea >= 0x8000 {
            if self.dsrpag == 0 {
                self.addr_error();
                return 0;
            }
            if self.dsrpag & 0x200 != 0 {
                self.extra = self.extra.max(4);
                return self.psv_byte(ea);
            }
            return 0;
        }
        if !self.implemented(ea) {
            self.addr_error();
            return 0;
        }
        self.mem[a]
    }

    pub fn rd16(&mut self, ea: u16) -> u16 {
        if ea & 1 != 0 {
            self.addr_error();
            return 0;
        }
        let a = ea as usize;
        if a < 0x1000 {
            if a < reg::CPU_END {
                if let Some(w) = self.core_read(a) {
                    return w;
                }
                return rd16m(&self.mem, a);
            }
            self.extra = self.extra.max(1);
            return self.sfr_read(a);
        }
        if ea >= 0x8000 {
            if self.dsrpag == 0 {
                self.addr_error();
                return 0;
            }
            if self.dsrpag & 0x200 != 0 {
                self.extra = self.extra.max(4);
                return u16::from(self.psv_byte(ea)) | u16::from(self.psv_byte(ea | 1)) << 8;
            }
            return 0;
        }
        if !self.implemented(ea) {
            self.addr_error();
            return 0;
        }
        rd16m(&self.mem, a)
    }

    pub fn wr8(&mut self, ea: u16, v: u8) {
        let a = ea as usize;
        self.last_write = Some((ea, 1));
        if a < 0x1000 {
            let shift = 8 * (a & 1);
            let mask = 0xFFu16 << shift;
            let word = u16::from(v) << shift;
            if a < reg::CPU_END {
                if self.core_write(a & !1, word, mask) {
                    return;
                }
                self.mem[a] = v;
                return;
            }
            self.sfr_write(a & !1, word, mask);
            return;
        }
        if ea >= 0x8000 || !self.implemented(ea) {
            if ea >= 0x8000 && self.dswpag != 0 {
                return; // no EDS memory on this part: ignored
            }
            self.addr_error();
            return;
        }
        self.mem[a] = v;
    }

    pub fn wr16(&mut self, ea: u16, v: u16) {
        self.last_write = Some((ea, 2));
        if ea & 1 != 0 {
            self.addr_error();
            return;
        }
        let a = ea as usize;
        if a < 0x1000 {
            if a < reg::CPU_END {
                if self.core_write(a, v, 0xFFFF) {
                    return;
                }
                wr16m(&mut self.mem, a, v);
                return;
            }
            self.sfr_write(a, v, 0xFFFF);
            return;
        }
        if ea >= 0x8000 || !self.implemented(ea) {
            if ea >= 0x8000 && self.dswpag != 0 {
                return;
            }
            self.addr_error();
            return;
        }
        wr16m(&mut self.mem, a, v);
    }

    /// Reads a data byte or word without side effects, for the debugger.
    pub fn peek(&self, ea: u32) -> u8 {
        let a = (ea & 0xFFFF) as usize;
        if a < reg::CPU_END {
            if let Some(w) = self.core_read(a & !1) {
                return (w >> (8 * (a & 1))) as u8;
            }
        }
        if a >= 0x8000 {
            return 0;
        }
        self.mem[a]
    }

    // ------------------------------------------------------------------------------------------------
    // Addressing
    // ------------------------------------------------------------------------------------------------

    pub(crate) fn w(&self, n: u8) -> u16 {
        self.w[n as usize & 15]
    }

    pub(crate) fn set_w(&mut self, n: u8, v: u16) {
        self.wmask |= 1 << (n & 15);
        if n == 15 {
            self.w15_direct = true;
        }
        self.w[n as usize & 15] = if n == 15 { v & !1 } else { v };
    }

    /// A pointer register updated by an addressing mode: it counts as written for the stall rule only for a destination.
    fn set_ptr(&mut self, n: u8, v: u16, write: bool) {
        if write {
            self.wmask |= 1 << (n & 15);
        }
        self.w[n as usize & 15] = if n == 15 { v & !1 } else { v };
    }


    /// Effective address of an indirect operand, updating the register; `None` for a register-direct operand.
    /// `size` is 1 (byte), 2 (word) or 4 (double); `write` selects the write AGU (bit-reversed addressing).
    pub(crate) fn ea(&mut self, e: Ea, size: u16, write: bool) -> Option<u16> {
        let st = |r: u8| if r == 15 && size == 1 { 2 } else { size };
        if !write {
            match e {
                Ea::Dir(_) => {}
                Ea::RegOff(r, b) => self.src_used |= 1 << r | 1 << b,
                _ => self.src_used |= 1 << e.reg(),
            }
        }
        match e {
            Ea::Dir(_) => None,
            Ea::Ind(r) => Some(self.w(r)),
            Ea::PostInc(r) => {
                let a = self.w(r);
                let n = self.next_addr(r, a, st(r) as i32, size, write);
                self.set_ptr(r, n, write);
                Some(a)
            }
            Ea::PostDec(r) => {
                let a = self.w(r);
                let n = self.next_addr(r, a, -(st(r) as i32), size, write);
                self.set_ptr(r, n, write);
                Some(a)
            }
            Ea::PreInc(r) => {
                let a = self.w(r);
                let n = self.next_addr(r, a, st(r) as i32, size, write);
                self.set_ptr(r, n, write);
                Some(n)
            }
            Ea::PreDec(r) => {
                let a = self.w(r);
                let n = self.next_addr(r, a, -(st(r) as i32), size, write);
                self.set_ptr(r, n, write);
                Some(n)
            }
            Ea::RegOff(r, b) => Some(self.w(r).wrapping_add(self.w(b))),
        }
    }

    /// The register value after a pre/post increment or decrement of `inc` bytes: the plain sum, or the value modulo
    /// addressing (`MODCON`, `XMODSRT`..) or bit-reversed addressing (`XBREV`) makes of it. Neither applies to W15.
    pub(crate) fn next_addr(&mut self, r: u8, a: u16, inc: i32, size: u16, write: bool) -> u16 {
        let plain = a.wrapping_add(inc as u16);
        if r == 15 {
            return plain;
        }
        let m = self.modcon;
        let bwm = ((m >> 8) & 15) as u8;
        // bit-reversed addressing: X write AGU, word accesses, increment only
        if write && self.xbrev & 0x8000 != 0 && r == bwm && size == 2 && inc > 0 {
            return bit_reverse_add(a, self.xbrev & 0x7FFF);
        }
        let xwm = (m & 15) as u8;
        let ywm = ((m >> 4) & 15) as u8;
        if m & 0x8000 != 0 && r == xwm {
            return modulo_wrap(plain, inc, self.xmodsrt, self.xmodend);
        }
        if m & 0x4000 != 0 && r == ywm {
            return modulo_wrap(plain, inc, self.ymodsrt, self.ymodend);
        }
        plain
    }

    pub(crate) fn read_ea(&mut self, e: Ea, byte: bool) -> u16 {
        match e {
            Ea::Dir(r) => {
                if byte {
                    self.w(r) & 0xFF
                } else {
                    self.w(r)
                }
            }
            _ => {
                let a = self.ea(e, if byte { 1 } else { 2 }, false).unwrap_or(0);
                if byte { u16::from(self.rd8(a)) } else { self.rd16(a) }
            }
        }
    }

    /// Reads the operand, returning its address too when it is in memory (read-modify-write instructions write it back).
    pub(crate) fn read_ea_addr(&mut self, e: Ea, byte: bool) -> (u16, Option<u16>) {
        match e {
            Ea::Dir(_) => (self.read_ea(e, byte), None),
            _ => {
                let a = self.ea(e, if byte { 1 } else { 2 }, false).unwrap_or(0);
                (if byte { u16::from(self.rd8(a)) } else { self.rd16(a) }, Some(a))
            }
        }
    }

    pub(crate) fn write_ea(&mut self, e: Ea, byte: bool, v: u16) {
        match e {
            Ea::Dir(r) => {
                if byte {
                    let o = self.w(r);
                    self.set_w(r, (o & 0xFF00) | (v & 0xFF));
                } else {
                    self.set_w(r, v);
                }
            }
            _ => {
                let a = self.ea(e, if byte { 1 } else { 2 }, true).unwrap_or(0);
                self.store(a, byte, v);
            }
        }
    }

    pub(crate) fn store(&mut self, a: u16, byte: bool, v: u16) {
        self.stack_check(a);
        if byte {
            self.wr8(a, v as u8)
        } else {
            self.wr16(a, v)
        }
    }

    fn stack_check(&mut self, _a: u16) {}

    // ------------------------------------------------------------------------------------------------
    // Stack
    // ------------------------------------------------------------------------------------------------

    pub(crate) fn push16(&mut self, v: u16) {
        self.wmask |= 1 << 15;
        self.w15_direct = false;
        let sp = self.w[15];
        if self.splim_set && sp > self.splim {
            self.stack_error();
            return;
        }
        self.wr16(sp, v);
        self.w[15] = sp.wrapping_add(2);
    }

    pub(crate) fn pop16(&mut self) -> u16 {
        self.src_used |= 1 << 15;
        let sp = self.w[15].wrapping_sub(2);
        if sp < 0x0800 && false {
            self.stack_error();
        }
        self.w[15] = sp;
        self.rd16(sp)
    }

    fn stack_error(&mut self) {
        self.mem[0x8C0] |= 0x04; // STKERR
        self.trap_pending = Some(Trap::Stack);
    }

    /// Pushes a return address (and the SFA bit in the least significant bit of the low word).
    pub(crate) fn push_pc(&mut self, ret: u32) {
        let lo = (ret as u16 & 0xFFFE) | u16::from(self.sfa);
        self.push16(lo);
        self.push16(((ret >> 16) & 0x7F) as u16);
        self.sfa = false;
        self.call_depth += 1;
    }

    pub(crate) fn pop_pc(&mut self) -> u32 {
        let hi = self.pop16();
        let lo = self.pop16();
        self.sfa = lo & 1 != 0;
        self.call_depth -= 1;
        (u32::from(hi & 0x7F) << 16) | u32::from(lo & 0xFFFE)
    }

    /// Return addresses found on the stack (innermost first): word pairs `{PC[15:1]:SFA, PC[22:16]}` just after a call.
    pub fn return_addresses(&self) -> Vec<u32> {
        let mut out = Vec::new();
        let mut sp = self.w[15] as i32 - 2;
        while sp >= 0x0802 && out.len() < 64 {
            let hi = rd16m(&self.mem, sp as usize);
            let lo = rd16m(&self.mem, sp as usize - 2);
            let ra = (u32::from(hi & 0x7F) << 16) | u32::from(lo & 0xFFFE);
            if hi & 0xFF80 == 0 && ra >= 4 && (ra as usize / 2) < self.prog.len() && self.is_call_before(ra) {
                out.push(ra);
                sp -= 4;
            } else {
                sp -= 2;
            }
        }
        out
    }

    fn is_call_before(&self, ra: u32) -> bool {
        let one = ra >= 2 && matches!(decode(self.fetch(ra - 2), 0).map(|d| d.inst), Some(Inst::Rcall { .. } | Inst::CallW { .. } | Inst::RcallW { .. }));
        let two = ra >= 4 && matches!(decode(self.fetch(ra - 4), self.fetch(ra - 2)).map(|d| d.inst), Some(Inst::Call { .. }));
        one || two
    }

    /// Address of the CALL (or RCALL) instruction whose return address is `ra`.
    pub fn call_before(&self, ra: u32) -> u32 {
        if ra >= 4 && matches!(decode(self.fetch(ra - 4), self.fetch(ra - 2)).map(|d| d.inst), Some(Inst::Call { .. })) { ra - 4 } else { ra.saturating_sub(2) }
    }

    /// Simulated time of the cycle count, in nanoseconds (an instruction cycle is two oscillator periods).
    pub fn time_ns(&self) -> f64 {
        self.cycles as f64 * 2000.0 / self.fosc_mhz()
    }

    // ------------------------------------------------------------------------------------------------
    // ALU
    // ------------------------------------------------------------------------------------------------

    pub(crate) fn mask(b: bool) -> u32 {
        if b { 0xFF } else { 0xFFFF }
    }

    /// `a + b + cin` with the flags; returns the result masked to the operand size.
    pub(crate) fn add_flags(&mut self, a: u32, b: u32, cin: u32, byte: bool, sticky_z: bool) -> u16 {
        let m = Self::mask(byte);
        let sign = if byte { 0x80 } else { 0x8000 };
        let (a, b) = (a & m, b & m);
        let sum = a + b + cin;
        let r = sum & m;
        let lowm = 0xF; // DC is the carry out of bit 3 for word operations too (MDB, checked)
        let dc = ((a & lowm) + (b & lowm) + cin) > lowm;
        let ov = (!(a ^ b) & (a ^ r) & sign) != 0;
        self.set_flag(C, sum > m);
        self.set_flag(DC, dc);
        self.set_flag(OV, ov);
        self.set_flag(N, r & sign != 0);
        if sticky_z {
            if r != 0 {
                self.sr &= !Z;
            }
        } else {
            self.set_flag(Z, r == 0);
        }
        r as u16
    }

    /// `a - b - borrow_in` where `cin` is the carry (not borrow) input: 1 for a plain subtract.
    pub(crate) fn sub_flags(&mut self, a: u32, b: u32, cin: u32, byte: bool, sticky_z: bool) -> u16 {
        let m = Self::mask(byte);
        let sign = if byte { 0x80 } else { 0x8000 };
        let (a, b) = (a & m, b & m);
        let nb = !b & m;
        let sum = a + nb + cin;
        let r = sum & m;
        let lowm = 0xF; // DC is the carry out of bit 3 for word operations too (MDB, checked)
        let dc = ((a & lowm) + (nb & lowm) + cin) > lowm;
        let ov = ((a ^ b) & (a ^ r) & sign) != 0;
        self.set_flag(C, sum > m);
        self.set_flag(DC, dc);
        self.set_flag(OV, ov);
        self.set_flag(N, r & sign != 0);
        if sticky_z {
            if r != 0 {
                self.sr &= !Z;
            }
        } else {
            self.set_flag(Z, r == 0);
        }
        r as u16
    }

    pub(crate) fn nz(&mut self, r: u32, byte: bool) {
        let sign = if byte { 0x80 } else { 0x8000 };
        self.set_flag(N, r & sign != 0);
        self.set_flag(Z, r & Self::mask(byte) == 0);
    }

    /// `op(a, b)` for the three-operand group; `a` is the first operand (`Wb`/`f`/`Wn`), `b` the second.
    pub(crate) fn alu(&mut self, op: A3, a: u16, b: u16, byte: bool) -> u16 {
        let (a, b) = (u32::from(a), u32::from(b));
        let c = u32::from(self.flag(C));
        match op {
            A3::Add => self.add_flags(a, b, 0, byte, false),
            A3::Addc => self.add_flags(a, b, c, byte, true),
            A3::Sub => self.sub_flags(a, b, 1, byte, false),
            A3::Subb => self.sub_flags(a, b, c, byte, true),
            A3::Subr => self.sub_flags(b, a, 1, byte, false),
            A3::Subbr => self.sub_flags(b, a, c, byte, true),
            A3::And => {
                let r = (a & b) & Self::mask(byte);
                self.nz(r, byte);
                r as u16
            }
            A3::Ior => {
                let r = (a | b) & Self::mask(byte);
                self.nz(r, byte);
                r as u16
            }
            A3::Xor => {
                let r = (a ^ b) & Self::mask(byte);
                self.nz(r, byte);
                r as u16
            }
        }
    }

    /// A file-register operand: byte address for byte operations, word address otherwise.
    fn file_addr(&mut self, f: u16, byte: bool) -> u16 {
        if byte { f } else { f & !1 }
    }

    pub(crate) fn rd_file(&mut self, f: u16, byte: bool) -> u16 {
        let a = self.file_addr(f, byte);
        if byte { u16::from(self.rd8(a)) } else { self.rd16(a) }
    }

    pub(crate) fn wr_file(&mut self, f: u16, byte: bool, v: u16) {
        let a = self.file_addr(f, byte);
        self.store(a, byte, v);
    }

    pub(crate) fn wreg_write(&mut self, byte: bool, v: u16) {
        if byte {
            self.w[0] = (self.w[0] & 0xFF00) | (v & 0xFF);
        } else {
            self.w[0] = v;
        }
    }

    pub(crate) fn wreg(&self, byte: bool) -> u16 {
        if byte { self.w[0] & 0xFF } else { self.w[0] }
    }

    pub(crate) fn unary(&mut self, op: Un, v: u16, byte: bool) -> u16 {
        let v32 = u32::from(v);
        let m = Self::mask(byte);
        let sign = if byte { 0x80 } else { 0x8000 };
        match op {
            Un::Inc => self.add_flags(v32, 1, 0, byte, false),
            Un::Inc2 => self.add_flags(v32, 2, 0, byte, false),
            Un::Dec => self.sub_flags(v32, 1, 1, byte, false),
            Un::Dec2 => self.sub_flags(v32, 2, 1, byte, false),
            Un::Neg => self.sub_flags(0, v32, 1, byte, false),
            Un::Com => {
                let r = !v32 & m;
                self.nz(r, byte);
                r as u16
            }
            Un::Sl => {
                let r = (v32 << 1) & m;
                self.set_flag(C, v32 & sign != 0);
                self.nz(r, byte);
                r as u16
            }
            Un::Lsr => {
                let r = v32 >> 1;
                self.set_flag(C, v32 & 1 != 0);
                self.nz(r, byte);
                r as u16
            }
            Un::Asr => {
                let r = (v32 >> 1) | (v32 & sign);
                self.set_flag(C, v32 & 1 != 0);
                self.nz(r, byte);
                r as u16
            }
            Un::Rlc => {
                let r = ((v32 << 1) | u32::from(self.flag(C))) & m;
                self.set_flag(C, v32 & sign != 0);
                self.nz(r, byte);
                r as u16
            }
            Un::Rlnc => {
                let r = ((v32 << 1) | u32::from(v32 & sign != 0)) & m;
                self.nz(r, byte);
                r as u16
            }
            Un::Rrc => {
                let r = (v32 >> 1) | if self.flag(C) { sign } else { 0 };
                self.set_flag(C, v32 & 1 != 0);
                self.nz(r, byte);
                r as u16
            }
            Un::Rrnc => {
                let r = (v32 >> 1) | if v32 & 1 != 0 { sign } else { 0 };
                self.nz(r, byte);
                r as u16
            }
        }
    }

    pub(crate) fn cond(&self, cc: Cc) -> bool {
        let (c, z, n, ov) = (self.flag(C), self.flag(Z), self.flag(N), self.flag(OV));
        match cc {
            Cc::Ov => ov,
            Cc::C => c,
            Cc::Z => z,
            Cc::N => n,
            Cc::Le => z || (n != ov),
            Cc::Lt => n != ov,
            Cc::Leu => !c || z,
            Cc::Always => true,
            Cc::Nov => !ov,
            Cc::Nc => !c,
            Cc::Nz => !z,
            Cc::Nn => !n,
            Cc::Gt => !z && (n == ov),
            Cc::Ge => n == ov,
            Cc::Gtu => c && !z,
            Cc::Oa => self.flag(OA),
            Cc::Ob => self.flag(OB),
            Cc::Sa => self.flag(SA),
            Cc::Sb => self.flag(SB),
        }
    }

    // ------------------------------------------------------------------------------------------------
    // Program space (table instructions)
    // ------------------------------------------------------------------------------------------------

    /// A byte of program memory at the 24-bit address (`TBLPAG:Ws`), the way TBLRDL/TBLRDH see it.
    pub(crate) fn table_word(&self, addr: u32, high: bool) -> u16 {
        let idx = addr & !1;
        let w = self.prog_or_special(idx);
        if high { ((w >> 16) & 0xFF) as u16 } else { w as u16 }
    }

    fn prog_or_special(&self, idx: u32) -> u32 {
        match idx {
            0xFF_0000 => self.dev_id & 0xFFFF,
            0xFF_0002 => (self.dev_id >> 16) & 0xFFFF,
            _ => self.fetch(idx),
        }
    }
}

fn rd16m(m: &[u8], a: usize) -> u16 {
    u16::from(m[a]) | u16::from(m[a + 1]) << 8
}

fn wr16m(m: &mut [u8], a: usize, v: u16) {
    m[a] = v as u8;
    m[a + 1] = (v >> 8) as u8;
}


/// Modulo addressing: the pointer after an update of `inc` bytes. An incrementing pointer that passes the end address
/// (an odd byte address) continues from the start; a decrementing one that passes the start continues from the end.
fn modulo_wrap(plain: u16, inc: i32, start: u16, end: u16) -> u16 {
    let p = u32::from(plain);
    let (s, e) = (u32::from(start), u32::from(end));
    if inc > 0 && p > e && e >= s {
        return (s + (p - e - 1)) as u16;
    }
    if inc < 0 && p < s && e >= s {
        return (e + 1 - (s - p)) as u16;
    }
    plain
}

/// Bit-reversed addressing: `a` plus the modifier `xb` (buffer size in words / 2) with the carry running from the
/// most significant bit of the field down (a reverse-carry addition). Bit 0 and the bits above the pivot are unchanged.
fn bit_reverse_add(a: u16, xb: u16) -> u16 {
    if xb == 0 {
        return a;
    }
    let nb = 16 - xb.leading_zeros(); // bits of the reversed field: the pivot is the highest set bit of XB
    let field_mask: u32 = ((1u32 << nb) - 1) << 1;
    let f = (u32::from(a) & field_mask) >> 1;
    let rev = |v: u32| v.reverse_bits() >> (32 - nb);
    let r = (rev(f) + 1) & ((1u32 << nb) - 1);
    ((u32::from(a) & !field_mask) | (rev(r) << 1)) as u16
}
