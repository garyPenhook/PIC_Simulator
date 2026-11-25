//! dsPIC33 (CK/CH/CD/EP/EV) instruction decoder and disassembler.
//!
//! Encodings are those of the 16-Bit MCU and DSC Programmer's Reference Manual (DS70000157G, section 5) for the
//! dsPIC33C, checked word by word against the MPLAB XC-DSC assembler (`scripts/dspic_encodings.py` writes the
//! table `tests/data/dspic_encodings.txt` that `tests/dspic_isa.rs` decodes). A word that no instruction uses decodes
//! to `None`; the core turns that into the illegal-opcode trap.
//!
//! Program addresses are the values the PC takes: two per instruction word, so the next instruction is `pc + 2`.

/// A data-space operand of a W-register instruction: the `ppp`/`qqq` mode field with its register.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ea {
    /// `Wn`
    Dir(u8),
    /// `[Wn]`
    Ind(u8),
    /// `[Wn--]`
    PostDec(u8),
    /// `[Wn++]`
    PostInc(u8),
    /// `[--Wn]`
    PreDec(u8),
    /// `[++Wn]`
    PreInc(u8),
    /// `[Wn+Wb]`: (Wn, Wb)
    RegOff(u8, u8),
}

impl Ea {
    pub fn from_mode(mode: u32, reg: u32, wb: u32) -> Option<Ea> {
        let r = reg as u8;
        Some(match mode {
            0 => Ea::Dir(r),
            1 => Ea::Ind(r),
            2 => Ea::PostDec(r),
            3 => Ea::PostInc(r),
            4 => Ea::PreDec(r),
            5 => Ea::PreInc(r),
            6 | 7 => Ea::RegOff(r, wb as u8),
            _ => return None,
        })
    }
    pub fn reg(self) -> u8 {
        match self {
            Ea::Dir(r) | Ea::Ind(r) | Ea::PostDec(r) | Ea::PostInc(r) | Ea::PreDec(r) | Ea::PreInc(r) | Ea::RegOff(r, _) => r,
        }
    }
    pub fn is_direct(self) -> bool {
        matches!(self, Ea::Dir(_))
    }
}

/// The second source of the three-operand ALU group: a `Ws` or a literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Src {
    Ws(Ea),
    Lit(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum A3 {
    Add,
    Addc,
    Sub,
    Subb,
    Subr,
    Subbr,
    And,
    Ior,
    Xor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Un {
    Inc,
    Inc2,
    Dec,
    Dec2,
    Com,
    Neg,
    Sl,
    Lsr,
    Asr,
    Rlc,
    Rlnc,
    Rrc,
    Rrnc,
}

/// Condition of a branch (`BRA cond`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cc {
    Ov,
    C,
    Z,
    N,
    Le,
    Lt,
    Leu,
    Always,
    Nov,
    Nc,
    Nz,
    Nn,
    Gt,
    Ge,
    Gtu,
    Oa,
    Ob,
    Sa,
    Sb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Gt,
    Lt,
    Ne,
    Eq,
}

/// An accumulator, A (false) or B (true).
pub type Acc = bool;

/// One MAC-class prefetch: which W register (8..=11), the update and where the data goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pf {
    /// `W8`..`W11`
    pub wn: u8,
    /// byte increment (-6..=6), or `None` for `[W9+W12]` / `[W11+W12]`
    pub inc: Option<i8>,
    /// destination `W4`..`W7`
    pub dst: u8,
}

