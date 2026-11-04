//! PIC18 core: the standard instruction set of DS40002214G Table 45-2 (pp.805-808), executed from the
//! program bytes pic-as wrote. Cycle counts are the table's: 1, +1 when the PC changes, +the length in
//! words of the skipped instruction for a taken skip, 2 for MOVFF/LFSR/TBLRD/TBLWT, 3 for MOVFFL.
//! Interrupts, resets other than the first, SLEEP and the Q41 hardware context are later milestones (S3).

use crate::mem::{DataMem, Special};
use crate::periph::{Periph, ResetKind};
use devdesc::Device;
use std::collections::{BTreeMap, BTreeSet};

pub const C: u8 = 1;
pub const DC: u8 = 2;
pub const Z: u8 = 4;
pub const OV: u8 = 8;
pub const N: u8 = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Running,
    /// SLEEP executed; ends by a watchdog time-out or an enabled interrupt flag.
    Sleeping,
}

pub struct Pic18 {
    pub m: DataMem,
    pub prog: Vec<u8>,
    /// Bytes outside program memory that TBLRD can see (user ID, configuration, EEPROM image, device ID).
    pub aux: BTreeMap<u32, u8>,
    pub pc: u32,
    pub cycles: u64,
    pub icount: u64,
    pub state: State,
    pub depth: u32,
    pub stack: Vec<u32>,
    pub shadow: (u8, u8, u8),
    /// Configuration fields as programmed by the loaded image.
    pub cfg: crate::config::Config,
    cfg_regs: Vec<devdesc::ConfigRegister>,
    /// SFR addresses of the registers whose reset value depends on the configuration.
    cfg_sfrs: BTreeMap<&'static str, usize>,
    pub p: Periph,
    /// Interrupts being served, innermost last (`true` = high priority).
    pub in_service: Vec<bool>,
    /// Registers saved by the vectored controller's hardware context (Q41), innermost last.
    ctx: Vec<Ctx>,
    /// PIRn/PIEn/IPRn addresses per interrupt register group n.
    irq: Vec<[usize; 3]>,
    intcon: usize,
    vic: bool,
    vec_high: u32,
    vec_low: u32,
    /// Messages about things the model does not do (undefined opcode, stray address), deduplicated.
    pub notes: BTreeSet<String>,
    wide_fsr: bool,
    stk_mask: u8,
    pc_mask: u32,
    /// The device description, to rebuild the peripherals on a reset that is not a power-on reset.
    dev: Device,
    /// Stimulus text applied again to the rebuilt peripherals after such a reset (set by `Machine::load_stimulus`).
    pub stim: Option<String>,
    /// MCLR-column reset values (Table 9-3): register address, bits set, bits cleared; `u`, `q`, `x` bits are left alone.
    mclr: Vec<(usize, u8, u8)>,
    pcon0: Option<usize>,
}

/// What the vectored interrupt controller saves on entry (DS40002214G 11.6 p.120-121).
#[derive(Clone, Default)]
struct Ctx {
    w: u8,
    status: u8,
    bsr: u8,
    fsr: [u8; 6],
    prod: [u8; 2],
    pclat: [u8; 2],
}

#[inline]
fn add8(a: u8, b: u8, cin: u8) -> (u8, u8) {
    let t = u16::from(a) + u16::from(b) + u16::from(cin);
    let r = t as u8;
    let mut f = 0;
    if t > 0xFF {
        f |= C;
    }
    if (a & 15) + (b & 15) + cin > 15 {
        f |= DC;
    }
    if (a ^ r) & (b ^ r) & 0x80 != 0 {
        f |= OV;
    }
    (r, f | zn(r))
}

#[inline]
fn zn(r: u8) -> u8 {
    (if r == 0 { Z } else { 0 }) | (r & 0x80) >> 3
}

/// Words in the instruction starting with `w` (what a skip steps over).
fn words(w: u16) -> u32 {
    match w >> 12 {
        0xC => 2,
        0xE if matches!(w >> 8, 0xEC..=0xEF) => 2,
        0x0 if w & 0xFFF0 == 0x0060 => 3,
        _ => 1,
    }
}

