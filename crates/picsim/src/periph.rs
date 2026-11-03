//! Peripheral models that live on the instruction clock: Timer0, Timer1, Timer2 and the interrupt flag
//! registers they set. Models are advanced lazily: `sync(now)` brings every timer to the cycle count `now`
//! (an instruction boundary or a register write), setting the PIRx flag bits on the cycle they happen.
//!
//! Rules and sources (DS40002043F is the PIC18F27/47Q10 data sheet; the Q41 timers have the same layout):
//! * Timer0 (ch.19, p.296-298): 8-bit mode compares TMR0L with the TMR0H buffer and resets TMR0L on the next clock
//!   (period TMR0H+1 clocks); 16-bit mode rolls over at FFFFh; TMR0H is buffered in 16-bit mode (read of TMR0L
//!   loads it, write of TMR0L stores it); prescaler 1:1..1:32768 (T0CKPS) and postscaler 1:1..1:16 (T0OUTPS) are
//!   cleared by a write to TMR0L, T0CON0 or T0CON1; TMR0IF is set when TMR0_out toggles, i.e. every T0OUTPS+1
//!   matches or rollovers; FOSC/4 source = T0CS 010 (Table 19-1).
//! * Timer1 (ch.20, p.305-313): 16-bit up counter, prescaler 1/2/4/8, rollover from FFFFh to 0000h sets TMR1IF;
//!   RD16 buffers TMR1H like Timer0 (20.6). Clock FOSC/4 = CS 1, FOSC = CS 2 (TMRxCLK).
//! * Timer2 (ch.21, p.324-327): T2TMR counts up, compares with T2PR, goes to 0 on the next clock; the postscaler
//!   counts matches and the flag is set when it reaches OUTPS+1; prescaler and postscaler are cleared by a write to
//!   T2TMR or T2CON, T2TMR is not; one-shot software start (MODE 01000) clears ON at the period match.
//!   Hardware gate/reset modes need pin models (S4) and are run as the software mode with a note.
//! Assumptions `[A#]` (to be measured against MDB):
//! * `[A1]` The flag is set on the clock that takes the count from the compare value back to 0 (period clocks apart).
//! * `[A2]` A write takes effect after the cycle of the writing instruction that fetched it, so the first counted cycle
//!   is the one after a one-cycle writing instruction.
//! * `[A3]` Timer0 keeps its count when T0EN is cleared; Timer1/Timer2 keep it when ON is cleared.

use crate::mem::{DataMem, Special};
use devdesc::Device;
use std::collections::{BTreeSet, VecDeque};

mod quart;
use quart::QUart;

/// A flag bit in a PIRx register.
#[derive(Clone, Copy, Debug)]
struct Flag {
    addr: usize,
    mask: u8,
}

impl Flag {
    fn find(dev: &Device, bit: &str) -> Option<Flag> {
        for s in dev.sfr.iter().filter(|s| s.name.starts_with("PIR")) {
            if let Some(f) = s.fields.iter().chain(s.legacy.iter()).find(|f| f.name == bit) {
                return Some(Flag { addr: s.addr as usize, mask: 1 << f.pos });
            }
        }
        None
    }
    fn set(self, m: &mut DataMem) {
        m.mem[self.addr] |= self.mask;
    }
    fn clear(self, m: &mut DataMem) {
        m.mem[self.addr] &= !self.mask;
    }
}

fn addr(dev: &Device, name: &str) -> Option<usize> {
    dev.find_sfr(name).map(|s| s.addr as usize)
}

/// Clocks produced by `dt` input ticks through a prescaler of `1 << shift` whose counter is `pre`.
#[inline]
fn prescale(pre: &mut u64, shift: u32, dt: u64) -> u64 {
    let t = *pre + dt;
    *pre = t & ((1u64 << shift) - 1);
    t >> shift
}

// Hook codes stored in `Special::Hook`.
const T0L: u8 = 1;
const T0H: u8 = 2;
const T0C0: u8 = 3;
const T0C1: u8 = 4;
const T1L: u8 = 5;
const T1H: u8 = 6;
const T1C: u8 = 7;
const T1CLK: u8 = 8;
const T2T: u8 = 9;
const T2P: u8 = 10;
const T2C: u8 = 11;
const T2HLT: u8 = 12;
const T2CLK: u8 = 13;
/// ADCON0 of the ADCC (basic mode).
const ADC0: u8 = 14;
/// EUSART n (0-based) uses hook codes UART0 + 5n + {TXREG 0, TXSTA 1, BAUDCON 2, RCREG 3, RCSTA 4}.
/// FVRCON: FVRRDY (read only) follows FVREN. `[A15]` at once; the data sheet gives only a typical start-up time of
/// 25 us (FVR04, DS40002043F p.707) and the models do not know Fosc. A note is added when it is first enabled.
const FVR0: u8 = 15;
const UART0: u8 = 16;
const UART_HOOKS: u8 = 5;
/// Q41 UART n (0-based) uses hook codes QUART0 + quart::HOOKS * n + sub-code (see `quart.rs`); at most 5 UARTs.
const QUART0: u8 = 112;
const QUART_END: u8 = QUART0 + 5 * quart::HOOKS;
/// T1GCON: GVAL (bit 2) is read only and follows the gate source.
const T1G: u8 = 40;
/// PORTx (index into `ports`) uses hook code PORT0 + index: a read gives the pin levels, a write goes to LATx.
const PORT0: u8 = 48;

