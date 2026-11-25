//! UART1 to UART3 of the dsPIC33CK (the "UART with protocol support" module, DS70005288): asynchronous 8 or 9 data bits,
//! the transmit and receive FIFOs, baud rate generator, status flags and interrupt requests. A transmitted character is
//! recorded in `Periph::tx`; characters for the receiver come from `[[uart]]` entries of a stimulus file.
//! Sources: DS70005399D Registers 15-1 to 15-5, DS70005288C 4.0.

use crate::dspic::*;
use crate::periph::RxStream;
use std::collections::VecDeque;

/// Register base, then (TX, RX, error, event) interrupt request numbers.
const UARTS: [(usize, [usize; 4]); 3] = [(0x238, [12, 11, 49, 189]), (0x260, [28, 27, 50, 190]), (0xF00, [58, 57, 56, 191])];
const FIFO: usize = 8;

#[derive(Clone, Default)]
pub struct UartState {
    pub tx_fifo: VecDeque<u16>,
    /// the character in the shifter and the cycle it finishes
    pub shifting: Option<(u16, u64)>,
    pub rx_fifo: VecDeque<(u16, bool)>,
    pub oerr: bool,
    pub ferr: bool,
    pub txcif: bool,
    pub queue: VecDeque<RxStream>,
    /// the cycle the character being received ends at, with its value and framing
    pub receiving: Option<(u64, u16, bool)>,
    pub was_on: bool,
}

impl Dspic {
    fn u_base(n: usize) -> usize {
        UARTS[n].0
    }

    fn u_word(&self, a: usize) -> u16 {
        u16::from(self.mem[a]) | u16::from(self.mem[a + 1]) << 8
    }

    fn u_on(&self, n: usize) -> bool {
        self.u_word(Self::u_base(n)) & 0x8000 != 0
    }

    /// Instruction cycles per bit.
    fn u_bit_cycles(&self, n: usize) -> u64 {
        let b = Self::u_base(n);
        let brg = u64::from(self.u_word(b + 8)) | u64::from(self.u_word(b + 0xA) & 0xF) << 16;
        let brgh = self.u_word(b) & 0x80 != 0;
        (if brgh { 4 } else { 16 } * (brg + 1)).max(1)
    }

    /// Bits of a character: start, data (8 or 9), stop(s).
    fn u_char_bits(&self, n: usize) -> u64 {
        let b = Self::u_base(n);
        let mode = self.u_word(b) & 0xF;
        let data = match mode {
            // 0 async 8-bit, 1 async 7-bit (no parity here), 2 8-bit with parity... the legacy parity modes add a bit
            0 => 8,
            1 => 7,
            2 | 3 => 8,
            4 | 5 => 9,
            _ => 8,
        };
        let parity = u64::from(matches!(mode, 2 | 3));
        let stop = if (self.u_word(b + 2) >> 4) & 3 >= 2 { 2 } else { 1 };
        1 + data + parity + stop
    }

    pub(crate) fn uart_at(&self, a: usize) -> Option<(usize, usize)> {
        for n in 0..3 {
            let b = Self::u_base(n);
            if a >= b && a < b + 0x14 && self.p.has_uart[n] {
                return Some((n, a - b));
            }
        }
        None
    }

    /// Reads a UART register; `None` is plain storage.
    pub(crate) fn uart_read(&mut self, n: usize, off: usize) -> Option<u16> {
        let b = Self::u_base(n);
        match off {
            4 => {
                let st = &self.p.uart[n];
                let mut v = self.u_word(b + 4) & 0xFF00;
                let trmt = st.tx_fifo.is_empty() && st.shifting.is_none();
                v |= u16::from(st.txcif) | u16::from(st.oerr) << 1 | u16::from(st.ferr) << 3 | u16::from(trmt) << 7;
                Some(v)
            }
            6 => {
                let st = &self.p.uart[n];
                let mut v = self.u_word(b + 6) & 0xFFC0 & !0x3F;
                v |= u16::from(!st.rx_fifo.is_empty()); // URXBF
                v |= u16::from(st.rx_fifo.is_empty()) << 1; // URXBE
                v |= 1 << 3; // RIDLE
                v |= u16::from(st.tx_fifo.len() >= FIFO) << 4; // UTXBF
                v |= u16::from(st.tx_fifo.is_empty()) << 5; // UTXBE
                v |= self.u_word(b + 6) & 0x7000 | self.u_word(b + 6) & 0x0700;
                Some(v)
            }
            0xC => {
                let st = &mut self.p.uart[n];
                let v = st.rx_fifo.pop_front().map_or(0, |(v, _)| v);
                Some(v)
            }
            _ => None,
        }
    }