impl Pic18 {
    pub fn new(dev: &Device) -> Result<Pic18, String> {
        let mut m = DataMem::new(dev)?;
        let p = Periph::bind(dev, &mut m);
        let intcon = dev
            .find_sfr("INTCON")
            .or_else(|| dev.find_sfr("INTCON0"))
            .map(|s| s.addr as usize)
            .ok_or_else(|| format!("{}: no INTCON", dev.device.name))?;
        let mut irq = Vec::new();
        for n in 0..16 {
            if let (Some(a), Some(b), Some(c)) =
                (dev.find_sfr(&format!("PIR{n}")), dev.find_sfr(&format!("PIE{n}")), dev.find_sfr(&format!("IPR{n}")))
            {
                irq.push([a.addr as usize, b.addr as usize, c.addr as usize]);
                for x in [a, b, c] {
                    if m.special[x.addr as usize] == Special::None {
                        m.special[x.addr as usize] = Special::Core;
                    }
                }
            } else {
                irq.push([0; 3]);
            }
        }
        m.special[intcon] = Special::Core;
        let depth = dev.memory.hw_stack.max(1);
        let hi = m.regs.fsr[0] + 1;
        let wide_fsr = m.implemented[hi as usize] > 0x0F;
        let prog = vec![0xFF; dev.memory.program.size as usize];
        let mut p = Pic18 {
            m,
            prog,
            aux: BTreeMap::new(),
            pc: dev.vectors.reset,
            cycles: 0,
            icount: 0,
            state: State::Running,
            depth,
            stack: vec![0; depth as usize + 1],
            shadow: (0, 0, 0),
            cfg: crate::config::Config::decode(&dev.config, &BTreeMap::new()),
            cfg_regs: dev.config.clone(),
            cfg_sfrs: ["OSCCON1", "OSCCON2", "OSCFRQ", "WDTCON0", "WDTCON1"]
                .into_iter()
                .filter_map(|n| dev.find_sfr(n).map(|s| (n, s.addr as usize)))
                .collect(),
            p,
            in_service: Vec::new(),
            ctx: Vec::new(),
            irq,
            intcon,
            vic: dev.find_sfr("SHADCON").is_some(),
            vec_high: dev.vectors.interrupt_high,
            vec_low: dev.vectors.interrupt_low,
            notes: BTreeSet::new(),
            wide_fsr,
            stk_mask: (depth + 1).next_power_of_two() as u8 - 1,
            pc_mask: (dev.memory.program.size - 1) | 1,
            dev: dev.clone(),
            stim: None,
            mclr: dev
                .sfr
                .iter()
                .filter(|s| s.width == 8 && s.mclr.len() == 8 && s.name != "STATUS" && s.name != "PCON0")
                .map(|s| {
                    let (mut set, mut clr) = (0u8, 0u8);
                    for (i, c) in s.mclr.chars().enumerate() {
                        match c {
                            '1' => set |= 0x80 >> i,
                            '0' => clr |= 0x80 >> i,
                            _ => {}
                        }
                    }
                    (s.addr as usize, set, clr)
                })
                .filter(|&(_, set, clr)| set | clr != 0)
                .collect(),
            pcon0: dev.find_sfr("PCON0").map(|s| s.addr as usize),
        };
        p.pc_mask = (dev.memory.program.size.next_power_of_two() - 1) & !1;
        Ok(p)
    }

    /// Put a hex image into program memory (and the auxiliary regions).
    pub fn load(&mut self, img: &crate::hex::Image) {
        for (&a, &b) in &img.bytes {
            if (a as usize) < self.prog.len() {
                self.prog[a as usize] = b;
            } else {
                self.aux.insert(a, b);
            }
        }
        self.apply_config();
    }

    /// Decode the configuration words and set the SFR reset values that depend on them. Called on every load, and
    /// a reset builds a fresh core and loads again, so it runs after every power-on reset.
    fn apply_config(&mut self) {
        self.cfg = crate::config::Config::decode(&self.cfg_regs, &self.aux);
        for (name, v) in crate::config::reset_sfrs(&self.cfg, self.vic) {
            if let Some(&a) = self.cfg_sfrs.get(name) {
                self.m.mem[a] = v & self.m.implemented[a];
            }
        }
        let (wdte, cps, cws, ccs) = (self.cfg.get("WDTE"), self.cfg.get("WDTCPS"), self.cfg.get("WDTCWS"), self.cfg.get("WDTCCS"));
        let modelled = self.p.wdt_config(wdte.unwrap_or(0) as u8, cps.unwrap_or(0x1F) as u8, cws.unwrap_or(7) as u8, ccs.unwrap_or(7) as u8);
        if !modelled && self.cfg.get("WDTE").is_some_and(|w| w != 0) {
            self.notes.insert("watchdog enabled by WDTE: a timeout reset is not modelled (CLRWDT does nothing)".into());
        }
    }

    // ----- data memory ---------------------------------------------------------------------------------------

    #[inline]
    fn fsr(&self, n: usize) -> u32 {
        let a = self.m.regs.fsr[n] as usize;
        u32::from(self.m.mem[a + 1]) << 8 | u32::from(self.m.mem[a])
    }

    #[inline]
    fn set_fsr(&mut self, n: usize, v: u32) {
        let a = self.m.regs.fsr[n] as usize;
        self.m.mem[a] = v as u8;
        self.m.mem[a + 1] = (v >> 8) as u8 & self.m.implemented[a + 1];
    }

    #[inline]
    fn w(&self) -> u8 {
        self.m.mem[self.m.regs.wreg as usize]
    }

    #[inline]
    fn set_w(&mut self, v: u8) {
        self.m.mem[self.m.regs.wreg as usize] = v;
    }

    #[inline]
    fn st(&self) -> u8 {
        self.m.mem[self.m.regs.status as usize]
    }

    /// Set the flags in `mask` to the matching bits of `v`.
    #[inline]
    fn flags(&mut self, mask: u8, v: u8) {
        let a = self.m.regs.status as usize;
        self.m.mem[a] = self.m.mem[a] & !mask | v & mask;
    }

    pub fn tos(&self) -> u32 {
        let sp = (self.m.mem[self.m.regs.stkptr as usize] & self.stk_mask) as usize;
        self.stack[sp.min(self.depth as usize)]
    }

    /// Effective address of a file-register operand for an instruction that reads and/or writes it. An access
    /// through an FSR indirect register is resolved once here (the FSR is stepped once, as the hardware does for a
    /// read-modify-write such as `addwf POSTINC0, f`), and the returned address is the real target; a target that is
    /// itself an indirect register reads 0 and ignores writes.
    fn ea_rmw(&mut self, f: u32, acc: bool) -> u32 {
        let a = self.m.ea(f, acc);
        if let Some(Special::Indirect(n, kind)) = self.m.special.get(a as usize).copied() {
            let n = n as usize;
            let v = self.fsr(n);
            let target = match kind {
                0 | 1 | 2 => v,
                3 => v.wrapping_add(1),
                _ => v.wrapping_add(i32::from(self.w() as i8) as u32),
            } & (self.m.size - 1);
            match kind {
                1 | 3 => self.set_fsr(n, v.wrapping_add(1)),
                2 => self.set_fsr(n, v.wrapping_sub(1)),
                _ => {}
            }
            return if matches!(self.m.special.get(target as usize), Some(Special::Indirect(..))) { u32::MAX } else { target };
        }
        a
    }

