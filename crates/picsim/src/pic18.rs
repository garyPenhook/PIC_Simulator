//! PIC18 core: the standard instruction set of DS40002214G Table 45-2 (pp.805-808), executed from the
//! program bytes pic-as wrote. Cycle counts are the table's: 1, +1 when the PC changes, +the length in
//! words of the skipped instruction for a taken skip, 2 for MOVFF/LFSR/TBLRD/TBLWT, 3 for MOVFFL.
//! Interrupts, resets other than the first, SLEEP and the Q41 hardware context are later milestones (S3).

use crate::mem::{DataMem, Special};
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
    /// SLEEP executed (no wake sources are modelled before S3).
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
    /// Messages about things the model does not do (undefined opcode, stray address), deduplicated.
    pub notes: BTreeSet<String>,
    wide_fsr: bool,
    stk_mask: u8,
    pc_mask: u32,
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
        let m = DataMem::new(dev)?;
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
            notes: BTreeSet::new(),
            wide_fsr,
            stk_mask: (depth + 1).next_power_of_two() as u8 - 1,
            pc_mask: (dev.memory.program.size - 1) | 1,
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

    fn tos(&self) -> u32 {
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

    pub fn read(&mut self, a: u32) -> u8 {
        self.m.note_access(a);
        let i = a as usize;
        if i >= self.m.mem.len() {
            return 0;
        }
        match self.m.special[i] {
            Special::None | Special::Core => self.m.mem[i],
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
    fn fetch(&self, at: u32) -> u16 {
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
            self.cycles += 1;
            return 1;
        }
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
        (self.cycles - start) as u32
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
                0x03 => self.state = State::Sleeping,
                0x04 => {} // CLRWDT: watchdog is not modelled before S3
                0x05 => {
                    let pc = self.pc;
                    self.push(pc);
                }
                0x06 => {
                    self.pop();
                }
                0x07 => self.daw(),
                0x08..=0x0B => self.tblrd((w & 3) as u8),
                0x0C..=0x0F => self.cycles += 1, // TBLWT: holding latches arrive with the NVM model (S5)
                0x10 | 0x11 => {
                    // RETFIE: re-enabling the interrupt is S3
                    self.cycles += 1;
                    self.pc = self.pop();
                    if w & 1 != 0 {
                        self.shadow_restore();
                    }
                }
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

    fn reset(&mut self) {
        // Software RESET: register reset values and peripheral state arrive with S3; the core restarts.
        self.pc = 0;
        self.cycles += 0;
        self.notes.insert("RESET executed: only the PC is reset".into());
    }

    fn undefined(&mut self, w: u16, at: u32) {
        self.notes.insert(format!("undefined opcode {w:04X} at {at:06X}, executed as NOP"));
    }
}