    pub(crate) fn uart_write(&mut self, n: usize, off: usize, v: u16, mask: u16) {
        let b = Self::u_base(n);
        let old = self.u_word(b + off);
        let new = (old & !mask) | (v & mask);
        match off {
            0 => {
                self.mem[b] = new as u8;
                self.mem[b + 1] = (new >> 8) as u8;
                let on = new & 0x8000 != 0;
                if !on {
                    let st = &mut self.p.uart[n];
                    st.tx_fifo.clear();
                    st.shifting = None;
                    st.rx_fifo.clear();
                    st.receiving = None;
                    st.oerr = false;
                    st.ferr = false;
                }
                let txen = on && new & 0x20 != 0;
                if txen && !self.p.uart[n].was_on {
                    self.raise_irq(UARTS[n].1[0]);
                }
                self.p.uart[n].was_on = txen;
            }
            4 => {
                // flags are cleared by writing 0 to them (OERR, FERR, TXCIF)
                if mask & 0xFF != 0 {
                    let st = &mut self.p.uart[n];
                    if v & 1 == 0 {
                        st.txcif = false;
                    }
                    if v & 2 == 0 {
                        st.oerr = false;
                    }
                    if v & 8 == 0 {
                        st.ferr = false;
                    }
                }
                let hi = (old & 0xFF00 & !mask) | (v & mask & 0xFF00);
                self.mem[b + 4 + 1] = (hi >> 8) as u8;
            }
            0x10 => {
                if self.u_on(n) && self.u_word(b) & 0x20 != 0 {
                    let st = &mut self.p.uart[n];
                    if st.tx_fifo.len() < FIFO {
                        st.tx_fifo.push_back(v & 0x1FF);
                    }
                    self.uart_start_tx(n);
                }
            }
            _ => {
                self.mem[b + off] = new as u8;
                self.mem[b + off + 1] = (new >> 8) as u8;
            }
        }
    }

    fn uart_start_tx(&mut self, n: usize) {
        if self.p.uart[n].shifting.is_some() {
            return;
        }
        let bits = self.u_char_bits(n) * self.u_bit_cycles(n);
        let now = self.cycles;
        let st = &mut self.p.uart[n];
        if let Some(c) = st.tx_fifo.pop_front() {
            st.shifting = Some((c, now + bits));
            self.raise_irq(UARTS[n].1[0]); // a place in the FIFO is free
        }
    }

    pub(crate) fn uart_advance(&mut self) {
        let now = self.cycles;
        for n in 0..3 {
            if !self.p.has_uart[n] || !self.u_on(n) {
                continue;
            }
            // transmitter
            while let Some((c, done)) = self.p.uart[n].shifting {
                if done > now {
                    break;
                }
                self.p.tx.push((n as u8 + 1, done, c as u8));
                self.p.uart[n].shifting = None;
                self.p.uart[n].txcif = true;
                let bits = self.u_char_bits(n) * self.u_bit_cycles(n);
                let st = &mut self.p.uart[n];
                if let Some(next) = st.tx_fifo.pop_front() {
                    st.shifting = Some((next, done + bits));
                    self.raise_irq(UARTS[n].1[0]);
                }
            }
            // receiver
            if self.u_word(Self::u_base(n)) & 0x10 == 0 {
                continue;
            }
            loop {
                let bit = self.u_bit_cycles(n);
                let chars = self.u_char_bits(n);
                if self.p.uart[n].receiving.is_none() {
                    let st = &mut self.p.uart[n];
                    let Some(front) = st.queue.front_mut() else { break };
                    if front.at > now {
                        break;
                    }
                    let Some(w) = front.words.pop_front() else {
                        st.queue.pop_front();
                        continue;
                    };
                    let start = front.at;
                    let sender_bit = front.bit.unwrap_or(bit);
                    let len = (chars) * sender_bit;
                    let bad = front.bad_stop;
                    front.at = start + len + front.gap;
                    if front.words.is_empty() {
                        st.queue.pop_front();
                    }
                    st.receiving = Some((start + len, w, bad));
                }
                let Some((end, w, bad)) = self.p.uart[n].receiving else { break };
                if end > now {
                    break;
                }
                self.p.uart[n].receiving = None;
                let st = &mut self.p.uart[n];
                if st.rx_fifo.len() >= FIFO {
                    st.oerr = true;
                } else {
                    st.rx_fifo.push_back((w, bad));
                    if bad {
                        st.ferr = true;
                    }
                }
                self.raise_irq(UARTS[n].1[1]);
            }
        }
    }

    /// Queues characters for the receiver of UART `port` (1..=3).
    pub fn uart_feed(&mut self, port: u8, rx: RxStream) -> bool {
        let n = usize::from(port).wrapping_sub(1);
        if n >= 3 || !self.p.has_uart[n] {
            return false;
        }
        let q = &mut self.p.uart[n].queue;
        let i = q.partition_point(|s| s.at <= rx.at);
        q.insert(i, rx);
        true
    }
}