/// EUSART1 transmitter of the Q10 style (TX1REG, TX1STA, BAUD1CON, SP1BRGL/H; DS40002043F ch.29 p.503-505).
/// * TXREG moves to the shift register at once when that is idle, otherwise it is held and moves in the cycle
///   the Stop bit ends `[A6]`; TXIF = TXEN and nothing held (29.1.1.4); TRMT = shift register idle (29.1.1.5).
/// * Bit time in instruction cycles (Table 29-1, Fosc/4 per cycle): BRG16=1,BRGH=1 -> n+1; exactly one of them
///   -> 4(n+1); neither -> 16(n+1). A character is start + 8 (9 with TX9) + stop bits.
/// * `[A7]` clearing TXEN abandons the shift register and a held byte. A write to TXREG while TXEN = 0 is dropped.
/// * `[A8]` TXIF does not dip for the instruction cycle after a write that goes straight to the shift register.
struct Uart {
    num: u8,
    reg: usize,
    sta: usize,
    baud: usize,
    brgl: usize,
    brgh: usize,
    flag: Flag,
    tsr_end: Option<u64>,
    pending: Option<u8>,
    rcreg: usize,
    rcsta: usize,
    rcflag: Flag,
    /// Characters the outside world sends, in order; the head is the one being sent.
    rx_in: VecDeque<RxStream>,
    /// Cycle the line is free again (end of the last character that started).
    rx_free: u64,
    /// The character being received: (cycle it reaches the FIFO, word, framing error).
    rx_cur: Option<(u64, u16, bool)>,
    /// Receive FIFO, two deep: (word with the ninth bit at bit 8, framing error).
    fifo: VecDeque<(u16, bool)>,
}

/// Characters sent to a receiver: the first start bit falls at cycle `at`, then back to back plus `gap` idle cycles.
/// `bit` is the sender's bit time in instruction cycles; without it the receiver's own rate is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RxStream {
    pub at: u64,
    pub words: VecDeque<u16>,
    pub bit: Option<u64>,
    pub gap: u64,
    /// Nine data bits (11 bits per character) instead of eight.
    pub nine: bool,
    /// The Stop bit is sent as 0.
    pub bad_stop: bool,
}

/// ADC with computation (ADCC) of the PIC18F27/47Q10, basic mode (DS40002043F ch.33 p.546-553, 566-576).
/// * ADGO = 1 with ADON = 1 starts a conversion; it takes ADPRE + ADACQ ADC clocks (0 = not included) and then
///   11.5 TAD (33.1.4 p.549); TAD = 2(ADCLK+1) Fosc periods (Table 33-2). It then loads ADRES (right or left
///   justified by ADFM, 10 bits), clears ADGO unless ADCONT = 1, and sets ADIF (33.2.2). Clearing ADGO or ADON
///   terminates it, ADRES keeps its value (33.2.3).
/// * The input is the stimulus value of the channel in ADPCH at the end of acquisition (0 without one).
/// * Not modelled, with a note: the FRC clock (ADCS = 1), computation (filter, threshold, ADPREV/ADACC), triggers
///   (ADACT) and double sampling.
/// * `[A12]` The conversion starts in the cycle of the instruction that sets ADGO. `[A13]` ADGO written 1 while
///   ADON = 0 reads back 0 and starts nothing. `[A14]` With ADCONT = 1 the next conversion starts in the cycle the
///   previous one ended.
struct Adc {
    con: usize,
    clk: usize,
    pre: usize,
    acq: usize,
    pch: usize,
    resl: usize,
    resh: usize,
    flag: Flag,
    bits: u32,
    /// Cycle the running conversion ends, and the value it will load.
    end: Option<(u64, u16)>,
    /// Stimulus: (channel, from cycle, code), in the order given.
    feed: Vec<(u8, u64, u16)>,
}

struct Timer0 {
    l: usize,
    h: usize,
    c0: usize,
    c1: usize,
    flag: Flag,
    cnt: u16,
    cmp: u8,
    pre: u64,
    post: u8,
    last: u64,
}

struct Timer1 {
    gate: Option<usize>,
    gpps: Option<usize>,
    l: usize,
    h: usize,
    con: usize,
    clk: usize,
    gcon: usize,
    flag: Flag,
    cnt: u16,
    pre: u64,
    last: u64,
}

struct Timer2 {
    tmr: usize,
    pr: usize,
    con: usize,
    hlt: usize,
    clk: usize,
    flag: Flag,
    cnt: u8,
    pre: u64,
    post: u8,
    last: u64,
}

/// A port with pins: PORTx reads the pins, a write goes to LATx (DS40002043F 17.1: the pin level is read, the
/// latch is written). Input pins with their ANSELx bit set read 0 (the digital input buffer is off, 17.2).
struct Port {
    letter: u8,
    port: usize,
    lat: usize,
    tris: usize,
    ansel: Option<usize>,
}

