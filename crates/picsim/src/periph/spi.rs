//! SPI of the PIC18-Q41 (IP block `DOS-02941_spi_8bit_v1`, DS40002214G ch.35 p.569-601), host mode only. Client mode
//! (MST = 0) is not modelled and logs a note. The outside world is a byte-level client given by the stimulus file
//! (`[[spi]]`): it answers every exchange with the next byte of its list (then `fill`) and the bytes the host sends are
//! logged. Chip select (SS_out, SSET, the transfer counter driving it), SS_in and PPS routing are not modelled.
//!
//! Sourced rules:
//! * Transfers by TXR/RXR (Table 35-1, 35.3 p.575-577): full duplex (1,1) transfers when the RxFIFO is not full and the
//!   TxFIFO is not empty (BMODE = 1) and also when the transfer counter is nonzero (BMODE = 0); transmit only (1,0)
//!   when the TxFIFO is not empty (and, BMODE = 0, the counter is nonzero), received data are not stored; receive only
//!   (0,1) when the RxFIFO is not full and the counter is nonzero, transmitting the top of the TxFIFO without removing
//!   it, or the most recently received byte when it is empty; (0,0) no transfers, TXB writes still fill the TxFIFO.
//! * Counter (SPIxTCNT 35.7.6, SPIxTWIDTH 35.7.7 p.594-595): BMODE = 1: TCNT is a byte count that decrements with each
//!   transfer and wraps below zero; every byte is TWIDTH bits (0 = 8). BMODE = 0: the total bit count is
//!   TCNT:TWIDTH (TCNT = bits 13-3, TWIDTH = bits 2-0), bytes are 8 bits and the last takes the remainder.
//!   TCZIF is set when the counter decrements to zero (35.7.12 p.600).
//! * SCK = clock / (2 (BAUD + 1)) (Equation 35-1 p.579); start delay at least half a baud period unless FST (35.3.6.3
//!   p.581-582). SPIxCON1, SPIxBAUD and SPIxCON0 (but EN) are not changed while EN = 1 (35.2.1 p.571).
//! * Flags: SPIxRXIF = RxFIFO not empty, SPIxTXIF = TxFIFO not full (both read only, 35.6.1-2 p.586); SPIxIF = any
//!   SPIxINTF bit with its SPIxINTE bit (35.2 p.571). STATUS: TXBE, RXBF, TXWE (write while the TxFIFO is full), RXRE
//!   (read of an empty RxFIFO, returns 0, 35.7.9 p.597), CLB empties both FIFOs (35.7.8 p.596).
//!   RXOIF: a transfer completed with the RxFIFO full and RXR = 1; SRMTIF: transfer complete (35.7.12 p.600).
//! Assumptions `[A#]`, not in the data sheet:
//! * `[A40]` Both FIFOs are two bytes deep (Figure 35-3 shows occupancy up to 2; the depth is not stated).
//! * `[A41]` A byte leaves the TxFIFO and enters the RxFIFO when its transfer ends (the figures show the occupancy
//!   changing at the end of the byte).
//! * `[A42]` Only the clock source FOSC (CLKSEL = 0) is modelled (others log a note and run as FOSC). Start delay
//!   `ceil((BAUD + 1 + 2) / 4)` instruction cycles (half a baud period plus two clocks of synchronisation), 1 cycle
//!   with FST; back to back bytes have no gap. A byte of w bits takes `ceil(w (BAUD + 1) / 2)` cycles.
//! * `[A43]` Bytes narrower than 8 bits use the low w bits of TXB and RXB. SDIP inverts what the client sends back.
//!   The client answers a whole byte at once, so LSBF and CKP/CKE/SMP do not change the data.
//! * `[A44]` A TXB write with EN = 0 is dropped.

use super::{addr, Flag};
use crate::mem::DataMem;
use devdesc::Device;
use std::collections::{BTreeSet, VecDeque};

pub const RXB: u8 = 0;
pub const TXB: u8 = 1;
pub const TCNTL: u8 = 2;
pub const TCNTH: u8 = 3;
pub const CON0: u8 = 4;
pub const CON1: u8 = 5;
pub const CON2: u8 = 6;
pub const STATUS: u8 = 7;
pub const TWIDTH: u8 = 8;
pub const BAUD: u8 = 9;
pub const INTF: u8 = 10;
pub const INTE: u8 = 11;
pub const HOOKS: u8 = 12;

const DEPTH: usize = 2;

/// What the outside world answers: the next byte of `data`, then `fill`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpiClient {
    pub data: VecDeque<u8>,
    pub fill: u8,
}

pub struct Spi {
    pub num: u8,
    rxb: usize,
    txb: usize,
    tcnt: [usize; 2],
    con0: usize,
    con1: usize,
    con2: usize,
    status: usize,
    twidth: usize,
    baud: usize,
    intf: usize,
    inte: usize,
    clk: usize,
    rxif: Flag,
    txif: Flag,
    spiif: Flag,
    tx: VecDeque<u8>,
    rx: VecDeque<u8>,
    /// The transfer in flight: (end cycle, byte sent, byte received, bits).
    cur: Option<(u64, u8, u8, u8)>,
    /// Cycle the shift register is free again, and the cycle software last gave it something to do.
    free: u64,
    armed: u64,
    /// A transfer just ended and the shift register has not been idle since (the next byte follows at once).
    chained: bool,
    last_rx: u8,
    pub client: SpiClient,
}

