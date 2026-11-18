//! MSSP of the PIC18-Q10 (DS40002043F ch.28 p.440-501), SPI host and I2C host modes. Slave modes, the firmware controlled
//! I2C host (SSPM = 1011) and bus collisions (multi-master) are not modelled and log a note. The outside world is given by
//! the stimulus file: a byte-level SPI client (`[[spi]]`, answers every exchanged byte with the next byte of its list, then
//! `fill`) and I2C clients on the bus (`[[i2c]]`, by 7-bit address). The bytes and conditions the host puts on the bus are
//! logged. The bus has pull-ups and no other host: an address nobody owns is not acknowledged and a read from nobody gives FFh.
//! The owner routes the host waveforms through PPS/TRIS and supplies sampled SPI input bits.
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
//!
//! I2C client (slave) mode, 7-bit addressing (SSPM = 0110, 1110), driven by an external host from the stimulus file
//! (`[[i2c_host]]`, byte level; SCL / SDA waveforms are put on the pins on PIC18 / K22 only (`[A164]`), no bus arbitration; 10-bit modes, general call and Sleep wake-up are not modelled):
//! * The first byte after a Start/Restart is compared with SSPxADD under SSPxMSK on A[7:1] (28.5.1.1 p.457, 28.5.9 p.474); a
//!   mismatch leaves the module idle without any indication (28.5.1 p.456). A match loads the byte into SSPxBUF (BF), copies R/W
//!   and clears D/A, acknowledges, and sets SSPxIF on the ninth falling edge (28.5.2 p.457, Figure 28-14 p.459, SSPxSTAT p.491-492).
//!   Overflow (BF or SSPOV set) gives NACK (28.5.2 p.457, 28.4.11 p.456); with BOEN = 0 SSPxBUF is updated only while SSPOV is clear
//!   (SSPxCON3 p.498). The same holds for data bytes (D/A = 1).
//! * SEN = 1 stretches the clock after each acknowledged received byte until CKP is set (28.5.2 p.457, Figure 28-15 p.460). A
//!   matching read address clears CKP after the ACK, whatever SEN or BF (28.5.3 p.463, 28.5.6.1 p.472).
//! * Transmit: software writes SSPxBUF (BF), sets CKP; BF clears after the eighth falling edge, ACKSTAT takes the host's answer on
//!   the ninth rising edge, SSPxIF is set on the ninth falling edge; after an ACK CKP is cleared again, after a NACK the module goes
//!   idle (28.5.3 p.463-466, Figure 28-18 p.465). WCOL is set by an SSPxBUF write while BF is set (SSPxCON1 p.493).
//! * AHEN / DHEN: after the eighth falling edge of a matching address / of a data byte CKP is cleared, ACKTIM is set and SSPxIF is
//!   set; the ACK is then ACKDT, given when CKP is set; SSPxIF is set again after an ACK, not after a NACK; ACKTIM clears on the
//!   ninth rising edge (28.5.2.2 p.458, Figure 28-16 p.461, SSPxCON3 p.497-498).
//! * SSPM = 1110 sets SSPxIF on every Start and Stop; with SSPM = 0110 SCIE / PCIE enable it (SSPxCON3 p.497, 28.4.10 p.456).
//! * Clearing CKP holds SCL low only once the host has pulled it low (28.5.7 p.472): the host never starts a byte, Restart or Stop
//!   while CKP = 0 (a client left with CKP = 0 stalls the bus, as in 28.5.6).
//! * `[A160]` The host script runs only while SSPEN = 1 with SSPM = 0110 or 1110, from cycle `at` of the stimulus or the enable if
//!   later; clearing SSPEN or changing SSPM drops the operation in flight and the rest of the script waits for the next enable.
//! * `[A161]` Host timing: SCL period `period` instruction cycles (default 40); a Start or Stop takes half a period, a byte 9 periods
//!   (eight data bits then the acknowledge clock); a held clock resumes one cycle after CKP is set.
//! * `[A162]` An overflowed received byte still sets SSPxIF on the ninth falling edge (the data sheet only says an interrupt is
//!   generated for each transferred byte); BF = 1 with SSPOV = 0 and BOEN = 0 updates SSPxBUF, then sets SSPOV, as `[A48]`; any
//!   overflow is NACKed whatever BOEN. With AHEN / DHEN software owns the ACK and overflow does not change it.
//! * `[A164]` The pins show the bus: SCL low from the host between bits (rising in the middle of the bit, falling at its end), SDA the
//!   data bit of whoever sends, the ninth clock with SDA low when the receiver acknowledges, Start / Stop as SDA edges while SCL is high.
//!   Both are open-drain (the board needs a pull-up, `[[pin]] level = 1`); with no host script they stay released. A clock stretched by
//!   CKP is SCL left low, so the next edge simply comes later. With SSPEN set the module owns both pins (TRISx must be input, LATx is ignored) and reads nothing back; the PIC16 core runs the host at byte level without pin waveforms.
//! * `[A165]` An outside device holding SCL low (a `[[pin]]` driving the SCL pin low) stops the host's clock: the rest of its planned
//!   waveform moves later by the time SCL stays held (seen at instruction boundaries, so to about an instruction). Held data (SDA)
//!   is not read back and there is no arbitration; a pulse shorter than the instruction it falls in is not seen. PIC18 / K22 only (PIC16 has no pin waveforms).
//! * `[A163]` A read the host makes of a client that is not addressed for reading returns FFh and touches no register.

