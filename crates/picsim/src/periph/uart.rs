//! EUSART of the Q10 style, shared by the PIC18 `Periph` and the PIC16 core (`pic16.rs`): TXnREG, TXnSTA, RCnSTA, BAUDnCON,
//! SPnBRGL/H (DS40002043F ch.29; PIC16F17126/46 DS40002343F ch.32 p.453-484 is the same module: TXIF, TRMT, the two-deep
//! receive FIFO and the baud formulas of Table 32-2 agree). Every function works on the data memory as a byte slice, so
//! the owner decides how flags reach the interrupt logic.
//!
//! * TXREG moves to the shift register at once when that is idle, otherwise it is held and moves in the cycle
//!   the Stop bit ends `[A6]`; TXIF = TXEN and nothing held (29.1.1.4); TRMT = shift register idle (29.1.1.5).
//! * Bit time in instruction cycles (Table 29-1, Fosc/4 per cycle): BRG16=1,BRGH=1 -> n+1; exactly one of them
//!   -> 4(n+1); neither -> 16(n+1). A character is start + 8 (9 with TX9) + stop bits.
//! * `[A7]` clearing TXEN abandons the shift register and a held byte. A write to TXREG while TXEN = 0 is dropped.
//! * `[A8]` TXIF does not dip for the instruction cycle after a write that goes straight to the shift register.

use crate::log::Log;
use super::{addr, Flag, RxStream};
use devdesc::Device;
use std::collections::{BTreeSet, VecDeque};

/// Number of hook codes per EUSART (TXREG, TXSTA, BAUDCON, RCREG, RCSTA).
pub(crate) const HOOKS: u8 = 5;
pub(crate) const RCREG: u8 = 3;

#[derive(Clone)]
pub(crate) struct Uart {
    pub num: u8,
    pub replies: Vec<super::UartReply>,
    sent: u8,
    pub reg: usize,
    pub sta: usize,
    pub baud: usize,
    pub brgl: usize,
    pub brgh: usize,
    pub flag: Flag,
    pub tsr_end: Option<u64>,
    pub pending: Option<u8>,
    /// Recent transmitted frames (start, bit cycles, word, number of data bits).
    tx_frames: VecDeque<(u64, u64, u16, u8)>,
    pending_ninth: bool,
    pin_paused: Option<u64>,
    pin_prev: Option<bool>,
    /// Next sample cycle, bit time, data width, sample index (0 = Start), accumulated word.
    pin_rx: Option<(u64, u64, u8, u8, u16)>,
    pub rcreg: usize,
    pub rcsta: usize,
    pub rcflag: Flag,
    /// Characters the outside world sends, in order; the head is the one being sent.
    pub rx_in: VecDeque<RxStream>,
    /// Cycle the line is free again (end of the last character that started).
    pub rx_free: u64,
    /// The character being received: (cycle it reaches the FIFO, word, framing error).
    pub rx_cur: Option<(u64, u16, bool)>,
    /// The part is in Sleep: the receiver sees nothing.
    pub asleep: bool,
    /// Receive FIFO, two deep: (word with the ninth bit at bit 8, framing error).
    pub fifo: VecDeque<(u16, bool)>,
}

impl Uart {
    /// Something is under way that `sync` must follow cycle by cycle (`Periph::tick` does not skip a sync meanwhile).
    pub fn busy(&self) -> bool {
        self.tsr_end.is_some() || self.pending.is_some() || !self.rx_in.is_empty() || self.rx_cur.is_some() || self.pin_rx.is_some()
    }