impl Spi {
    pub fn find(dev: &Device, n: u8) -> Option<(Spi, [usize; 12])> {
        let a = |s: &str| addr(dev, &format!("SPI{n}{s}"));
        let s = Spi {
            num: n,
            rxb: a("RXB")?,
            txb: a("TXB")?,
            tcnt: [a("TCNTL")?, a("TCNTH")?],
            con0: a("CON0")?,
            con1: a("CON1")?,
            con2: a("CON2")?,
            status: a("STATUS")?,
            twidth: a("TWIDTH")?,
            baud: a("BAUD")?,
            intf: a("INTF")?,
            inte: a("INTE")?,
            clk: a("CLK")?,
            rxif: Flag::find(dev, &format!("SPI{n}RXIF"))?,
            txif: Flag::find(dev, &format!("SPI{n}TXIF"))?,
            spiif: Flag::find(dev, &format!("SPI{n}IF"))?,
            tx: VecDeque::new(),
            rx: VecDeque::new(),
            cur: None,
            free: 0,
            armed: 0,
            chained: false,
            last_rx: 0,
            client: SpiClient::default(),
        };
        let regs = [s.rxb, s.txb, s.tcnt[0], s.tcnt[1], s.con0, s.con1, s.con2, s.status, s.twidth, s.baud, s.intf, s.inte];
        Some((s, regs))
    }

    fn en(&self, m: &DataMem) -> bool {
        m.mem[self.con0] & 0x80 != 0
    }

    fn bmode(&self, m: &DataMem) -> bool {
        m.mem[self.con0] & 1 != 0
    }

    fn tcnt_val(&self, m: &DataMem) -> u32 {
        u32::from(m.mem[self.tcnt[0]]) | u32::from(m.mem[self.tcnt[1]] & 7) << 8
    }

    /// BMODE = 0: total bits left, TCNT:TWIDTH.
    fn bits_left(&self, m: &DataMem) -> u32 {
        self.tcnt_val(m) << 3 | u32::from(m.mem[self.twidth] & 7)
    }

    fn set_bits_left(&self, m: &mut DataMem, n: u32) {
        let c = n >> 3;
        m.mem[self.tcnt[0]] = c as u8;
        m.mem[self.tcnt[1]] = (c >> 8) as u8 & 7;
        m.mem[self.twidth] = (n & 7) as u8;
    }

    fn reset(&mut self, m: &mut DataMem) {
        self.tx.clear();
        self.rx.clear();
        self.cur = None;
        m.mem[self.con2] &= 0x3F; // BUSY, SSFLT
        m.mem[self.status] &= 0x20;
        m.mem[self.intf] = 0;
    }

    fn bits(&self, m: &mut DataMem) {
        m.mem[self.status] = m.mem[self.status] & 0x88 | u8::from(self.tx.is_empty()) << 5 | u8::from(self.rx.len() >= DEPTH);
        m.mem[self.con2] = m.mem[self.con2] & !0x80 | u8::from(self.cur.is_some()) << 7;
        let on = self.en(m);
        if on && !self.rx.is_empty() { self.rxif.set(m) } else { self.rxif.clear(m) }
        if on && self.tx.len() < DEPTH { self.txif.set(m) } else { self.txif.clear(m) }
        if m.mem[self.intf] & m.mem[self.inte] != 0 { self.spiif.set(m) } else { self.spiif.clear(m) }
    }

