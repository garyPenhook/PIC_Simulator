//! Instruction execution of the dsPIC33 core (`impl Dspic`): `step` runs one instruction, or one iteration of a
//! `REPEAT`, after taking a pending trap or interrupt.

use crate::dspic::*;
use crate::dspic_isa::*;
use crate::dspic_isa::Un as UnOp;

impl Dspic {
    /// Runs one instruction cycle group: an exception entry, or one instruction (one `REPEAT` iteration).
    pub fn step(&mut self) {
        self.extra = 0;
        if self.sleeping.is_some() {
            // asleep: the clock runs, the CPU does not; the peripherals wake it (`wake`)
            self.cycles += 1;
            self.advance_periph();
            self.check_wake();
            return;
        }
        if self.take_exception() {
            self.advance_periph();
            return;
        }
        let pc = self.pc;
        let w0 = self.fetch(pc);
        let w1 = self.fetch(pc + 2);
        let Some(d) = decode(w0, w1) else {
            self.illegal();
            self.cycles += 1;
            self.advance_periph();
            return;
        };
        let was_repeat = self.ra;
        let mut next = pc + 2 * u32::from(d.words);
        let mut cyc: u32 = 1;
        let mut jumped = false;
        self.wmask = 0;
        self.src_used = 0;
        self.w15_direct = false;
        self.exec(&d, pc, &mut next, &mut cyc, &mut jumped);
        self.icount += 1;
        // read-after-write: a source address register the previous instruction wrote costs one stall cycle
        let stall = u32::from(self.prev_wmask & self.src_used != 0);
        // MDB gives no stall after a direct write of W15 (checked); pointer updates of W15 (push, call) do
        self.prev_wmask = self.wmask & !(u16::from(self.w15_direct) << 15);
        let total = cyc + self.extra + stall;
        // REPEAT: the instruction stays selected until the count runs out
        let mut staying = false;
        if was_repeat {
            if self.rcount > 0 {
                self.rcount -= 1;
                self.repeat_iter += 1;
                next = pc;
                staying = true;
            } else {
                self.ra = false;
                self.repeat_iter = 0;
            }
        }
        // DO: the instruction at the end address decides between another pass and leaving the loop
        if !staying && !jumped && self.da() && pc == self.doend {
            if self.edt || self.dcount == 0 {
                self.edt = false;
                self.end_do();
            } else {
                self.dcount -= 1;
                next = self.dostart;
            }
        }
        self.pc = next & 0x7F_FFFE;
        self.cycles += u64::from(total);
        if self.disicnt > 0 {
            self.disicnt = self.disicnt.saturating_sub(total as u16);
            if self.disicnt == 0 {
                self.mem[0x8C1] &= !0x40;
            }
        }
        self.advance_periph();
        if self.event == Event::Reset {
            self.event = Event::None;
            self.reset_with(0x0040); // SWR
        }
    }

    fn end_do(&mut self) {
        // leave the loop: restore the outer loop's registers if there are any
        if let Some((s, e, c)) = self.do_stack.pop() {
            self.dostart = s;
            self.doend = e;
            self.dcount = c;
        }
        self.dl_level = self.dl_level.saturating_sub(1);
    }

    pub(crate) fn illegal(&mut self) {
        // an unimplemented opcode is an address error trap on the dsPIC33 (the "illegal opcode trap")
        self.mem[0x8C0] |= 0x08;
        self.trap_pending = Some(Trap::Address);
    }

    fn math_error(&mut self, bit: u16) {
        // INTCON1: MATHERR bit 4 plus the cause (DIV0ERR bit 6, SFTACERR bit 7)
        let v = u16::from(self.mem[0x8C0]) | u16::from(self.mem[0x8C1]) << 8 | 0x10 | bit;
        self.mem[0x8C0] = v as u8;
        self.mem[0x8C1] = (v >> 8) as u8;
        self.trap_pending = Some(Trap::Math);
    }

    /// Number of words the instruction at `pc` occupies (for the skip instructions).
    fn words_at(&self, pc: u32) -> u8 {
        decode(self.fetch(pc), self.fetch(pc + 2)).map_or(1, |d| d.words)
    }

    fn skip(&mut self, pc: u32, next: &mut u32, cyc: &mut u32) {
        let n = self.words_at(pc + 2);
        *next = pc + 2 + 2 * u32::from(n);
        *cyc = 1 + u32::from(n);
    }

