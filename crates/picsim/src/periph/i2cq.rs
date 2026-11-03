//! I2C host of the PIC18-Q41 (DS40002214G ch.36 p.603-660), host mode, 7-bit addressing (I2CxCON0 MODE = 100). Client modes,
//! 10-bit addressing, bus time-out, bus collisions and clock stretching by a client are not modelled (a note is logged for
//! the modes). The outside world is the `[[i2c]]` clients of the stimulus file (as for the Q10 MSSP); the bytes and
//! conditions the host puts on the bus go to the same log. The bus has pull-ups and no other host. PPS, TRIS and the pins
//! are not modelled. I2CxCLK other than FOSC/4 logs a note and runs as FOSC/4.
//!
//! Sourced rules (register bits checked against the register summary p.695):
//! * One SCL period is (BAUD + 1) prescaled clocks times 5, or times 4 with FME = 1 (36.4.2.2); I2CxCLK = FOSC/4 is one
//!   clock per instruction cycle.
//! * Flags (36.3.14.2 p.619-621): I2CxTXIF = I2CxCNT != 0 and TXBE = 1 and MMA = 1; I2CxRXIF = RXBF and MMA; I2CxIF = any
//!   I2CxPIR flag with its I2CxPIE bit; I2CxEIF = any I2CxERR flag with its enable. CNTIF is set after the 9th falling
//!   edge when the count reaches zero; PCIF on a Stop, RSCIF on a Restart, SCIF on a Start. NACKIF is set on a NACK
//!   while MMA = 1 and by TXWE, RXRE, TXU, RXO. Flags are cleared by software; I2CxRXB read clears RXBF; CLRBF empties
//!   both buffers.
//!
//! Assumptions `[A#]`, not in the data sheet (ported from `scripts/picsim.py` of the Basic compiler, numbers continue
//! after A50):
//! * `[A51]` Writing I2CxTXB with TXBE = 1 clears TXBE at once. With ABD = 1 the first TXB write starts the transfer after
//!   the bus free time (BFRE); the address moves into the shift register one SCL period after the Start (TXBE = 1 again,
//!   MMA = 1 and SCIF at the Start).
//! * `[A52]` At the 8th falling edge of a transmitted byte, if TXBE = 1 and I2CxCNT != 0 the host sets MDR and stretches SCL
//!   until TXB is written; otherwise it goes on to the 9th clock.
//! * `[A53]` At the 9th falling edge after an ACK a pending TXB byte moves into the shift register and I2CxCNT decrements;
//!   with nothing pending and I2CxCNT = 0 the host sets CNTIF and sends the Stop, or (RSEN = 1) sets MDR and waits.
//! * `[A54]` On a NACK from the client: RSEN = 0 sends the Stop by itself, RSEN = 1 sets MDR and waits. NACKIF is set.
//! * `[A55]` At the MDR pause for a Restart I2CxCNT may be written; a write to TXB, or S = 1 in I2CxCON0, issues the Restart
//!   and the TXB byte is sent as the next address. RSCIF is set one SCL period after the Restart begins, the address
//!   moves to the shift register two periods after.
//! * `[A56]` Host receive: the byte goes to RXB, RXBF = 1 and the count decrements at the 8th edge; if RXBF is still 1 at the
//!   7th edge of the next byte the host sets MDR and stretches until RXB is read; the 9th clock sends ACKDT while the count
//!   is not zero and ACKCNT when it is, then CNTIF is set and the Stop follows (RSEN = 0) or MDR is set (RSEN = 1).
//!   A NACK before the count ends is a Stop. The host's own NACK sets NACKIF (worst case for a driver).
//! * `[A57]` P = 1 while MMA = 1 ends the transfer with a Stop within one SCL period and clears MDR.
//! * `[A58]` EN = 0 resets the state machine (MMA = 0, MDR = 0, TXBE = 1, RXBF = 0, count kept). On EN = 1 SCIF and PCIF may
//!   be set (errata), modelled as set 3 cycles after the enable. BFRE becomes 1 eight prescaled clocks after the enable
//!   or a Stop.
//! * `[A59]` A TXB write while TXBE = 0 sets TXWE and NACKIF and is dropped; a read of RXB with RXBF = 0 sets RXRE and
//!   NACKIF and gives the old byte.
//! * `[A60]` A TXB write with EN = 0 or MODE != 100 only fills TXB.

