//! The DSP engine of the dsPIC33 core: the MAC class (MAC, MSC, MPY, MPY.N, ED, EDAC, MOVSAC, CLR) with its X and Y
//! prefetches and accumulator write-back, and the accumulator instructions (ADD, SUB, NEG, LAC, SAC, SFTAC, MAX, MIN,
//! MINZ, NORM). Behaviour is that of the Enhanced CPU family reference manual (DS70005158C, section 7) and the
//! instruction descriptions of DS70000157G, checked against MDB.

use crate::dspic::*;
use crate::dspic_isa::*;

fn sext40(v: i64) -> i64 {
    (v << 24) >> 24
}

impl Dspic {
    fn accn(&self, a: Acc) -> i64 {
        self.acc[a as usize]
    }

    /// Stores an exact accumulator result with the overflow and saturation rules of CORCON (section 7.3.2).
    fn acc_store(&mut self, a: Acc, v: i64) {
        let i = a as usize;
        let (sat_bit, o_flag, s_flag) = if a { (CC_SATB, OB, SB) } else { (CC_SATA, OA, SA) };
        let v40 = sext40(v);
        let wide = v != v40; // beyond 40 bits: the sign is destroyed
        let over32 = v40 < -(1i64 << 31) || v40 > (1i64 << 31) - 1;
        if self.corcon & sat_bit != 0 {
            if self.corcon & CC_ACCSAT != 0 {
                // 9.31 saturation: only a 40-bit overflow saturates; guard-bit use is an overflow (OA)
                if wide {
                    self.acc[i] = if v < 0 { -(1i64 << 39) } else { (1i64 << 39) - 1 };
                    self.sr |= s_flag | o_flag;
                } else {
                    self.acc[i] = v40;
                    if over32 {
                        self.sr |= o_flag;
                    }
                }
            } else if over32 || wide {
                // 1.31 saturation
                self.acc[i] = if v < 0 || (wide && v40 < 0 && v < 0) { -(1i64 << 31) } else { (1i64 << 31) - 1 };
                if v > 0 {
                    self.acc[i] = (1i64 << 31) - 1;
                }
                self.sr |= s_flag;
            } else {
                self.acc[i] = v40;
            }
        } else {
            self.acc[i] = v40;
            if over32 {
                self.sr |= o_flag;
            }
            if wide {
                self.sr |= s_flag;
            }
        }
        self.sr_acc_flags();
        self.acc_traps(a, over32, wide);
    }

    fn sr_acc_flags(&mut self) {
        // OAB / SAB are derived when SR is read
    }

    fn acc_traps(&mut self, a: Acc, over32: bool, wide: bool) {
        let intcon1 = u16::from(self.mem[0x8C0]) | u16::from(self.mem[0x8C1]) << 8;
        let (ovte, ovflag, covflag) = if a { (0x0200u16, 0x2000u16, 0x0800u16) } else { (0x0400u16, 0x4000u16, 0x1000u16) };
        let mut trap = false;
        let mut v = intcon1;
        let sat = self.corcon & if a { CC_SATB } else { CC_SATA } != 0;
        if over32 && intcon1 & ovte != 0 && !(sat && self.corcon & CC_ACCSAT == 0) {
            v |= ovflag;
            trap = true;
        }
        if wide && !sat && intcon1 & 0x0100 != 0 {
            v |= covflag;
            trap = true;
        }
        if trap {
            v |= 0x10; // MATHERR
            self.mem[0x8C0] = v as u8;
            self.mem[0x8C1] = (v >> 8) as u8;
            self.trap_pending = Some(Trap::Math);
        }
    }

    /// Product of two multiplicands as the DSP multiplier sees them (`CORCON.US`, `CORCON.IF`), as a 40-bit value.
    fn mac_product(&self, m: (u8, u8)) -> i64 {
        let (x, y) = (self.w[m.0 as usize], self.w[m.1 as usize]);
        let us = (self.corcon >> 12) & 3;
        let signed = |reg: u8, v: u16| -> i64 {
            match us {
                1 => i64::from(v),
                2 => {
                    if reg & 1 != 0 {
                        i64::from(v as i16)
                    } else {
                        i64::from(v)
                    }
                }
                _ => i64::from(v as i16),
            }
        };
        let p = signed(m.0, x) * signed(m.1, y);
        if self.corcon & CC_IF != 0 { p } else { p << 1 }
    }

    /// One prefetch: read the word at the effective address into `dst` (the registers are written by the caller after
    /// the multiply has used them), update the pointer.
    fn prefetch(&mut self, p: Pf) -> (u8, u16) {
        let a = match p.inc {
            None => self.w[p.wn as usize].wrapping_add(self.w[12]),
            Some(inc) => {
                let a = self.w[p.wn as usize];
                if inc != 0 {
                    let n = self.next_addr(p.wn, a, i32::from(inc), 2, false);
                    self.set_w(p.wn, n);
                }
                a
            }
        };
        (p.dst, self.rd16(a))
    }