/// Accumulator write-back of a MAC-class instruction: `W13` or `[W13]+=2`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Awb {
    None,
    W13,
    W13Inc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacOp {
    Mac,
    Msc,
    Mpy,
    MpyN,
    Ed,
    Edac,
    Movsac,
    Clr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitOp {
    Bset,
    Bclr,
    Btg,
    Btsc,
    Btss,
    Btst,
    Btsts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BfSrc {
    Ws(Ea),
    F(u16),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BfIns {
    Wns(u8),
    Lit(u8),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BfDst {
    Ws(Ea),
    F(u16),
}

/// Operand of the three-register shift instructions (`SL Wb,#lit4,Wnd` / `SL Wb,Wns,Wnd`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShSrc {
    Lit(u8),
    W(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inst {
    Nop,
    Nopr,
    Alu3 { op: A3, b: bool, wb: u8, s: Src, d: Ea },
    /// `ADD f{,WREG}`: `w_dst`: the result goes to WREG, else to `f`
    AluF { op: A3, b: bool, f: u16, w_dst: bool },
    /// `ADD #lit10,Wn`
    AluLit { op: A3, b: bool, k: u16, wn: u8 },
    Un { op: Un, b: bool, s: Ea, d: Ea },
    UnF { op: Un, b: bool, f: u16, w_dst: bool },
    /// `SL`/`LSR`/`ASR Wb,#lit4|Wns,Wnd`
    Shift { op: Un, wb: u8, src: ShSrc, wd: u8 },
    /// `CLR{.B} Wd` / `SETM{.B} Wd`
    ClrW { set: bool, b: bool, d: Ea },
    /// `CLR{.B} f` / `SETM{.B} f` (`w_dst`: WREG is the destination)
    ClrF { set: bool, b: bool, f: u16, w_dst: bool },
    /// `CP`, `CPB` with `Wb,Ws` or `Wb,#lit`
    Cp { borrow: bool, b: bool, wb: u8, s: Src },
    CpF { borrow: bool, b: bool, f: u16 },
    Cp0 { b: bool, s: Ea },
    Cp0F { b: bool, f: u16 },
    /// `CPSxx`: compare and skip
    Cps { cmp: Cmp, b: bool, wb: u8, wn: u8 },
    /// `CPBxx`: compare and branch by `off` words
    Cpb { cmp: Cmp, b: bool, wb: u8, wn: u8, off: i8 },
    Bra { cc: Cc, off: i32 },
    BraW { wn: u8 },
    Goto { addr: u32 },
    GotoW { wn: u8 },
    Call { addr: u32 },
    CallW { wn: u8 },
    Rcall { off: i32 },
    RcallW { wn: u8 },
    Return,
    Retfie,
    Retlw { b: bool, k: u16, wn: u8 },
    Repeat { k: u16 },
    RepeatW { wn: u8 },
    Do { k: u16, off: i32 },
    DoW { wn: u8, off: i32 },
    Lnk { k: u16 },
    Ulnk,
    Disi { k: u16 },
    Pwrsav { k: u8 },
    Clrwdt,
    Reset,
    PushS,
    PopS,
    Bootswp,
    CtxtswpLit { k: u8 },
    CtxtswpW { wn: u8 },
    /// `MOVPAG #lit,DSRPAG|DSWPAG|TBLPAG`: `p` is 0, 1, 2
    MovpagLit { p: u8, k: u16 },
    MovpagW { p: u8, wn: u8 },
    /// `MOV{.B} f{,WREG}`
    MovFW { b: bool, f: u16, w_dst: bool },
    /// `MOV{.B} WREG,f`
    MovWF { b: bool, f: u16 },
    /// `MOV f,Wnd`: `f` is a byte address
    MovFR { f: u16, wn: u8 },
    /// `MOV Wns,f`
    MovRF { f: u16, wn: u8 },
    MovLit8 { k: u8, wn: u8 },
    MovLit16 { k: u16, wn: u8 },
    /// `MOV{.B} [Ws+Slit10],Wnd` (`off` in bytes)
    MovOffR { b: bool, off: i16, ws: u8, wn: u8 },
    /// `MOV{.B} Wns,[Wd+Slit10]`
    MovOffW { b: bool, off: i16, wd: u8, wn: u8 },
    /// `MOV{.B} Ws,Wd`
    Mov { b: bool, s: Ea, d: Ea },
    /// `MOV.D`
    MovD { s: Ea, d: Ea },
    /// `MUL.xx`/`MULW.xx`: `su` is 0 UU, 1 US, 2 SU, 3 SS; `wide` is a 32-bit result in Wnd:Wnd+1 (MUL), else 16 bits (MULW)
    Mul { su: u8, wide: bool, wb: u8, s: Src, wd: u8 },
    /// `MUL.xx Wb,Ws,Acc` / `MUL.xx Wb,#lit5,Acc`
    MulAcc { su: u8, wb: u8, s: Src, acc: Acc },
    /// `MUL{.B} f`: unsigned `WREG * f` into W3:W2
    MulF { b: bool, f: u16 },
    Tbl { write: bool, high: bool, b: bool, s: Ea, d: Ea },
    /// `BSET`... of a bit of the byte at byte address `f` (a word form is the byte form of the odd or even byte)
    Bit { op: BitOp, f: u16, bit: u8 },
    /// `BSET`... of a `Ws` bit (`z`: BTST.Z)
    BitW { op: BitOp, b: bool, s: Ea, bit: u8, z: bool },
    /// `BTST.C/Z Ws,Wb`
    BtstR { z: bool, s: Ea, wb: u8 },
    /// `BSW.C/Z Ws,Wb`
    Bsw { z: bool, s: Ea, wb: u8 },
    /// `DIV.S/U{W|D}`; `two` for DIV2
    Div { signed: bool, double: bool, wm: u8, wn: u8, two: bool },
    Divf { wm: u8, wn: u8, two: bool },
    Ff1 { left: bool, s: Ea, wd: u8 },
    Fbcl { s: Ea, wd: u8 },
    Se { s: Ea, wd: u8 },
    Ze { s: Ea, wd: u8 },
    Swap { b: bool, wn: u8 },
    Exch { s: u8, d: u8 },
    Daw { wn: u8 },
    Push { f: u16 },
    Pop { f: u16 },
    /// `FLIM Wb,Ws` / `FLIM.V Wb,Ws,Wnd`
    Flim { v: bool, wb: u8, s: Ea, wd: u8 },
    Bfext { bit: u8, wid: u8, src: BfSrc, wd: u8 },
    Bfins { bit: u8, wid: u8, src: BfIns, dst: BfDst },
    AddAcc { acc: Acc },
    SubAcc { acc: Acc },
    NegAcc { acc: Acc },
    /// `ADD Ws,#Slit4,Acc`
    AddWs { s: Ea, sh: i8, acc: Acc },
    /// `LAC Ws,#Slit4,Acc`
    Lac { s: Ea, sh: i8, acc: Acc },
    LacD { s: Ea, sh: i8, acc: Acc },
    Sac { round: bool, acc: Acc, sh: i8, d: Ea },
    SacD { acc: Acc, sh: i8, wd: u8 },
    SftacLit { acc: Acc, k: i8 },
    SftacW { acc: Acc, wn: u8 },
    Max { acc: Acc, v: Option<Ea> },
    Min { acc: Acc, v: Option<Ea> },
    Minz { acc: Acc, v: Option<Ea> },
    Norm { acc: Acc, d: Ea },
    /// MAC class: `m` is the pair of multiplicand registers
    Mac { op: MacOp, acc: Acc, m: (u8, u8), x: Option<Pf>, y: Option<Pf>, awb: Awb },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub inst: Inst,
    /// instruction words (1 or 2)
    pub words: u8,
}

fn sext(v: u32, bits: u32) -> i32 {
    let s = 32 - bits;
    ((v << s) as i32) >> s
}

/// `Ws` at `[6:4]`/`[3:0]` (the `ppp ssss` pair).
fn src_ea(w: u32) -> Option<Ea> {
    Ea::from_mode((w >> 4) & 7, w & 15, (w >> 15) & 15).filter(|e| !matches!(e, Ea::RegOff(..)))
}
/// `Wd` at `[13:11]`/`[10:7]` (the `qqq dddd` pair).
fn dst_ea(w: u32) -> Option<Ea> {
    Ea::from_mode((w >> 11) & 7, (w >> 7) & 15, (w >> 15) & 15).filter(|e| !matches!(e, Ea::RegOff(..)))
}

fn pf(i: u32, dst: u32, y: bool) -> Option<Pf> {
    let wn = if i & 8 != 0 { 9 } else { 8 } + if y { 2 } else { 0 };
    let inc = match i & 7 {
        0 => Some(0),
        1 => Some(2),
        2 => Some(4),
        3 => Some(6),
        4 => return if i & 8 != 0 { Some(Pf { wn, inc: None, dst: dst as u8 }) } else { None },
        5 => Some(-6),
        6 => Some(-4),
        _ => Some(-2),
    };
    Some(Pf { wn, inc, dst: dst as u8 })
}

fn alu3_op(top5: u32) -> Option<A3> {
    Some(match top5 {
        0b01000 => A3::Add,
        0b01001 => A3::Addc,
        0b01010 => A3::Sub,
        0b01011 => A3::Subb,
        0b00010 => A3::Subr,
        0b00011 => A3::Subbr,
        0b01100 => A3::And,
        0b01101 => A3::Xor,
        0b01110 => A3::Ior,
        _ => return None,
    })
}

/// The three-operand group `op{.B} Wb,Ws|#lit5,Wd`.
fn alu3(w: u32, op: A3) -> Option<Decoded> {
    let b = (w >> 14) & 1 != 0;
    let wb = ((w >> 15) & 15) as u8;
    let d = dst_ea(w)?;
    let s = if (w >> 5) & 3 == 3 { Src::Lit((w & 31) as u16) } else { Src::Ws(src_ea(w)?) };
    Some(Decoded { inst: Inst::Alu3 { op, b, wb, s, d }, words: 1 })
}

fn decode_bit(w: u32) -> Option<Decoded> {
    let one = |inst| Some(Decoded { inst, words: 1 });
    let top = w >> 16;
    let bbb = ((w >> 13) & 7) as u8;
    if top & 8 != 0 && top != 0xAD {
        // file-register forms `1010 1ooo bbbf ffff ffff fffb`: the byte address is f:b, the bit number bbb (0..7). A word
        // form `BSET f,#9` is the byte form of the odd byte, bit 1.
        let op = match top & 7 {
            0 => BitOp::Bset,
            1 => BitOp::Bclr,
            2 => BitOp::Btg,
            3 => BitOp::Btst,
            4 => BitOp::Btsts,
            6 => BitOp::Btss,
            7 => BitOp::Btsc,
            _ => return None,
        };
        return one(Inst::Bit { op, f: ((((w >> 1) & 0xFFF) << 1) | (w & 1)) as u16, bit: bbb });
    }
    let reg = w & 15;
    let mode = (w >> 4) & 7;
    if mode > 5 {
        return None;
    }
    let s = Ea::from_mode(mode, reg, 0)?;
    let bit4 = ((w >> 12) & 15) as u8;
    match top {
        0xA0..=0xA2 => {
            // BSET (0), BCLR (1), BTG (2) Ws,#bit4: `1010 0ooo bbbb 0B00 0ppp ssss`
            if w & 0x0B80 != 0 {
                return None;
            }
            let b = (w >> 10) & 1 != 0;
            let op = [BitOp::Bset, BitOp::Bclr, BitOp::Btg][(top & 7) as usize];
            one(Inst::BitW { op, b, s, bit: if b { bit4 & 7 } else { bit4 }, z: false })
        }
        0xA3 | 0xA4 => {
            // BTST (3) / BTSTS (4) Ws,#bit4: `1010 0ooo bbbb Z000 0ppp ssss`
            if w & 0x0780 != 0 {
                return None;
            }
            one(Inst::BitW { op: if top == 0xA3 { BitOp::Btst } else { BitOp::Btsts }, b: false, s, bit: bit4, z: (w >> 11) & 1 != 0 })
        }
        0xA5 => {
            // BTST.C/Z Ws,Wb: `1010 0101 Zwww w000 0ppp ssss`
            if w & 0x0780 != 0 {
                return None;
            }
            one(Inst::BtstR { z: (w >> 15) & 1 != 0, s, wb: ((w >> 11) & 15) as u8 })
        }
        0xA6 | 0xA7 => {
            // BTSS (6) / BTSC (7) Ws,#bit4: `1010 011o bbbb 0000 0ppp ssss`
            if w & 0x0F80 != 0 {
                return None;
            }
            one(Inst::BitW { op: if top == 0xA6 { BitOp::Btss } else { BitOp::Btsc }, b: false, s, bit: bit4, z: false })
        }
        0xAD => {
            // BSW.C/Z Ws,Wb: `1010 1101 Zwww w000 0ppp ssss`
            if w & 0x0780 != 0 {
                return None;
            }
            one(Inst::Bsw { z: (w >> 15) & 1 != 0, s, wb: ((w >> 11) & 15) as u8 })
        }
        _ => None,
    }
}

fn decode_mul(w: u32) -> Option<Decoded> {
    let one = |inst| Some(Decoded { inst, words: 1 });
    let top = w >> 16;
    let su = match (top, (w >> 15) & 1) {
        (0xB8, 0) => 0,
        (0xB8, _) => 1,
        (0xB9, 0) => 2,
        _ => 3,
    };
    let wb = ((w >> 11) & 15) as u8;
    let dd = (w >> 7) & 15;
    let lit = (w >> 5) & 3 == 3;
    if lit && (su == 1 || su == 3) {
        return None;
    }
    let s = if lit { Src::Lit((w & 31) as u16) } else { Src::Ws(src_ea(w & !0xF8000)?) };
    // `MUL.xx Wb,Ws,Acc`: Wnd field = 111A
    if dd >> 1 == 7 {
        // MULW to W14 is not encodable; the field means the accumulator
        if (w >> 15) & 1 != 0 && top == 0xB8 && false {
            return None;
        }
        return one(Inst::MulAcc { su, wb, s, acc: dd & 1 != 0 });
    }
    // MUL (32 bit) clears bit 7; MULW (16 bit) sets it
    let wide = dd & 1 == 0;
    one(Inst::Mul { su, wide, wb, s, wd: (dd & 14) as u8 })
}

fn decode_movd(w: u32) -> Option<Decoded> {
    let one = |inst| Some(Decoded { inst, words: 1 });
    let r = (w & 15) as u8;
    if w & 0x8000 == 0 {
        // MOV.D Ws,Wnd: `1011 1110 0000 0ddd 0ppp ssss`
        if w & 0x7800 != 0 || w & 0x80 != 0 {
            return None;
        }
        let s = Ea::from_mode((w >> 4) & 7, w as u32 & 15, 0).filter(|e| !matches!(e, Ea::RegOff(..)))?;
        if s.is_direct() && r & 1 != 0 {
            return None;
        }
        one(Inst::MovD { s, d: Ea::Dir(((w >> 8) & 7) as u8 * 2) })
    } else {
        // MOV.D Wns,Wd: `1011 1110 1qqq dddd 0000 sss0`
        if w & 0x70 != 0 || r & 1 != 0 {
            return None;
        }
        let d = Ea::from_mode((w >> 11) & 7, (w >> 7) & 15, 0).filter(|e| !matches!(e, Ea::RegOff(..)))?;
        one(Inst::MovD { s: Ea::Dir(r), d })
    }
}

fn decode_mac(w: u32) -> Option<Decoded> {
    let one = |inst| Some(Decoded { inst, words: 1 });
    let acc = (w >> 15) & 1 != 0;
    let mmm = (w >> 16) & 7;
    let sub = (w >> 14) & 1 != 0;
    let dx = 4 + ((w >> 12) & 3);
    let dy = 4 + ((w >> 10) & 3);
    let x = pf((w >> 6) & 15, dx, false);
    let y = pf((w >> 2) & 15, dy, true);
    let (x, y) = (if (w >> 6) & 15 == 4 { None } else { x }, if (w >> 2) & 15 == 4 { None } else { y });
    if (w >> 6) & 15 != 4 && x.is_none() || (w >> 2) & 15 != 4 && y.is_none() {
        return None;
    }
    let awb = match w & 3 {
        0 => Awb::W13,
        1 => Awb::W13Inc,
        _ => Awb::None,
    };
    let m = match mmm {
        0 => (4, 5),
        1 => (4, 6),
        2 => (4, 7),
        4 => (5, 6),
        5 => (5, 7),
        6 => (6, 7),
        3 => {
            // CLR Acc: `1100 0011 A0xx yyii iijj jjaa` (the other accumulator is written back)
            if sub {
                return None;
            }
            return one(Inst::Mac { op: MacOp::Clr, acc, m: (0, 0), x, y, awb });
        }
        _ => {
            if sub {
                return None;
            }
            return one(Inst::Mac { op: MacOp::Movsac, acc, m: (0, 0), x, y, awb });
        }
    };
    let (op, awb) = match (sub, w & 3) {
        (false, 3) => (MacOp::Mpy, Awb::None),
        (true, 3) => (MacOp::MpyN, Awb::None),
        (false, _) => (MacOp::Mac, awb),
        (true, _) => (MacOp::Msc, awb),
    };
    one(Inst::Mac { op, acc, m, x, y, awb })
}

/// `MAC`/`MPY`/`ED`/`EDAC` of a squared register: `1111 00mm A?xx yyii iijj jjoo`.
fn decode_mac_sq(w: u32) -> Option<Decoded> {
    let acc = (w >> 15) & 1 != 0;
    let r = 4 + ((w >> 16) & 3) as u8;
    let ed = (w >> 14) & 1 != 0;
    let dx = 4 + ((w >> 12) & 3);
    let dy = 4 + ((w >> 10) & 3);
    let xi = (w >> 6) & 15;
    let yj = (w >> 2) & 15;
    let x = if xi == 4 { None } else { Some(pf(xi, dx, false)?) };
    let y = if yj == 4 { None } else { Some(pf(yj, dy, true)?) };
    let op = match (ed, w & 3) {
        (false, 0) => MacOp::Mac,
        (false, 1) => MacOp::Mpy,
        (true, 3) => MacOp::Ed,
        (true, 2) => MacOp::Edac,
        _ => return None,
    };
    if ed && (x.is_none() || y.is_none() || (w >> 10) & 3 != 0) {
        return None;
    }
    // ED/EDAC store the difference of the two prefetched words in one register, `xx`
    let y = if ed { y.map(|p| Pf { dst: dx as u8, ..p }) } else { y };
    Some(Decoded { inst: Inst::Mac { op, acc, m: (r, r), x, y, awb: Awb::None }, words: 1 })
}

fn decode_maxmin(w: u32) -> Option<Decoded> {
    let one = |inst| Some(Decoded { inst, words: 1 });
    let acc = (w >> 15) & 1 != 0;
    let kind = (w >> 12) & 7;
    if kind == 6 {
        // NORM Acc,Wd: `1100 1110 A110 0000 0qqq dddd`
        if w & 0xF80 != 0 {
            return None;
        }
        return one(Inst::Norm { acc, d: Ea::from_mode((w >> 4) & 7, w & 15, 0).filter(|e| !matches!(e, Ea::RegOff(..)))? });
    }
    // `A0?x x?00 0qqq dddd`: bit 14 selects MIN/MINZ, xx = 11 gives the .V form with a destination
    let minmax = (w >> 13) & 3;
    let v = match (w >> 11) & 3 {
        0 => {
            if w & 0xFF != 0 {
                return None;
            }
            None
        }
        3 => Some(Ea::from_mode((w >> 4) & 7, w & 15, 0).filter(|e| !matches!(e, Ea::RegOff(..)))?),
        _ => return None,
    };
    let z = (w >> 10) & 1 != 0;
    match (minmax, z) {
        (0, false) if w & 0x700 == 0 => one(Inst::Max { acc, v }),
        (1, false) if w & 0x700 == 0 => one(Inst::Min { acc, v }),
        (1, true) if w & 0x700 == 0x400 => one(Inst::Minz { acc, v }),
        _ => None,
    }
}

fn decode_shift(w: u32, top: u32) -> Option<Decoded> {
    let one = |inst| Some(Decoded { inst, words: 1 });
    let hi = (w >> 15) & 1 != 0;
    let b = (w >> 14) & 1 != 0;
    let op = match (top & 3, hi) {
        (0, false) => Un::Sl,
        (1, false) => Un::Lsr,
        (1, true) => Un::Asr,
        (2, false) => Un::Rlnc,
        (2, true) => Un::Rlc,
        (3, false) => Un::Rrnc,
        (3, true) => Un::Rrc,
        _ => return None,
    };
    if top & 4 == 0 {
        one(Inst::Un { op, b, s: src_ea(w & 0xFFFF)?, d: dst_ea(w & 0xFFFF)? })
    } else {
        one(Inst::UnF { op, b, f: (w & 0x1FFF) as u16, w_dst: (w >> 13) & 1 == 0 })
    }
}

fn decode_div(w: u32) -> Option<Decoded> {
    let one = |inst| Some(Decoded { inst, words: 1 });
    let top = w >> 16;
    let wn = (w & 15) as u8;
    if top == 0xD9 {
        // DIVF `1101 1001 0ttt t000 0000 ssss`, DIVF2 `... 0010 ssss`
        if w & 0x8000 != 0 || w & 0x40 != 0 {
            return None;
        }
        let two = match (w >> 4) & 3 {
            0 => false,
            2 => true,
            _ => return None,
        };
        return one(Inst::Divf { wm: ((w >> 11) & 15) as u8, wn, two });
    }
    let signed = (w >> 15) & 1 == 0;
    let two = match (w >> 4) & 3 {
        0 => false,
        2 => true,
        _ => return None,
    };
    let double = (w >> 6) & 1 != 0;
    one(Inst::Div { signed, double, wm: ((w >> 7) & 15) as u8, wn, two })
}

fn decode_bf(w: u32, w2: u32) -> Option<Decoded> {
    let two = |inst| Some(Decoded { inst, words: 2 });
    let l = (w & 15) as u8;
    let m = ((w >> 4) & 15) as u8;
    if m < l {
        return None;
    }
    let wid = m - l + 1;
    let reg = ((w >> 8) & 15) as u8;
    let ws2 = |w2: u32| Ea::from_mode((w2 >> 4) & 7, w2 & 15, 0).filter(|e| !matches!(e, Ea::RegOff(..)));
    match (w >> 12) & 15 {
        0x0 => two(Inst::Bfins { bit: l, wid, src: BfIns::Wns(reg), dst: BfDst::Ws(ws2(w2)?) }),
        0x2 if w2 & 1 == 0 => two(Inst::Bfins { bit: l, wid, src: BfIns::Wns(reg), dst: BfDst::F((w2 & 0xFFFF) as u16) }),
        0x4 => {
            if w & 0xF00 != 0 {
                return None;
            }
            two(Inst::Bfins { bit: l, wid, src: BfIns::Lit(((w2 >> 8) & 0xFF) as u8), dst: BfDst::Ws(ws2(w2 & 0xFF)?) })
        }
        0x8 => two(Inst::Bfext { bit: l, wid, src: BfSrc::Ws(ws2(w2)?), wd: reg }),
        0xA if w2 & 1 == 0 => two(Inst::Bfext { bit: l, wid, src: BfSrc::F((w2 & 0xFFFF) as u16), wd: reg }),
        _ => None,
    }
}

/// Decodes the instruction word `w` (24 bits); `w2` is the word after it, used by the two-word instructions.
pub fn decode(w: u32, w2: u32) -> Option<Decoded> {
    let w = w & 0xFF_FFFF;
    let top = w >> 16;
    let one = |inst| Some(Decoded { inst, words: 1 });
    let two = |inst| Some(Decoded { inst, words: 2 });
    let b = (w >> 14) & 1 != 0;
    match top {
        0x00 => one(Inst::Nop),
        0x01 => {
            let wn = (w & 15) as u8;
            if w & 0xF0 != 0 {
                return None;
            }
            match (w >> 8) & 0xFF {
                0x00 => one(Inst::CallW { wn }),
                0x02 => one(Inst::RcallW { wn }),
                0x04 => one(Inst::GotoW { wn }),
                0x06 => one(Inst::BraW { wn }),
                _ => None,
            }
        }
        0x02 | 0x04 => {
            if w & 1 != 0 {
                return None;
            }
            let addr = ((w2 & 0x7F) << 16) | (w & 0xFFFE);
            if top == 0x02 { two(Inst::Call { addr }) } else { two(Inst::Goto { addr }) }
        }
        0x05 => {
            if w & 0x8000 != 0 {
                return None;
            }
            one(Inst::Retlw { b, k: ((w >> 4) & 0x3FF) as u16, wn: (w & 15) as u8 })
        }
        0x06 => match w {
            0x06_0000 => one(Inst::Return),
            0x06_4000 => one(Inst::Retfie),
            _ => None,
        },
        0x07 => one(Inst::Rcall { off: sext(w & 0xFFFF, 16) }),
        0x08 => {
            let off = sext(w2 & 0xFFFF, 16);
            if w & 0x8000 != 0 {
                if w & 0x7FF0 != 0 {
                    return None;
                }
                two(Inst::DoW { wn: (w & 15) as u8, off })
            } else {
                two(Inst::Do { k: (w & 0x7FFF) as u16, off })
            }
        }
        0x09 => {
            if w & 0x8000 != 0 {
                if w & 0x7FF0 != 0 {
                    return None;
                }
                one(Inst::RepeatW { wn: (w & 15) as u8 })
            } else {
                one(Inst::Repeat { k: (w & 0x7FFF) as u16 })
            }
        }
        0x0A => decode_bf(w, w2),
        0x0C..=0x0F => one(Inst::Bra { cc: [Cc::Oa, Cc::Ob, Cc::Sa, Cc::Sb][(top & 3) as usize], off: sext(w & 0xFFFF, 16) }),
        0x10..=0x1F => alu3(w, alu3_op(top >> 3)?),
        0x20..=0x2F => one(Inst::MovLit16 { k: ((w >> 4) & 0xFFFF) as u16, wn: (w & 15) as u8 }),
        0x30..=0x3E => {
            let cc = [Cc::Ov, Cc::C, Cc::Z, Cc::N, Cc::Le, Cc::Lt, Cc::Leu, Cc::Always, Cc::Nov, Cc::Nc, Cc::Nz, Cc::Nn, Cc::Gt, Cc::Ge, Cc::Gtu][(top & 15) as usize];
            one(Inst::Bra { cc, off: sext(w & 0xFFFF, 16) })
        }
        0x40..=0x77 => alu3(w, alu3_op(top >> 3)?),
        0x78..=0x7F => {
            // MOV{.B} Ws,Wd, with `[Ws+Wb]` / `[Wd+Wb]` for the mode 11x
            let wb = (w >> 15) & 15;
            let s = Ea::from_mode((w >> 4) & 7, w & 15, wb)?;
            let d = Ea::from_mode((w >> 11) & 7, (w >> 7) & 15, wb)?;
            one(Inst::Mov { b, s, d })
        }
        0x80..=0x87 => one(Inst::MovFR { f: (((w >> 4) & 0x7FFF) << 1) as u16, wn: (w & 15) as u8 }),
        0x88..=0x8F => one(Inst::MovRF { f: (((w >> 4) & 0x7FFF) << 1) as u16, wn: (w & 15) as u8 }),
        0x90..=0x9F => {
            // `1001 0kkk kBkk kddd dkkk ssss`
            let k = (((w >> 16) & 7) << 7) | (((w >> 15) & 1) << 6) | (((w >> 12) & 3) << 4) | (((w >> 11) & 1) << 3) | ((w >> 4) & 7);
            let k = sext(k, 10) as i16;
            let off = if b { k } else { k * 2 };
            let reg_d = ((w >> 7) & 15) as u8;
            let reg_s = (w & 15) as u8;
            if top & 8 == 0 { one(Inst::MovOffR { b, off, ws: reg_s, wn: reg_d }) } else { one(Inst::MovOffW { b, off, wd: reg_d, wn: reg_s }) }
        }
        0xA0..=0xAF => decode_bit(w),
        0xB0..=0xB3 => {
            let hi = (w >> 15) & 1 != 0;
            let k = ((w >> 4) & 0x3FF) as u16;
            let wn = (w & 15) as u8;
            let op = match (top, hi) {
                (0xB0, false) => A3::Add,
                (0xB0, true) => A3::Addc,
                (0xB1, false) => A3::Sub,
                (0xB1, true) => A3::Subb,
                (0xB2, false) => A3::And,
                (0xB2, true) => A3::Xor,
                (0xB3, false) => A3::Ior,
                _ => {
                    return if w & 0xF000 == 0xC000 { one(Inst::MovLit8 { k: ((w >> 4) & 0xFF) as u8, wn }) } else { None };
                }
            };
            if b && k > 0xFF {
                return None;
            }
            one(Inst::AluLit { op, b, k, wn })
        }
        0xB4..=0xB7 => {
            let hi = (w >> 15) & 1 != 0;
            let f = (w & 0x1FFF) as u16;
            let op = match (top, hi) {
                (0xB4, false) => A3::Add,
                (0xB4, true) => A3::Addc,
                (0xB5, false) => A3::Sub,
                (0xB5, true) => A3::Subb,
                (0xB6, false) => A3::And,
                (0xB6, true) => A3::Xor,
                (0xB7, false) => A3::Ior,
                _ => {
                    return if w & 0x2000 != 0 { one(Inst::MovWF { b, f }) } else { None };
                }
            };
            one(Inst::AluF { op, b, f, w_dst: (w >> 13) & 1 == 0 })
        }
        0xB8 | 0xB9 => decode_mul(w),
        0xBA | 0xBB => {
            let write = top == 0xBB;
            let high = (w >> 15) & 1 != 0;
            let s = Ea::from_mode((w >> 4) & 7, w & 15, 0).filter(|e| !matches!(e, Ea::RegOff(..)))?;
            let d = Ea::from_mode((w >> 11) & 7, (w >> 7) & 15, 0).filter(|e| !matches!(e, Ea::RegOff(..)))?;
            // TBLRD reads through `[Ws]`, TBLWT writes through `[Wd]`
            if (!write && s.is_direct()) || (write && d.is_direct()) {
                return None;
            }
            one(Inst::Tbl { write, high, b, s, d })
        }
        0xBC => {
            if w & 0xA000 != 0 {
                return None;
            }
            one(Inst::MulF { b, f: (w & 0x1FFF) as u16 })
        }
        0xBD => one(Inst::AluF { op: if (w >> 15) & 1 != 0 { A3::Subbr } else { A3::Subr }, b, f: (w & 0x1FFF) as u16, w_dst: (w >> 13) & 1 == 0 }),
        0xBE => decode_movd(w),
        0xBF => {
            if w & 0x8000 == 0 {
                return None;
            }
            one(Inst::MovFW { b, f: (w & 0x1FFF) as u16, w_dst: (w >> 13) & 1 == 0 })
        }
        0xC0..=0xC7 => decode_mac(w),
        0xC8 => {
            let acc = (w >> 15) & 1 != 0;
            if w & 0x7F00 != 0 {
                return None;
            }
            if w & 0xC0 == 0x40 {
                one(Inst::SftacLit { acc, k: sext(w & 0x3F, 6) as i8 })
            } else if w & 0xF0 == 0 {
                one(Inst::SftacW { acc, wn: (w & 15) as u8 })
            } else {
                None
            }
        }
        0xC9 | 0xCA => {
            let acc = (w >> 15) & 1 != 0;
            let sh = sext((w >> 7) & 15, 4) as i8;
            let s = Ea::from_mode((w >> 4) & 7, w & 15, (w >> 11) & 15)?;
            if top == 0xC9 { one(Inst::AddWs { s, sh, acc }) } else { one(Inst::Lac { s, sh, acc }) }
        }
        0xCB => {
            let acc = (w >> 15) & 1 != 0;
            if w & 0x4FFF != 0 {
                return None;
            }
            match (w >> 12) & 3 {
                0 => one(Inst::AddAcc { acc }),
                1 => one(Inst::NegAcc { acc }),
                3 => one(Inst::SubAcc { acc }),
                _ => None,
            }
        }
        0xCC | 0xCD => {
            let acc = (w >> 15) & 1 != 0;
            let sh = sext((w >> 7) & 15, 4) as i8;
            one(Inst::Sac { round: top == 0xCD, acc, sh, d: Ea::from_mode((w >> 4) & 7, w & 15, (w >> 11) & 15)? })
        }
        0xCE => decode_maxmin(w),
        0xCF => {
            let s = Ea::from_mode((w >> 4) & 7, w & 15, 0).filter(|e| !matches!(e, Ea::RegOff(..)))?;
            one(Inst::Ff1 { left: (w >> 15) & 1 != 0, s, wd: ((w >> 7) & 15) as u8 })
        }
        0xD0..=0xD7 => decode_shift(w, top - 0xD0),
        0xD8 | 0xD9 => decode_div(w),
        0xDB => one(Inst::LacD { s: Ea::from_mode((w >> 4) & 7, w & 15, 0).filter(|e| !matches!(e, Ea::RegOff(..)))?, sh: sext((w >> 7) & 15, 4) as i8, acc: (w >> 15) & 1 != 0 }),
        0xDC => one(Inst::SacD { acc: (w >> 15) & 1 != 0, sh: sext((w >> 7) & 15, 4) as i8, wd: (w & 15) as u8 }),
        0xDD | 0xDE => {
            let op = match (top, (w >> 15) & 1) {
                (0xDD, 0) => Un::Sl,
                (0xDE, 0) => Un::Lsr,
                (0xDE, _) => Un::Asr,
                _ => return None,
            };
            let src = match (w >> 4) & 7 {
                0 => ShSrc::W((w & 15) as u8),
                4 => ShSrc::Lit((w & 15) as u8),
                _ => return None,
            };
            one(Inst::Shift { op, wb: ((w >> 11) & 15) as u8, src, wd: ((w >> 7) & 15) as u8 })
        }
        0xDF => one(Inst::Fbcl { s: Ea::from_mode((w >> 4) & 7, w & 15, 0).filter(|e| !matches!(e, Ea::RegOff(..)))?, wd: ((w >> 7) & 15) as u8 }),
        0xE0 => {
            if w & 0xF980 != 0 {
                return None;
            }
            one(Inst::Cp0 { b: (w >> 10) & 1 != 0, s: Ea::from_mode((w >> 4) & 7, w & 15, 0).filter(|e| !matches!(e, Ea::RegOff(..)))? })
        }
        0xE1 => {
            let b = (w >> 10) & 1 != 0;
            let borrow = (w >> 15) & 1 != 0;
            let wb = ((w >> 11) & 15) as u8;
            if (w >> 5) & 3 == 3 {
                one(Inst::Cp { borrow, b, wb, s: Src::Lit((((w >> 7) & 7) << 5 | (w & 31)) as u16) })
            } else {
                if w & 0x380 != 0 {
                    return None;
                }
                one(Inst::Cp { borrow, b, wb, s: Src::Ws(Ea::from_mode((w >> 4) & 7, w & 15, 0).filter(|e| !matches!(e, Ea::RegOff(..)))?) })
            }
        }
        0xE2 => {
            if w & 0xA000 != 0 {
                return None;
            }
            one(Inst::Cp0F { b, f: (w & 0x1FFF) as u16 })
        }
        0xE3 => {
            if w & 0x2000 != 0 {
                return None;
            }
            one(Inst::CpF { borrow: (w >> 15) & 1 != 0, b, f: (w & 0x1FFF) as u16 })
        }
        0xE4 | 0xE5 => {
            let wb = ((w >> 11) & 15) as u8;
            let s = Ea::from_mode((w >> 4) & 7, w & 15, 0).filter(|e| !matches!(e, Ea::RegOff(..)))?;
            if top == 0xE4 {
                if w & 0x87F == 0 || w & 0x380 != 0 {
                    return None;
                }
                one(Inst::Flim { v: false, wb, s, wd: 0 })
            } else {
                one(Inst::Flim { v: true, wb, s, wd: ((w >> 7) & 15) as u8 })
            }
        }
        0xE6 | 0xE7 => {
            let cmp = match (top, (w >> 15) & 1) {
                (0xE6, 0) => Cmp::Gt,
                (0xE6, _) => Cmp::Lt,
                (_, 0) => Cmp::Ne,
                _ => Cmp::Eq,
            };
            let b = (w >> 10) & 1 != 0;
            let wb = ((w >> 11) & 15) as u8;
            let wn = (w & 15) as u8;
            let n = (((w >> 8) & 3) << 4) | ((w >> 4) & 15);
            if n == 1 {
                one(Inst::Cps { cmp, b, wb, wn })
            } else {
                one(Inst::Cpb { cmp, b, wb, wn, off: sext(n, 6) as i8 })
            }
        }
        0xE8..=0xEA => {
            let hi = (w >> 15) & 1 != 0;
            let op = match (top, hi) {
                (0xE8, false) => Un::Inc,
                (0xE8, true) => Un::Inc2,
                (0xE9, false) => Un::Dec,
                (0xE9, true) => Un::Dec2,
                (_, false) => Un::Neg,
                _ => Un::Com,
            };
            one(Inst::Un { op, b, s: src_ea(w & 0xFFFF)?, d: dst_ea(w & 0xFFFF)? })
        }
        0xEB => {
            if w & 0x7F != 0 {
                return None;
            }
            one(Inst::ClrW { set: (w >> 15) & 1 != 0, b, d: dst_ea(w & 0xFFFF)? })
        }
        0xEC..=0xEE => {
            let hi = (w >> 15) & 1 != 0;
            let op = match (top, hi) {
                (0xEC, false) => Un::Inc,
                (0xEC, true) => Un::Inc2,
                (0xED, false) => Un::Dec,
                (0xED, true) => Un::Dec2,
                (_, false) => Un::Neg,
                _ => Un::Com,
            };
            one(Inst::UnF { op, b, f: (w & 0x1FFF) as u16, w_dst: (w >> 13) & 1 == 0 })
        }
        0xEF => one(Inst::ClrF { set: (w >> 15) & 1 != 0, b, f: (w & 0x1FFF) as u16, w_dst: (w >> 13) & 1 == 0 }),
        0xF0..=0xF3 => decode_mac_sq(w),
        0xF8 | 0xF9 => {
            if w & 1 != 0 {
                return None;
            }
            let f = (w & 0xFFFE) as u16;
            if top == 0xF8 { one(Inst::Push { f }) } else { one(Inst::Pop { f }) }
        }
        0xFA => {
            if w & 0x8000 != 0 {
                if w == 0xFA_8000 { one(Inst::Ulnk) } else { None }
            } else {
                if w & 1 != 0 || w & 0x4000 != 0 {
                    return None;
                }
                one(Inst::Lnk { k: (w & 0x3FFE) as u16 })
            }
        }
        0xFB => {
            if w & 0x7800 != 0 || w & 0x380 != 0 && false {
                return None;
            }
            let s = Ea::from_mode((w >> 4) & 7, w & 15, 0).filter(|e| !matches!(e, Ea::RegOff(..)))?;
            let wd = ((w >> 7) & 15) as u8;
            if w & 0x8000 == 0 { one(Inst::Se { s, wd }) } else { one(Inst::Ze { s, wd }) }
        }
        0xFC => {
            if w & 0xC000 != 0 {
                return None;
            }
            one(Inst::Disi { k: (w & 0x3FFF) as u16 })
        }
        0xFD => {
            let wn = (w & 15) as u8;
            match (w >> 14) & 3 {
                0 if w & 0x8000 == 0 => one(Inst::Exch { s: wn, d: ((w >> 7) & 15) as u8 }),
                1 if w & 0x8000 == 0 => one(Inst::Daw { wn }),
                _ if w & 0x8000 != 0 => one(Inst::Swap { b, wn }),
                _ => None,
            }
        }
        0xFE => match (w >> 12) & 0xF {
            0x0 if w == 0xFE_0000 => one(Inst::Reset),
            0x2 if w == 0xFE_2000 => one(Inst::Bootswp),
            0x4 => one(Inst::Pwrsav { k: (w & 1) as u8 }),
            0x6 if w == 0xFE_6000 => one(Inst::Clrwdt),
            0x8 if w == 0xFE_8000 => one(Inst::PopS),
            0xA if w == 0xFE_A000 => one(Inst::PushS),
            0xC if (w >> 10) & 3 != 3 => one(Inst::MovpagLit { p: ((w >> 10) & 3) as u8, k: (w & 0x3FF) as u16 }),
            0xD if (w >> 10) & 3 != 3 && w & 0x3F0 == 0 => one(Inst::MovpagW { p: ((w >> 10) & 3) as u8, wn: (w & 15) as u8 }),
            0xE if w & 0xFF8 == 0 => one(Inst::CtxtswpLit { k: (w & 7) as u8 }),
            0xF if w & 0xFF0 == 0 => one(Inst::CtxtswpW { wn: (w & 15) as u8 }),
            _ => None,
        },
        0xFF => one(Inst::Nopr),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Disassembly
// ---------------------------------------------------------------------------------------------------------------

fn w(n: u8) -> String {
    format!("w{n}")
}

fn ea(e: Ea) -> String {
    match e {
        Ea::Dir(r) => w(r),
        Ea::Ind(r) => format!("[w{r}]"),
        Ea::PostDec(r) => format!("[w{r}--]"),
        Ea::PostInc(r) => format!("[w{r}++]"),
        Ea::PreDec(r) => format!("[--w{r}]"),
        Ea::PreInc(r) => format!("[++w{r}]"),
        Ea::RegOff(r, b) => format!("[w{r}+w{b}]"),
    }
}

fn src(s: Src) -> String {
    match s {
        Src::Ws(e) => ea(e),
        Src::Lit(k) => format!("#{k}"),
    }
}

fn sfx(b: bool) -> &'static str {
    if b { ".b" } else { "" }
}

fn a3_name(op: A3) -> &'static str {
    match op {
        A3::Add => "add",
        A3::Addc => "addc",
        A3::Sub => "sub",
        A3::Subb => "subb",
        A3::Subr => "subr",
        A3::Subbr => "subbr",
        A3::And => "and",
        A3::Ior => "ior",
        A3::Xor => "xor",
    }
}

fn un_name(op: Un) -> &'static str {
    match op {
        Un::Inc => "inc",
        Un::Inc2 => "inc2",
        Un::Dec => "dec",
        Un::Dec2 => "dec2",
        Un::Com => "com",
        Un::Neg => "neg",
        Un::Sl => "sl",
        Un::Lsr => "lsr",
        Un::Asr => "asr",
        Un::Rlc => "rlc",
        Un::Rlnc => "rlnc",
        Un::Rrc => "rrc",
        Un::Rrnc => "rrnc",
    }
}

fn cc_name(cc: Cc) -> &'static str {
    match cc {
        Cc::Ov => "ov",
        Cc::C => "c",
        Cc::Z => "z",
        Cc::N => "n",
        Cc::Le => "le",
        Cc::Lt => "lt",
        Cc::Leu => "leu",
        Cc::Always => "",
        Cc::Nov => "nov",
        Cc::Nc => "nc",
        Cc::Nz => "nz",
        Cc::Nn => "nn",
        Cc::Gt => "gt",
        Cc::Ge => "ge",
        Cc::Gtu => "gtu",
        Cc::Oa => "oa",
        Cc::Ob => "ob",
        Cc::Sa => "sa",
        Cc::Sb => "sb",
    }
}

fn acc(a: Acc) -> &'static str {
    if a { "b" } else { "a" }
}

fn pfs(p: Option<Pf>) -> String {
    match p {
        None => String::new(),
        Some(p) => match p.inc {
            Some(0) => format!(",[w{}],w{}", p.wn, p.dst),
            Some(i) if i > 0 => format!(",[w{}]+={i},w{}", p.wn, p.dst),
            Some(i) => format!(",[w{}]-={},w{}", p.wn, -i, p.dst),
            None => format!(",[w{}+w12],w{}", p.wn, p.dst),
        },
    }
}

fn awbs(a: Awb) -> &'static str {
    match a {
        Awb::None => "",
        Awb::W13 => ",w13",
        Awb::W13Inc => ",[w13]+=2",
    }
}

fn bit_name(op: BitOp) -> &'static str {
    match op {
        BitOp::Bset => "bset",
        BitOp::Bclr => "bclr",
        BitOp::Btg => "btg",
        BitOp::Btsc => "btsc",
        BitOp::Btss => "btss",
        BitOp::Btst => "btst",
        BitOp::Btsts => "btsts",
    }
}