    /// A register as a display shows it: what the CPU would read, without side effects (no FIFO pops, no flags cleared).
    pub fn peek(&self, a: u32) -> u8 {
        let i = a as usize;
        let Some(&stored) = self.m.mem.get(i) else { return 0 };
        match self.m.special[i] {
            Special::Hook(k) => self.p.peek_hook(k, &self.m, self.cycles).unwrap_or(stored),
            Special::Pcl => self.pc as u8,
            Special::Tos(k) => (self.tos() >> (8 * u32::from(k))) as u8,
            _ => stored,
        }
    }

    pub fn read(&mut self, a: u32) -> u8 {
        self.m.note_access(a);
        let i = a as usize;
        if i >= self.m.mem.len() {
            return 0;
        }
        match self.m.special[i] {
            Special::None | Special::Core => self.m.mem[i],
            Special::Hook(k) => {
                if Periph::is_stored(k) { self.m.mem[i] } else { self.p.read(k, &mut self.m, self.cycles) }
            }
            Special::Pcl => {
                let pc = self.pc;
                self.m.mem[self.m.regs.pclath as usize] = (pc >> 8) as u8;
                self.m.mem[self.m.regs.pclatu as usize] = (pc >> 16) as u8 & self.m.implemented[self.m.regs.pclatu as usize];
                pc as u8
            }
            Special::Tos(k) => (self.tos() >> (8 * u32::from(k))) as u8,
            Special::Indirect(n, kind) => {
                let n = n as usize;
                let f = self.fsr(n);
                let target = match kind {
                    0 | 1 | 2 => f,
                    3 => f.wrapping_add(1),
                    _ => f.wrapping_add(i32::from(self.w() as i8) as u32),
                } & (self.m.size - 1);
                let v = if matches!(self.m.special.get(target as usize), Some(Special::Indirect(..))) {
                    0
                } else {
                    self.read(target)
                };
                match kind {
                    1 | 3 => self.set_fsr(n, f.wrapping_add(1)),
                    2 => self.set_fsr(n, f.wrapping_sub(1)),
                    _ => {}
                }
                v
            }
        }
    }

    pub fn write(&mut self, a: u32, v: u8) {
        self.m.note_access(a);
        let i = a as usize;
        if i >= self.m.mem.len() {
            return;
        }
        match self.m.special[i] {
            Special::None | Special::Core => self.m.mem[i] = v & self.m.implemented[i],
            Special::Hook(k) => {
                let now = self.cycles;
                self.p.sync(&mut self.m, now);
                self.p.write(k, v & self.m.implemented[i], now, &mut self.m, &mut self.prog);
                self.cycles += self.p.take_stall(); // the CPU waits for a program flash write or erase
            }
            Special::Pcl => {
                self.m.mem[self.m.regs.pcl as usize] = v;
                let hi = u32::from(self.m.mem[self.m.regs.pclatu as usize]) << 16
                    | u32::from(self.m.mem[self.m.regs.pclath as usize]) << 8;
                self.pc = (hi | u32::from(v)) & self.pc_mask;
                self.cycles += 1;
            }
            Special::Tos(k) => {
                let sp = (self.m.mem[self.m.regs.stkptr as usize] & self.stk_mask) as usize;
                if sp > 0 && sp <= self.depth as usize {
                    let sh = 8 * u32::from(k);
                    self.stack[sp] = self.stack[sp] & !(0xFF << sh) | u32::from(v) << sh;
                }
            }
            Special::Indirect(n, kind) => {
                let n = n as usize;
                let f = self.fsr(n);
                let target = match kind {
                    0 | 1 | 2 => f,
                    3 => f.wrapping_add(1),
                    _ => f.wrapping_add(i32::from(self.w() as i8) as u32),
                } & (self.m.size - 1);
                if !matches!(self.m.special.get(target as usize), Some(Special::Indirect(..))) {
                    self.write(target, v);
                }
                match kind {
                    1 | 3 => self.set_fsr(n, f.wrapping_add(1)),
                    2 => self.set_fsr(n, f.wrapping_sub(1)),
                    _ => {}
                }
            }
        }
    }

    // ----- hardware stack ------------------------------------------------------------------------------------

    fn push(&mut self, v: u32) {
        let a = self.m.regs.stkptr as usize;
        let sp = u32::from(self.m.mem[a] & self.stk_mask);
        if sp >= self.depth {
            self.m.mem[a] |= 0x80; // STKFUL
        } else {
            self.stack[sp as usize + 1] = v;
            self.m.mem[a] = self.m.mem[a] & !self.stk_mask | (sp as u8 + 1);
        }
    }

    fn pop(&mut self) -> u32 {
        let a = self.m.regs.stkptr as usize;
        let sp = self.m.mem[a] & self.stk_mask;
        if sp == 0 {
            self.m.mem[a] |= 0x40; // STKUNF
            return 0;
        }
        let v = self.stack[sp as usize];
        self.m.mem[a] = self.m.mem[a] & !self.stk_mask | (sp - 1);
        v
    }

    // ----- program memory ------------------------------------------------------------------------------------