use super::mssp::{I2cClient, I2cEv};
use super::{addr, Flag};
use crate::mem::DataMem;
use devdesc::Device;
use std::collections::BTreeSet;

pub const CON0: u8 = 0;
pub const CON1: u8 = 1;
pub const STAT0: u8 = 2;
pub const STAT1: u8 = 3;
pub const PIR: u8 = 4;
pub const ERR: u8 = 5;
pub const CNTL: u8 = 6;
pub const CNTH: u8 = 7;
pub const TXB: u8 = 8;
pub const RXB: u8 = 9;
pub const HOOKS: u8 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ev {
    Spurious,
    TryStart,
    LoadShift,
    RestartFlag,
    Tx8,
    Tx9,
    Rx7,
    Rx8,
    Rx9,
    StopDone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum St {
    Idle,
    StartPending,
    Starting,
    Restarting,
    Tx,
    StallTx,
    PauseRestart,
    Rx,
    StallRx,
    Stopping,
}

pub struct I2cQ {
    pub num: u8,
    con0: usize,
    con1: usize,
    con2: usize,
    stat0: usize,
    stat1: usize,
    pir: usize,
    pie: usize,
    err: usize,
    cnt: [usize; 2],
    txb_a: usize,
    rxb_a: usize,
    baud: usize,
    clk: usize,
    rxif: Flag,
    txif: Flag,
    i2cif: Flag,
    eif: Flag,
    pub i2c: Vec<I2cClient>,
    q: Vec<(u64, u64, Ev)>,
    seq: u64,
    st: St,
    // hardware owned bits
    s: bool,
    mdr: bool,
    mma: bool,
    ackstat: bool,
    p: bool,
    txbe: bool,
    rxbf: bool,
    txwe: bool,
    rxre: bool,
    cnt_v: u16,
    txb: u8,
    rxb: u8,
    pirv: u8,
    errv: u8,
    shift: u8,
    is_addr: bool,
    rw: bool,
    bfre_at: Option<u64>,
    sel: Option<(usize, bool)>,
    expect_addr: bool,
    read_idx: Option<usize>,
}

impl I2cQ {
    pub fn find(dev: &Device, n: u8) -> Option<(I2cQ, [usize; 10])> {
        let a = |s: &str| addr(dev, &format!("I2C{n}{s}"));
        let s = I2cQ {
            num: n,
            con0: a("CON0")?,
            con1: a("CON1")?,
            con2: a("CON2")?,
            stat0: a("STAT0")?,
            stat1: a("STAT1")?,
            pir: a("PIR")?,
            pie: a("PIE")?,
            err: a("ERR")?,
            cnt: [a("CNTL")?, a("CNTH")?],
            txb_a: a("TXB")?,
            rxb_a: a("RXB")?,
            baud: a("BAUD")?,
            clk: a("CLK")?,
            rxif: Flag::find(dev, &format!("I2C{n}RXIF"))?,
            txif: Flag::find(dev, &format!("I2C{n}TXIF"))?,
            i2cif: Flag::find(dev, &format!("I2C{n}IF"))?,
            eif: Flag::find(dev, &format!("I2C{n}EIF"))?,
            i2c: Vec::new(),
            q: Vec::new(),
            seq: 0,
            st: St::Idle,
            s: false,
            mdr: false,
            mma: false,
            ackstat: false,
            p: false,
            txbe: true,
            rxbf: false,
            txwe: false,
            rxre: false,
            cnt_v: 0,
            txb: 0,
            rxb: 0,
            pirv: 0,
            errv: 0,
            shift: 0,
            is_addr: false,
            rw: false,
            bfre_at: None,
            sel: None,
            expect_addr: false,
            read_idx: None,
        };
        let regs = [s.con0, s.con1, s.stat0, s.stat1, s.pir, s.err, s.cnt[0], s.cnt[1], s.txb_a, s.rxb_a];
        Some((s, regs))
    }

    fn en(&self, m: &DataMem) -> bool {
        m.mem[self.con0] & 0x80 != 0
    }
    fn host(&self, m: &DataMem) -> bool {
        self.en(m) && m.mem[self.con0] & 7 == 4
    }
    fn rsen(&self, m: &DataMem) -> bool {
        m.mem[self.con0] & 0x40 != 0
    }
    /// One SCL period in instruction cycles.
    fn t(&self, m: &DataMem) -> u64 {
        (if m.mem[self.con2] & 0x20 != 0 { 4 } else { 5 }) * (u64::from(m.mem[self.baud]) + 1)
    }
    fn pre(&self, m: &DataMem) -> u64 {
        u64::from(m.mem[self.baud]) + 1
    }
    fn sched(&mut self, at: u64, e: Ev) {
        self.seq += 1;
        let i = self.q.partition_point(|&(t, s, _)| (t, s) <= (at, self.seq));
        self.q.insert(i, (at, self.seq, e));
    }
    fn abort(&mut self) {
        self.q.clear();
    }

    /// Hardware owned bits and derived flags into the registers.
    fn publish(&mut self, m: &mut DataMem, now: u64) {
        let bfre = self.en(m) && !self.mma && self.bfre_at.is_some_and(|b| now >= b);
        let c0 = m.mem[self.con0] & 0xC7 | u8::from(self.s) << 5 | u8::from(self.mdr) << 3;
        m.mem[self.con0] = c0 & !0x10; // CSTR is client only
        m.mem[self.con1] = m.mem[self.con1] & 0xC0 | u8::from(self.ackstat) << 5 | u8::from(self.p) << 3;
        m.mem[self.stat0] = u8::from(bfre) << 7 | u8::from(self.mma) << 5 | u8::from(self.mma && self.rw) << 4;
        m.mem[self.stat1] = u8::from(self.txwe) << 7 | u8::from(self.txbe) << 5 | u8::from(self.rxre) << 3 | u8::from(self.rxbf);
        m.mem[self.pir] = self.pirv;
        m.mem[self.err] = m.mem[self.err] & 7 | self.errv;
        m.mem[self.cnt[0]] = self.cnt_v as u8;
        m.mem[self.cnt[1]] = (self.cnt_v >> 8) as u8;
        m.mem[self.txb_a] = self.txb;
        m.mem[self.rxb_a] = self.rxb;
        let on = |f: Flag, v: bool, m: &mut DataMem| if v { f.set(m) } else { f.clear(m) };
        on(self.rxif, self.mma && self.rxbf, m);
        on(self.txif, self.mma && self.txbe && self.cnt_v != 0, m);
        on(self.i2cif, self.pirv & m.mem[self.pie] != 0, m);
        on(self.eif, (self.errv >> 4) & m.mem[self.err] & 7 != 0, m);
    }

    fn reset(&mut self) {
        self.abort();
        self.st = St::Idle;
        self.s = false;
        self.mdr = false;
        self.mma = false;
        self.p = false;
        self.txbe = true;
        self.rxbf = false;
        self.bfre_at = None;
        self.sel = None;
        self.expect_addr = false;
        self.read_idx = None;
    }

    pub fn sync(&mut self, m: &mut DataMem, now: u64, ev: &mut Vec<u64>, i2c_log: &mut Vec<(u8, u64, I2cEv)>, notes: &mut BTreeSet<String>) {
        if self.en(m) {
            let mode = m.mem[self.con0] & 7;
            if mode != 4 {
                notes.insert(format!("I2C{} MODE = {mode:03b} (client modes, 10-bit addressing) is not modelled", self.num));
            } else if m.mem[self.clk] & 0x1F != 0 {
                notes.insert(format!("I2C{} I2CxCLK other than FOSC/4 is not modelled; it runs as FOSC/4", self.num));
            }
        }
        let before = (self.pirv, self.errv, self.rxbf, self.txbe, self.mma, self.cnt_v);
        while let Some(&(t, _, e)) = self.q.first() {
            if t > now {
                break;
            }
            self.q.remove(0);
            self.step(m, t, e, i2c_log);
        }
        self.publish(m, now);
        if before != (self.pirv, self.errv, self.rxbf, self.txbe, self.mma, self.cnt_v) {
            ev.push(now);
        }
    }

    fn step(&mut self, m: &mut DataMem, t: u64, e: Ev, log: &mut Vec<(u8, u64, I2cEv)>) {
        let tt = self.t(m);
        match e {
            Ev::Spurious => self.pirv |= 0x05, // SCIF, PCIF
            Ev::TryStart => {
                if self.st != St::StartPending {
                    return;
                }
                self.mma = true;
                self.pirv |= 1;
                self.expect_addr = true;
                self.sel = None;
                log.push((self.num, t, I2cEv::Start));
                self.st = St::Starting;
                self.is_addr = true;
                self.sched(t + tt, Ev::LoadShift);
            }
            Ev::LoadShift => {
                self.s = false;
                self.shift = self.txb;
                self.txbe = true;
                if self.is_addr {
                    self.rw = self.shift & 1 != 0;
                }
                self.st = St::Tx;
                self.sched(t + 8 * tt, Ev::Tx8);
            }
            Ev::RestartFlag => self.pirv |= 2,
            Ev::Tx8 => {
                if self.txbe && self.cnt_v != 0 && !(self.is_addr && self.rw) {
                    self.mdr = true;
                    self.st = St::StallTx;
                    return;
                }
                self.sched(t + tt, Ev::Tx9);
            }
            Ev::Tx9 => {
                let (b, was_addr) = (self.shift, self.is_addr);
                let ack = self.client_write(b, t, log);
                self.ackstat = !ack;
                if !ack {
                    self.errv |= 0x10;
                    if self.rsen(m) {
                        self.mdr = true;
                        self.st = St::PauseRestart;
                    } else {
                        self.begin_stop(t, tt);
                    }
                    return;
                }
                self.is_addr = false;
                if was_addr && self.rw {
                    self.st = St::Rx;
                    self.sched(t + 7 * tt, Ev::Rx7);
                    return;
                }
                if !self.txbe {
                    self.shift = self.txb;
                    self.txbe = true;
                    self.cnt_v = self.cnt_v.saturating_sub(1);
                    self.st = St::Tx;
                    self.sched(t + 8 * tt, Ev::Tx8);
                    return;
                }
                if self.cnt_v == 0 {
                    self.pirv |= 0x80;
                    if self.rsen(m) {
                        self.mdr = true;
                        self.st = St::PauseRestart;
                    } else {
                        self.begin_stop(t, tt);
                    }
                    return;
                }
                self.mdr = true;
                self.st = St::StallTx;
            }
            Ev::Rx7 => {
                if self.rxbf {
                    self.mdr = true;
                    self.st = St::StallRx;
                    return;
                }
                self.sched(t + tt, Ev::Rx8);
            }
            Ev::Rx8 => {
                self.rxb = match self.sel {
                    Some((i, true)) => {
                        let c = &mut self.i2c[i];
                        c.data.pop_front().unwrap_or(c.fill)
                    }
                    _ => 0xFF,
                };
                self.rxbf = true;
                self.cnt_v = self.cnt_v.saturating_sub(1);
                self.read_idx = Some(log.len());
                log.push((self.num, t, I2cEv::Read(self.rxb, true)));
                self.sched(t + tt, Ev::Rx9);
            }
            Ev::Rx9 => {
                let nack = if self.cnt_v == 0 { m.mem[self.con1] & 0x80 != 0 } else { m.mem[self.con1] & 0x40 != 0 };
                if let Some(i) = self.read_idx.take()
                    && let I2cEv::Read(b, _) = log[i].2
                {
                    log[i].2 = I2cEv::Read(b, !nack);
                }
                if nack {
                    self.errv |= 0x10;
                }
                if self.cnt_v == 0 || nack {
                    self.pirv |= 0x80;
                    if self.rsen(m) && self.cnt_v == 0 {
                        self.mdr = true;
                        self.st = St::PauseRestart;
                    } else {
                        self.begin_stop(t, tt);
                    }
                    return;
                }
                self.st = St::Rx;
                self.sched(t + 7 * tt, Ev::Rx7);
            }
            Ev::StopDone => {
                self.mma = false;
                self.mdr = false;
                self.p = false;
                self.s = false;
                self.pirv |= 4;
                self.expect_addr = false;
                self.sel = None;
                log.push((self.num, t, I2cEv::Stop));
                self.st = St::Idle;
                self.bfre_at = Some(t + 8 * self.pre(m));
            }
        }
    }

    fn begin_stop(&mut self, t: u64, tt: u64) {
        self.st = St::Stopping;
        self.sched(t + tt, Ev::StopDone);
    }

    /// A byte the host writes: the address after a Start or Restart, otherwise data. Returns the client's acknowledge.
    fn client_write(&mut self, b: u8, t: u64, log: &mut Vec<(u8, u64, I2cEv)>) -> bool {
        if self.expect_addr {
            self.expect_addr = false;
            self.sel = self.i2c.iter().position(|c| c.address == b >> 1).map(|i| (i, b & 1 != 0));
            let ack = self.sel.is_some();
            log.push((self.num, t, I2cEv::Addr(b, ack)));
            ack
        } else {
            let ack = matches!(self.sel, Some((_, false)));
            log.push((self.num, t, I2cEv::Write(b, ack)));
            ack
        }
    }

    pub fn read_rxb(&mut self, m: &mut DataMem, now: u64) -> u8 {
        if self.rxbf {
            self.rxbf = false;
            if self.st == St::StallRx {
                self.mdr = false;
                self.st = St::Rx;
                let tt = self.t(m);
                self.sched(now + tt, Ev::Rx8);
            }
        } else {
            self.rxre = true; // [A59]
            self.errv |= 0x10;
        }
        self.publish(m, now);
        self.rxb
    }

    fn do_restart(&mut self, m: &mut DataMem, now: u64, log: &mut Vec<(u8, u64, I2cEv)>) {
        let tt = self.t(m);
        self.mdr = false;
        self.s = true;
        self.st = St::Restarting;
        self.is_addr = true;
        self.expect_addr = true;
        self.sel = None;
        log.push((self.num, now, I2cEv::Restart));
        self.sched(now + tt, Ev::RestartFlag);
        self.sched(now + 2 * tt, Ev::LoadShift);
    }

    pub fn write(&mut self, sub: u8, v: u8, now: u64, m: &mut DataMem, log: &mut Vec<(u8, u64, I2cEv)>) {
        let tt = self.t(m);
        match sub {
            CON0 => {
                let (was, en) = (self.en(m), v & 0x80 != 0);
                m.mem[self.con0] = v & 0xC7 | m.mem[self.con0] & 0x28;
                if en && !was {
                    self.bfre_at = Some(now + 8 * self.pre(m));
                    self.sched(now + 3, Ev::Spurious); // [A58]
                } else if was && !en {
                    self.reset();
                }
                if v & 0x20 != 0 && self.mma && self.st == St::PauseRestart {
                    self.do_restart(m, now, log);
                }
            }
            CON1 => {
                m.mem[self.con1] = v & 0xC0 | m.mem[self.con1] & 0x3F;
                if v & 8 != 0 && self.mma {
                    self.p = true; // [A57]
                    self.abort();
                    self.mdr = false;
                    self.st = St::Stopping;
                    self.sched(now + tt, Ev::StopDone);
                }
            }
            STAT0 => {}
            STAT1 => {
                if v & 0x80 == 0 {
                    self.txwe = false;
                }
                if v & 0x08 == 0 {
                    self.rxre = false;
                }
                if v & 4 != 0 {
                    self.txbe = true;
                    self.rxbf = false;
                    if self.st == St::StartPending {
                        self.abort();
                        self.st = St::Idle;
                    }
                }
            }
            PIR => self.pirv = v & 0xDF,
            ERR => {
                self.errv = v & 0x70;
                m.mem[self.err] = m.mem[self.err] & 0x70 | v & 7;
            }
            CNTL => self.cnt_v = self.cnt_v & 0xFF00 | u16::from(v),
            CNTH => self.cnt_v = self.cnt_v & 0xFF | u16::from(v) << 8,
            TXB => {
                if !self.txbe {
                    self.txwe = true; // [A59]
                    self.errv |= 0x10;
                } else {
                    self.txb = v;
                    self.txbe = false;
                    if self.host(m) {
                        match self.st {
                            St::Idle if !self.mma => {
                                self.st = St::StartPending;
                                let at = self.bfre_at.map_or(now, |b| b.max(now)); // [A51]
                                self.sched(at, Ev::TryStart);
                            }
                            St::StallTx => {
                                self.mdr = false;
                                self.st = St::Tx;
                                self.sched(now + tt, Ev::Tx9);
                            }
                            St::PauseRestart => self.do_restart(m, now, log),
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
        self.publish(m, now);
    }
}