fn target(pc: u32, rel: bool, to: u32) -> String {
    if rel { format!(".{:+}", to as i64 - pc as i64) } else { format!("0x{to:X}") }
}

/// Assembler text of an instruction at `pc` (relative targets as `.+N` when `rel`).
pub fn format(inst: &Inst, pc: u32, rel: bool) -> String {
    use Inst::*;
    let np = pc.wrapping_add(2);
    match *inst {
        Nop => "nop".into(),
        Nopr => "nopr".into(),
        Alu3 { op, b, wb, s, d } => format!("{}{} w{wb},{},{}", a3_name(op), sfx(b), src(s), ea(d)),
        AluF { op, b, f, w_dst } => format!("{}{} 0x{f:X}{}", a3_name(op), sfx(b), if w_dst { ",wreg" } else { "" }),
        AluLit { op, b, k, wn } => format!("{}{} #{k},w{wn}", a3_name(op), sfx(b)),
        Un { op, b, s, d } => format!("{}{} {},{}", un_name(op), sfx(b), ea(s), ea(d)),
        UnF { op, b, f, w_dst } => format!("{}{} 0x{f:X}{}", un_name(op), sfx(b), if w_dst { ",wreg" } else { "" }),
        Shift { op, wb, src, wd } => format!(
            "{} w{wb},{},w{wd}",
            un_name(op),
            match src {
                ShSrc::Lit(k) => format!("#{k}"),
                ShSrc::W(n) => w(n),
            }
        ),
        ClrW { set, b, d } => format!("{}{} {}", if set { "setm" } else { "clr" }, sfx(b), ea(d)),
        ClrF { set, b, f, w_dst } => {
            if w_dst { format!("{}{} wreg", if set { "setm" } else { "clr" }, sfx(b)) } else { format!("{}{} 0x{f:X}", if set { "setm" } else { "clr" }, sfx(b)) }
        }
        Cp { borrow, b, wb, s } => format!("{}{} w{wb},{}", if borrow { "cpb" } else { "cp" }, sfx(b), src(s)),
        CpF { borrow, b, f } => format!("{}{} 0x{f:X}", if borrow { "cpb" } else { "cp" }, sfx(b)),
        Cp0 { b, s } => format!("cp0{} {}", sfx(b), ea(s)),
        Cp0F { b, f } => format!("cp0{} 0x{f:X}", sfx(b)),
        Cps { cmp, b, wb, wn } => format!("cps{}{} w{wb},w{wn}", cmp_name(cmp), sfx(b)),
        Cpb { cmp, b, wb, wn, off } => format!("cpb{}{} w{wb},w{wn},{}", cmp_name(cmp), sfx(b), target(pc, rel, (np as i64 + 2 * off as i64) as u32)),
        Bra { cc, off } => {
            let t = target(pc, rel, (np as i64 + 2 * off as i64) as u32);
            if cc == Cc::Always { format!("bra {t}") } else { format!("bra {},{t}", cc_name(cc)) }
        }
        BraW { wn } => format!("bra w{wn}"),
        Goto { addr } => format!("goto 0x{addr:X}"),
        GotoW { wn } => format!("goto w{wn}"),
        Call { addr } => format!("call 0x{addr:X}"),
        CallW { wn } => format!("call w{wn}"),
        Rcall { off } => format!("rcall {}", target(pc, rel, (np as i64 + 2 * off as i64) as u32)),
        RcallW { wn } => format!("rcall w{wn}"),
        Return => "return".into(),
        Retfie => "retfie".into(),
        Retlw { b, k, wn } => format!("retlw{} #{k},w{wn}", sfx(b)),
        Repeat { k } => format!("repeat #{k}"),
        RepeatW { wn } => format!("repeat w{wn}"),
        Do { k, off } => format!("do #{k},{}", target(pc, rel, (pc as i64 + 4 + 2 * off as i64) as u32)),
        DoW { wn, off } => format!("do w{wn},{}", target(pc, rel, (pc as i64 + 4 + 2 * off as i64) as u32)),
        Lnk { k } => format!("lnk #{k}"),
        Ulnk => "ulnk".into(),
        Disi { k } => format!("disi #{k}"),
        Pwrsav { k } => format!("pwrsav #{k}"),
        Clrwdt => "clrwdt".into(),
        Reset => "reset".into(),
        PushS => "push.s".into(),
        PopS => "pop.s".into(),
        Bootswp => "bootswp".into(),
        CtxtswpLit { k } => format!("ctxtswp #{k}"),
        CtxtswpW { wn } => format!("ctxtswp w{wn}"),
        MovpagLit { p, k } => format!("movpag #{k},{}", ["DSRPAG", "DSWPAG", "TBLPAG", "?"][p as usize]),
        MovpagW { p, wn } => format!("movpag w{wn},{}", ["DSRPAG", "DSWPAG", "TBLPAG", "?"][p as usize]),
        MovFW { b, f, w_dst } => {
            if w_dst { format!("mov{} 0x{f:X},wreg", sfx(b)) } else { format!("mov{} 0x{f:X}", sfx(b)) }
        }
        MovWF { b, f } => format!("mov{} wreg,0x{f:X}", sfx(b)),
        MovFR { f, wn } => format!("mov 0x{f:X},w{wn}"),
        MovRF { f, wn } => format!("mov w{wn},0x{f:X}"),
        MovLit8 { k, wn } => format!("mov.b #{k},w{wn}"),
        MovLit16 { k, wn } => format!("mov #{k},w{wn}"),
        MovOffR { b, off, ws, wn } => format!("mov{} [w{ws}{:+}],w{wn}", sfx(b), off),
        MovOffW { b, off, wd, wn } => format!("mov{} w{wn},[w{wd}{:+}]", sfx(b), off),
        Mov { b, s, d } => format!("mov{} {},{}", sfx(b), ea(s), ea(d)),
        MovD { s, d } => format!("mov.d {},{}", ea(s), ea(d)),
        Mul { su, wide, wb, s, wd } => format!("mul{}.{} w{wb},{},w{wd}", if wide { "" } else { "w" }, ["uu", "us", "su", "ss"][su as usize], src(s)),
        MulAcc { su, wb, s, acc: a } => format!("mul.{} w{wb},{},{}", ["uu", "us", "su", "ss"][su as usize], src(s), acc(a)),
        MulF { b, f } => format!("mul{} 0x{f:X}", sfx(b)),
        Tbl { write, high, b, s, d } => {
            let n = format!("tbl{}{}{}", if write { "wt" } else { "rd" }, if high { "h" } else { "l" }, sfx(b));
            if write { format!("{n} {},{}", ea(s), ea(d)) } else { format!("{n} {},{}", ea(s), ea(d)) }
        }
        Bit { op, f, bit } => format!("{}.b 0x{f:X},#{bit}", bit_name(op)),
        BitW { op, b, s, bit, z } => {
            let suffix = match op {
                BitOp::Btst | BitOp::Btsts => {
                    if z { ".z" } else { ".c" }
                }
                _ => sfx(b),
            };
            format!("{}{suffix} {},#{bit}", bit_name(op), ea(s))
        }
        BtstR { z, s, wb } => format!("btst.{} {},w{wb}", if z { "z" } else { "c" }, ea(s)),
        Bsw { z, s, wb } => format!("bsw.{} {},w{wb}", if z { "z" } else { "c" }, ea(s)),
        Div { signed, double, wm, wn, two } => format!("div{}.{}{} w{wm},w{wn}", if two { "2" } else { "" }, if signed { "s" } else { "u" }, if double { "d" } else { "w" }),
        Divf { wm, wn, two } => format!("divf{} w{wm},w{wn}", if two { "2" } else { "" }),
        Ff1 { left, s, wd } => format!("ff1{} {},w{wd}", if left { "l" } else { "r" }, ea(s)),
        Fbcl { s, wd } => format!("fbcl {},w{wd}", ea(s)),
        Se { s, wd } => format!("se {},w{wd}", ea(s)),
        Ze { s, wd } => format!("ze {},w{wd}", ea(s)),
        Swap { b, wn } => format!("swap{} w{wn}", sfx(b)),
        Exch { s, d } => format!("exch w{s},w{d}"),
        Daw { wn } => format!("daw.b w{wn}"),
        Push { f } => format!("push 0x{f:X}"),
        Pop { f } => format!("pop 0x{f:X}"),
        Flim { v, wb, s, wd } => {
            if v { format!("flim.v w{wb},{},w{wd}", ea(s)) } else { format!("flim w{wb},{}", ea(s)) }
        }
        Bfext { bit, wid, src, wd } => match src {
            BfSrc::Ws(e) => format!("bfext #{bit},#{wid},{},w{wd}", ea(e)),
            BfSrc::F(f) => format!("bfext #{bit},#{wid},0x{f:X},w{wd}"),
        },
        Bfins { bit, wid, src, dst } => {
            let d = match dst {
                BfDst::Ws(e) => ea(e),
                BfDst::F(f) => format!("0x{f:X}"),
            };
            match src {
                BfIns::Wns(n) => format!("bfins #{bit},#{wid},w{n},{d}"),
                BfIns::Lit(k) => format!("bfins #{bit},#{wid},#{k},{d}"),
            }
        }
        AddAcc { acc: a } => format!("add {}", acc(a)),
        SubAcc { acc: a } => format!("sub {}", acc(a)),
        NegAcc { acc: a } => format!("neg {}", acc(a)),
        AddWs { s, sh, acc: a } => format!("add {},#{sh},{}", ea(s), acc(a)),
        Lac { s, sh, acc: a } => format!("lac {},#{sh},{}", ea(s), acc(a)),
        LacD { s, sh, acc: a } => format!("lac.d {},#{sh},{}", ea(s), acc(a)),
        Sac { round, acc: a, sh, d } => format!("sac{} {},#{sh},{}", if round { ".r" } else { "" }, acc(a), ea(d)),
        SacD { acc: a, sh, wd } => format!("sac.d {},#{sh},w{wd}", acc(a)),
        SftacLit { acc: a, k } => format!("sftac {},#{k}", acc(a)),
        SftacW { acc: a, wn } => format!("sftac {},w{wn}", acc(a)),
        Max { acc: a, v } => match v {
            None => format!("max {}", acc(a)),
            Some(d) => format!("max.v {},{}", acc(a), ea(d)),
        },
        Min { acc: a, v } => match v {
            None => format!("min {}", acc(a)),
            Some(d) => format!("min.v {},{}", acc(a), ea(d)),
        },
        Minz { acc: a, v } => match v {
            None => format!("minz {}", acc(a)),
            Some(d) => format!("minz.v {},{}", acc(a), ea(d)),
        },
        Norm { acc: a, d } => format!("norm {},{}", acc(a), ea(d)),
        Mac { op, acc: a, m, x, y, awb } => {
            let mm = format!("w{}*w{}", m.0, m.1);
            match op {
                MacOp::Mac => format!("mac {mm},{}{}{}{}", acc(a), pfs(x), pfs(y), awbs(awb)),
                MacOp::Msc => format!("msc {mm},{}{}{}{}", acc(a), pfs(x), pfs(y), awbs(awb)),
                MacOp::Mpy => format!("mpy {mm},{}{}{}", acc(a), pfs(x), pfs(y)),
                MacOp::MpyN => format!("mpy.n {mm},{}{}{}", acc(a), pfs(x), pfs(y)),
                MacOp::Ed => format!("ed {mm},{}{}{}", acc(a), pfs_nodst(x), pfs_ed(y)),
                MacOp::Edac => format!("edac {mm},{}{}{}", acc(a), pfs_nodst(x), pfs_ed(y)),
                MacOp::Movsac => format!("movsac {}{}{}{}", acc(a), pfs(x), pfs(y), awbs(awb)),
                MacOp::Clr => format!("clr {}{}{}{}", acc(a), pfs(x), pfs(y), awbs(awb)),
            }
        }
    }
}

// ED/EDAC: `ed w4*w4,a,[w8]+=2,[w10]+=2,w5`: both prefetches name no destination, then one register for the difference.
fn pfs_nodst(p: Option<Pf>) -> String {
    match p {
        None => String::new(),
        Some(p) => match p.inc {
            Some(0) => format!(",[w{}]", p.wn),
            Some(i) if i > 0 => format!(",[w{}]+={i}", p.wn),
            Some(i) => format!(",[w{}]-={}", p.wn, -i),
            None => format!(",[w{}+w12]", p.wn),
        },
    }
}
fn pfs_ed(p: Option<Pf>) -> String {
    match p {
        None => String::new(),
        Some(p) => format!("{},w{}", pfs_nodst(Some(Pf { dst: 0, ..p })), p.dst),
    }
}

fn cmp_name(c: Cmp) -> &'static str {
    match c {
        Cmp::Gt => "gt",
        Cmp::Lt => "lt",
        Cmp::Ne => "ne",
        Cmp::Eq => "eq",
    }
}