pub struct Periph {
    ports: Vec<Port>,
    /// What the outside world drives on the input pins: (from cycle, port index, pin mask, levels), stable-sorted by cycle.
    drive: Vec<(u64, usize, u8, u8)>,
    t0: Option<Timer0>,
    t1: Option<Timer1>,
    t2: Option<Timer2>,
    uarts: Vec<Uart>,
    quarts: Vec<QUart>,
    adc: Option<Adc>,
    fvr: Option<usize>,
    /// Bytes the transmitters started to send: (EUSART number 1-based, cycle it started, byte).
    pub tx: Vec<(u8, u64, u8)>,
    pub notes: BTreeSet<String>,
    /// Cycle of the latest flag set by `sync` since the core last looked (`take_event`).
    event: Option<u64>,
}

/// Cycle on which clock number `j` (1-based) of a prescaler `1 << sh` that held `pre0` ticks arrives, when the
/// input delivers `ratio` ticks per instruction cycle, counted from cycle `base`.
#[inline]
fn at(base: u64, pre0: u64, sh: u32, j: u64, ratio: u64) -> u64 {
    base + (((j << sh) - pre0) + ratio - 1) / ratio
}

impl Periph {
    /// Forget flag events from before the instruction that is about to run.
    pub fn clear_event(&mut self) {
        self.event = None;
    }

    pub fn peek_event(&self) -> Option<u64> {
        self.event
    }

    pub fn take_event(&mut self) -> Option<u64> {
        self.event.take()
    }

    fn raised(&mut self, cyc: u64) {
        self.event = Some(self.event.map_or(cyc, |e| e.max(cyc)));
    }

    /// Find the timers the part has and mark their registers as hooked in `m`.
    pub fn bind(dev: &Device, m: &mut DataMem) -> Periph {
        let mut p = Periph { ports: Vec::new(), drive: Vec::new(), t0: None, t1: None, t2: None, uarts: Vec::new(), quarts: Vec::new(), adc: None, fvr: None, tx: Vec::new(), notes: BTreeSet::new(), event: None };
        let mut hook = |a: usize, k: u8| m.special[a] = Special::Hook(k);
        if let (Some(l), Some(h), Some(c0), Some(c1), Some(flag)) =
            (addr(dev, "TMR0L"), addr(dev, "TMR0H"), addr(dev, "T0CON0"), addr(dev, "T0CON1"), Flag::find(dev, "TMR0IF"))
        {
            for (a, k) in [(l, T0L), (h, T0H), (c0, T0C0), (c1, T0C1)] {
                hook(a, k);
            }
            p.t0 = Some(Timer0 { l, h, c0, c1, flag, cnt: 0, cmp: 0xFF, pre: 0, post: 0, last: 0 });
        }
        if let (Some(l), Some(h), Some(con), Some(clk), Some(gcon), Some(flag)) = (
            addr(dev, "TMR1L"),
            addr(dev, "TMR1H"),
            addr(dev, "T1CON"),
            addr(dev, "T1CLK"),
            addr(dev, "T1GCON"),
            Flag::find(dev, "TMR1IF"),
        ) {
            for (a, k) in [(l, T1L), (h, T1H), (con, T1C), (clk, T1CLK), (gcon, T1G)] {
                hook(a, k);
            }
            p.t1 = Some(Timer1 { gate: addr(dev, "T1GATE"), gpps: addr(dev, "T1GPPS"), l, h, con, clk, gcon, flag, cnt: 0, pre: 0, last: 0 });
        }
        if let (Some(tmr), Some(pr), Some(con), Some(hlt), Some(clk), Some(flag)) = (
            addr(dev, "T2TMR"),
            addr(dev, "T2PR"),
            addr(dev, "T2CON"),
            addr(dev, "T2HLT"),
            addr(dev, "T2CLKCON"),
            Flag::find(dev, "TMR2IF"),
        ) {
            for (a, k) in [(tmr, T2T), (pr, T2P), (con, T2C), (hlt, T2HLT), (clk, T2CLK)] {
                hook(a, k);
            }
            p.t2 = Some(Timer2 { tmr, pr, con, hlt, clk, flag, cnt: 0, pre: 0, post: 0, last: 0 });
        }
        if let (Some(con), Some(clk), Some(pre), Some(acq), Some(pch), Some(resl), Some(resh), Some(flag)) = (
            addr(dev, "ADCON0"),
            addr(dev, "ADCLK"),
            addr(dev, "ADPRE"),
            addr(dev, "ADACQ"),
            addr(dev, "ADPCH"),
            addr(dev, "ADRESL"),
            addr(dev, "ADRESH"),
            Flag::find(dev, "ADIF"),
        ) {
            // Only the Q10 ADC is sourced so far; the Q41 and PIC16 converters differ in resolution and timing.
            if addr(dev, "ADCPCON").is_none() && addr(dev, "PRODL").is_some() {
                hook(con, ADC0);
                p.adc = Some(Adc { con, clk, pre, acq, pch, resl, resh, flag, bits: 10, end: None, feed: Vec::new() });
            } else {
                p.notes.insert("the ADC of this part is not modelled; ADGO never clears".into());
            }
        }
        for letter in b'A'..=b'H' {
            let l = letter as char;
            if let (Some(port), Some(lat), Some(tris)) = (addr(dev, &format!("PORT{l}")), addr(dev, &format!("LAT{l}")), addr(dev, &format!("TRIS{l}"))) {
                hook(port, PORT0 + p.ports.len() as u8);
                p.ports.push(Port { letter, port, lat, tris, ansel: addr(dev, &format!("ANSEL{l}")) });
            }
        }
        if let Some(a) = addr(dev, "FVRCON") {
            hook(a, FVR0);
            p.fvr = Some(a);
        }
        for n in 1..=2u8 {
            if let (Some(reg), Some(sta), Some(baud), Some(brgl), Some(brgh), Some(flag), Some(rcreg), Some(rcsta), Some(rcflag)) = (
                addr(dev, &format!("TX{n}REG")),
                addr(dev, &format!("TX{n}STA")),
                addr(dev, &format!("BAUD{n}CON")),
                addr(dev, &format!("SP{n}BRGL")),
                addr(dev, &format!("SP{n}BRGH")),
                Flag::find(dev, &format!("TX{n}IF")),
                addr(dev, &format!("RC{n}REG")),
                addr(dev, &format!("RC{n}STA")),
                Flag::find(dev, &format!("RC{n}IF")),
            ) {
                let k = UART0 + UART_HOOKS * p.uarts.len() as u8;
                for (a, o) in [(reg, 0), (sta, 1), (baud, 2), (rcreg, 3), (rcsta, 4)] {
                    hook(a, k + o);
                }
                p.uarts.push(Uart {
                    num: n, reg, sta, baud, brgl, brgh, flag, tsr_end: None, pending: None, rcreg, rcsta, rcflag,
                    rx_in: VecDeque::new(), rx_free: 0, rx_cur: None, fifo: VecDeque::new(),
                });
            }
        }
        for n in 1..=5u8 {
            if let Some((q, regs)) = QUart::find(dev, n) {
                let k = QUART0 + quart::HOOKS * p.quarts.len() as u8;
                for (i, a) in regs.into_iter().enumerate() {
                    hook(a, k + i as u8);
                }
                p.quarts.push(q);
            }
        }
        p
    }

