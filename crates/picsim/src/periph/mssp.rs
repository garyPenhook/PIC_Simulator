//! MSSP of the PIC18-Q10 (DS40002043F ch.28 p.440-501), SPI host and I2C host modes. Slave modes, the firmware controlled
//! I2C host (SSPM = 1011) and bus collisions (multi-master) are not modelled and log a note. The outside world is given by
//! the stimulus file: a byte-level SPI client (`[[spi]]`, answers every exchanged byte with the next byte of its list, then
//! `fill`) and I2C clients on the bus (`[[i2c]]`, by 7-bit address). The bytes and conditions the host puts on the bus are
//! logged. The bus has pull-ups and no other host: an address nobody owns is not acknowledged and a read from nobody gives FFh.
//! PPS, TRIS and the pins are not modelled (the module runs once SSPEN is set).
//!
//! Sourced rules, SPI host (SSPM = 0000-0011, 1010):
//! * The transfer starts when SSPxBUF is written (28.2.1 p.445); a write while a byte is shifting is ignored and sets WCOL
//!   (28.2 p.444). After eight bits the byte moves to SSPxBUF, BF and SSPxIF are set, and a read of SSPxBUF clears BF (p.444).
//!   A write to SSPxBUF also loads SSPxBUF (28.1.1 p.442). SSPOV is not used in host mode (SSPxCON1 note 1 p.494).
//! * SCK = FOSC/4, FOSC/16, FOSC/64, TMR2 output/2, or FOSC/(4 (SSPxADD + 1)) (28.2.1 p.445, SSPxCON1 p.494); SSPxADD = 0
//!   is not supported. The sample and edge bits (SMP, CKE, CKP) do not change the exchanged byte.
//!
//! Sourced rules, I2C host (SSPM = 1000):
//! * SCL = FOSC / (4 (SSPxADD + 1)) (Equation 28-1 p.489): one SCL period is SSPxADD + 1 instruction cycles, TBRG (a half
//!   period) is (SSPxADD + 1) / 2 cycles. SSPxADD below 3 is not valid (28.7 p.488-489, noted and run anyway).
//! * No queuing: SEN, RSEN, PEN, RCEN, ACKEN are not set while the module is active, and SSPxBUF is not written (WCOL)
//!   (28.6 p.474, 28.6.3 p.475, SSPxCON2 note 2 p.496).
//! * Start: TBRG, SDA low (S set), TBRG, SEN clears and SSPxIF is set (28.6.4 p.476). Stop: SDA low, TBRG, SCL high, TBRG,
//!   SDA high (P set), TBRG, PEN clears and SSPxIF is set (28.6.9 p.482). Repeated start: SDA high TBRG, SCL high, both
//!   high TBRG, SDA low TBRG, RSEN clears and SSPxIF is set (28.6.5 p.476). S and P are cleared by a disabled module (28.6 p.474).
//! * Transmit: the write sets BF; the eight bits shift out one SCL period each; BF clears after the eighth bit; ACKSTAT is
//!   written on the ninth clock (0 = acknowledged); SSPxIF is set after the ninth clock (28.6.6 p.477-478). STAT.R/W is 1 while
//!   a transmit is in progress (SSPxSTAT p.492).
//! * Receive (RCEN): after the eighth bit the byte moves to SSPxBUF, RCEN clears, BF and SSPxIF are set (28.6.7 p.479). SSPOV is
//!   set when eight bits arrive while BF is set (28.6.7.2 p.480); with BOEN = 0 SSPxBUF is updated only while SSPOV is clear
//!   (SSPxCON3 p.498). ACKEN sends ACKDT in one SCL period, then ACKEN clears and SSPxIF is set (28.6.8 p.482).
//!
//! Assumptions `[A#]`, not in the data sheet:
//! * `[A45]` The SPI transfer starts in the cycle of the write to SSPxBUF; a byte takes 8 bit times (FOSC/4 = 1, FOSC/16 = 4,
//!   FOSC/64 = 16, FOSC/(4 (SSPxADD + 1)) = SSPxADD + 1 instruction cycles). Timer2 as the clock (SSPM = 0011) is not
//!   modelled and runs as FOSC/4.
//! * `[A46]` The I2C sequences take k TBRG, rounded up to whole cycles: Start 2 (S at 1), Repeated start 3 (S at 2), Stop 3 (P at
//!   2), a byte sent 18 (BF clears at 16, ACKSTAT at 17), a byte received 16, Acknowledge 2. The data sheet gives the steps but no counts.
//! * `[A47]` With several bits of SSPxCON2 written at once the lowest of SEN, RSEN, PEN, RCEN, ACKEN starts and the rest are dropped.
//! * `[A48]` The first overflow (BF set, SSPOV clear, BOEN = 0) still updates SSPxBUF, then SSPOV is set; the data sheet says
//!   SSPxBUF is updated only when SSPOV is clear.
//! * `[A49]` An I2C client acknowledges its address and every byte written to it, a read of the last byte is up to the host's NACK.
//!   Clearing SSPEN or changing SSPM aborts a running operation and clears BF, S, P and R/W; WCOL and SSPOV stay.
//! * `[A50]` An SSPxBUF write with SSPEN = 0 only fills SSPxBUF.