use crate::log::Log;
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

/// One thing the external I2C host does (`[[i2c_host]]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOp {
    Start,
    Restart,
    Stop,
    /// A byte the host sends (the address byte included, 8 bits).
    Write(u8),
    /// A byte the host reads; `true` = the host acknowledges it.
    Read(bool),
}

/// An external I2C host that talks to the MSSP in client mode, byte level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2cHost {
    pub ops: VecDeque<HostOp>,
    /// SCL period in instruction cycles.
    pub period: u64,
    /// The script does not start before this instruction cycle.
    pub at: u64,
}

impl Default for I2cHost {
    fn default() -> Self { I2cHost { ops: VecDeque::new(), period: 40, at: 0 } }
}

/// Where the host is in its current operation. `due` of the host says when the next phase happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ph {
    /// Take the next operation.
    Next,
    /// The Repeated start / Stop condition happens now.
    Cond,
    StopEv,
    /// The eighth falling edge of a byte the host writes. `addr` = address byte.
    W8 { b: u8, addr: bool },
    /// Clock held after the eighth edge (AHEN / DHEN) until CKP is set.
    WHold { b: u8, addr: bool, mine: bool },
    /// Ninth rising / falling edge of a written byte. `mine` = the module took part; `ack` = SDA low on the ninth clock.
    W9r { b: u8, addr: bool, mine: bool, ack: bool, held: bool },
    W9f { b: u8, addr: bool, mine: bool, ack: bool, held: bool },
    /// Eighth falling edge, ninth rising and falling edges of a byte the host reads.
    R8 { b: u8, mine: bool, ack: bool },
    R9r { b: u8, mine: bool, ack: bool },
    R9f { b: u8, mine: bool, ack: bool },
}

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
    /// Register memory mode: first write byte selects an 8-bit pointer, later bytes write sequentially.
    pub memory: Option<Box<[u8; 256]>>,
    pub readonly: bool,
    pub pointer: u8,
    pub pointer_pending: bool,
}

impl I2cClient {
    pub fn addressed(&mut self, read: bool) { self.pointer_pending = !read; }

    pub fn write_byte(&mut self, byte: u8) {
        let Some(memory) = &mut self.memory else { return; };
        if self.pointer_pending { self.pointer = byte; self.pointer_pending = false; }
        else {
            if !self.readonly { memory[usize::from(self.pointer)] = byte; }
            self.pointer = self.pointer.wrapping_add(1);
        }
    }

    pub fn read_byte(&mut self) -> u8 {
        if let Some(memory) = &self.memory {
            let byte = memory[usize::from(self.pointer)];
            self.pointer = self.pointer.wrapping_add(1);
            byte
        } else { self.data.pop_front().unwrap_or(self.fill) }
    }
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
    pub byte_client: bool,
    pub pin_received: u8,
    // Fosc ticks, SCK/SCL and SDO/SDA. Retained through the following pin-consumer scan.
    wire: VecDeque<(u64, bool, bool)>,
    pin_paused: Option<u64>,
    samples: VecDeque<u64>,
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
    read_idx: Option<u64>,
    msk: Option<usize>,
    /// The external host that drives the client mode, and its progress.
    pub host: Option<I2cHost>,
    ph: Ph,
    hdue: Option<u64>,
    /// The host waits for CKP.
    hblock: bool,
    harmed: bool,
    /// Client mode: addressed by the host, true = the host reads.
    cl_sel: Option<bool>,
    /// Client mode: SCL is held low from outside; the cycle up to which the host's schedule has been stalled.
    held: Option<u64>,
}