    fn mac_prefetch(&mut self, x: Option<Pf>, y: Option<Pf>) {
        let mut loads = Vec::new();
        if let Some(p) = x {
            loads.push(self.prefetch(p));
        }
        if let Some(p) = y {
            loads.push(self.prefetch(p));
        }
        for (d, v) in loads {
            self.w[d as usize] = v;
        }
    }

    /// The accumulator written back to W13: rounded, saturated to 16 bits (a 1.15 value).
    fn awb_value(&self, a: Acc) -> u16 {
        self.sac_value(a, 0, true)
    }

    fn mac_awb(&mut self, acc: Acc, awb: Awb) {
        match awb {
            Awb::None => {}
            Awb::W13 => {
                let v = self.awb_value(!acc);
                self.w[13] = v;
            }
            Awb::W13Inc => {
                let v = self.awb_value(!acc);
                let a = self.w[13];
                self.store(a, false, v);
                self.w[13] = a.wrapping_add(2);
            }
        }
    }

    /// The 16-bit value `SAC` stores: the accumulator shifted by `sh` (positive right), rounded when asked, limited to
    /// 16 bits when `CORCON.SATDW` is set.
    pub(crate) fn sac_value(&self, a: Acc, sh: i8, round: bool) -> u16 {
        let mut v = self.accn(a);
        v = if sh >= 0 { v >> sh } else { sext40(v << (-sh)) };
        if round {
            let lsw = v & 0xFFFF;
            let bump = if self.corcon & CC_RND != 0 {
                // conventional (biased) rounding
                lsw >= 0x8000
            } else {
                lsw > 0x8000 || (lsw == 0x8000 && (v >> 16) & 1 == 1)
            };
            if bump {
                v = sext40(v + 0x1_0000);
            }
        }
        let hi = v >> 16; // 24 bits with the guard bits
        if self.corcon & CC_SATDW != 0 {
            hi.clamp(-32768, 32767) as u16
        } else {
            hi as u16
        }
    }

    pub(crate) fn exec_dsp(&mut self, inst: &Inst) {
        use Inst::*;
        match *inst {
            Mac { op, acc, m, x, y, awb } => {
                match op {
                    MacOp::Mac | MacOp::Msc | MacOp::Mpy | MacOp::MpyN => {
                        let p = self.mac_product(m);
                        let cur = self.accn(acc);
                        let r = match op {
                            MacOp::Mac => cur + p,
                            MacOp::Msc => cur - p,
                            MacOp::Mpy => p,
                            _ => -p,
                        };
                        self.mac_prefetch(x, y);
                        self.mac_awb(acc, awb);
                        self.acc_store(acc, r);
                    }
                    MacOp::Ed | MacOp::Edac => {
                        // the prefetched words go to no register but their difference goes to Wxd; the product is
                        // the square of the difference of the two prefetched words
                        let (xp, yp) = (x.unwrap(), y.unwrap());
                        let xa = self.prefetch_addr(xp);
                        let ya = self.prefetch_addr(yp);
                        let xv = self.rd16(xa);
                        let yv = self.rd16(ya);
                        let diff = xv.wrapping_sub(yv);
                        let d = i64::from(diff as i16);
                        let p = if self.corcon & CC_IF != 0 { d * d } else { (d * d) << 1 };
                        let cur = self.accn(acc);
                        let r = if op == MacOp::Edac { cur + p } else { p };
                        self.w[xp.dst as usize] = diff;
                        self.acc_store(acc, r);
                    }
                    MacOp::Movsac | MacOp::Clr => {
                        self.mac_prefetch(x, y);
                        self.mac_awb(acc, awb);
                        if op == MacOp::Clr {
                            self.acc[acc as usize] = 0;
                            // OA/SA of the cleared accumulator are cleared
                            self.sr &= !(if acc { OB | SB } else { OA | SA });
                        }
                    }
                }
            }
            AddAcc { acc } => {
                let r = self.accn(acc) + self.accn(!acc);
                self.acc_store(acc, r);
            }
            SubAcc { acc } => {
                let r = self.accn(acc) - self.accn(!acc);
                self.acc_store(acc, r);
            }
            NegAcc { acc } => {
                let r = -self.accn(acc);
                self.acc_store(acc, r);
            }
            AddWs { s, sh, acc } => {
                let v = self.read_ea(s, false);
                let ext = i64::from(v as i16) << 16;
                let shifted = if sh >= 0 { ext >> sh } else { ext << (-sh) };
                let r = self.accn(acc) + shifted;
                self.acc_store(acc, r);
            }
            Lac { s, sh, acc } => {
                let v = self.read_ea(s, false);
                let ext = i64::from(v as i16) << 16;
                let shifted = if sh >= 0 { ext >> sh } else { ext << (-sh) };
                self.acc_store(acc, shifted);
            }
            LacD { s, sh, acc } => {
                // a 32-bit value: Ws:Ws+1 (low word first) as the accumulator's lower 32 bits
                let (lo, hi) = match s {
                    Ea::Dir(n) => (self.w[n as usize], self.w[(n as usize + 1) & 15]),
                    _ => {
                        let a = self.ea(s, 4, false).unwrap_or(0);
                        (self.rd16(a), self.rd16(a.wrapping_add(2)))
                    }
                };
                let ext = i64::from(((u32::from(hi) << 16) | u32::from(lo)) as i32);
                let shifted = if sh >= 0 { ext >> sh } else { ext << (-sh) };
                self.acc_store(acc, shifted);
            }
            Sac { round, acc, sh, d } => {
                let v = self.sac_value(acc, sh, round);
                self.write_ea(d, false, v);
            }
            SacD { acc, sh, wd } => {
                // 32 bits of the shifted accumulator: Wnd = low word, Wnd+1 = high word
                let mut v = self.accn(acc);
                v = if sh >= 0 { v >> sh } else { sext40(v << (-sh)) };
                let w32 = if self.corcon & CC_SATDW != 0 { v.clamp(-(1i64 << 31), (1i64 << 31) - 1) } else { v } as u32;
                self.w[wd as usize] = w32 as u16;
                self.w[(wd as usize + 1) & 15] = (w32 >> 16) as u16;
            }
            SftacLit { acc, k } => self.sftac(acc, i32::from(k)),
            SftacW { acc, wn } => {
                let k = self.w[wn as usize] as i16;
                self.sftac(acc, i32::from(k));
            }
            Max { acc, v } => self.max_min(acc, v, 0),
            Min { acc, v } => self.max_min(acc, v, 1),
            Minz { acc, v } => self.max_min(acc, v, 2),
            Norm { acc, d } => self.norm(acc, d),
            _ => {}
        }
    }