    #[inline]
    pub fn fetch(&self, at: u32) -> u16 {
        let a = (at & self.pc_mask) as usize;
        if a + 1 < self.prog.len() { u16::from(self.prog[a]) | u16::from(self.prog[a + 1]) << 8 } else { 0xFFFF }
    }

    fn table_byte(&self, a: u32) -> u8 {
        if (a as usize) < self.prog.len() {
            self.prog[a as usize]
        } else {
            self.aux.get(&a).copied().unwrap_or(0xFF)
        }
    }

    // ----- execution -----------------------------------------------------------------------------------------

    fn skip(&mut self) -> u32 {
        let n = self.fetch(self.pc);
        let k = words(n);
        self.pc = self.pc.wrapping_add(2 * k) & self.pc_mask;
        k
    }

    /// Execute one instruction; returns the cycles it took (also added to `self.cycles`).
    pub fn step(&mut self) -> u32 {
        let start = self.cycles;
        if self.state != State::Running {
            return self.sleep_step();
        }
        let ic0 = self.m.mem[self.intcon];
        self.p.clear_event();
        let w = self.fetch(self.pc);
        let at = self.pc;
        self.pc = (self.pc + 2) & self.pc_mask;
        self.cycles += 1;
        self.icount += 1;
        let f = u32::from(w & 0xFF);
        let acc = w & 0x100 != 0;
        let d_file = w & 0x200 != 0;
        match w >> 12 {
            0x0 => self.group0(w, at),
            0x1 => {
                let (a, wv) = (self.ea_rmw(f, acc), self.w());
                let v = self.read(a);
                let r = match (w >> 10) & 3 {
                    0 => v | wv,
                    1 => v & wv,
                    2 => v ^ wv,
                    _ => !v,
                };
                self.dest(d_file, a, r);
                self.flags(Z | N, zn(r));
            }
            0x2 => {
                let a = self.ea_rmw(f, acc);
                let v = self.read(a);
                match (w >> 10) & 3 {
                    0 => {
                        let (r, fl) = add8(v, self.w(), self.st() & C);
                        self.dest(d_file, a, r);
                        self.flags(C | DC | Z | OV | N, fl);
                    }
                    1 => {
                        let (r, fl) = add8(v, self.w(), 0);
                        self.dest(d_file, a, r);
                        self.flags(C | DC | Z | OV | N, fl);
                    }
                    2 => {
                        let (r, fl) = add8(v, 1, 0);
                        self.dest(d_file, a, r);
                        self.flags(C | DC | Z | OV | N, fl);
                    }
                    _ => {
                        let r = v.wrapping_sub(1);
                        self.dest(d_file, a, r);
                        if r == 0 {
                            let k = self.skip();
                            self.cycles += u64::from(k);
                        }
                    }
                }
            }
            0x3 => {
                let a = self.ea_rmw(f, acc);
                let v = self.read(a);
                let cin = self.st() & C;
                match (w >> 10) & 3 {
                    0 => {
                        let r = v >> 1 | cin << 7;
                        self.dest(d_file, a, r);
                        self.flags(C | Z | N, v & 1 | zn(r));
                    }
                    1 => {
                        let r = v << 1 | cin;
                        self.dest(d_file, a, r);
                        self.flags(C | Z | N, v >> 7 | zn(r));
                    }
                    2 => self.dest(d_file, a, v.rotate_left(4)),
                    _ => {
                        let r = v.wrapping_add(1);
                        self.dest(d_file, a, r);
                        if r == 0 {
                            let k = self.skip();
                            self.cycles += u64::from(k);
                        }
                    }
                }
            }
            0x4 => {
                let a = self.ea_rmw(f, acc);
                let v = self.read(a);
                match (w >> 10) & 3 {
                    0 => {
                        let r = v.rotate_right(1);
                        self.dest(d_file, a, r);
                        self.flags(Z | N, zn(r));
                    }
                    1 => {
                        let r = v.rotate_left(1);
                        self.dest(d_file, a, r);
                        self.flags(Z | N, zn(r));
                    }
                    2 => {
                        let r = v.wrapping_add(1);
                        self.dest(d_file, a, r);
                        if r != 0 {
                            let k = self.skip();
                            self.cycles += u64::from(k);
                        }
                    }
                    _ => {
                        let r = v.wrapping_sub(1);
                        self.dest(d_file, a, r);
                        if r != 0 {
                            let k = self.skip();
                            self.cycles += u64::from(k);
                        }
                    }
                }
            }
            0x5 => {
                let a = self.ea_rmw(f, acc);
                let v = self.read(a);
                let wv = self.w();
                let cin = self.st() & C;
                match (w >> 10) & 3 {
                    0 => {
                        self.dest(d_file, a, v);
                        self.flags(Z | N, zn(v));
                    }
                    1 => {
                        let (r, fl) = add8(wv, !v, cin);
                        self.dest(d_file, a, r);
                        self.flags(C | DC | Z | OV | N, fl);
                    }
                    2 => {
                        let (r, fl) = add8(v, !wv, cin);
                        self.dest(d_file, a, r);
                        self.flags(C | DC | Z | OV | N, fl);
                    }
                    _ => {
                        let (r, fl) = add8(v, !wv, 1);
                        self.dest(d_file, a, r);
                        self.flags(C | DC | Z | OV | N, fl);
                    }
                }
            }
            0x6 => {
                let a = self.ea_rmw(f, acc);
                match (w >> 9) & 7 {
                    0 | 1 | 2 | 3 => {
                        let v = self.read(a);
                        let wv = self.w();
                        let go = match (w >> 9) & 7 {
                            0 => v < wv,
                            1 => v == wv,
                            2 => v > wv,
                            _ => v == 0,
                        };
                        if go {
                            let k = self.skip();
                            self.cycles += u64::from(k);
                        }
                    }
                    4 => self.write(a, 0xFF),
                    5 => {
                        self.write(a, 0);
                        self.flags(Z, Z);
                    }
                    6 => {
                        let v = self.read(a);
                        let (r, fl) = add8(0, !v, 1);
                        self.write(a, r);
                        self.flags(C | DC | Z | OV | N, fl);
                    }
                    _ => {
                        let wv = self.w();
                        self.write(a, wv);
                    }
                }
            }
            0x7 | 0x8 | 0x9 => {
                let a = self.ea_rmw(f, acc);
                let bit = 1u8 << ((w >> 9) & 7);
                let v = self.read(a);
                let r = match w >> 12 {
                    7 => v ^ bit,
                    8 => v | bit,
                    _ => v & !bit,
                };
                self.write(a, r);
            }
            0xA | 0xB => {
                let a = self.ea_rmw(f, acc);
                let bit = 1u8 << ((w >> 9) & 7);
                let set = self.read(a) & bit != 0;
                if set == (w >> 12 == 0xA) {
                    let k = self.skip();
                    self.cycles += u64::from(k);
                }
            }
            0xC => {
                let w2 = self.fetch(self.pc);
                self.pc = (self.pc + 2) & self.pc_mask;
                let (s, d) = (u32::from(w & 0xFFF), u32::from(w2 & 0xFFF));
                let v = self.read(s);
                self.write(d, v);
                self.cycles += 1;
            }
            0xD => {
                if w & 0x800 != 0 {
                    let ret = self.pc;
                    self.push(ret);
                }
                let off = i32::from((w & 0x7FF) as i16) - if w & 0x400 != 0 { 0x800 } else { 0 };
                self.pc = self.pc.wrapping_add((off * 2) as u32) & self.pc_mask;
                self.cycles += 1;
            }
            0xE => self.groupe(w),
            _ => {} // 0xFxxx executed alone is a NOP
        }
        let _ = at;
        let now = self.cycles;
        self.p.sync(&mut self.m, now);
        if let Some(k) = self.p.take_reset() {
            self.reset_cond(k);
            return (self.cycles - start) as u32;
        }
        self.p.board_poll(&self.m, now);
        self.irq_check(ic0);
        if !self.p.notes.is_empty() {
            let n = std::mem::take(&mut self.p.notes);
            self.notes.extend(n);
        }
        (self.cycles - start) as u32
    }