use super::{addr, Flag};
use devdesc::Device;
use std::collections::{BTreeSet, VecDeque};

pub const BUF: u8 = 0;
pub const STAT: u8 = 1;
pub const CON1: u8 = 2;
pub const CON2: u8 = 3;
pub const ADD: u8 = 4;
pub const CON3: u8 = 5;
pub const HOOKS: u8 = 6;

pub use super::spi::SpiClient;

/// An I2C client on the bus: 7-bit address, the bytes it sends when read (then `fill`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct I2cClient {
    pub address: u8,
    pub data: VecDeque<u8>,
    pub fill: u8,
    /// 0 = from the stimulus file; n = board part n-1 (`board.rs`).
    pub tag: u8,
    /// The part is not wired as the code expects: it does not answer.
    pub absent: bool,
}

/// What the host put on the I2C bus. `ack` is the receiver's answer: the client for an address or a byte written, the host for a byte read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2cEv {
    Start,
    Restart,
    /// Address byte as sent (7-bit address << 1 | R/W).
    Addr(u8, bool),
    Write(u8, bool),
    Read(u8, bool),
    Stop,
}

#[derive(Debug, Clone, Copy)]
enum Step {
    /// SPI byte done: (byte sent, cycle it started).
    SpiDone(u8, u64),
    /// S set (restart: it is a repeated start).
    StartS(bool),
    StopP,
    TxBf,
    TxAck(u8),
    TxDone,
    RxDone,
    AckDone(bool),
    /// Clear the SSPxCON2 enable bit (mask) and set SSPxIF.
    Finish(u8),
}

#[derive(Clone)]
pub struct Mssp {
    pub num: u8,
    buf: usize,
    add: usize,
    stat: usize,
    con1: usize,
    con2: usize,
    con3: usize,
    sspif: Flag,
    steps: VecDeque<(u64, Step)>,
    pub spi: SpiClient,
    pub i2c: Vec<I2cClient>,
    /// The next byte after a Start is an address.
    expect_addr: bool,
    /// Client addressed (index into `i2c`) and whether the host reads from it.
    sel: Option<(usize, bool)>,
    /// Index in the I2C log of the byte just read, to put the host's acknowledge in.
    read_idx: Option<usize>,
}

impl Mssp {
    /// Something is under way that `sync` must follow cycle by cycle (`Periph::tick` does not skip a sync meanwhile).
    pub fn busy(&self) -> bool {
        !self.steps.is_empty()
    }

    pub fn find(dev: &Device, n: u8) -> Option<(Mssp, [usize; 6])> {
        let a = |s: &str| addr(dev, &format!("SSP{n}{s}"));
        let s = Mssp {
            num: n,
            buf: a("BUF")?,
            add: a("ADD")?,
            stat: a("STAT")?,
            con1: a("CON1")?,
            con2: a("CON2")?,
            con3: a("CON3")?,
            sspif: Flag::find(dev, &format!("SSP{n}IF"))?,
            steps: VecDeque::new(),
            spi: SpiClient::default(),
            i2c: Vec::new(),
            expect_addr: false,
            sel: None,
            read_idx: None,
        };
        let regs = [s.buf, s.stat, s.con1, s.con2, s.add, s.con3];
        Some((s, regs))
    }

    /// The SSPxIF flag: register address and mask.
    pub fn sspif(&self) -> (usize, u8) {
        (self.sspif.addr, self.sspif.mask)
    }

