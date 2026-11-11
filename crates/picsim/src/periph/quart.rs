//! UART of the PIC18-Q41 (IP block `DOS-03076_uart_protocol_0_v1`, DS40002214G ch.34 p.519-566), asynchronous modes
//! only: MODE 0000 8-bit, 0001 7-bit, 0010/0011 8-bit with odd/even parity in the 9th bit, 0100 address mode
//! (9th bit = address flag). LIN, DMX, DALI, auto-baud, flow control, checksum, break and wake-up are not modelled
//! (a note is logged when a program turns one on).
//!
//! Rules from the data sheet:
//! * Bit time in instruction cycles (Fosc/4 each; Table 34-2 p.540): BRGS = 1 -> UxBRG+1, BRGS = 0 -> 4(UxBRG+1).
//!   A character is Start + 7/8/9 data bits + 1, 1.5 or 2 Stop bits (STP, 34.8 p.535). `UxBRG` takes a write only
//!   while ON = 0 (34.20.8 note 2).
//! * Transmitter (34.2.1 p.519-520, 34.10 p.536): a write to UxTXB goes straight to the shift register (TSR) when
//!   that is free, else it waits in UxTXB and moves to the TSR at the beginning of the previous character's Stop bit,
//!   and its Start bit follows the end of the previous Stop bits. UxTXIF = ON and TXEN and UxTXB empty; TXBE = UxTXB
//!   empty, TXBF = full, TXWRE is set by a write into a full UxTXB. TXMTIF = TSR empty and idle (set at the end of the
//!   Stop bits), read only. Data written with TXEN = 0 sits in the TSR and is sent when TXEN is set. Writing TXBE = 1
//!   flushes UxTXB and an inactive TSR (a character being shifted finishes). Clearing ON discards everything.
//! * Receiver (34.2.2 p.522-524, 34.8, 34.8.1, 34.9 p.536): two-character FIFO; the character is stored in the middle
//!   of the first Stop bit (second with STP = 10); STPMD = 1 delays that to the end of the last Stop bit. UxRXIF = FIFO
//!   not empty and not suppressed by FERIF with FERIE or PERIF with PERIE. FERIF and PERIF describe the top character.
//!   A character completing with the FIFO full is lost and sets RXFOIF (stays set until software clears it). RXBE = 1
//!   written flushes the FIFO. Clearing ON resets the receiver.
//! Assumptions `[A#]` (not in the data sheet), numbered after those in periph.rs (`docs/FACTS-SIM.md`):
//! * `[A18]` UxTXIF follows a write at once (no one-cycle dip); Figure 34-3 marks "1 TCY" on UxTXIF, which may mean one.
//! * `[A19]` Clearing TXEN lets the character being shifted finish and starts no further one; a character held in the
//!   TSR is sent when TXEN is set again. A write to UxTXB with ON = 0 is stored and ignored.
//! * `[A20]` ON = 0 resets the status flags to their POR values (TXMTIF = 1) and clears TXWRE and both buffers.
//! * `[A21]` A character whose Start bit falls while the receiver is off (ON or RXEN clear, or an unsupported MODE) is
//!   lost; with RXFOIF set and RUNOVF = 0 the receiver is stopped, so such a character is lost too; with RUNOVF = 1 it
//!   is received and discarded. RXIDL is low from the Start bit until the character is stored.
//! * `[A22]` An empty FIFO reads the last character again. Address mode stores every character, the 9th bit in PERIF
//!   (address matching with UxP2/UxP3 is not modelled; the errata DS80000901H 1.5.2 says not to use the mode anyway).
//! * `[A23]` UxTXB writes are dropped when the buffer is full (only TXWRE shows it). In MODE 0001 the top bit is not sent.

use super::{addr, Flag, RxStream};
use crate::mem::DataMem;
use devdesc::Device;
use std::collections::{BTreeSet, VecDeque};

/// Hook sub-codes of one UART (code = base + QUART_HOOKS * index + sub).
pub const TXB: u8 = 0;
/// The only read hook: pops the receive FIFO.
pub const RXB: u8 = 1;
pub const CON0: u8 = 2;
pub const CON1: u8 = 3;
pub const BRGL: u8 = 4;
pub const BRGH: u8 = 5;
pub const FIFO: u8 = 6;
pub const ERRIR: u8 = 7;
pub const P1L: u8 = 8;
pub const HOOKS: u8 = 9;

/// A character received: (data, framing error, parity error or address flag).
type Rx = (u8, bool, bool);