    // ----- sleep ---------------------------------------------------------------------------------------------

    /// Some interrupt flag is set with its enable bit (and PEIE for the groups after PIR0), whatever GIE says
    /// (DS40002043F 7.2.1-7.2.2 p.69).
    fn wake_irq(&self) -> bool {
        let peie = self.m.mem[self.intcon] & 0x40 != 0;
        self.irq.iter().enumerate().any(|(n, g)| g[0] != 0 && (n == 0 || self.vic || peie) && self.m.mem[g[0]] & self.m.mem[g[1]] != 0)
    }

    /// SLEEP: a wake-up interrupt already pending makes it a NOP; otherwise PD = 0, TO = 1, the watchdog is cleared `[A93]`.
    fn sleep(&mut self) {
        if self.wake_irq() {
            return;
        }
        let st = self.m.regs.status as usize;
        self.m.mem[st] = self.m.mem[st] & !0x20 | 0x40;
        self.state = State::Sleeping;
        let now = self.cycles;
        self.p.enter_sleep(&self.m, now);
    }

    /// Nothing can end this Sleep: no interrupt is enabled and the watchdog does not count.
    pub fn sleep_dead(&self) -> bool {
        self.state == State::Sleeping && !self.irq.iter().any(|g| g[0] != 0 && self.m.mem[g[1]] != 0) && self.p.wdt_sleep_deadline(&self.m).is_none()
    }

    /// One step while asleep: a cycle, or a jump to the watchdog time-out when no interrupt is enabled. Wake-up by the watchdog
    /// (TO = PD = 0) or by an interrupt (TO = 1, PD = 0) continues at PC + 2, which the SLEEP fetch already left in the PC (Table 9-3 p.90).
    fn sleep_step(&mut self) -> u32 {
        let start = self.cycles;
        let mut next = start + 1;
        if !self.irq.iter().any(|g| g[0] != 0 && self.m.mem[g[1]] != 0)
            && let Some(d) = self.p.wdt_sleep_deadline(&self.m)
        {
            next = d.max(next);
        }
        self.cycles = next;
        self.p.sync(&mut self.m, next);
        let st = self.m.regs.status as usize;
        let by_wdt = self.p.wdt_woke();
        if by_wdt || self.wake_irq() {
            self.m.mem[st] = if by_wdt { self.m.mem[st] & !0x60 } else { self.m.mem[st] & !0x20 | 0x40 };
            self.state = State::Running;
            self.p.exit_sleep(next);
        }
        (self.cycles - start) as u32
    }

    // ----- interrupts ----------------------------------------------------------------------------------------