    /// A reset of the part: whatever was in flight is gone (the registers take their reset values elsewhere).
    pub fn abort(&mut self) {
        self.steps.clear();
        self.expect_addr = false;
        self.sel = None;
        self.read_idx = None;
    }

    fn en(&self, m: &[u8]) -> bool {
        m[self.con1] & 0x20 != 0
    }

    /// The part slept for `d` cycles with the module's clock stopped: everything in flight is `d` cycles later.
    pub fn shift(&mut self, d: u64) {
        for (t, s) in &mut self.steps {
            *t += d;
            if let Step::SpiDone(_, start) = s {
                *start += d;
            }
        }
    }

    /// Cycles of `k` TBRG from `t`.
    fn tbrg(&self, m: &[u8], t: u64, k: u64) -> u64 {
        t + (k * (u64::from(m[self.add]) + 1)).div_ceil(2)
    }

    /// Apply the steps due up to cycle `now`.
    pub fn sync(&mut self, m: &mut [u8], now: u64, ev: &mut Vec<u64>, spi_log: &mut Vec<(u8, u64, u8)>, i2c_log: &mut Vec<(u8, u64, I2cEv)>, notes: &mut BTreeSet<String>) {
        if !self.en(m) {
            return;
        }
        let mode = m[self.con1] & 15;
        if !matches!(mode, 0..=3 | 8 | 10) {
            notes.insert(format!("MSSP{} mode SSPM = {mode:04b} (slave, firmware controlled host) is not modelled", self.num));
            return;
        }
        while let Some(&(t, st)) = self.steps.front() {
            if t > now {
                break;
            }
            self.steps.pop_front();
            match st {
                Step::SpiDone(sent, start) => {
                    let got = self.spi.data.pop_front().unwrap_or(self.spi.fill);
                    spi_log.push((self.num, start, sent));
                    m[self.buf] = got;
                    m[self.stat] |= 1;
                    self.sspif.set_in(m);
                    ev.push(t);
                }
                Step::StartS(restart) => {
                    m[self.stat] = m[self.stat] & !0x10 | 8;
                    self.expect_addr = true;
                    self.sel = None;
                    i2c_log.push((self.num, t, if restart { I2cEv::Restart } else { I2cEv::Start }));
                }
                Step::StopP => {
                    m[self.stat] = m[self.stat] & !8 | 0x10;
                    self.expect_addr = false;
                    self.sel = None;
                    i2c_log.push((self.num, t, I2cEv::Stop));
                }
                Step::TxBf => m[self.stat] &= !1,
                Step::TxAck(b) => {
                    let ack = self.client_write(b, t, i2c_log);
                    m[self.con2] = m[self.con2] & !0x40 | u8::from(!ack) << 6;
                }
                Step::TxDone => {
                    m[self.stat] &= !4;
                    self.sspif.set_in(m);
                    ev.push(t);
                }
                Step::RxDone => {
                    let b = match self.sel {
                        Some((i, true)) => {
                            let c = &mut self.i2c[i];
                            c.data.pop_front().unwrap_or(c.fill)
                        }
                        _ => 0xFF,
                    };
                    let (bf, ov, boen) = (m[self.stat] & 1 != 0, m[self.con1] & 0x40 != 0, m[self.con3] & 0x10 != 0);
                    if bf {
                        m[self.con1] |= 0x40; // SSPOV
                    }
                    if !ov || boen {
                        m[self.buf] = b;
                    }
                    m[self.stat] |= 1;
                    m[self.con2] &= !8;
                    self.sspif.set_in(m);
                    ev.push(t);
                    self.read_idx = Some(i2c_log.len());
                    i2c_log.push((self.num, t, I2cEv::Read(b, true)));
                }
                Step::AckDone(nack) => {
                    if let Some(i) = self.read_idx.take()
                        && let I2cEv::Read(b, _) = i2c_log[i].2
                    {
                        i2c_log[i].2 = I2cEv::Read(b, !nack);
                    }
                    m[self.con2] &= !0x10;
                    self.sspif.set_in(m);
                    ev.push(t);
                }
                Step::Finish(bit) => {
                    m[self.con2] &= !bit;
                    self.sspif.set_in(m);
                    ev.push(t);
                }
            }
        }
    }