impl Mssp {
    pub fn enabled(&self, m: &[u8]) -> bool { self.en(m) }
    /// I2C host or client mode: the module owns SCL / SDA as open-drain pins.
    pub fn i2c_pins(&self, m: &[u8]) -> bool { self.en(m) && matches!(m[self.con1] & 15, 6 | 8 | 14) }
    pub fn freeze_pins(&mut self, now: u64) { self.pin_paused = Some(4 * now); }
    pub fn pin_level(&self, tick: u64) -> (bool, bool) {
        let tick = self.pin_paused.map_or(tick, |t| tick.min(t));
        self.wire.iter().rev().find(|e| e.0 <= tick).map_or((false, false), |e| (e.1, e.2))
    }
    pub fn wire_times(&self, from: u64, to: u64) -> impl Iterator<Item=u64> + '_ {
        self.wire.iter().map(|e| e.0).filter(move |&t| from < t && t <= to)
    }
    pub fn pin_times(&self, from: u64, to: u64) -> impl Iterator<Item=u64> + '_ {
        self.wire.iter().map(|e| (e.0 + 3) / 4 + 1).filter(move |&t| from < t && t <= to)
    }
    pub fn take_samples(&mut self, now: u64) -> Vec<u64> {
        let mut times = Vec::new();
        while self.samples.front().is_some_and(|&t| t <= now) { times.push(self.samples.pop_front().unwrap()); }
        times
    }
    fn wire_at(&mut self, tick: u64, clk: bool, data: bool) {
        self.wire.push_back((tick, clk, data));
        while self.wire.len() > 512 { self.wire.pop_front(); }
    }
    fn wire_byte(&mut self, now: u64, period: u64, byte: u8, idle: bool, cke: bool, ack: Option<bool>) {
        let start = 4 * now;
        for n in 0..8u64 {
            let data = byte & (0x80 >> n) != 0;
            let t = start + n * 4 * period;
            self.wire_at(t, idle, if cke { data } else { self.pin_level(t).1 });
            self.wire_at(t + 2 * period, !idle, data);
            self.wire_at(t + 4 * period, idle, data);
        }
        if let Some(ack) = ack {
            let t = start + 32 * period;
            self.wire_at(t, false, !ack);
            self.wire_at(t + 2 * period, true, !ack);
            self.wire_at(t + 4 * period, false, true);
        }
        while self.wire.len() > 512 { self.wire.pop_front(); }
    }

    /// Something is under way that `sync` must follow cycle by cycle (`Periph::tick` does not skip a sync meanwhile).
    pub fn busy(&self) -> bool {
        !self.steps.is_empty() || self.hdue.is_some()
    }

    pub fn find(dev: &Device, n: u8) -> Option<(Mssp, [usize; 6])> {
        let a = |s: &str| addr(dev, &format!("SSP{n}{s}"));
        let s = Mssp {
            num: n, byte_client: false, pin_received: 0, wire: VecDeque::new(), pin_paused: None, samples: VecDeque::new(),
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
            msk: addr(dev, &format!("SSP{n}MSK")),
            host: None, ph: Ph::Next, hdue: None, hblock: false, harmed: false, cl_sel: None, held: None,
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
        self.steps.clear(); self.wire.clear(); self.samples.clear();
        self.expect_addr = false;
        self.sel = None;
        self.read_idx = None;
        self.host_stop();
    }

    /// The module no longer listens: the host operation in flight is gone, the rest of the script waits.
    fn host_stop(&mut self) {
        self.held = None;
        self.ph = Ph::Next; self.hdue = None; self.hblock = false; self.harmed = false; self.cl_sel = None;
    }

    fn en(&self, m: &[u8]) -> bool {
        m[self.con1] & 0x20 != 0
    }

    /// The part slept for `d` cycles with the module's clock stopped: everything in flight is `d` cycles later.
    pub fn shift(&mut self, d: u64) {
        self.pin_paused = None;
        for e in &mut self.wire { e.0 += 4 * d; }
        for t in &mut self.samples { *t += d; }
        if let Some(t) = &mut self.hdue { *t += d; }
        for (t, s) in &mut self.steps {
            *t += d;
            if let Step::SpiDone(_, start) = s {
                *start += d;
            }
        }
    }

    pub fn has_host(&self) -> bool { self.host.is_some() }
    /// Client mode: `low` says an outside device holds SCL low at cycle `now` (`[A165]`). The host's clock stops while it does:
    /// everything it has planned after the last look moves later by the time that passed. Call before `sync`.
    pub fn scl_hold(&mut self, now: u64, low: bool) {
        if self.host.is_none() { self.held = None; return; }
        match self.held {
            // a step that fell due before the hold was seen waits with the rest: it moves with the schedule, not behind it
            None if low => { self.held = Some(now); if let Some(t) = &mut self.hdue { *t = (*t).max(now); } }
            Some(p) => {
                let d = now.saturating_sub(p);
                if d != 0 {
                    for e in &mut self.wire { if e.0 > 4 * p { e.0 += 4 * d; } }
                    if let Some(t) = &mut self.hdue { *t += d; }
                }
                self.held = low.then_some(now);
            }
            None => {}
        }
    }

    /// Cycles of `k` TBRG from `t`.
    fn tbrg(&self, m: &[u8], t: u64, k: u64) -> u64 {
        t + (k * (u64::from(m[self.add]) + 1)).div_ceil(2)
    }

    /// Apply the steps due up to cycle `now`.
    pub fn sync(&mut self, m: &mut [u8], now: u64, ev: &mut Vec<u64>, spi_log: &mut Log<(u8, u64, u8)>, i2c_log: &mut Log<(u8, u64, I2cEv)>, notes: &mut BTreeSet<String>) {
        if !self.en(m) {
            return;
        }
        let mode = m[self.con1] & 15;
        if matches!(mode, 6 | 14) {
            if self.host.is_none() {
                notes.insert(format!("MSSP{} I2C client mode is modelled only with an [[i2c_host]] stimulus (byte level on PIC16, with pin waveforms on PIC18 / K22); nothing drives the bus", self.num));
            }
            while self.held.is_none() && let Some(t) = self.hdue && t <= now {
                self.hdue = None;
                self.host_step(m, t, ev, i2c_log);
            }
            return;
        }
        if !matches!(mode, 0..=3 | 8 | 10) {
            notes.insert(format!("MSSP{} mode SSPM = {mode:04b} (10-bit client, firmware controlled host, SPI client) is not modelled", self.num));
            return;
        }
        while let Some(&(t, st)) = self.steps.front() {
            if t > now {
                break;
            }
            self.steps.pop_front();
            match st {
                Step::SpiDone(sent, start) => {
                    let got = if self.byte_client { self.spi.data.pop_front().unwrap_or(self.spi.fill) } else { self.pin_received };
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
                            c.read_byte()
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
                    self.read_idx = Some(i2c_log.total());
                    i2c_log.push((self.num, t, I2cEv::Read(b, true)));
                }
                Step::AckDone(nack) => {
                    if let Some(i) = self.read_idx.take()
                        && let Some(e) = i2c_log.get_abs_mut(i)
                        && let I2cEv::Read(b, _) = e.2
                    {
                        e.2 = I2cEv::Read(b, !nack);
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


    /// Half an SCL period in cycles.
    fn half(&self) -> u64 { (self.host.as_ref().map_or(40, |h| h.period) / 2).max(1) }
    fn period(&self) -> u64 { self.host.as_ref().map_or(40, |h| h.period).max(2) }

    fn set_ckp(&mut self, m: &mut [u8], on: bool) { if on { m[self.con1] |= 0x10 } else { m[self.con1] &= !0x10 } }

    fn addr_match(&self, m: &[u8], b: u8) -> bool {
        let mask = self.msk.map_or(0xFF, |a| m[a]);
        (b ^ m[self.add]) & mask & 0xFE == 0
    }

    /// A byte arrives from the host: load SSPxBUF per the overflow rules. Returns true when it is acknowledged by hardware.
    fn load_rx(&mut self, m: &mut [u8], b: u8) -> bool {
        let (bf, ov, boen) = (m[self.stat] & 1 != 0, m[self.con1] & 0x40 != 0, m[self.con3] & 0x10 != 0);
        if !ov || boen { m[self.buf] = b; m[self.stat] |= 1; }
        if bf { m[self.con1] |= 0x40; }
        !(bf || ov)
    }

    /// One phase of the external host at cycle `t`.
    fn host_step(&mut self, m: &mut [u8], t: u64, ev: &mut Vec<u64>, log: &mut Log<(u8, u64, I2cEv)>) {
        let (period, half) = (self.period(), self.half());
        let mode = m[self.con1] & 15;
        let ckp = m[self.con1] & 0x10 != 0;
        match self.ph {
            Ph::Next => {
                let Some(&op) = self.host.as_ref().and_then(|h| h.ops.front()) else { return; };
                // Start can happen on an idle bus; everything else needs SCL released by the client.
                if !ckp && op != HostOp::Start { self.hblock = true; return; }
                self.host.as_mut().unwrap().ops.pop_front();
                match op {
                    HostOp::Start => self.cond(m, t, false, ev, log),
                    HostOp::Restart => {
                        // SDA released, SCL released, then the Start.
                        self.wire_at(4 * t, false, true);
                        self.wire_at(4 * t + 2 * half, true, true);
                        self.ph = Ph::Cond; self.hdue = Some(t + half);
                    }
                    HostOp::Stop => {
                        // SDA pulled low with SCL low, SCL released, then SDA released.
                        self.wire_at(4 * t, false, false);
                        self.wire_at(4 * t + 2 * half, true, false);
                        self.ph = Ph::StopEv; self.hdue = Some(t + half);
                    }
                    HostOp::Write(b) => {
                        let addr = self.expect_addr;
                        self.wire_bits(t, period, b);
                        self.ph = Ph::W8 { b, addr };
                        self.hdue = Some(t + 8 * period);
                    }
                    HostOp::Read(ack) => {
                        let mine = self.cl_sel == Some(true);
                        let b = if mine { m[self.buf] } else { 0xFF };
                        self.wire_bits(t, period, b);
                        self.ph = Ph::R8 { b, mine, ack };
                        self.hdue = Some(t + 8 * period);
                    }
                }
            }
            Ph::Cond => self.cond(m, t, true, ev, log),
            Ph::StopEv => {
                self.cl_sel = None; self.expect_addr = false;
                m[self.stat] = m[self.stat] & !8 | 0x10;
                if mode == 14 || m[self.con3] & 0x40 != 0 { self.sspif.set_in(m); ev.push(t); }
                log.push((self.num, t, I2cEv::Stop));
                self.wire_at(4 * t, true, true);
                self.ph = Ph::Next;
                self.hdue = Some(t + half);
            }
            Ph::W8 { b, addr } => {
                let (ahen, dhen) = (m[self.con3] & 2 != 0, m[self.con3] & 1 != 0);
                let mine = if addr { self.addr_match(m, b) } else { matches!(self.cl_sel, Some(false)) };
                if !mine {
                    if addr { self.cl_sel = None; }
                    self.expect_addr = false;
                    self.wire_ack(t, half, false);
                    self.ph = Ph::W9r { b, addr, mine: false, ack: false, held: false };
                    self.hdue = Some(t + half);
                    return;
                }
                self.expect_addr = false;
                let acked = self.load_rx(m, b);
                if addr { m[self.stat] = m[self.stat] & !0x24 | (b & 1) << 2; self.cl_sel = Some(b & 1 != 0); }
                else { m[self.stat] |= 0x20; }
                if (addr && ahen) || (!addr && dhen) {
                    m[self.con3] |= 0x80;
                    self.set_ckp(m, false);
                    self.sspif.set_in(m);
                    ev.push(t);
                    self.ph = Ph::WHold { b, addr, mine };
                    self.hblock = true;
                } else {
                    if addr && !acked { self.cl_sel = None; }
                    self.wire_ack(t, half, acked);
                    self.ph = Ph::W9r { b, addr, mine, ack: acked, held: false };
                    self.hdue = Some(t + half);
                }
            }
            Ph::WHold { b, addr, mine } => {
                // CKP was set: the acknowledge is ACKDT.
                let ack = m[self.con2] & 0x20 == 0;
                if addr && !ack { self.cl_sel = None; }
                self.wire_ack(t, half, ack);
                self.ph = Ph::W9r { b, addr, mine, ack, held: true };
                self.hdue = Some(t + half);
            }
            Ph::W9r { b, addr, mine, ack, held } => {
                if held { m[self.con3] &= !0x80; }
                self.ph = Ph::W9f { b, addr, mine, ack, held };
                self.hdue = Some(t + half);
            }
            Ph::W9f { b, addr, mine, ack, held } => {
                log.push((self.num, t, if addr { I2cEv::Addr(b, ack) } else { I2cEv::Write(b, ack) }));
                if mine && (!held || ack) { self.sspif.set_in(m); ev.push(t); }
                if mine && ack && (self.cl_sel == Some(true) || m[self.con2] & 1 != 0) { self.set_ckp(m, false); }
                self.ph = Ph::Next;
                self.hdue = Some(t);
            }
            Ph::R8 { b, mine, ack } => {
                self.wire_ack(t, half, ack);
                if mine { m[self.stat] = m[self.stat] & !1 | 0x20; }
                self.ph = Ph::R9r { b, mine, ack };
                self.hdue = Some(t + half);
            }
            Ph::R9r { b, mine, ack } => {
                if mine { m[self.con2] = m[self.con2] & !0x40 | u8::from(!ack) << 6; }
                self.ph = Ph::R9f { b, mine, ack };
                self.hdue = Some(t + half);
            }
            Ph::R9f { b, mine, ack } => {
                log.push((self.num, t, I2cEv::Read(b, ack)));
                if mine {
                    self.sspif.set_in(m); ev.push(t);
                    if ack { self.set_ckp(m, false); } else { self.cl_sel = None; }
                }
                self.ph = Ph::Next;
                self.hdue = Some(t);
            }
        }
    }

    /// A Start or Repeated start seen by the client at cycle `t`; the host goes on half a period later.
    fn cond(&mut self, m: &mut [u8], t: u64, restart: bool, ev: &mut Vec<u64>, log: &mut Log<(u8, u64, I2cEv)>) {
        m[self.stat] = m[self.stat] & !0x10 | 8;
        self.expect_addr = true;
        self.cl_sel = None;
        if m[self.con1] & 15 == 14 || m[self.con3] & 0x20 != 0 { self.sspif.set_in(m); ev.push(t); }
        log.push((self.num, t, if restart { I2cEv::Restart } else { I2cEv::Start }));
        // SDA falls while SCL is high; SCL follows half a period later.
        self.wire_at(4 * t, true, false);
        self.wire_at(4 * (t + self.half()), false, false);
        self.ph = Ph::Next;
        self.hdue = Some(t + self.half());
    }

    /// The eight data bits of a byte on the bus (whoever sends them) from cycle `t`, SCL low between bits, and the last falling edge.
    fn wire_bits(&mut self, t: u64, period: u64, byte: u8) {
        for n in 0..8u64 {
            let d = byte & (0x80 >> n) != 0;
            let s = 4 * (t + n * period);
            self.wire_at(s, false, d);
            self.wire_at(s + 2 * period, true, d);
        }
        self.wire_at(4 * (t + 8 * period), false, byte & 1 != 0);
    }

    /// The ninth clock from the eighth falling edge at `t`: the receiver pulls SDA low to acknowledge.
    fn wire_ack(&mut self, t: u64, half: u64, ack: bool) {
        self.wire_at(4 * t, false, !ack);
        self.wire_at(4 * (t + half), true, !ack);
        self.wire_at(4 * (t + 2 * half), false, true);
    }

    /// CKP was written 1 while the host waited for it.
    fn host_resume(&mut self, now: u64) {
        if self.hblock { self.hblock = false; self.hdue = Some(now + 1); }
    }

    /// The module is switched on in a client mode: the host script starts (not before its `at`).
    fn host_arm(&mut self, now: u64) {
        if self.harmed { return; }
        if let Some(h) = &self.host {
            self.harmed = true;
            self.ph = Ph::Next;
            self.hdue = Some(h.at.max(now));
        }
    }

    /// A byte the host writes: the address after a Start, otherwise data. Returns the client's acknowledge.
    fn client_write(&mut self, b: u8, t: u64, log: &mut Log<(u8, u64, I2cEv)>) -> bool {
        if self.expect_addr {
            self.expect_addr = false;
            self.sel = self.i2c.iter().position(|c| c.address == b >> 1 && !c.absent).map(|i| (i, b & 1 != 0));
            if let Some((i, read)) = self.sel { self.i2c[i].addressed(read); }
            let ack = self.sel.is_some();
            log.push((self.num, t, I2cEv::Addr(b, ack)));
            ack
        } else {
            let ack = matches!(self.sel, Some((_, false)));
            if let Some((i, false)) = self.sel { self.i2c[i].write_byte(b); }
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
                        self.pin_received = 0;
                        self.samples = (1..=8).map(|n| now + n * cycles).collect();
                        self.wire_byte(now, cycles, v, m[self.con1] & 0x10 != 0, m[self.stat] & 0x40 != 0, None);
                        self.steps.push_back((now + 8 * cycles, Step::SpiDone(v, now)));
                    }
                    8 => {
                        if busy {
                            m[self.con1] |= 0x80;
                            return;
                        }
                        m[self.buf] = v;
                        m[self.stat] |= 5; // BF, R/W
                        let ack = if self.expect_addr { self.i2c.iter().any(|c| c.address == v >> 1 && !c.absent) } else { matches!(self.sel, Some((_, false))) };
                        self.wire_byte(now, u64::from(m[self.add]) + 1, v, false, true, Some(ack));
                        let t = |k| self.tbrg(m, now, k);
                        let steps = [(t(16), Step::TxBf), (t(17), Step::TxAck(v)), (t(18), Step::TxDone)];
                        self.steps.extend(steps);
                    }
                    6 | 14 => {
                        if self.cl_sel == Some(true) {
                            if m[self.stat] & 1 != 0 {
                                m[self.con1] |= 0x80; // WCOL: still holds the previous word
                                return;
                            }
                            m[self.stat] |= 1;
                        }
                        m[self.buf] = v;
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
                    self.wire.clear(); self.samples.clear();
                    m[self.stat] &= 0xC0;
                    m[self.con2] &= !0x1F;
                    self.expect_addr = false;
                    self.sel = None;
                    self.read_idx = None;
                    self.host_stop();
                }
                if old & 0x10 == 0 && v & 0x10 != 0 { self.host_resume(now); }
                if v & 0x20 != 0 && matches!(v & 15, 6 | 14) { self.host_arm(now); }
                if v & 0x20 != 0 && self.wire.is_empty() { self.wire_at(now * 4, matches!(v & 15, 6 | 8 | 14) || v & 0x10 != 0, matches!(v & 15, 6 | 8 | 14)); }
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
                let half = (u64::from(m[self.add]) + 1) * 2;
                let start = now * 4;
                match bit {
                    1 => { self.wire_at(start, true, true); self.wire_at(start + half, true, false); self.wire_at(start + 2 * half, false, false); }
                    2 => { self.wire_at(start, false, true); self.wire_at(start + half, true, true); self.wire_at(start + 2 * half, true, false); self.wire_at(start + 3 * half, false, false); }
                    4 => { self.wire_at(start, false, false); self.wire_at(start + half, true, false); self.wire_at(start + 2 * half, true, true); }
                    8 => {
                        let byte = self.sel.filter(|s| s.1).map(|(i, _)| {
                            let c = &self.i2c[i];
                            c.memory.as_ref().map_or_else(|| c.data.front().copied().unwrap_or(c.fill), |mem| mem[usize::from(c.pointer)])
                        }).unwrap_or(0xFF);
                        self.wire_byte(now, u64::from(m[self.add]) + 1, byte, false, true, None);
                    }
                    16 => { let data = v & 0x20 != 0; self.wire_at(start, false, data); self.wire_at(start + half, true, data); self.wire_at(start + 2 * half, false, true); }
                    _ => {}
                }
                self.steps.extend(steps);
            }
            ADD => m[self.add] = v,
            CON3 => m[self.con3] = v & 0x7F | m[self.con3] & 0x80, // ACKTIM is hardware only
            _ => {}
        }
    }
}