    fn wd(&mut self, n: u8, v: u16, byte: bool) {
        if byte {
            let o = self.w[n as usize];
            self.set_w(n, (o & 0xFF00) | (v & 0xFF));
        } else {
            self.set_w(n, v);
        }
    }

    fn exec(&mut self, d: &Decoded, pc: u32, next: &mut u32, cyc: &mut u32, jumped: &mut bool) {
        use Inst::*;
        let np = pc + 2;
        let mut jump = |s: &mut Self, to: u32, c: u32, next: &mut u32, cyc: &mut u32| {
            let _ = &s;
            *next = to;
            *cyc = c;
            *jumped = true;
        };
        match d.inst {
            Nop | Nopr => {}
            Alu3 { op, b, wb, s, d: dst } => {
                // MDB: a destination `[++Wn]` / `[--Wn]` updates Wn before a direct source Wn is read
                let pre = match (dst, s) {
                    (Ea::PreInc(r) | Ea::PreDec(r), Src::Ws(Ea::Dir(sr))) if r == sr => dst_addr_first(self, dst, b),
                    _ => None,
                };
                let a = if b { self.w(wb) & 0xFF } else { self.w(wb) };
                let sv = match s {
                    Src::Ws(e) => self.read_ea(e, b),
                    Src::Lit(k) => k,
                };
                let r = self.alu(op, a, sv, b);
                match pre {
                    Some(addr) => self.store(addr, b, r),
                    None => self.write_ea(dst, b, r),
                }
            }
            AluF { op, b, f, w_dst } => {
                let fv = self.rd_file(f, b);
                let wr = self.wreg(b);
                let r = self.alu(op, fv, wr, b);
                // the first operand is f, the second WREG: SUB f - WREG, SUBR WREG - f
                if w_dst {
                    self.wreg_write(b, r);
                } else {
                    self.wr_file(f, b, r);
                }
            }
            AluLit { op, b, k, wn } => {
                let a = if b { self.w(wn) & 0xFF } else { self.w(wn) };
                let r = self.alu(op, a, k, b);
                self.wd(wn, r, b);
            }
            Un { op, b, s, d: dst } => {
                let v = self.read_ea(s, b);
                let r = self.unary(op, v, b);
                self.write_ea(dst, b, r);
            }
            UnF { op, b, f, w_dst } => {
                let v = self.rd_file(f, b);
                let r = self.unary(op, v, b);
                if w_dst {
                    self.wreg_write(b, r);
                } else {
                    self.wr_file(f, b, r);
                }
            }
            Shift { op, wb, src, wd } => {
                let v = u32::from(self.w(wb));
                let n = match src {
                    ShSrc::Lit(k) => u32::from(k),
                    ShSrc::W(r) => u32::from(self.w(r)) & 0x1F,
                };
                let n = n.min(16);
                let r: u32 = match op {
                    UnOp::Sl => (v << n) & 0xFFFF,
                    UnOp::Lsr => v >> n,
                    _ => (((v as u16 as i16) as i32) >> n.min(15)) as u32 & 0xFFFF,
                };
                // a shift by 16 or more clears (logical) or fills with the sign (arithmetic)
                let r = match (op, n) {
                    (UnOp::Sl, 16) | (UnOp::Lsr, 16) => 0,
                    (UnOp::Asr, 16) => if v & 0x8000 != 0 { 0xFFFF } else { 0 },
                    _ => r,
                };
                self.set_w(wd, r as u16);
                self.nz(r, false);
            }
            ClrW { set, b, d: dst } => {
                let v = if set { 0xFFFF } else { 0 };
                self.write_ea(dst, b, v);
            }
            ClrF { set, b, f, w_dst } => {
                let v = if set { 0xFFFF } else { 0 };
                if w_dst {
                    self.wreg_write(b, v);
                } else {
                    self.wr_file(f, b, v);
                }
            }
            Cp { borrow, b, wb, s } => {
                let a = u32::from(if b { self.w(wb) & 0xFF } else { self.w(wb) });
                let sv = u32::from(match s {
                    Src::Ws(e) => self.read_ea(e, b),
                    Src::Lit(k) => k,
                });
                let c = u32::from(self.flag(C));
                if borrow {
                    self.sub_flags(a, sv, c, b, true);
                } else {
                    self.sub_flags(a, sv, 1, b, false);
                }
            }
            CpF { borrow, b, f } => {
                let a = u32::from(self.rd_file(f, b));
                let wv = u32::from(self.wreg(b));
                let c = u32::from(self.flag(C));
                if borrow {
                    self.sub_flags(a, wv, c, b, true);
                } else {
                    self.sub_flags(a, wv, 1, b, false);
                }
            }
            Cp0 { b, s } => {
                let v = u32::from(self.read_ea(s, b));
                self.sub_flags(v, 0, 1, b, false);
            }
            Cp0F { b, f } => {
                let v = u32::from(self.rd_file(f, b));
                self.sub_flags(v, 0, 1, b, false);
            }
            Cps { cmp, b, wb, wn } => {
                let (x, y) = (if b { self.w(wb) & 0xFF } else { self.w(wb) }, if b { self.w(wn) & 0xFF } else { self.w(wn) });
                self.sub_flags(u32::from(x), u32::from(y), 1, b, false);
                // flags are updated like CP; the skip tests Wb against Wn
                let take = self.cmp_cond(cmp, x, y, b);
                if take {
                    self.skip(pc, next, cyc);
                }
            }
            Cpb { cmp, b, wb, wn, off } => {
                let (x, y) = (if b { self.w(wb) & 0xFF } else { self.w(wb) }, if b { self.w(wn) & 0xFF } else { self.w(wn) });
                self.sub_flags(u32::from(x), u32::from(y), 1, b, false);
                if self.cmp_cond(cmp, x, y, b) {
                    jump(self, (np as i64 + 2 * i64::from(off)) as u32, 5, next, cyc);
                }
            }
            Bra { cc, off } => {
                if self.cond(cc) {
                    jump(self, (np as i64 + 2 * i64::from(off)) as u32, 4, next, cyc);
                }
            }
            BraW { wn } => {
                let to = (np as i64 + 2 * i64::from(self.w(wn) as i16)) as u32;
                jump(self, to, 4, next, cyc);
            }
            Goto { addr } => jump(self, addr, 4, next, cyc),
            GotoW { wn } => jump(self, u32::from(self.w(wn) & !1), 4, next, cyc),
            Call { addr } => {
                self.push_pc(pc + 4);
                jump(self, addr, 4, next, cyc);
            }
            CallW { wn } => {
                self.push_pc(np);
                jump(self, u32::from(self.w(wn) & !1), 4, next, cyc);
            }
            Rcall { off } => {
                self.push_pc(np);
                jump(self, (np as i64 + 2 * i64::from(off)) as u32, 4, next, cyc);
            }
            RcallW { wn } => {
                self.push_pc(np);
                let to = (np as i64 + 2 * i64::from(self.w(wn) as i16)) as u32;
                jump(self, to, 4, next, cyc);
            }
            Return => {
                let to = self.pop_pc();
                let c = if self.exception_pending() { 5 } else { 6 };
                jump(self, to, c, next, cyc);
            }
            Retlw { b, k, wn } => {
                let to = self.pop_pc();
                self.wd(wn, k, b);
                let c = if self.exception_pending() { 5 } else { 6 };
                jump(self, to, c, next, cyc);
            }
            Retfie => {
                let hi = self.pop16();
                let lo = self.pop16();
                self.call_depth -= 1;
                // SRL and IPL3 come back from the stack, RA among them
                self.sr = (self.sr & 0xFF00) | ((hi >> 8) & !RA);
                self.ra = (hi >> 8) & RA != 0;
                self.corcon = (self.corcon & !CC_IPL3) | if hi & 0x80 != 0 { CC_IPL3 } else { 0 };
                self.sfa = lo & 1 != 0;
                let to = (u32::from(hi & 0x7F) << 16) | u32::from(lo & 0xFFFE);
                let c = if self.exception_pending() { 5 } else { 6 };
                jump(self, to, c, next, cyc);
                self.context_return();
            }
            Repeat { k } => self.do_repeat(k, k),
            RepeatW { wn } => {
                let k = self.w(wn);
                self.do_repeat(k, k);
            }
            Do { k, off } => {
                self.do_start(k & 0x7FFF, pc, off);
                *cyc = 2;
            }
            DoW { wn, off } => {
                let k = self.w(wn) & 0x3FFF;
                self.do_start(k, pc, off);
                *cyc = 2;
            }
            Lnk { k } => {
                let w14 = self.w[14];
                self.push16(w14);
                self.w[14] = self.w[15];
                self.w[15] = self.w[15].wrapping_add(k);
                self.sfa = true;
            }
            Ulnk => {
                self.w[15] = self.w[14];
                self.w[14] = self.pop16();
                self.sfa = false;
            }
            Disi { k } => {
                self.disicnt = k;
                self.mem[0x8C1] |= 0x40;
            }
            Pwrsav { k } => {
                self.sleeping = Some(k);
                self.on_pwrsav(k);
            }
            Clrwdt => self.p.clrwdt(),
            Reset => {
                self.event = Event::Reset;
            }
            PushS => {
                self.shadow = Some(([self.w[0], self.w[1], self.w[2], self.w[3]], self.sr & (C | Z | OV | N | DC)));
            }
            PopS => {
                if let Some((w, f)) = self.shadow {
                    self.w[0] = w[0];
                    self.w[1] = w[1];
                    self.w[2] = w[2];
                    self.w[3] = w[3];
                    self.sr = (self.sr & !(C | Z | OV | N | DC)) | f;
                }
            }
            Bootswp => {}
            CtxtswpLit { k } => self.ctxtswp(k as usize),
            CtxtswpW { wn } => {
                let k = (self.w(wn) & 7) as usize;
                self.ctxtswp(k);
            }
            MovpagLit { p, k } => self.movpag(p, k),
            MovpagW { p, wn } => {
                let v = self.w(wn);
                self.movpag(p, v);
            }
            MovFW { b, f, w_dst } => {
                let v = self.rd_file(f, b);
                self.nz(u32::from(v), b);
                if w_dst {
                    self.wreg_write(b, v);
                } else {
                    self.wr_file(f, b, v);
                }
            }
            MovWF { b, f } => {
                let v = self.wreg(b);
                self.wr_file(f, b, v);
            }
            MovFR { f, wn } => {
                let v = self.rd16(f);
                self.set_w(wn, v);
            }
            MovRF { f, wn } => {
                let v = self.w(wn);
                self.store(f, false, v);
            }
            MovLit8 { k, wn } => self.wd(wn, u16::from(k), true),
            MovLit16 { k, wn } => self.set_w(wn, k),
            MovOffR { b, off, ws, wn } => {
                let a = self.w(ws).wrapping_add(off as u16);
                let v = if b { u16::from(self.rd8(a)) } else { self.rd16(a) };
                self.wd(wn, v, b);
            }
            MovOffW { b, off, wd, wn } => {
                let a = self.w(wd).wrapping_add(off as u16);
                let v = self.w(wn);
                self.store(a, b, v);
            }
            Mov { b, s, d: dst } => {
                let v = self.read_ea(s, b);
                self.write_ea(dst, b, v);
            }
            MovD { s, d: dst } => self.mov_d(s, dst, cyc),
            Mul { su, wide, wb, s, wd } => {
                let a = self.w(wb);
                let sv = match s {
                    Src::Ws(e) => self.read_ea(e, false),
                    Src::Lit(k) => k,
                };
                let p = mul_su(su, a, sv);
                self.set_w(wd, p as u16);
                if wide {
                    self.set_w(wd + 1, (p >> 16) as u16);
                }
            }
            MulAcc { su, wb, s, acc } => {
                let a = self.w(wb);
                let sv = match s {
                    Src::Ws(e) => self.read_ea(e, false),
                    Src::Lit(k) => k,
                };
                // the product is signed or unsigned per the instruction; MDB shifts it left unless CORCON.IF selects integers
                let p = match su {
                    0 => i64::from(a) * i64::from(sv),
                    1 => i64::from(a) * i64::from(sv as i16),
                    2 => i64::from(a as i16) * i64::from(sv),
                    _ => i64::from(a as i16) * i64::from(sv as i16),
                };
                self.acc[acc as usize] = if self.corcon & CC_IF != 0 { p } else { p << 1 };
                self.acc_flags_after(acc, false);
            }
            MulF { b, f } => {
                let v = self.rd_file(f, b);
                if b {
                    let p = (self.w[0] & 0xFF) * (v & 0xFF);
                    self.w[2] = p;
                } else {
                    let p = u32::from(self.w[0]) * u32::from(v);
                    self.w[2] = p as u16;
                    self.w[3] = (p >> 16) as u16;
                }
            }
            Tbl { write, high, b, s, d: dst } => self.tbl(write, high, b, s, dst, cyc),
            Bit { op, f, bit } => self.bit_file(op, f, bit, pc, next, cyc),
            BitW { op, b, s, bit, z } => self.bit_ws(op, b, s, bit, z, pc, next, cyc),
            BtstR { z, s, wb } => {
                let bit = (self.w(wb) & 15) as u8;
                self.bit_ws(BitOp::Btst, false, s, bit, z, pc, next, cyc);
            }
            Bsw { z, s, wb } => {
                let bit = (self.w(wb) & 15) as u8;
                let v = if z { !self.flag(Z) } else { self.flag(C) };
                let (val, addr) = self.read_ea_addr(s, false);
                let nv = if v { val | (1 << bit) } else { val & !(1 << bit) };
                match (addr, s) {
                    (Some(a), _) => self.store(a, false, nv),
                    (None, Ea::Dir(r)) => self.set_w(r, nv),
                    _ => {}
                }
            }
            Div { signed, double, wm, wn, two } => self.divide(signed, double, wm, wn, two),
            Divf { wm, wn, two } => self.divf(wm, wn, two),
            Ff1 { left, s, wd } => {
                let v = self.read_ea(s, false);
                let r = if v == 0 {
                    0
                } else if left {
                    v.leading_zeros() as u16 + 1
                } else {
                    v.trailing_zeros() as u16 + 1
                };
                self.set_w(wd, r);
                self.set_flag_c(v == 0);
            }
            Fbcl { s, wd } => {
                let v = self.read_ea(s, false);
                let sign = v & 0x8000;
                let mut shift = 0u16;
                let mut t = v << 1;
                let max = 15;
                while shift < max && t & 0x8000 == sign {
                    t <<= 1;
                    shift += 1;
                }
                let r = (0u16).wrapping_sub(if shift == max && v == 0 || v == 0xFFFF { 15 } else { shift });
                self.set_w(wd, r);
                self.set_flag_c(shift == max);
            }
            Se { s, wd } => {
                let v = self.read_ea(s, true);
                let r = crate::dspic::sext8(v);
                self.set_w(wd, r);
                self.nz(u32::from(r), false);
                self.set_flag_c(r != 0 && false || v & 0x80 == 0);
            }
            Ze { s, wd } => {
                let v = self.read_ea(s, true);
                self.set_w(wd, v & 0xFF);
                self.nz(u32::from(v & 0xFF), false);
                self.set_flag_c(true);
            }
            Swap { b, wn } => {
                let v = self.w(wn);
                if b {
                    self.set_w(wn, (v & 0xFF00) | ((v & 0x0F) << 4) | ((v >> 4) & 0x0F));
                } else {
                    self.set_w(wn, v.rotate_left(8));
                }
            }
            Exch { s, d: dst } => {
                let (a, b) = (self.w(s), self.w(dst));
                self.set_w(s, b);
                self.set_w(dst, a);
            }
            Daw { wn } => {
                let v = self.w(wn) as u8;
                let mut r = v;
                let mut c = self.flag(C);
                if (v & 0x0F) > 9 || self.flag(DC) {
                    r = r.wrapping_add(6);
                }
                if (r >> 4) > 9 || c {
                    r = r.wrapping_add(0x60);
                    c = true;
                }
                self.wd(wn, u16::from(r), true);
                self.set_flag_c(c);
            }
            Push { f } => {
                let v = self.rd16(f);
                self.push16(v);
            }
            Pop { f } => {
                let v = self.pop16();
                self.store(f, false, v);
            }
            Flim { v, wb, s, wd } => self.flim(v, wb, s, wd),
            Bfext { bit, wid, src, wd } => {
                let val = match src {
                    BfSrc::Ws(e) => self.read_ea(e, false),
                    BfSrc::F(f) => self.rd16(f),
                };
                let mask = if wid >= 16 { 0xFFFF } else { (1u16 << wid) - 1 };
                let r = (val >> bit) & mask;
                self.set_w(wd, r);
                self.nz(u32::from(r), false);
            }
            Bfins { bit, wid, src, dst } => {
                let sv = match src {
                    BfIns::Wns(n) => self.w(n),
                    BfIns::Lit(k) => u16::from(k),
                };
                let mask = if wid >= 16 { 0xFFFF } else { (1u16 << wid) - 1 };
                let field = (sv & mask) << bit;
                let m = mask << bit;
                match dst {
                    BfDst::F(f) => {
                        let o = self.rd16(f);
                        self.store(f, false, (o & !m) | field);
                    }
                    BfDst::Ws(e) => {
                        let (o, a) = self.read_ea_addr(e, false);
                        let nv = (o & !m) | field;
                        match (a, e) {
                            (Some(a), _) => self.store(a, false, nv),
                            (None, Ea::Dir(r)) => self.set_w(r, nv),
                            _ => {}
                        }
                    }
                }
            }
            AddAcc { .. } | SubAcc { .. } | NegAcc { .. } | AddWs { .. } | Lac { .. } | LacD { .. } | Sac { .. } | SacD { .. } | SftacLit { .. } | SftacW { .. } | Max { .. } | Min { .. } | Minz { .. } | Norm { .. } | Mac { .. } => {
                self.exec_dsp(&d.inst);
            }
        }
        // cycles of an instruction that did not jump: 1, plus what the operands added
        if !*jumped {
            *cyc = (*cyc).max(1);
        }
        if d.words == 2 && !*jumped && matches!(d.inst, Inst::Bfext { .. } | Inst::Bfins { .. }) {
            *cyc = 2;
        }
    }