    /// A byte the host writes: the address after a Start, otherwise data. Returns the client's acknowledge.
    fn client_write(&mut self, b: u8, t: u64, log: &mut Vec<(u8, u64, I2cEv)>) -> bool {
        if self.expect_addr {
            self.expect_addr = false;
            self.sel = self.i2c.iter().position(|c| c.address == b >> 1 && !c.absent).map(|i| (i, b & 1 != 0));
            let ack = self.sel.is_some();
            log.push((self.num, t, I2cEv::Addr(b, ack)));
            ack
        } else {
            let ack = matches!(self.sel, Some((_, false)));
            log.push((self.num, t, I2cEv::Write(b, ack)));
            ack
        }
    }

    pub fn read_buf(&mut self, m: &mut [u8]) -> u8 {
        m[self.stat] &= !1;
        m[self.buf]
    }

    pub fn write(&mut self, sub: u8, v: u8, now: u64, m: &mut [u8], notes: &mut BTreeSet<String>) {
        let en = self.en(m);
        let busy = !self.steps.is_empty();
        match sub {
            BUF => {
                if !en {
                    m[self.buf] = v; // [A50]
                    return;
                }
                match m[self.con1] & 15 {
                    0..=3 | 10 => {
                        if busy {
                            m[self.con1] |= 0x80; // WCOL
                            return;
                        }
                        m[self.buf] = v;
                        let cycles = match m[self.con1] & 15 {
                            1 => 4,
                            2 => 16,
                            3 => {
                                notes.insert(format!("MSSP{} SPI clock TMR2 output/2 is not modelled; it runs as FOSC/4", self.num));
                                1
                            }
                            10 => {
                                if m[self.add] == 0 {
                                    notes.insert(format!("MSSP{} SSPxADD = 0 is not supported as SPI clock (runs as 1)", self.num));
                                }
                                u64::from(m[self.add]) + 1
                            }
                            _ => 1,
                        };
                        self.steps.push_back((now + 8 * cycles, Step::SpiDone(v, now)));
                    }
                    8 => {
                        if busy {
                            m[self.con1] |= 0x80;
                            return;
                        }
                        m[self.buf] = v;
                        m[self.stat] |= 5; // BF, R/W
                        let t = |k| self.tbrg(m, now, k);
                        let steps = [(t(16), Step::TxBf), (t(17), Step::TxAck(v)), (t(18), Step::TxDone)];
                        self.steps.extend(steps);
                    }
                    _ => m[self.buf] = v,
                }
            }
            STAT => m[self.stat] = m[self.stat] & 0x3F | v & 0xC0,
            CON1 => {
                let old = m[self.con1];
                // WCOL and SSPOV are set by hardware only; software can clear them.
                m[self.con1] = v & 0x3F | old & v & 0xC0;
                if (old & 0x20 != 0 && v & 0x20 == 0) || (old ^ v) & 15 != 0 {
                    // SSPEN cleared or SSPM changed: abort, clear BF, S, P, R/W. [A49]
                    self.steps.clear();
                    m[self.stat] &= 0xC0;
                    m[self.con2] &= !0x1F;
                    self.expect_addr = false;
                    self.sel = None;
                    self.read_idx = None;
                }
            }
            CON2 => {
                if !(en && m[self.con1] & 15 == 8) {
                    m[self.con2] = v;
                    return;
                }
                if busy {
                    // The five enable bits are locked while active.
                    m[self.con2] = v & 0xE0 | m[self.con2] & 0x1F;
                    return;
                }
                if m[self.add] < 3 {
                    notes.insert(format!("MSSP{} SSPxADD below 3 is not valid for the I2C baud rate generator", self.num));
                }
                let x = v & 0x1F;
                let bit = x & x.wrapping_neg(); // lowest set bit [A47]
                m[self.con2] = v & 0xE0 | bit;
                let t = |k| self.tbrg(m, now, k);
                let steps: Vec<(u64, Step)> = match bit {
                    0x01 => vec![(t(1), Step::StartS(false)), (t(2), Step::Finish(0x01))],
                    0x02 => vec![(t(2), Step::StartS(true)), (t(3), Step::Finish(0x02))],
                    0x04 => vec![(t(2), Step::StopP), (t(3), Step::Finish(0x04))],
                    0x08 => vec![(t(16), Step::RxDone)],
                    0x10 => vec![(t(2), Step::AckDone(v & 0x20 != 0))],
                    _ => vec![],
                };
                self.steps.extend(steps);
            }
            ADD => m[self.add] = v,
            CON3 => m[self.con3] = v,
            _ => {}
        }
    }
}