    /// EUSART `n` of the device and its registers in hook order (TXREG, TXSTA, BAUDCON, RCREG, RCSTA).
    pub fn find(dev: &Device, n: u8) -> Option<(Uart, [usize; 5])> {
        let reg = addr(dev, &format!("TX{n}REG"))?;
        let sta = addr(dev, &format!("TX{n}STA"))?;
        let baud = addr(dev, &format!("BAUD{n}CON"))?;
        // SPnBRGL/H, or SPBRGn/SPBRGHn on the PIC18(L)F2x/4xK22, whose EUSART has the same TXSTAx, RCSTAx and BAUDCONx bits
        // (DS40001412H Registers 16-1 to 16-3 p.269-271; the pack's aliases give the TXnREG, TXnSTA, BAUDnCON, RCnREG, RCnSTA names).
        let brgl = addr(dev, &format!("SP{n}BRGL")).or_else(|| addr(dev, &format!("SPBRG{n}")))?;
        let brgh = addr(dev, &format!("SP{n}BRGH")).or_else(|| addr(dev, &format!("SPBRGH{n}")))?;
        let flag = Flag::find(dev, &format!("TX{n}IF")).or_else(|| (n == 1).then(|| Flag::find(dev, "TXIF")).flatten())?;
        let rcreg = addr(dev, &format!("RC{n}REG"))?;
        let rcsta = addr(dev, &format!("RC{n}STA"))?;
        let rcflag = Flag::find(dev, &format!("RC{n}IF")).or_else(|| (n == 1).then(|| Flag::find(dev, "RCIF")).flatten())?;
        let u = Uart {
            replies: Vec::new(), sent: 0,
            num: n, reg, sta, baud, brgl, brgh, flag, tsr_end: None, pending: None, rcreg, rcsta, rcflag,
            tx_frames: VecDeque::new(), pending_ninth: false, pin_paused: None, pin_prev: None, pin_rx: None,
            rx_in: VecDeque::new(), rx_free: 0, rx_cur: None, asleep: false, fifo: VecDeque::new(),
        };
        Some((u, [reg, sta, baud, rcreg, rcsta]))
    }

    /// Instruction cycles one bit takes at the current baud settings.
    pub fn bit_cycles(&self, mem: &[u8]) -> u64 {
        let sta = mem[self.sta];
        let brg16 = mem[self.baud] & 8 != 0;
        let n = if brg16 { u64::from(mem[self.brgh]) << 8 | u64::from(mem[self.brgl]) } else { u64::from(mem[self.brgl]) };
        match (brg16, sta & 4 != 0) {
            (true, true) => n + 1,
            (false, false) => 16 * (n + 1),
            _ => 4 * (n + 1),
        }
    }

    fn start_frame(&mut self, mem: &[u8], at: u64, byte: u8, ninth: bool) {
        self.sent = byte;
        let width = if mem[self.sta] & 0x40 != 0 { 9 } else { 8 };
        let word = u16::from(byte) | if width == 9 { u16::from(ninth) << 8 } else { 0 };
        self.tx_frames.push_back((at, self.bit_cycles(mem), word, width));
        while self.tx_frames.len() > 3 { self.tx_frames.pop_front(); }
    }

    /// Asynchronous TX waveform, including idle and SCKP polarity. SPEN gates the pin layer only;
    /// the existing character-log interface continues to accept TXEN-only firmware.
    pub fn tx_level(&self, mem: &[u8], at: u64) -> Option<bool> {
        if mem[self.rcsta] & 0x80 == 0 || mem[self.sta] & 0x30 != 0x20 { return None; }
        Some(self.tx_signal(at) ^ (mem[self.baud] & 0x10 != 0))
    }

    pub fn tx_signal(&self, at: u64) -> bool {
        let at = self.pin_paused.map_or(at, |paused| at.min(paused));
        let mut high = true;
        if let Some(&(start, bit, word, width)) = self.tx_frames.iter().rev().find(|f| f.0 <= at) {
            let n = (at - start) / bit;
            high = if n == 0 { false } else if n <= u64::from(width) { word >> (n - 1) & 1 != 0 } else { true };
        }
        high
    }