#[derive(Clone)]
pub struct QUart {
    pub num: u8,
    con0: usize,
    con1: usize,
    con2: usize,
    errir: usize,
    errie: usize,
    fifo: usize,
    brgl: usize,
    brgh: usize,
    txb: usize,
    rxb: usize,
    p1l: usize,
    txif: Flag,
    rxif: Flag,
    eif: Option<Flag>,
    /// Character in the shift register: (cycle its first Stop bit begins, cycle its last Stop bit ends).
    cur: Option<(u64, u64)>,
    /// Loaded into the TSR, waiting for the previous character to end or for TXEN.
    next: Option<u16>,
    /// Held in UxTXB.
    pending: Option<u16>,
    rx_in: VecDeque<RxStream>,
    rx_free: u64,
    /// The character in flight: (cycle it reaches the FIFO, word, framing error).
    rx_cur: Option<(u64, u16, bool)>,
    /// The part is in Sleep: the receiver sees nothing.
    asleep: bool,
    rx_fifo: VecDeque<Rx>,
}

impl QUart {
    pub fn find(dev: &Device, n: u8) -> Option<(QUart, [usize; 9])> {
        let a = |s: &str| addr(dev, &format!("U{n}{s}"));
        let (con0, con1, con2, errir, errie, fifo, brgl, brgh, txb, rxb, p1l) =
            (a("CON0")?, a("CON1")?, a("CON2")?, a("ERRIR")?, a("ERRIE")?, a("FIFO")?, a("BRGL")?, a("BRGH")?, a("TXB")?, a("RXB")?, a("P1L")?);
        let q = QUart {
            num: n,
            con0,
            con1,
            con2,
            errir,
            errie,
            fifo,
            brgl,
            brgh,
            txb,
            rxb,
            p1l,
            txif: Flag::find(dev, &format!("U{n}TXIF"))?,
            rxif: Flag::find(dev, &format!("U{n}RXIF"))?,
            eif: Flag::find(dev, &format!("U{n}EIF")),
            cur: None,
            next: None,
            pending: None,
            rx_in: VecDeque::new(),
            rx_free: 0,
            rx_cur: None,
            asleep: false,
            rx_fifo: VecDeque::new(),
        };
        // Registers in hook-sub-code order.
        let regs = [txb, rxb, con0, con1, brgl, brgh, fifo, errir, p1l];
        Some((q, regs))
    }

    pub fn rx_send(&mut self, s: RxStream) {
        self.rx_in.push_back(s);
    }

    fn on(&self, m: &DataMem) -> bool {
        m.mem[self.con1] & 0x80 != 0
    }

    fn mode(&self, m: &DataMem) -> u8 {
        m.mem[self.con0] & 15
    }

    /// Data bits on the wire (parity and address modes: 8 + the 9th bit).
    fn data_bits(&self, m: &DataMem) -> u64 {
        match self.mode(m) {
            1 => 7,
            2..=4 => 9,
            _ => 8,
        }
    }

    /// Half bit times of the Stop bits: 1, 1.5, 2, 2.
    fn stop_halves(&self, m: &DataMem) -> u64 {
        match m.mem[self.con2] >> 4 & 3 {
            0 => 2,
            1 => 3,
            _ => 4,
        }
    }

    /// Instruction cycles one bit takes.
    fn bit_cycles(&self, m: &DataMem) -> u64 {
        let n = u64::from(m.mem[self.brgh]) << 8 | u64::from(m.mem[self.brgl]);
        if m.mem[self.con0] & 0x80 != 0 { n + 1 } else { 4 * (n + 1) }
    }

    /// Cycles for `halves` half bit times.
    fn span(&self, m: &DataMem, halves: u64) -> u64 {
        self.bit_cycles(m).saturating_mul(halves) / 2
    }

    /// The character is complete when the TSR moved it out: put the TSR's character on the line at cycle `t`.
    fn start_next(&mut self, m: &DataMem, t: u64, tx: &mut Vec<(u8, u64, u8)>) {
        let Some(w) = self.next.take() else { return };
        let mask = if self.mode(m) == 1 { 0x7F } else { 0xFF };
        tx.push((self.num, t, w as u8 & mask));
        let stop = self.stop_halves(m);
        let end = t + self.span(m, 2 * (1 + self.data_bits(m)) + stop);
        self.cur = Some((end - self.span(m, stop), end));
    }

    fn send(&mut self, m: &mut DataMem, w: u16, now: u64, tx: &mut Vec<(u8, u64, u8)>) {
        if !self.on(m) {
            return;
        }
        if self.pending.is_some() {
            m.mem[self.fifo] |= 0x80; // TXWRE
            return;
        }
        let tsr_free = self.next.is_none() && self.cur.is_none_or(|(ss, _)| now >= ss);
        if !tsr_free {
            self.pending = Some(w);
            return;
        }
        self.next = Some(w);
        if self.cur.is_none() && m.mem[self.con0] & 0x20 != 0 {
            self.start_next(m, now, tx);
        }
    }