    fn set_flag_c(&mut self, v: bool) {
        if v {
            self.sr |= C
        } else {
            self.sr &= !C
        }
    }

    fn set_flag_z(&mut self, v: bool) {
        if v {
            self.sr |= Z
        } else {
            self.sr &= !Z
        }
    }

    fn cmp_cond(&self, cmp: Cmp, x: u16, y: u16, byte: bool) -> bool {
        // CPSEQ/CPBEQ: Wb = Wn; GT, LT: signed; NE
        let (sx, sy) = if byte { (i32::from(x as u8 as i8), i32::from(y as u8 as i8)) } else { (i32::from(x as i16), i32::from(y as i16)) };
        match cmp {
            Cmp::Eq => x == y,
            Cmp::Ne => x != y,
            Cmp::Gt => sx > sy,
            Cmp::Lt => sx < sy,
        }
    }

    fn do_repeat(&mut self, count: u16, _k: u16) {
        self.rcount = count & 0x7FFF;
        self.repeat_iter = 0;
        self.ra = count != 0;
        // the instruction after REPEAT runs rcount + 1 times; with rcount 0 it runs once
    }

    fn do_start(&mut self, k: u16, pc: u32, off: i32) {
        if self.dl_level >= 4 {
            self.mem[0x8C4 + 1] |= 0; // soft trap: DOOVR (INTCON3 bit 4)
            self.mem[0x8C4] |= 0x10;
            self.trap_pending = Some(Trap::Soft);
            return;
        }
        if self.dl_level > 0 {
            self.do_stack.push((self.dostart, self.doend, self.dcount));
        }
        self.dostart = pc + 4;
        self.doend = (pc as i64 + 4 + 2 * i64::from(off)) as u32 & 0x7F_FFFE;
        self.dcount = k;
        self.dl_level += 1;
    }