    pub fn tx_times(&self, from: u64, to: u64) -> impl Iterator<Item = u64> + '_ {
        self.tx_frames.iter().flat_map(|&(start, bit, _, width)| (0..=u64::from(width) + 2).map(move |n| start + bit * n))
            .filter(move |&t| from < t && t <= to && self.pin_paused.is_none_or(|paused| t <= paused))
    }

    /// Sample a resolved RX pin at bit centers. Character stimuli use the existing stream path.
    pub fn rx_pin(&mut self, mem: &mut [u8], at: u64, high: bool) {
        if !self.rx_on(mem) || !self.rx_in.is_empty() || self.rx_cur.is_some() {
            self.pin_rx = None;
            self.pin_prev = Some(high);
            return;
        }
        let falling = self.pin_prev == Some(true) && !high;
        self.pin_prev = Some(high);
        if self.pin_rx.is_none() && falling {
            let bit = self.bit_cycles(mem);
            let width = if mem[self.rcsta] & 0x40 != 0 { 9 } else { 8 };
            self.pin_rx = Some((at + bit / 2, bit, width, 0, 0));
        }
        if let Some((next, bit, width, index, mut word)) = self.pin_rx {
            if at < next { return; }
            if index == 0 && high { self.pin_rx = None; return; }
            if index > 0 && index <= width { word |= u16::from(high) << (index - 1); }
            if index == width + 1 {
                self.pin_rx = None;
                if mem[self.rcsta] & 0x48 != 0x48 || word & 0x100 != 0 {
                    if self.fifo.len() < 2 { self.fifo.push_back((word, !high)); }
                    else { mem[self.rcsta] |= 2; }
                }
            } else {
                self.pin_rx = Some((next + bit, bit, width, index + 1, word));
            }
        }
    }

    pub fn freeze_pins(&mut self, at: u64) {
        self.pin_paused = Some(at);
    }

    pub fn reset_pins(&mut self) {
        self.pin_paused = None;
        self.tx_frames.clear();
        self.pin_prev = None;
        self.pin_rx = None;
    }

    /// Instruction cycles one transmitted character takes at the current baud settings.
    pub fn char_cycles(&self, mem: &[u8]) -> u64 {
        self.bit_cycles(mem) * (if mem[self.sta] & 0x40 != 0 { 11 } else { 10 })
    }

    /// The receiver listens: serial port and receiver enabled, asynchronous, no overrun pending.
    fn rx_on(&self, mem: &[u8]) -> bool {
        !self.asleep && mem[self.rcsta] & 0x92 == 0x90 && mem[self.sta] & 0x10 == 0
    }

    /// TXIF and TRMT follow the transmitter state.
    pub fn bits(&self, mem: &mut [u8]) {
        let txen = mem[self.sta] & 0x20 != 0;
        if txen && self.pending.is_none() { self.flag.set_in(mem) } else { self.flag.clear_in(mem) }
        if self.tsr_end.is_none() { mem[self.sta] |= 2 } else { mem[self.sta] &= !2 }
        // RCIF: receiver enabled and an unread character in the FIFO (29.1.2.3). FERR and RX9D describe the top one.
        let (w, fe) = self.fifo.front().copied().unwrap_or((0, false));
        if mem[self.rcsta] & 0x90 == 0x90 && !self.fifo.is_empty() { self.rcflag.set_in(mem) } else { self.rcflag.clear_in(mem) }
        mem[self.rcsta] = mem[self.rcsta] & !5 | u8::from(fe) << 2 | (w >> 8) as u8 & 1;
        if self.rx_cur.is_none() && self.pin_rx.is_none() { mem[self.baud] |= 0x40 } else { mem[self.baud] &= !0x40 }
    }

    /// Receiver: start the next queued character when the line is free, finish the one in flight.
    /// `[A9]` a character is taken into the FIFO when its Stop bit has been sampled, half a bit time into the Stop bit
    /// (29.1.2.2); a character whose Start bit falls while the receiver is off is lost, as the hardware waits for
    /// the next falling edge. Pushes the cycles at which RCIF rose to `ev`.
    pub fn rx(&mut self, mem: &mut [u8], now: u64, ev: &mut Vec<u64>) {
        loop {
            if let Some((end, word, fe)) = self.rx_cur {
                if end > now {
                    return;
                }
                self.rx_cur = None;
                let skip = mem[self.rcsta] & 0x48 == 0x48 && word & 0x100 == 0; // ADDEN with RX9: only address bytes
                if skip {
                    continue;
                }
                if self.fifo.len() < 2 {
                    self.fifo.push_back((word, fe));
                    ev.push(end);
                } else {
                    mem[self.rcsta] |= 2; // third character complete before the FIFO was read: OERR
                }
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
            let bit = st.bit.unwrap_or_else(|| self.bit_cycles(mem));
            let st = self.rx_in.front_mut().unwrap();
            let n = if st.nine { 11 } else { 10 };
            self.rx_free = bit.saturating_mul(n).saturating_add(start).saturating_add(st.gap);
            st.at = self.rx_free;
            let (bad, nine) = (st.bad_stop, st.nine);
            if self.rx_on(mem) {
                // The receiver's own width decides how many bits it takes in; the sender's decides the timing.
                let nr = if mem[self.rcsta] & 0x40 != 0 { 11 } else { 10 };
                let word = if nine && nr == 11 { word & 0x1FF } else { word & 0xFF };
                self.rx_cur = Some((start.saturating_add(bit.saturating_mul(2 * nr - 1) / 2), word, bad));
            }
        }
    }

    /// Bring the transmitter and the receiver to cycle `now` and refresh the flag bits.
    pub fn sync(&mut self, mem: &mut [u8], now: u64, ev: &mut Vec<u64>, tx: &mut Log<(u8, u64, u8)>, notes: &mut BTreeSet<String>) {
        if mem[self.sta] & 0x10 != 0 {
            notes.insert(format!("EUSART{} synchronous mode is not modelled; nothing is sent", self.num));
        } else {
            // Asynchronous-only features that are not modelled (BAUDxCON bits 0, 1; TXxSTA bit 3; DS40002519D p.428, 432):
            // the bits stay set, so a program polling for the hardware to clear them waits for ever.
            if mem[self.baud] & 1 != 0 {
                notes.insert(format!("EUSART{} auto-baud detect (ABDEN) is not modelled; ABDEN never clears", self.num));
            }
            if mem[self.baud] & 2 != 0 {
                notes.insert(format!("EUSART{} wake-up on receive (WUE) is not modelled; WUE never clears", self.num));
            }
            if mem[self.sta] & 8 != 0 {
                notes.insert(format!("EUSART{} break character (SENDB) is not modelled; no break is sent and SENDB never clears", self.num));
            }
        }
        while let Some(e) = self.tsr_end.filter(|e| *e <= now) {
            self.tsr_end = None;
            super::uart_replies(&mut self.replies, self.sent, e, &mut self.rx_in);
            if let Some(b) = self.pending.take() {
                self.start_frame(mem, e, b, self.pending_ninth);
                tx.push((self.num, e, b));
                self.tsr_end = Some(e + self.char_cycles(mem));
                ev.push(e);
            }
        }
        self.rx(mem, now, ev);
        self.bits(mem);
    }

    /// RCREG read: the oldest FIFO entry. `[A10]` An empty FIFO reads the last character again (the data sheet does not say).
    pub fn read_rc(&mut self, mem: &mut [u8]) -> u8 {
        if let Some((w, _)) = self.fifo.pop_front() {
            mem[self.rcreg] = w as u8;
        }
        self.bits(mem);
        mem[self.rcreg]
    }

    /// Write hook `sub` (0 TXREG, 1 TXSTA, 2 BAUDCON, 3 RCREG, 4 RCSTA) at cycle `now`.
    pub fn write(&mut self, sub: u8, v: u8, now: u64, mem: &mut [u8], tx: &mut Log<(u8, u64, u8)>) {
        match sub {
            0 => {
                mem[self.reg] = v;
                if mem[self.sta] & 0x20 != 0 {
                    if self.tsr_end.is_none() {
                        self.start_frame(mem, now, v, mem[self.sta] & 1 != 0);
                        tx.push((self.num, now, v));
                        self.tsr_end = Some(now + self.char_cycles(mem));
                    } else {
                        self.pending = Some(v);
                        self.pending_ninth = mem[self.sta] & 1 != 0;
                    }
                }
            }
            1 => {
                mem[self.sta] = v & !2 | mem[self.sta] & 2; // TRMT is read only
                if v & 0x20 == 0 {
                    self.tsr_end = None;
                    self.pending = None;
                }
            }
            2 => mem[self.baud] = v & !0xC0 | mem[self.baud] & 0xC0, // ABDOVF and RCIDL are read only
            3 => {} // RCREG is read only
            _ => {
                // OERR, FERR and RX9D belong to the hardware. `[A11]` Clearing CREN clears OERR and abandons the
                // character in flight (the FIFO keeps its characters); clearing SPEN resets the receiver.
                let mut new = v & !7 | mem[self.rcsta] & 7;
                if v & 0x80 == 0 {
                    self.fifo.clear();
                    self.rx_cur = None;
                    new &= !7;
                } else if v & 0x10 == 0 {
                    self.rx_cur = None;
                    new &= !2;
                }
                if v & 0x90 != 0x90 { self.pin_rx = None; self.pin_prev = None; }
                mem[self.rcsta] = new;
            }
        }
        self.bits(mem);
    }

    /// The part goes to Sleep (`true`) or wakes (`false`, `d` cycles later).
    pub fn sleep(&mut self, asleep: bool, d: u64) {
        self.asleep = asleep;
        if !asleep { self.pin_paused = None; }
        if asleep {
            self.rx_cur = None;
            self.pin_rx = None;
            self.pin_prev = None;
        } else if let Some(e) = &mut self.tsr_end {
            *e += d;
            for f in &mut self.tx_frames { f.0 += d; }
        }
    }
}