    /// Take an interrupt if one is enabled and pending after the instruction that just finished.
    /// Rules: DS40002043F 15.1-15.3 p.193-194 (Q10), DS40002214G 11.5-11.8 p.118-124 (Q41, `MVECEN = 0`).
    /// `[A4]` Entry costs two cycles (return address pushed, vector fetched), like a CALL.
    fn irq_check(&mut self, ic0: u8) {
        // A flag set in the last cycle of the instruction that clears GIE is still taken `[A6]` (MDB; the Q10 code
        // that clears GIE re-checks it for exactly this reason).
        let mut c = self.m.mem[self.intcon];
        if c & 0x80 == 0 && ic0 & 0x80 != 0 && self.p.peek_event() == Some(self.cycles) {
            c = ic0;
        }
        if c & 0x80 == 0 {
            return;
        }
        let ipen = c & 0x20 != 0;
        let giel = c & 0x40 != 0;
        let (mut hi, mut lo) = (false, false);
        for (n, g) in self.irq.iter().enumerate() {
            if g[0] == 0 {
                continue;
            }
            let f = self.m.mem[g[0]] & self.m.mem[g[1]];
            if f == 0 {
                continue;
            }
            if !ipen {
                // Without priorities PEIE gates the peripheral groups (PIR1 and up) on the Q10, whose PIR0 sources
                // are core interrupts `[A5]`; the Q41 has no PEIE (GIEL is reserved).
                if self.vic || n == 0 || giel {
                    hi = true;
                }
            } else {
                let ip = self.m.mem[g[2]];
                hi |= f & ip != 0;
                lo |= f & !ip != 0;
            }
        }
        if !hi && !lo {
            return;
        }
        let blocked_hi = self.vic && if ipen { self.in_service.contains(&true) } else { !self.in_service.is_empty() };
        let high = if hi && !blocked_hi {
            true
        } else if lo && giel && !(self.vic && !self.in_service.is_empty()) {
            false
        } else {
            return;
        };
        if self.vic && self.cfg.get("MVECEN") == Some(1) {
            self.notes.insert("MVECEN = 1 (vector table) is not simulated: the interrupt went to the legacy vector".into());
        }
        let ret = self.pc;
        self.push(ret);
        self.pc = (if high { self.vec_high } else { self.vec_low }) & self.pc_mask;
        // MDB reaches the vector three cycles after the flag was set, whichever cycle of a multi-cycle instruction
        // that was; never less than two cycles after the instruction `[A4]`. A flag that was already pending
        // (interrupts were enabled later) counts as set one cycle before the end of the instruction.
        let end = self.cycles;
        self.cycles = match self.p.take_event() {
            Some(e) => (e + 3).max(end + 2),
            // Already pending and enabled by this very instruction (BSF INTCON, GIE): MDB enters one cycle after it
            // `[A7]`; otherwise (RETFIE re-enabling, PIE written) two.
            None => end + 2,
        };
        if self.vic {
            let r = &self.m.regs;
            let mut fsr = [0u8; 6];
            for n in 0..3 {
                fsr[2 * n] = self.m.mem[r.fsr[n] as usize];
                fsr[2 * n + 1] = self.m.mem[r.fsr[n] as usize + 1];
            }
            let ctx = Ctx {
                w: self.w(),
                status: self.st(),
                bsr: self.m.mem[r.bsr as usize],
                fsr,
                prod: [self.m.mem[r.prodl as usize], self.m.mem[r.prodh as usize]],
                pclat: [self.m.mem[r.pclath as usize], self.m.mem[r.pclatu as usize]],
            };
            self.ctx.push(ctx);
        } else {
            self.shadow_save();
            let bit = if ipen && !high { 0x40 } else { 0x80 };
            self.m.mem[self.intcon] &= !bit;
        }
        self.in_service.push(high);
        let now = self.cycles;
        self.p.sync(&mut self.m, now);
    }

    fn retfie(&mut self, s: bool) {
        self.cycles += 1;
        self.pc = self.pop();
        let level = self.in_service.pop();
        if self.vic {
            // The controller tracks the level itself; GIE is not changed `DS40002214G 11.8.1 p.123`.
            if let Some(c) = self.ctx.pop()
                && s
            {
                let r = self.m.regs.clone();
                self.set_w(c.w);
                self.m.mem[r.status as usize] = c.status;
                self.m.mem[r.bsr as usize] = c.bsr;
                for n in 0..3 {
                    self.m.mem[r.fsr[n] as usize] = c.fsr[2 * n];
                    self.m.mem[r.fsr[n] as usize + 1] = c.fsr[2 * n + 1];
                }
                self.m.mem[r.prodl as usize] = c.prod[0];
                self.m.mem[r.prodh as usize] = c.prod[1];
                self.m.mem[r.pclath as usize] = c.pclat[0];
                self.m.mem[r.pclatu as usize] = c.pclat[1];
            }
        } else {
            if s {
                self.shadow_restore();
            }
            let c = &mut self.m.mem[self.intcon];
            // RETFIE sets GIE/GIEH, or GIEL when returning from a low-priority handler `DS40002043F p.194`.
            *c |= if level == Some(false) && *c & 0x20 != 0 { 0x40 } else { 0x80 };
        }
    }

    #[inline]
    fn dest(&mut self, to_file: bool, a: u32, r: u8) {
        if to_file {
            self.write(a, r);
        } else {
            self.set_w(r);
        }
    }

    fn lit_alu(&mut self, op: u16, k: u8) {
        let wv = self.w();
        let (r, fl, mask) = match op {
            0x8 => {
                let (r, fl) = add8(k, !wv, 1);
                (r, fl, C | DC | Z | OV | N)
            }
            0x9 => (wv | k, zn(wv | k), Z | N),
            0xA => (wv ^ k, zn(wv ^ k), Z | N),
            0xB => (wv & k, zn(wv & k), Z | N),
            _ => {
                let (r, fl) = add8(wv, k, 0);
                (r, fl, C | DC | Z | OV | N)
            }
        };
        self.set_w(r);
        self.flags(mask, fl);
    }