    fn movpag(&mut self, p: u8, k: u16) {
        match p {
            0 => self.dsrpag = k & 0x3FF,
            1 => self.dswpag = k & 0x1FF,
            _ => self.tblpag = k & 0xFF,
        }
    }

    fn bit_file(&mut self, op: BitOp, f: u16, bit: u8, pc: u32, next: &mut u32, cyc: &mut u32) {
        let v = self.rd8(f);
        self.bit_common(op, u16::from(v), bit, pc, next, cyc, |s, nv| s.store(f, true, nv));
    }

    #[allow(clippy::too_many_arguments)]
    fn bit_ws(&mut self, op: BitOp, b: bool, s: Ea, bit: u8, z: bool, pc: u32, next: &mut u32, cyc: &mut u32) {
        let (v, addr) = self.read_ea_addr(s, b);
        let direct = if let Ea::Dir(r) = s { Some(r) } else { None };
        let val = u32::from(v);
        let on = val >> bit & 1 != 0;
        match op {
            BitOp::Btst | BitOp::Btsts => {
                if z {
                    self.set_flag_z(!on);
                } else {
                    self.set_flag_c(on);
                }
                if op == BitOp::Btsts {
                    let nv = (val | (1 << bit)) as u16;
                    if let Some(a) = addr {
                        self.store(a, b, nv);
                    } else if let Some(r) = direct {
                        self.wd(r, nv, b);
                    }
                }
            }
            BitOp::Btsc | BitOp::Btss => {
                let skip = if op == BitOp::Btsc { !on } else { on };
                if skip {
                    self.skip(pc, next, cyc);
                }
            }
            _ => {
                let nv = match op {
                    BitOp::Bset => val | (1 << bit),
                    BitOp::Bclr => val & !(1 << bit),
                    _ => val ^ (1 << bit),
                } as u16;
                if let Some(a) = addr {
                    self.store(a, b, nv);
                } else if let Some(r) = direct {
                    self.wd(r, nv, b);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn bit_common(&mut self, op: BitOp, v: u16, bit: u8, pc: u32, next: &mut u32, cyc: &mut u32, mut write: impl FnMut(&mut Self, u16)) {
        let val = u32::from(v);
        let on = val >> bit & 1 != 0;
        match op {
            BitOp::Btst => self.set_flag_z(!on),
            BitOp::Btsts => {
                self.set_flag_z(!on);
                write(self, (val | (1 << bit)) as u16);
            }
            BitOp::Btsc | BitOp::Btss => {
                let skip = if op == BitOp::Btsc { !on } else { on };
                if skip {
                    self.skip(pc, next, cyc);
                }
            }
            BitOp::Bset => write(self, (val | (1 << bit)) as u16),
            BitOp::Bclr => write(self, (val & !(1 << bit)) as u16),
            BitOp::Btg => write(self, (val ^ (1 << bit)) as u16),
        }
    }

    fn mov_d(&mut self, s: Ea, dst: Ea, cyc: &mut u32) {
        *cyc = 2;
        let (lo, hi) = match s {
            Ea::Dir(n) => (self.w(n), self.w(n + 1)),
            _ => {
                let a = self.ea(s, 4, false).unwrap_or(0);
                (self.rd16(a), self.rd16(a.wrapping_add(2)))
            }
        };
        match dst {
            Ea::Dir(n) => {
                self.set_w(n, lo);
                self.set_w(n + 1, hi);
            }
            _ => {
                let a = self.ea(dst, 4, true).unwrap_or(0);
                self.store(a, false, lo);
                self.store(a.wrapping_add(2), false, hi);
            }
        }
    }

    fn tbl(&mut self, write: bool, high: bool, b: bool, s: Ea, dst: Ea, cyc: &mut u32) {
        if write {
            *cyc = 2;
            let v = self.read_ea(s, b);
            let a = self.ea(dst, if b { 1 } else { 2 }, true).unwrap_or(0);
            let addr = (u32::from(self.tblpag) << 16) | u32::from(a);
            self.table_write(addr, high, b, v);
        } else {
            *cyc = 5;
            let a = self.ea(s, if b { 1 } else { 2 }, false).unwrap_or(0);
            let addr = (u32::from(self.tblpag) << 16) | u32::from(a);
            let w = self.table_word(addr, high);
            let v = if b {
                // byte reads pick a byte of the word by address bit 0
                if addr & 1 == 0 { w & 0xFF } else { w >> 8 }
            } else {
                w
            };
            let v = if high && b && addr & 1 != 0 { 0 } else { v };
            self.write_ea(dst, b, v);
        }
    }

    fn divide(&mut self, signed: bool, double: bool, wm: u8, wn: u8, two: bool) {
        if self.repeat_iter > 0 && self.ra {
            // the divide is done in the first pass; the remaining passes of the REPEAT do nothing
            return;
        }
        let divisor = self.w(wn);
        if divisor == 0 {
            self.math_error(0x40);
            return;
        }
        let lo = self.w(wm);
        let hi = if double { self.w(wm + 1) } else if signed && lo & 0x8000 != 0 { 0xFFFF } else { 0 };
        let dividend32 = (u32::from(hi) << 16) | u32::from(lo);
        let (q, r, ov): (u16, u16, bool);
        if signed {
            let n = dividend32 as i32;
            let d = i32::from(divisor as i16);
            let qq = n.wrapping_div(d);
            let rr = n.wrapping_rem(d);
            ov = !(-32768..=32767).contains(&qq) || (n == i32::MIN && d == -1);
            q = qq as u16;
            r = rr as u16;
        } else {
            let n = dividend32;
            let d = u32::from(divisor);
            let qq = n / d;
            let rr = n % d;
            ov = qq > 0xFFFF;
            q = qq as u16;
            r = rr as u16;
        }
        if two {
            // DIV2: the dividend registers are preserved; quotient to W0, remainder to W1 as DIV does
            self.w[0] = q;
            self.w[1] = r;
        } else {
            self.w[0] = q;
            self.w[1] = r;
        }
        self.set_flag(Z, r == 0);
        self.set_flag(N, if signed { r & 0x8000 != 0 } else { false });
        self.set_flag(OV, ov);
        self.set_flag(C, false);
    }

    fn divf(&mut self, wm: u8, wn: u8, two: bool) {
        if self.repeat_iter > 0 && self.ra {
            return;
        }
        let divisor = self.w(wn) as i16;
        if divisor == 0 {
            self.math_error(0x40);
            return;
        }
        // fractional divide: (Wm << 16) / Wn, W0 = quotient, W1 = remainder
        let dividend = i64::from(self.w(wm) as i16) << 16;
        let q = dividend / i64::from(divisor);
        let r = dividend % i64::from(divisor);
        let _ = two;
        let ov = !(-32768..=32767).contains(&q);
        self.w[0] = q as u16;
        self.w[1] = (r >> 0) as u16;
        self.set_flag(Z, self.w[1] == 0);
        self.set_flag(N, self.w[1] & 0x8000 != 0);
        self.set_flag(OV, ov);
        self.set_flag(C, false);
    }

    fn flim(&mut self, v: bool, wb: u8, s: Ea, wd: u8) {
        // FLIM Wb,Ws: limit Wb to the range -Ws..Ws (signed); with .V the excess goes to Wnd
        let limit = self.read_ea(s, false) as i16;
        let x = self.w(wb) as i16;
        let mut res = x;
        let mut excess: i16 = 0;
        let lim = i32::from(limit);
        let xv = i32::from(x);
        if xv > lim {
            res = limit;
            excess = (xv - lim) as i16;
        } else if xv < -lim {
            res = limit.wrapping_neg();
            excess = (xv + lim) as i16;
        }
        self.set_w(wb, res as u16);
        if v {
            self.set_w(wd, excess as u16);
        }
        self.set_flag(N, res < 0);
        self.set_flag(Z, res == 0);
        self.set_flag(OV, false);
        self.set_flag(C, false);
    }

    fn ctxtswp(&mut self, k: usize) {
        if k > 4 || k == self.ctx {
            return;
        }
        let w15 = self.w[15];
        let cur = self.w;
        self.alt[self.ctx] = cur;
        self.w = self.alt[k];
        self.w[15] = w15; // W15 is not part of an alternate array
        self.ctx = k;
        self.mem[reg::CTXTSTAT] = (self.mem[reg::CTXTSTAT] & 0xF8) | k as u8;
        self.mem[reg::CTXTSTAT + 1] = (self.mem[reg::CTXTSTAT + 1] & 0xF8) | 0;
    }

    pub(crate) fn context_return(&mut self) {}

    fn exception_pending(&self) -> bool {
        self.trap_pending.is_some() || self.irq_ready().is_some()
    }
}

fn mul_su(su: u8, a: u16, b: u16) -> u32 {
    match su {
        0 => u32::from(a) * u32::from(b),
        1 => (i64::from(b as i16) * i64::from(a)) as u32,
        2 => (i64::from(a as i16) * i64::from(b)) as u32,
        _ => (i32::from(a as i16) * i32::from(b as i16)) as u32,
    }
}

/// The destination address of a pre-modified operand, with the pointer register updated now.
fn dst_addr_first(c: &mut Dspic, e: Ea, byte: bool) -> Option<u16> {
    c.ea(e, if byte { 1 } else { 2 }, true)
}