    /// Queue characters for the receiver of EUSART `num` (1-based). False when the part has no such EUSART.
    pub fn rx_send(&mut self, num: u8, s: RxStream) -> bool {
        if let Some(q) = self.quarts.iter_mut().find(|q| q.num == num) {
            q.rx_send(s);
            return true;
        }
        match self.uarts.iter_mut().find(|u| u.num == num) {
            Some(u) => {
                u.rx_in.push_back(s);
                true
            }
            None => false,
        }
    }

    /// Instruction cycles one bit takes at the current baud settings.
    fn uart_bit_cycles(u: &Uart, m: &DataMem) -> u64 {
        let sta = m.mem[u.sta];
        let brg16 = m.mem[u.baud] & 8 != 0;
        let n = if brg16 { u64::from(m.mem[u.brgh]) << 8 | u64::from(m.mem[u.brgl]) } else { u64::from(m.mem[u.brgl]) };
        match (brg16, sta & 4 != 0) {
            (true, true) => n + 1,
            (false, false) => 16 * (n + 1),
            _ => 4 * (n + 1),
        }
    }

    /// Instruction cycles one transmitted character takes at the current baud settings.
    fn uart_char_cycles(u: &Uart, m: &DataMem) -> u64 {
        Self::uart_bit_cycles(u, m) * (if m.mem[u.sta] & 0x40 != 0 { 11 } else { 10 })
    }

    /// The receiver listens: serial port and receiver enabled, asynchronous, no overrun pending.
    fn uart_rx_on(u: &Uart, m: &DataMem) -> bool {
        m.mem[u.rcsta] & 0x92 == 0x90 && m.mem[u.sta] & 0x10 == 0
    }

    /// TXIF and TRMT follow the transmitter state.
    fn uart_bits(u: &Uart, m: &mut DataMem) {
        let txen = m.mem[u.sta] & 0x20 != 0;
        if txen && u.pending.is_none() { u.flag.set(m) } else { u.flag.clear(m) }
        if u.tsr_end.is_none() { m.mem[u.sta] |= 2 } else { m.mem[u.sta] &= !2 }
        // RCIF: receiver enabled and an unread character in the FIFO (29.1.2.3). FERR and RX9D describe the top one.
        let (w, fe) = u.fifo.front().copied().unwrap_or((0, false));
        if m.mem[u.rcsta] & 0x90 == 0x90 && !u.fifo.is_empty() { u.rcflag.set(m) } else { u.rcflag.clear(m) }
        m.mem[u.rcsta] = m.mem[u.rcsta] & !5 | u8::from(fe) << 2 | (w >> 8) as u8 & 1;
        if u.rx_cur.is_none() { m.mem[u.baud] |= 0x40 } else { m.mem[u.baud] &= !0x40 }
    }