    /// The part slept for `d` cycles with the module's clock stopped: the character in the shift register is `d` cycles later.
    pub fn shift(&mut self, d: u64) {
        if let Some(c) = &mut self.cur {
            (c.0, c.1) = (c.0 + d, c.1 + d);
        }
    }

    /// Entering Sleep: the character being received is lost, and until the wake-up the receiver sees nothing.
    pub fn sleep(&mut self, on: bool) {
        self.asleep = on;
        if on {
            self.rx_cur = None;
        }
    }

    /// In Sleep the stimulus goes on; the characters it sends are lost.
    pub fn sync_asleep(&mut self, m: &mut DataMem, now: u64, ev: &mut Vec<u64>) {
        self.rx(m, now, ev);
    }

    /// Everything ON = 0 holds in reset.
    fn reset(&mut self, m: &mut DataMem) {
        self.cur = None;
        self.next = None;
        self.pending = None;
        self.rx_cur = None;
        self.rx_fifo.clear();
        m.mem[self.errir] = 0x80;
        m.mem[self.fifo] &= 0x40; // TXWRE cleared, STPMD is a setting
    }

    /// Status bits that follow the state: UxFIFO, UxERRIR (TXMTIF, FERIF, PERIF), UxTXIF, UxRXIF, UxEIF.
    fn bits(&self, m: &mut DataMem) {
        let on = self.on(m);
        let busy = self.cur.is_some() || self.next.is_some() || self.pending.is_some();
        let front = self.rx_fifo.front().copied();
        let mut f = m.mem[self.fifo] & 0xC0 | 0x04; // TXWRE, STPMD kept; XON = 1
        if self.pending.is_none() {
            f |= 0x20;
        } else {
            f |= 0x10;
        }
        if self.rx_cur.is_none() {
            f |= 0x08;
        }
        f |= match self.rx_fifo.len() {
            0 => 2,
            1 => 0,
            _ => 1,
        };
        m.mem[self.fifo] = f;
        let mut e = m.mem[self.errir] & !0xC8;
        e |= u8::from(!busy) << 7 | u8::from(front.is_some_and(|r| r.2)) << 6 | u8::from(front.is_some_and(|r| r.1)) << 3;
        m.mem[self.errir] = e;
        let ie = m.mem[self.errie];
        if on && m.mem[self.con0] & 0x20 != 0 && self.pending.is_none() { self.txif.set(m) } else { self.txif.clear(m) }
        let suppressed = e & ie & 0x48 != 0; // FERIF with FERIE, PERIF with PERIE
        if !self.rx_fifo.is_empty() && !suppressed { self.rxif.set(m) } else { self.rxif.clear(m) }
        if let Some(f) = self.eif {
            if e & ie != 0 { f.set(m) } else { f.clear(m) }
        }
    }

    pub fn sync(&mut self, m: &mut DataMem, now: u64, ev: &mut Vec<u64>, tx: &mut Vec<(u8, u64, u8)>, notes: &mut BTreeSet<String>) {
        let n = self.num;
        if self.on(m) {
            if self.mode(m) > 4 {
                notes.insert(format!("UART{n} MODE {} (LIN, DMX or DALI) is not modelled; nothing is sent or received", self.mode(m)));
            }
            if m.mem[self.con0] & 0x40 != 0 {
                notes.insert(format!("UART{n} auto-baud (ABDEN) is not modelled"));
            }
            if m.mem[self.con2] & 3 != 0 || m.mem[self.con2] & 8 != 0 {
                notes.insert(format!("UART{n} flow control and checksum (FLO, C0EN) are not modelled"));
            }
            if m.mem[self.con1] & 0x1B != 0 {
                notes.insert(format!("UART{n} break, wake-up and RXBIMD (SENDB, BRKOVR, WUE) are not modelled"));
            }
            if self.mode(m) <= 4 {
                let txen = m.mem[self.con0] & 0x20 != 0;
                while let Some((ss, end)) = self.cur {
                    if self.next.is_none() && self.pending.is_some() && ss <= now {
                        self.next = self.pending.take();
                        ev.push(ss);
                    }
                    if end > now {
                        break;
                    }
                    self.cur = None;
                    if self.next.is_some() && txen {
                        self.start_next(m, end, tx); // the next Start bit follows the last Stop bit
                    } else {
                        ev.push(end);
                    }
                }
            }
            self.rx(m, now, ev);
        } else {
            self.reset(m);
            // Stimulus characters sent while the port is off are lost, as on the wire.
            self.rx(m, now, ev);
        }
        self.bits(m);
    }

    fn rx_on(&self, m: &DataMem) -> bool {
        !self.asleep && self.on(m) && m.mem[self.con0] & 0x10 != 0 && self.mode(m) <= 4
    }