    fn group0(&mut self, w: u16, at: u32) {
        match w >> 8 {
            0x00 => match w & 0xFF {
                0x00 => {}
                0x03 => self.sleep(),
                0x04 => {
                    let now = self.cycles;
                    self.p.clrwdt(&self.m, now);
                }
                0x05 => {
                    let pc = self.pc;
                    self.push(pc);
                }
                0x06 => {
                    self.pop();
                }
                0x07 => self.daw(),
                0x08..=0x0B => self.tblrd((w & 3) as u8),
                0x0C..=0x0F => self.tblwt((w & 3) as u8),
                0x10 | 0x11 => self.retfie(w & 1 != 0),
                0x12 | 0x13 => {
                    self.cycles += 1;
                    self.pc = self.pop();
                    if w & 1 != 0 {
                        self.shadow_restore();
                    }
                }
                0x14 => {
                    let pc = self.pc;
                    self.push(pc);
                    self.pc = (u32::from(self.m.mem[self.m.regs.pclatu as usize]) << 16
                        | u32::from(self.m.mem[self.m.regs.pclath as usize]) << 8
                        | u32::from(self.w()))
                        & self.pc_mask;
                    self.cycles += 1;
                }
                0x60..=0x6F => self.movffl(w),
                0xFF => self.reset(),
                _ => self.undefined(w, at),
            },
            0x01 => {
                let b = (w & 0x3F) as u8;
                let a = self.m.regs.bsr as usize;
                self.m.mem[a] = b & self.m.implemented[a];
            }
            0x02 | 0x03 => {
                let a = self.ea_rmw(u32::from(w & 0xFF), w & 0x100 != 0);
                let p = u16::from(self.read(a)) * u16::from(self.w());
                self.m.mem[self.m.regs.prodl as usize] = p as u8;
                self.m.mem[self.m.regs.prodh as usize] = (p >> 8) as u8;
            }
            0x04..=0x07 => {
                let a = self.ea_rmw(u32::from(w & 0xFF), w & 0x100 != 0);
                let v = self.read(a);
                let (r, fl) = add8(v, 0xFF, 0);
                self.dest(w & 0x200 != 0, a, r);
                self.flags(C | DC | Z | OV | N, fl);
            }
            0x0C => {
                self.cycles += 1;
                self.pc = self.pop();
                self.set_w(w as u8);
            }
            0x0D => {
                let p = u16::from(w as u8) * u16::from(self.w());
                self.m.mem[self.m.regs.prodl as usize] = p as u8;
                self.m.mem[self.m.regs.prodh as usize] = (p >> 8) as u8;
            }
            0x0E => self.set_w(w as u8),
            0x08..=0x0B | 0x0F => self.lit_alu(w >> 8, w as u8),
            _ => self.undefined(w, at),
        }
    }

    fn groupe(&mut self, w: u16) {
        match (w >> 8) & 0xF {
            c @ 0..=7 => {
                let st = self.st();
                let go = match c {
                    0 => st & Z != 0,
                    1 => st & Z == 0,
                    2 => st & C != 0,
                    3 => st & C == 0,
                    4 => st & OV != 0,
                    5 => st & OV == 0,
                    6 => st & N != 0,
                    _ => st & N == 0,
                };
                if go {
                    let off = i32::from(w as u8 as i8);
                    self.pc = self.pc.wrapping_add((off * 2) as u32) & self.pc_mask;
                    self.cycles += 1;
                }
            }
            8 | 9 => {
                let n = ((w >> 6) & 3) as usize;
                let k = u32::from(w & 0x3F);
                if n < 3 {
                    let v = self.fsr(n);
                    let r = if w >> 8 & 1 == 0 { v.wrapping_add(k) } else { v.wrapping_sub(k) };
                    self.set_fsr(n, r);
                }
            }
            0xC | 0xD => {
                let w2 = self.fetch(self.pc);
                self.pc = (self.pc + 2) & self.pc_mask;
                let target = (u32::from(w2 & 0xFFF) << 8 | u32::from(w & 0xFF)) << 1;
                let ret = self.pc;
                self.push(ret);
                if w & 0x100 != 0 {
                    self.shadow_save();
                }
                self.pc = target & self.pc_mask;
                self.cycles += 1;
            }
            0xE => {
                let w2 = self.fetch(self.pc);
                self.pc = (self.pc + 2) & self.pc_mask;
                let n = ((w >> 4) & 3) as usize;
                let k = if self.wide_fsr {
                    u32::from(w & 0xF) << 10 | u32::from(w2 & 0x3FF)
                } else {
                    u32::from(w & 0xF) << 8 | u32::from(w2 & 0xFF)
                };
                if n < 3 {
                    self.set_fsr(n, k);
                }
                self.cycles += 1;
            }
            0xF => {
                let w2 = self.fetch(self.pc);
                self.pc = (self.pc + 2) & self.pc_mask;
                self.pc = ((u32::from(w2 & 0xFFF) << 8 | u32::from(w & 0xFF)) << 1) & self.pc_mask;
                self.cycles += 1;
            }
            _ => self.undefined(w, self.pc.wrapping_sub(2)),
        }
    }