    /// Receiver: start the next queued character when the line is free, finish the one in flight.
    /// `[A9]` a character is taken into the FIFO when its Stop bit has been sampled, half a bit time into the Stop bit
    /// (29.1.2.2); a character whose Start bit falls while the receiver is off is lost, as the hardware waits for
    /// the next falling edge. Returns the cycles at which RCIF rose.
    fn uart_rx(u: &mut Uart, m: &mut DataMem, now: u64, ev: &mut Vec<u64>) {
        loop {
            if let Some((end, word, fe)) = u.rx_cur {
                if end > now {
                    return;
                }
                u.rx_cur = None;
                let skip = m.mem[u.rcsta] & 0x48 == 0x48 && word & 0x100 == 0; // ADDEN with RX9: only address bytes
                if skip {
                    continue;
                }
                if u.fifo.len() < 2 {
                    u.fifo.push_back((word, fe));
                    ev.push(end);
                } else {
                    m.mem[u.rcsta] |= 2; // third character complete before the FIFO was read: OERR
                }
                continue;
            }
            let Some(st) = u.rx_in.front_mut() else { return };
            let start = st.at.max(u.rx_free);
            if start > now {
                return;
            }
            let Some(word) = st.words.pop_front() else {
                u.rx_in.pop_front();
                continue;
            };
            let bit = st.bit.unwrap_or_else(|| Self::uart_bit_cycles(u, m));
            let st = u.rx_in.front_mut().unwrap();
            let n = if st.nine { 11 } else { 10 };
            u.rx_free = bit.saturating_mul(n).saturating_add(start).saturating_add(st.gap);
            st.at = u.rx_free;
            let (bad, nine) = (st.bad_stop, st.nine);
            if Self::uart_rx_on(u, m) {
                // The receiver's own width decides how many bits it takes in; the sender's decides the timing.
                let nr = if m.mem[u.rcsta] & 0x40 != 0 { 11 } else { 10 };
                let word = if nine && nr == 11 { word & 0x1FF } else { word & 0xFF };
                u.rx_cur = Some((start.saturating_add(bit.saturating_mul(2 * nr - 1) / 2), word, bad));
            }
        }
    }

    /// Bring every running timer to cycle `now`.
    pub fn sync(&mut self, m: &mut DataMem, now: u64) {
        let mut ev: Vec<u64> = Vec::new();
        if let Some(t) = &mut self.t0 {
            let (base, pre0) = (t.last, t.pre);
            let dt = now - t.last;
            t.last = now;
            let c0 = m.mem[t.c0];
            let c1 = m.mem[t.c1];
            if c0 & 0x80 != 0 && dt > 0 {
                if c1 >> 5 != 2 {
                    self.notes.insert(format!("Timer0 clock source T0CS={} is not modelled (only FOSC/4); it does not count", c1 >> 5));
                } else {
                    let clocks = prescale(&mut t.pre, u32::from(c1 & 15), dt);
                    let outps = c0 & 15;
                    let wide = c0 & 0x10 != 0;
                    for j in 1..=clocks {
                        let wrap = if wide {
                            t.cnt = t.cnt.wrapping_add(1);
                            t.cnt == 0
                        } else if t.cnt as u8 == m.mem[t.h] {
                            t.cnt = 0;
                            t.cmp = m.mem[t.h];
                            true
                        } else {
                            t.cnt += 1;
                            false
                        };
                        if wrap {
                            if t.post >= outps {
                                t.post = 0;
                                m.mem[t.c0] ^= 0x20; // T0OUT toggles
                                t.flag.set(m);
                                ev.push(at(base, pre0, u32::from(c1 & 15), j, 1));
                            } else {
                                t.post += 1;
                            }
                        }
                    }
                }
            }
            m.mem[t.l] = t.cnt as u8;
        }
        if let Some(t) = &mut self.t1 {
            let (base, pre0) = (t.last, t.pre);
            let dt = now - t.last;
            t.last = now;
            let con = m.mem[t.con];
            if con & 1 != 0 && dt > 0 {
                let cs = m.mem[t.clk] & 0x1F;
                if cs != 1 && cs != 2 {
                    self.notes.insert(format!("Timer1 clock source CS={cs} is not modelled (only FOSC/4 and FOSC); it does not count"));
                } else {
                    let ticks = if cs == 2 { dt * 4 } else { dt };
                    let clocks = prescale(&mut t.pre, u32::from((con >> 4) & 3), ticks);
                    let sh = u32::from((con >> 4) & 3);
                    let ratio = if cs == 2 { 4 } else { 1 };
                    let total = u64::from(t.cnt) + clocks;
                    if total >= 65536 {
                        t.flag.set(m);
                        let last_wrap = (total / 65536) * 65536 - u64::from(t.cnt);
                        ev.push(at(base, pre0, sh, last_wrap, ratio));
                    }
                    t.cnt = total as u16;
                }
            }
            if m.mem[t.con] & 2 == 0 {
                m.mem[t.h] = (t.cnt >> 8) as u8;
            }
            m.mem[t.l] = t.cnt as u8;
        }
        if let Some(t) = &mut self.t2 {
            let (base, pre0) = (t.last, t.pre);
            let dt = now - t.last;
            t.last = now;
            let con = m.mem[t.con];
            if con & 0x80 != 0 && dt > 0 {
                let cs = m.mem[t.clk] & 0x1F;
                let mode = m.mem[t.hlt] & 0x1F;
                if cs != 1 && cs != 2 {
                    self.notes.insert(format!("Timer2 clock source CS={cs} is not modelled (only FOSC/4 and FOSC); it does not count"));
                } else {
                    if mode != 0 && mode != 8 {
                        self.notes.insert(format!("Timer2 MODE={mode:#04x} needs an external reset/gate source (S4); run as the software mode"));
                    }
                    let one_shot = mode >> 3 == 1;
                    let ticks = if cs == 2 { dt * 4 } else { dt };
                    let clocks = prescale(&mut t.pre, u32::from((con >> 4) & 7), ticks);
                    let outps = con & 15;
                    let pr = m.mem[t.pr];
                    let mut left = clocks;
                    let sh = u32::from((con >> 4) & 7);
                    let ratio = if cs == 2 { 4 } else { 1 };
                    while left > 0 {
                        // Jump straight to the next period end when no event can happen before it.
                        if t.cnt != pr && t.cnt < pr && left > 1 {
                            let step = u64::from(pr - t.cnt).min(left - 1);
                            t.cnt += step as u8;
                            left -= step;
                            continue;
                        }
                        let j = clocks - left + 1;
                        left -= 1;
                        if t.cnt == pr {
                            t.cnt = 0;
                            if one_shot {
                                m.mem[t.con] &= 0x7F;
                                t.pre = 0;
                                t.post = 0;
                                t.flag.set(m);
                                ev.push(at(base, pre0, sh, j, ratio));
                                break;
                            }
                            if t.post >= outps {
                                t.post = 0;
                                t.flag.set(m);
                                ev.push(at(base, pre0, sh, j, ratio));
                            } else {
                                t.post += 1;
                            }
                        } else {
                            t.cnt = t.cnt.wrapping_add(1);
                        }
                    }
                }
            }
            m.mem[t.tmr] = t.cnt;
        }
        if let Some(a) = &mut self.adc {
            while let Some((e, v)) = a.end.filter(|(e, _)| *e <= now) {
                a.end = None;
                let word = if m.mem[a.con] & 4 != 0 { v } else { v << (16 - a.bits) };
                m.mem[a.resl] = word as u8;
                m.mem[a.resh] = (word >> 8) as u8;
                a.flag.set(m);
                ev.push(e);
                if m.mem[a.con] & 0x40 != 0 {
                    Self::adc_start(a, m, e); // [A14]
                } else {
                    m.mem[a.con] &= !1;
                }
            }
        }
        for u in &mut self.uarts {
            if m.mem[u.sta] & 0x10 != 0 {
                self.notes.insert(format!("EUSART{} synchronous mode is not modelled; nothing is sent", u.num));
            }
            while let Some(e) = u.tsr_end.filter(|e| *e <= now) {
                u.tsr_end = None;
                if let Some(b) = u.pending.take() {
                    self.tx.push((u.num, e, b));
                    u.tsr_end = Some(e + Self::uart_char_cycles(u, m));
                    ev.push(e);
                }
            }
            Self::uart_rx(u, m, now, &mut ev);
            Self::uart_bits(u, m);
        }
        for q in &mut self.quarts {
            q.sync(m, now, &mut ev, &mut self.tx, &mut self.notes);
        }
        for e in ev {
            self.raised(e);
        }
    }