    /// Receiver: start the next queued character when the line is free, finish the one in flight.
    fn rx(&mut self, m: &mut DataMem, now: u64, ev: &mut Vec<u64>) {
        loop {
            if let Some((end, word, fe)) = self.rx_cur {
                if end > now {
                    return;
                }
                self.rx_cur = None;
                if !self.rx_on(m) {
                    continue;
                }
                let full = self.rx_fifo.len() >= 2 || m.mem[self.errir] & 2 != 0;
                if full {
                    m.mem[self.errir] |= 2; // RXFOIF
                    ev.push(end);
                    continue;
                }
                let data = word as u8;
                let ninth = word & 0x100 != 0;
                let (data, pe) = match self.mode(m) {
                    1 => (data & 0x7F, false),
                    2 | 3 => {
                        let ones = data.count_ones() + u32::from(ninth);
                        (data, (ones % 2 == 0) == (self.mode(m) == 2)) // odd parity wants an odd count
                    }
                    4 => (data, ninth),
                    _ => (data, false),
                };
                self.rx_fifo.push_back((data, fe, pe));
                ev.push(end);
                continue;
            }
            let Some(st) = self.rx_in.front_mut() else { return };
            let start = st.at.max(self.rx_free);
            if start > now {
                return;
            }
            let Some(word) = st.words.pop_front() else {
                self.rx_in.pop_front();
                continue;
            };
            let own = self.bit_cycles(m);
            let bit = self.rx_in.front().and_then(|s| s.bit).unwrap_or(own);
            let st = self.rx_in.front_mut().unwrap();
            let sent_bits = if st.nine { 11 } else { 10 };
            self.rx_free = bit.saturating_mul(sent_bits).saturating_add(start).saturating_add(st.gap);
            st.at = self.rx_free;
            let (bad, nine) = (st.bad_stop, st.nine);
            let stopped = m.mem[self.errir] & 2 != 0 && m.mem[self.con2] & 0x80 == 0;
            if self.rx_on(m) && !stopped {
                let d = self.data_bits(m);
                let word = if d == 9 && nine { word & 0x1FF } else { word & if d == 7 { 0x7F } else { 0xFF } };
                let con2 = m.mem[self.con2];
                let first = 2 * (1 + d);
                let halves = if m.mem[self.fifo] & 0x40 != 0 {
                    first + if con2 >> 4 & 3 == 3 { 2 } else { self.stop_halves(m) } // STPMD: end of the last Stop bit
                } else if con2 >> 4 & 3 == 2 {
                    first + 3 // middle of the second Stop bit
                } else {
                    first + 1 // middle of the first Stop bit
                };
                self.rx_cur = Some((start.saturating_add(bit.saturating_mul(halves) / 2), word, bad));
            }
        }
    }

    pub fn read_rxb(&mut self, m: &mut DataMem) -> u8 {
        if let Some((d, _, _)) = self.rx_fifo.pop_front() {
            m.mem[self.rxb] = d;
        }
        self.bits(m);
        m.mem[self.rxb]
    }

    pub fn write(&mut self, sub: u8, v: u8, now: u64, m: &mut DataMem, tx: &mut Vec<(u8, u64, u8)>) {
        match sub {
            TXB => {
                m.mem[self.txb] = v;
                self.send(m, u16::from(v), now, tx);
            }
            P1L => {
                m.mem[self.p1l] = v;
                if self.mode(m) == 4 {
                    self.send(m, 0x100 | u16::from(v), now, tx);
                }
            }
            CON0 => {
                m.mem[self.con0] = v;
                if self.on(m) && v & 0x20 != 0 && self.cur.is_none() && self.next.is_some() {
                    self.start_next(m, now, tx);
                }
            }
            CON1 => {
                let was = self.on(m);
                m.mem[self.con1] = v;
                if was && v & 0x80 == 0 {
                    self.reset(m);
                }
            }
            BRGL | BRGH => {
                if !self.on(m) {
                    m.mem[if sub == BRGL { self.brgl } else { self.brgh }] = v;
                }
            }
            FIFO => {
                // TXWRE and STPMD are written; TXBE = 1 flushes the transmit side, RXBE = 1 the receive FIFO.
                m.mem[self.fifo] = v & 0xC0 | m.mem[self.fifo] & 0x3F;
                if v & 0x20 != 0 {
                    self.pending = None;
                    if self.cur.is_none() {
                        self.next = None;
                    }
                }
                if v & 2 != 0 {
                    self.rx_fifo.clear();
                }
            }
            ERRIR | _ => {
                // ERRIR: TXMTIF, FERIF and PERIF follow the state; the others take the written value.
                m.mem[self.errir] = v & !0xC8 | m.mem[self.errir] & 0xC8;
            }
        }
        self.bits(m);
    }
}