    fn prefetch_addr(&mut self, p: Pf) -> u16 {
        match p.inc {
            None => self.w[p.wn as usize].wrapping_add(self.w[12]),
            Some(inc) => {
                let a = self.w[p.wn as usize];
                if inc != 0 {
                    let n = self.next_addr(p.wn, a, i32::from(inc), 2, false);
                    self.set_w(p.wn, n);
                }
                a
            }
        }
    }

    fn sftac(&mut self, acc: Acc, k: i32) {
        if !(-16..=16).contains(&k) {
            // SFTACERR: the shift is not done
            let v = u16::from(self.mem[0x8C0]) | u16::from(self.mem[0x8C1]) << 8 | 0x90;
            self.mem[0x8C0] = v as u8;
            self.mem[0x8C1] = (v >> 8) as u8;
            self.trap_pending = Some(Trap::Math);
            return;
        }
        let a = self.accn(acc);
        let r = if k >= 0 { a >> k } else { a << (-k) };
        self.acc_store(acc, r);
    }

    fn max_min(&mut self, acc: Acc, v: Option<Ea>, kind: u8) {
        if kind == 2 && !self.flag(Z) {
            return; // MINZ executes as a NOP when Z is clear
        }
        let (t, l) = (self.accn(acc), self.accn(!acc));
        let (limited, diff) = match kind {
            0 => (t - l > 0, t - l),
            _ => (t - l < 0, t - l),
        };
        if limited {
            self.acc[acc as usize] = l;
            self.set_nz(false, false, kind != 0);
            let _ = diff;
        } else {
            self.set_nz(true, false, false);
        }
        if let Some(d) = v {
            let w = if limited {
                match kind {
                    0 => 1u16,
                    _ => 0xFFFF,
                }
            } else {
                0
            };
            self.write_ea(d, false, w);
        }
        self.sr &= !OV;
    }

    fn set_nz(&mut self, z: bool, _ov: bool, n: bool) {
        self.sr = (self.sr & !(Z | N)) | if z { Z } else { 0 } | if n { N } else { 0 };
    }

    fn norm(&mut self, acc: Acc, d: Ea) {
        let a = self.accn(acc);
        // number of left shifts that keep the value within the 1.31 range, or right shifts that remove an overflow
        let over = a < -(1i64 << 31) || a > (1i64 << 31) - 1;
        let result: Option<i32>;
        if over {
            // right shifts needed until it fits in 32 bits
            let mut n = 0;
            let mut v = a;
            while (v < -(1i64 << 31) || v > (1i64 << 31) - 1) && n < 8 {
                v >>= 1;
                n += 1;
            }
            self.acc[acc as usize] = v;
            result = Some(n);
        } else if a == 0 || a == -1 {
            result = None;
        } else {
            // left shifts until bit 30 differs from bit 31
            let mut n = 0;
            let mut v = a;
            while n < 31 && ((v >> 30) & 1) == ((v >> 31) & 1) {
                v <<= 1;
                n += 1;
            }
            if n == 0 {
                result = None;
            } else {
                self.acc[acc as usize] = sext40(v);
                result = Some(-(n as i32));
            }
        }
        match result {
            None => {
                self.write_ea(d, false, 0);
                self.set_nz(true, false, false);
            }
            Some(n) => {
                self.write_ea(d, false, n as u16);
                self.set_nz(false, false, n < 0);
            }
        }
        self.sr &= !(if acc { OB } else { OA });
    }

    pub(crate) fn acc_flags_after(&mut self, acc: Acc, _flag: bool) {
        // MUL.xx to an accumulator: the 32-bit product is stored; OA/SA are not changed
        let _ = acc;
    }
}