    pub fn read(&mut self, k: u8, m: &mut DataMem, now: u64) -> u8 {
        match k {
            PORT0..=0x6F => {
                let v = self.port_levels(usize::from(k - PORT0), m, now);
                m.mem[self.ports[usize::from(k - PORT0)].port] = v;
                v
            }
            T1G => {
                let t = self.t1.as_ref().unwrap();
                let g = m.mem[t.gcon] & !4;
                g | if self.gate_value(m, now) { 4 } else { 0 }
            }
            T0L => {
                let t = self.t0.as_ref().unwrap();
                if m.mem[t.c0] & 0x10 != 0 {
                    m.mem[t.h] = (t.cnt >> 8) as u8;
                }
                t.cnt as u8
            }
            T1L => {
                let t = self.t1.as_ref().unwrap();
                if m.mem[t.con] & 2 != 0 {
                    m.mem[t.h] = (t.cnt >> 8) as u8;
                }
                t.cnt as u8
            }
            T1H => {
                let t = self.t1.as_ref().unwrap();
                if m.mem[t.con] & 2 != 0 { m.mem[t.h] } else { (t.cnt >> 8) as u8 }
            }
            T2T => self.t2.as_ref().unwrap().cnt,
            QUART0..QUART_END => {
                let i = usize::from(k - QUART0) / usize::from(quart::HOOKS);
                self.quarts[i].read_rxb(m)
            }
            UART0..=39 => {
                let u = &mut self.uarts[usize::from(k - UART0) / usize::from(UART_HOOKS)];
                // `[A10]` An empty FIFO reads the last character again (the data sheet does not say).
                if let Some((w, _)) = u.fifo.pop_front() {
                    m.mem[u.rcreg] = w as u8;
                }
                Self::uart_bits(u, m);
                m.mem[u.rcreg]
            }
            _ => unreachable!("read of hook {k} has no model"),
        }
    }

    /// Plain registers (TMR0H, T0CON0/1, T1CON, ...) read the stored byte.
    pub fn is_stored(k: u8) -> bool {
        !(matches!(k, T0L | T1L | T1H | T2T | T1G | PORT0..=0x6F) || (UART0..40).contains(&k) && (k - UART0) % UART_HOOKS == 3
            || (QUART0..QUART_END).contains(&k) && (k - QUART0) % quart::HOOKS == quart::RXB)
    }