    /// Start and finish transfers up to cycle `now`; `log` collects (SPI number, start cycle, byte sent).
    pub fn sync(&mut self, m: &mut DataMem, now: u64, ev: &mut Vec<u64>, log: &mut Vec<(u8, u64, u8)>, notes: &mut BTreeSet<String>) {
        if !self.en(m) {
            self.bits(m);
            return;
        }
        if m.mem[self.con0] & 2 == 0 {
            notes.insert(format!("SPI{} client mode (MST = 0) is not modelled", self.num));
            self.bits(m);
            return;
        }
        if m.mem[self.clk] & 15 != 0 {
            notes.insert(format!("SPI{} clock source other than FOSC is not modelled; it runs as FOSC", self.num));
        }
        loop {
            if let Some((end, sent, got, bits)) = self.cur {
                if end > now {
                    break;
                }
                self.cur = None;
                self.free = end;
                self.chained = true;
                let con2 = m.mem[self.con2];
                let (txr, rxr) = (con2 & 2 != 0, con2 & 1 != 0);
                if txr {
                    self.tx.pop_front();
                }
                if rxr {
                    if self.rx.len() < DEPTH {
                        self.rx.push_back(got);
                    } else {
                        m.mem[self.intf] |= 4; // RXOIF
                    }
                }
                self.last_rx = got;
                // Counter: bytes (BMODE = 1, wraps) or bits (BMODE = 0).
                if self.bmode(m) {
                    let c = self.tcnt_val(m).wrapping_sub(1) & 0x7FF;
                    m.mem[self.tcnt[0]] = c as u8;
                    m.mem[self.tcnt[1]] = (c >> 8) as u8;
                    if c == 0 {
                        m.mem[self.intf] |= 0x40;
                    }
                } else {
                    let left = self.bits_left(m).saturating_sub(u32::from(bits));
                    self.set_bits_left(m, left);
                    if left == 0 {
                        m.mem[self.intf] |= 0x40;
                    }
                }
                let _ = sent;
                ev.push(end);
                if !self.can_start(m) {
                    m.mem[self.intf] |= 0x80; // SRMTIF
                }
                continue;
            }
            if !self.can_start(m) {
                self.chained = false;
                break;
            }
            let idle = !(self.chained && self.armed <= self.free);
            self.chained = false;
            let t0 = self.free.max(self.armed);
            if t0 > now {
                break;
            }
            let n = u64::from(m.mem[self.baud]) + 1;
            let delay = if idle { if m.mem[self.con1] & 0x10 != 0 { 1 } else { (n + 2).div_ceil(4) } } else { 0 };
            let start = t0 + delay;
            let con2 = m.mem[self.con2];
            let bits = self.next_bits(m);
            let mask = if bits >= 8 { 0xFF } else { (1u8 << bits) - 1 };
            let sent = if con2 & 2 != 0 { self.tx.front().copied().unwrap_or(0) } else { self.tx.front().copied().unwrap_or(self.last_rx) } & mask;
            let mut got = self.client.data.pop_front().unwrap_or(self.client.fill);
            if m.mem[self.con1] & 2 != 0 {
                got = !got; // SDIP
            }
            let end = start + (u64::from(bits) * n).div_ceil(2);
            log.push((self.num, start, sent));
            m.mem[self.intf] &= !0x80; // SRMTIF clears when a transfer starts
            self.cur = Some((end, sent, got & mask, bits));
        }
        self.bits(m);
    }

    /// Bits the next transfer carries.
    fn next_bits(&self, m: &DataMem) -> u8 {
        if self.bmode(m) {
            match m.mem[self.twidth] & 7 {
                0 => 8,
                w => w,
            }
        } else {
            self.bits_left(m).clamp(1, 8) as u8
        }
    }

    fn can_start(&self, m: &DataMem) -> bool {
        let con2 = m.mem[self.con2];
        let (txr, rxr) = (con2 & 2 != 0, con2 & 1 != 0);
        let counted = if self.bmode(m) { self.tcnt_val(m) != 0 } else { self.bits_left(m) != 0 };
        let tx_ok = !self.tx.is_empty();
        let rx_ok = self.rx.len() < DEPTH;
        match (txr, rxr) {
            (true, true) => tx_ok && rx_ok && (self.bmode(m) || counted),
            (true, false) => tx_ok && (self.bmode(m) || counted),
            (false, true) => rx_ok && counted,
            _ => false,
        }
    }

    pub fn read_rxb(&mut self, m: &mut DataMem, now: u64) -> u8 {
        let v = match self.rx.pop_front() {
            Some(v) => v,
            None => {
                m.mem[self.status] |= 8; // RXRE
                0
            }
        };
        self.armed = self.armed.max(now);
        self.bits(m);
        v
    }

    pub fn write(&mut self, sub: u8, v: u8, now: u64, m: &mut DataMem) {
        let en = self.en(m);
        match sub {
            TXB => {
                m.mem[self.txb] = v;
                if !en {
                    return;
                }
                if self.tx.len() >= DEPTH {
                    m.mem[self.status] |= 0x80; // TXWE
                } else {
                    self.tx.push_back(v);
                    self.armed = now;
                }
            }
            TCNTL | TCNTH => {
                if self.cur.is_some() {
                    return;
                }
                m.mem[self.tcnt[usize::from(sub == TCNTH)]] = v;
                self.armed = now;
            }
            TWIDTH => {
                if self.cur.is_none() {
                    m.mem[self.twidth] = v;
                    self.armed = now;
                }
            }
            CON0 => {
                let old = m.mem[self.con0];
                m.mem[self.con0] = if en { v & 0x80 | old & 0x7F } else { v };
                if en && v & 0x80 == 0 {
                    self.reset(m);
                } else if !en && v & 0x80 != 0 {
                    self.armed = now;
                }
            }
            CON1 | BAUD => {
                if !en {
                    m.mem[if sub == CON1 { self.con1 } else { self.baud }] = v;
                }
            }
            CON2 => {
                if self.cur.is_none() {
                    m.mem[self.con2] = v & 7 | m.mem[self.con2] & 0xC0;
                    self.armed = now;
                }
            }
            STATUS => {
                // TXWE and RXRE clear by software; CLB empties the FIFOs and reads back 0.
                m.mem[self.status] &= v | 0x77;
                if v & 4 != 0 {
                    self.tx.clear();
                    self.rx.clear();
                }
            }
            INTF => {
                m.mem[self.intf] = v & 0xF6;
            }
            INTE => m.mem[self.inte] = v & 0xF6,
            _ => {}
        }
        self.bits(m);
    }
}