    fn movffl(&mut self, w: u16) {
        let w2 = self.fetch(self.pc);
        let w3 = self.fetch(self.pc + 2);
        self.pc = (self.pc + 4) & self.pc_mask;
        // fs = word1<3:0> : word2<11:2>;  fd = word2<1:0> : word3<11:0>   (DS40002214G Table 45-2)
        let s = u32::from(w & 0xF) << 10 | u32::from(w2 & 0xFFF) >> 2;
        let d = u32::from(w2 & 3) << 12 | u32::from(w3 & 0xFFF);
        let v = self.read(s);
        self.write(d, v);
        self.cycles += 2;
    }

    fn tblrd(&mut self, mode: u8) {
        let a = self.m.regs.tblptr as usize;
        let ptr = |m: &DataMem| u32::from(m.mem[a + 2]) << 16 | u32::from(m.mem[a + 1]) << 8 | u32::from(m.mem[a]);
        let mut p = ptr(&self.m);
        if mode == 3 {
            p = p.wrapping_add(1);
        }
        let b = self.table_byte(p & 0x3FFFFF);
        self.m.mem[self.m.regs.tablat as usize] = b;
        match mode {
            1 => p = p.wrapping_add(1),
            2 => p = p.wrapping_sub(1),
            _ => {}
        }
        self.m.mem[a] = p as u8;
        self.m.mem[a + 1] = (p >> 8) as u8;
        self.m.mem[a + 2] = (p >> 16) as u8 & self.m.implemented[a + 2];
        self.cycles += 1;
    }

    /// TBLWT: TABLAT goes to the holding register (Q10) or buffer RAM byte (Q41) that TBLPTR[7:0] selects, not to
    /// flash (DS40002043F 12.1.4 p.151; DS40002214G 10.3.10.5 p.99). The pointer moves as for TBLRD.
    fn tblwt(&mut self, mode: u8) {
        let a = self.m.regs.tblptr as usize;
        let mut p = u32::from(self.m.mem[a + 2]) << 16 | u32::from(self.m.mem[a + 1]) << 8 | u32::from(self.m.mem[a]);
        if mode == 3 {
            p = p.wrapping_add(1);
        }
        match self.p.hold_base() {
            Some(base) => {
                let t = self.m.mem[self.m.regs.tablat as usize];
                self.m.mem[base + (p & 0xFF) as usize] = t;
            }
            None => {
                self.notes.insert("TBLWT on a part without holding registers is not modelled".into());
            }
        }
        match mode {
            1 => p = p.wrapping_add(1),
            2 => p = p.wrapping_sub(1),
            _ => {}
        }
        self.m.mem[a] = p as u8;
        self.m.mem[a + 1] = (p >> 8) as u8;
        self.m.mem[a + 2] = (p >> 16) as u8 & self.m.implemented[a + 2];
        self.cycles += 1;
    }

    fn daw(&mut self) {
        let mut v = self.w();
        let st = self.st();
        let mut c = st & C;
        if v & 0xF > 9 || st & DC != 0 {
            let (r, fl) = add8(v, 6, 0);
            v = r;
            if fl & C != 0 {
                c = C;
            }
        }
        if v >> 4 > 9 || c != 0 {
            v = v.wrapping_add(0x60);
            c = C;
        }
        self.set_w(v);
        self.flags(C, c);
    }

    fn shadow_save(&mut self) {
        self.shadow = (self.w(), self.st(), self.m.mem[self.m.regs.bsr as usize]);
    }

    fn shadow_restore(&mut self) {
        let (w, s, b) = self.shadow;
        self.set_w(w);
        self.m.mem[self.m.regs.status as usize] = s;
        self.m.mem[self.m.regs.bsr as usize] = b;
    }

    /// A reset that is not a power-on reset (RESET instruction, watchdog): the registers take their MCLR-column reset values, the
    /// cause bits of STATUS and PCON0 change as in Table 9-3 (DS40002043F p.89-90), the program counter and the stack pointer go to
    /// the reset state, data RAM and the program stay, and the peripherals start over (rebuilt, the stimulus applied again; the
    /// logs, the notes and the board stay). `[A91]`-`[A93]` in FACTS-SIM.md.
    pub fn reset_cond(&mut self, kind: ResetKind) {
        for &(a, set, clr) in &self.mclr {
            self.m.mem[a] = (self.m.mem[a] & !clr | set) & self.m.implemented[a];
        }
        let status = self.m.regs.status as usize;
        match kind {
            ResetKind::Wdt => {
                self.m.mem[status] &= !0x40; // nTO
                self.clear_pcon(0x10); // RWDT
            }
            ResetKind::WdtWindow => self.clear_pcon(0x20), // WDTWV
            ResetKind::Instruction => self.clear_pcon(0x04), // RI
        }
        self.apply_config();
        self.pc = self.dev.vectors.reset;
        self.state = State::Running;
        self.in_service.clear();
        self.ctx.clear();
        self.shadow = (0, 0, 0);
        let mut np = Periph::bind(&self.dev, &mut self.m);
        if let Some(t) = &self.stim
            && let Ok(st) = crate::stimulus::Stimulus::parse(t)
        {
            let _ = st.apply(&self.dev, &mut np);
        }
        let old = std::mem::replace(&mut self.p, np);
        self.p.carry_over(old);
        self.apply_config(); // the new peripherals need the watchdog configuration
    }

    fn clear_pcon(&mut self, bit: u8) {
        if let Some(a) = self.pcon0 {
            self.m.mem[a] &= !bit;
        }
    }

    fn reset(&mut self) {
        self.reset_cond(ResetKind::Instruction);
    }

    fn undefined(&mut self, w: u16, at: u32) {
        self.notes.insert(format!("undefined opcode {w:04X} at {at:06X}, executed as NOP"));
    }
}