    /// A write: `v` is already masked to the implemented bits. The caller has synced to the writing cycle.
    pub fn write(&mut self, k: u8, v: u8, now: u64, m: &mut DataMem) {
        match k {
            UART0..=39 => self.uart_write(k, v, now, m),
            QUART0..QUART_END => {
                let i = usize::from(k - QUART0) / usize::from(quart::HOOKS);
                self.quarts[i].write((k - QUART0) % quart::HOOKS, v, now, m, &mut self.tx);
            }
            T1G => {
                let a = self.t1.as_ref().unwrap().gcon;
                m.mem[a] = v & !4 | m.mem[a] & 4; // GVAL is read only
            }
            PORT0..=0x6F => {
                let p = &self.ports[usize::from(k - PORT0)];
                m.mem[p.lat] = v;
                m.mem[p.port] = v;
            }
            FVR0 => {
                let a = self.fvr.unwrap();
                if v & 0x80 != 0 {
                    self.notes.insert("FVR start-up time (25 us typ.) is not modelled; FVRRDY is set at once".into());
                }
                m.mem[a] = v & !0x40 | (v >> 1) & 0x40;
            }
            ADC0 => {
                let a = self.adc.as_mut().unwrap();
                let (on, go) = (v & 0x80 != 0, v & 1 != 0);
                let mut new = v & !1;
                if !(on && go) {
                    a.end = None;
                } else {
                    new |= 1;
                    if v & 0x10 != 0 {
                        self.notes.insert("the ADC FRC clock (ADCS = 1) is not modelled; ADGO never clears".into());
                    } else if a.end.is_none() {
                        m.mem[a.con] = new;
                        Self::adc_start(a, m, now);
                    }
                }
                m.mem[a.con] = new;
            }
            T0L => {
                let t = self.t0.as_mut().unwrap();
                t.pre = 0;
                t.post = 0;
                if m.mem[t.c0] & 0x10 != 0 {
                    t.cnt = u16::from(m.mem[t.h]) << 8 | u16::from(v);
                } else {
                    t.cnt = u16::from(v);
                }
                m.mem[t.l] = v;
            }
            T0H => {
                let t = self.t0.as_mut().unwrap();
                m.mem[t.h] = v;
                if m.mem[t.c0] & 0x80 == 0 {
                    t.cmp = v;
                }
            }
            T0C0 => {
                let t = self.t0.as_mut().unwrap();
                let was_on = m.mem[t.c0] & 0x80 != 0;
                let out = m.mem[t.c0] & 0x20; // T0OUT is read only
                m.mem[t.c0] = v & !0x20 | out;
                t.pre = 0;
                t.post = 0;
                if !was_on && v & 0x80 != 0 {
                    t.cmp = m.mem[t.h];
                }
            }
            T0C1 => {
                let t = self.t0.as_mut().unwrap();
                m.mem[t.c1] = v;
                t.pre = 0;
                t.post = 0;
            }
            T1L => {
                let t = self.t1.as_mut().unwrap();
                t.pre = 0;
                if m.mem[t.con] & 2 != 0 {
                    t.cnt = u16::from(m.mem[t.h]) << 8 | u16::from(v);
                } else {
                    t.cnt = t.cnt & 0xFF00 | u16::from(v);
                }
                m.mem[t.l] = v;
            }
            T1H => {
                let t = self.t1.as_mut().unwrap();
                t.pre = 0;
                if m.mem[t.con] & 2 == 0 {
                    t.cnt = t.cnt & 0xFF | u16::from(v) << 8;
                }
                m.mem[t.h] = v;
            }
            T1C => {
                let t = self.t1.as_mut().unwrap();
                m.mem[t.con] = v;
                if v & 1 != 0 && m.mem[t.gcon] & 0x80 != 0 {
                    self.notes.insert("Timer1 gate (GE) is not modelled; the timer counts regardless of the gate".into());
                }
            }
            T1CLK | T2P | T2HLT | T2CLK => {
                let a = self.addr_of(k);
                m.mem[a] = v;
            }
            T2T => {
                let t = self.t2.as_mut().unwrap();
                t.cnt = v;
                t.pre = 0;
                t.post = 0;
                m.mem[t.tmr] = v;
            }
            T2C => {
                let t = self.t2.as_mut().unwrap();
                m.mem[t.con] = v;
                t.pre = 0;
                t.post = 0;
            }
            _ => unreachable!("write of hook {k} has no model"),
        }
    }

    fn uart_write(&mut self, k: u8, v: u8, now: u64, m: &mut DataMem) {
        let (i, sub) = (usize::from(k - UART0) / usize::from(UART_HOOKS), (k - UART0) % UART_HOOKS);
        let u = &mut self.uarts[i];
        match sub {
            0 => {
                m.mem[u.reg] = v;
                if m.mem[u.sta] & 0x20 != 0 {
                    if u.tsr_end.is_none() {
                        self.tx.push((u.num, now, v));
                        u.tsr_end = Some(now + Self::uart_char_cycles(u, m));
                    } else {
                        u.pending = Some(v);
                    }
                }
            }
            1 => {
                m.mem[u.sta] = v & !2 | m.mem[u.sta] & 2; // TRMT is read only
                if v & 0x20 == 0 {
                    u.tsr_end = None;
                    u.pending = None;
                }
            }
            2 => m.mem[u.baud] = v & !0xC0 | m.mem[u.baud] & 0xC0, // ABDOVF and RCIDL are read only
            3 => {} // RCREG is read only
            _ => {
                // OERR, FERR and RX9D belong to the hardware. `[A11]` Clearing CREN clears OERR and abandons the
                // character in flight (the FIFO keeps its characters); clearing SPEN resets the receiver.
                let mut new = v & !7 | m.mem[u.rcsta] & 7;
                if v & 0x80 == 0 {
                    u.fifo.clear();
                    u.rx_cur = None;
                    new &= !7;
                } else if v & 0x10 == 0 {
                    u.rx_cur = None;
                    new &= !2;
                }
                m.mem[u.rcsta] = new;
            }
        }
        Self::uart_bits(u, m);
    }

    /// Start a conversion at cycle `t`: end cycle and the code it will load.
    fn adc_start(a: &mut Adc, m: &DataMem, t: u64) {
        let n = u64::from(m.mem[a.clk] & 0x3F) + 1; // TAD = 2n Fosc periods
        let lead = u64::from(m.mem[a.pre]) + u64::from(m.mem[a.acq]);
        let sample = t + (2 * n * lead).div_ceil(4);
        let end = t + (n * (2 * lead + 23)).div_ceil(4);
        let ch = m.mem[a.pch] & 0x3F;
        let code = a.feed.iter().filter(|(c, at, _)| *c == ch && *at <= sample).last().map_or(0, |f| f.2);
        a.end = Some((end, code.min((1u16 << a.bits) - 1)));
    }

    /// Levels of the pins of port `i` at cycle `now`. An output pin shows its latch, an input pin what the outside world
    /// drives (0 until a stimulus says otherwise `[A16]`: no pull-up is modelled), and 0 when ANSELx makes it analog.
    fn port_levels(&self, i: usize, m: &DataMem, now: u64) -> u8 {
        let p = &self.ports[i];
        let mut ext = 0u8;
        for &(at, pi, mask, val) in &self.drive {
            if at <= now && pi == i {
                ext = ext & !mask | val & mask;
            }
        }
        let tris = m.mem[p.tris];
        let ana = p.ansel.map_or(0, |a| m.mem[a]);
        (m.mem[p.lat] & !tris | ext & tris & !ana) & m.implemented[p.port]
    }

    /// Level of the pin a PPS input register selects (PORT field at bits 5:3, PIN at 2:0).
    fn pps_pin(&self, m: &DataMem, reg: usize, now: u64) -> bool {
        let v = m.mem[reg];
        let letter = b'A' + (v >> 3 & 7);
        self.ports.iter().position(|p| p.letter == letter).is_some_and(|i| self.port_levels(i, m, now) >> (v & 7) & 1 != 0)
    }

    /// Timer1 gate value (GVAL): the gate source after the polarity bit. `[A17]` MDB (probed with every GE/GPOL/GTM/GSPM
    /// combination on a low pin) reads GVAL = 0 while GE = 0, although the data sheet says GVAL is unaffected by GE
    /// (DS40002043F 20.14.2 p.318); the model follows MDB. Single-pulse and toggle mode do not change it either.
    /// Only the pin source (GSS = 0, T1GPPS) is modelled; others read as a low signal.
    fn gate_value(&mut self, m: &DataMem, now: u64) -> bool {
        let t = self.t1.as_ref().unwrap();
        let (gate, gpps, gcon) = (t.gate, t.gpps, t.gcon);
        if m.mem[gcon] & 0x80 == 0 {
            return false;
        }
        let src = match (gate.map(|g| m.mem[g] & 31), gpps) {
            (Some(0), Some(r)) => self.pps_pin(m, r, now),
            _ => {
                self.notes.insert("Timer1 gate source other than the T1GPPS pin is not modelled; GVAL sees it low".into());
                false
            }
        };
        src == (m.mem[gcon] & 0x40 != 0)
    }

    /// Stimulus: the pins `mask` of port `letter` are driven to `val` from cycle `at` on.
    pub fn pin_feed(&mut self, letter: u8, at: u64, mask: u8, val: u8) -> bool {
        match self.ports.iter().position(|p| p.letter == letter) {
            Some(i) => {
                self.drive.push((at, i, mask, val));
                self.drive.sort_by_key(|d| d.0);
                true
            }
            None => false,
        }
    }

    /// Stimulus: the input of ADC channel `ch` is code `v` (full scale = the part's resolution) from cycle `at` on.
    pub fn adc_feed(&mut self, ch: u8, at: u64, v: u16) -> bool {
        match &mut self.adc {
            Some(a) => {
                a.feed.push((ch, at, v));
                a.feed.sort_by_key(|f| f.1); // stable: same cycle keeps file order
                true
            }
            None => false,
        }
    }

    fn addr_of(&self, k: u8) -> usize {
        match k {
            T1CLK => self.t1.as_ref().unwrap().clk,
            T2P => self.t2.as_ref().unwrap().pr,
            T2HLT => self.t2.as_ref().unwrap().hlt,
            _ => self.t2.as_ref().unwrap().clk,
        }
    }
}
