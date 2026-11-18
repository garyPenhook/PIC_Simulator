//! Peripheral models that live on the instruction clock: Timer0, Timers 1-6 and the interrupt flag
//! registers they set. Models are advanced lazily: `sync(now)` brings every timer to the cycle count `now`
//! (an instruction boundary or a register write), setting the PIRx flag bits on the cycle they happen.
//!
//! Rules and sources (DS40002043F is the PIC18F27/47Q10 data sheet; the Q41 timers have the same layout):
//! * Timer0 (ch.19, p.296-298): 8-bit mode compares TMR0L with the TMR0H buffer and resets TMR0L on the next clock
//!   (period TMR0H+1 clocks); 16-bit mode rolls over at FFFFh; TMR0H is buffered in 16-bit mode (read of TMR0L
//!   loads it, write of TMR0L stores it); prescaler 1:1..1:32768 (T0CKPS) and postscaler 1:1..1:16 (T0OUTPS) are
//!   cleared by a write to TMR0L, T0CON0 or T0CON1; TMR0IF is set when TMR0_out toggles, i.e. every T0OUTPS+1
//!   matches or rollovers; FOSC/4 source = T0CS 010 (Table 19-1); the pin, LFINTOSC and SOSC too `[A102]`.
//! * Timer1 (ch.20, p.305-313): 16-bit up counter, prescaler 1/2/4/8, rollover from FFFFh to 0000h sets TMR1IF;
//!   RD16 buffers TMR1H like Timer0 (20.6). Clock FOSC/4 = CS 1, FOSC = CS 2 (TMRxCLK); the pin, LFINTOSC and SOSC too `[A102]`.
//! * Timer2 (ch.21, p.324-327): T2TMR counts up, compares with T2PR, goes to 0 on the next clock; the postscaler
//!   counts matches and the flag is set when it reaches OUTPS+1; prescaler and postscaler are cleared by a write to
//!   T2TMR or T2CON, T2TMR is not; one-shot software start (MODE 01000) clears ON at the period match.
//!   Hardware gate/reset modes need pin models (S4) and are run as the software mode with a note.
//! Assumptions `[A#]` (to be measured against MDB):
//! * `[A1]` The flag is set on the clock that takes the count from the compare value back to 0 (period clocks apart).
//! * `[A2]` A write takes effect after the cycle of the writing instruction that fetched it, so the first counted cycle
//!   is the one after a one-cycle writing instruction.
//! * `[A3]` Timer0 keeps its count when T0EN is cleared; Timer1/Timer2 keep it when ON is cleared.

use crate::log::Log;
use crate::mem::{DataMem, Special};
use devdesc::Device;
use std::collections::{BTreeSet, VecDeque};

pub(crate) mod adc;
mod board;
mod ccp;
pub(crate) mod ccp16;
pub(crate) mod clb;
mod clc;
mod crc;
pub(crate) mod pwmq;
pub(crate) mod pwm16;
mod crcq;
pub(crate) mod wdt;
mod cmp;
mod cwg;
mod flash;
mod nvm;
pub(crate) mod nvm16;
mod nvmq;
mod i2cq;
pub(crate) mod mssp;
mod quart;
pub(crate) mod uart;
mod spi;
pub use board::{parse_pin, pin_name, Board, Bus, Led, Oled, Pin, PinView};
use ccp::{Ccp, Pwm};
use clc::Clc;
use crc::Crc;
use wdt::Wdt;
use cmp::Cmp;
use cwg::Cwg;
use i2cq::I2cQ;
pub(crate) use mssp::Mssp;
use nvm::Nvm;
use nvmq::NvmQ;
use quart::QUart;
use uart::Uart;
use spi::Spi;
pub use mssp::{I2cClient, I2cEv};
pub use spi::SpiClient;

/// Why the part resets without a power-on (DS40002043F Table 9-3 p.89-90).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetKind {
    /// WWDT time-out.
    Wdt,
    /// CLRWDT outside the open window, or without arming.
    WdtWindow,
    /// The RESET instruction.
    Instruction,
    /// A push onto a full stack with STVREN = 1.
    StackOverflow,
    /// A pop from an empty stack with STVREN = 1.
    StackUnderflow,
    /// The MCLR pin driven low while it is the reset input.
    Mclr,
    /// A vectored interrupt whose table entry or service routine lies outside the program memory (Q41).
    MemoryViolation,
}

/// Interrupt-on-change and the INT0-INT2 pins `[A101]`.
#[derive(Clone, Default)]
struct Ioc {
    /// (index into `ports`, IOCxF, IOCxN, IOCxP)
    ports: Vec<(usize, usize, usize, usize)>,
    /// (INTxPPS, INTxIF, INTCON register and mask of INTxEDG)
    ints: Vec<(usize, Flag, usize, u8)>,
    iocif: Option<Flag>,
    /// K22 PORTB mismatch latch: port index, IOCB, RBIF, last read/written pins.
    k22: Option<(usize, usize, Flag, u8)>,
    /// The stimulus has been looked at up to this cycle.
    scan: u64,
}

/// A flag bit in a PIRx register.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Flag {
    pub(crate) addr: usize,
    pub(crate) mask: u8,
}

impl Flag {
    fn find(dev: &Device, bit: &str) -> Option<Flag> {
        // PIRx first; then INTCON, which holds TMR0IF on the PIC18(L)F2x/4xK22 (DS40001412H Register 9-1 p.109).
        for s in dev.sfr.iter().filter(|s| s.name.starts_with("PIR")).chain(dev.sfr.iter().filter(|s| s.name.starts_with("INTCON"))) {
            if let Some(f) = s.fields.iter().chain(s.legacy.iter()).find(|f| f.name == bit) {
                return Some(Flag { addr: s.addr as usize, mask: 1 << f.pos });
            }
        }
        None
    }
    fn set(self, m: &mut DataMem) {
        self.set_in(&mut m.mem);
        if let Some(q) = &mut m.dma_requests { q.push((self.addr, self.mask)); }
    }
    fn clear(self, m: &mut DataMem) {
        self.clear_in(&mut m.mem);
    }
    fn set_in(self, mem: &mut [u8]) {
        mem[self.addr] |= self.mask;
    }
    fn clear_in(self, mem: &mut [u8]) {
        mem[self.addr] &= !self.mask;
    }
}

/// A pin that has no PPS input register stands in its place as `FIXED_PIN + PORT * 8 + PIN` (the PPS code of the pin), beyond
/// every data address: `pps_pin` decodes it, and it is never a register to hook or watch. Used for the fixed T1CKI (RC0) and
/// T1G (RB5) of the PIC18(L)F2x/4xK22 (DS40001412H Tables 2 and 3 p.6-7).
pub(crate) const FIXED_PIN: usize = 1 << 20;

/// The output source code of an RxyPPS register, or of a fixed routing `FIXED_PIN + code`.
fn out_code(m: &DataMem, reg: usize) -> u8 {
    if reg >= FIXED_PIN { (reg - FIXED_PIN) as u8 } else { m.mem[reg] & 31 }
}

fn addr(dev: &Device, name: &str) -> Option<usize> {
    dev.find_sfr(name).map(|s| s.addr as usize)
}

/// Peripheral-module-disable bit, if supplied by this part's register description.
fn pmd(dev: &Device, name: &str) -> Option<(usize, u8)> {
    dev.sfr.iter().find_map(|s| s.fields.iter().find(|f| f.name == name).map(|f| (s.addr as usize, 1 << f.pos)))
}
fn pmd_off(m: &DataMem, bit: Option<(usize, u8)>) -> bool { bit.is_some_and(|(a, mask)| m.mem[a] & mask != 0) }

/// Clocks produced by `dt` input ticks through a prescaler of `1 << shift` whose counter is `pre`.
#[inline]
/// `PICSIM_CHECK_LAZY` is set (read once).
pub(crate) fn check_lazy() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PICSIM_CHECK_LAZY").is_some_and(|v| v != "0"))
}

fn prescale(pre: &mut u64, shift: u32, dt: u64) -> u64 {
    let t = *pre + dt;
    *pre = t & ((1u64 << shift) - 1);
    t >> shift
}

/// Where a Timer0 / Timer1-type clock comes from.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Clk {
    /// This many counter ticks per instruction cycle (Fosc/4: 1, Fosc: 4).
    Cyc(u64),
    /// The pin of TxCKIPPS; `true` counts falling edges (Timer0 CS = 001).
    Pin(bool),
    /// An oscillator of this frequency in Hz that does not stop in Sleep.
    Hz(u64),
    /// Not modelled: it does not count.
    Other,
}

const LFINTOSC_HZ: u64 = 31_000;
const SOSC_HZ: u64 = 32_768;

/// Timer0 clock source T0CS (Q10: DS40002043F Table 19-1 p.296; Q41: T0CON1 DS40002214G p.360).
fn t0_clk(q41: bool, cs: u8) -> Clk {
    match cs {
        0 => Clk::Pin(false),
        1 => Clk::Pin(true),
        2 => Clk::Cyc(1),
        4 => Clk::Hz(LFINTOSC_HZ),
        5 if !q41 => Clk::Hz(SOSC_HZ),
        6 if q41 => Clk::Hz(SOSC_HZ),
        _ => Clk::Other,
    }
}

/// Timer1/3/5 clock source CS (Q10: DS40002043F Table 20-2 p.306; Q41: Table 25-4 DS40002214G p.377).
fn t1_clk(q41: bool, cs: u8) -> Clk {
    match cs {
        0 => Clk::Pin(false),
        1 => Clk::Cyc(1),
        2 => Clk::Cyc(4),
        4 => Clk::Hz(LFINTOSC_HZ),
        6 if !q41 => Clk::Hz(SOSC_HZ),
        7 if q41 => Clk::Hz(SOSC_HZ),
        _ => Clk::Other,
    }
}

/// How the edges of one clock fall in a window of cycles.
enum How {
    /// `r` ticks per cycle, evenly.
    Ratio(u64),
    /// Independent oscillator, with elapsed time `ns` at `base`; preserve its phase across Fosc changes.
    Time { hz: u64, fosc: u64, e0: u64, base: u64, ns: f64 },
    /// The cycles of the pin edges, in order.
    List(Vec<u64>),
}

/// The counter ticks that a clock makes between two cycles.
struct Edges {
    n: u64,
    how: How,
}

impl Edges {
    /// Cycle of the `j`-th prescaler output clock after `base`, with `pre0` ticks already in the prescaler (shift `sh`).
    fn at(&self, base: u64, pre0: u64, sh: u32, j: u64) -> u64 {
        match &self.how {
            How::Ratio(r) => at(base, pre0, sh, j, *r),
            How::Time { hz, fosc, e0, base, ns } => {
                let k = e0 + (j << sh) - pre0;
                base + (((k as f64 * 1e9 / *hz as f64 - ns) * *fosc as f64 / 4e9) - 1e-9).ceil().max(0.0) as u64
            }
            How::List(l) => l[((j << sh) - pre0 - 1) as usize],
        }
    }
}

// Hook codes stored in `Special::Hook`.
const T0L: u8 = 1;
const T0H: u8 = 2;
const T0C0: u8 = 3;
const T0C1: u8 = 4;
const T1L: u8 = 5;
const T1CLK: u8 = 8;
const T2T: u8 = 9;
const T2CLK: u8 = 13;
/// Timer1-type timers (index 0 = Timer1, 1 = Timer3, 2 = Timer5): sub-code 0 TMRxL, 1 TMRxH, 2 TxCON, 3 TxCLK, 4 TxGCON.
/// Index 0 keeps the first hook codes, the others use 226..=235.
fn t1_hook(i: usize, sub: u8) -> u8 {
    match (i, sub) {
        (0, 4) => T1G,
        (0, s) => T1L + s,
        (i, s) => 226 + 5 * (i as u8 - 1) + s,
    }
}

/// Inverse of `t1_hook`: (timer index, sub-code).
fn t1_dec(k: u8) -> Option<(usize, u8)> {
    match k {
        T1L..=T1CLK => Some((0, k - T1L)),
        T1G => Some((0, 4)),
        226..=235 => Some((1 + usize::from(k - 226) / 5, (k - 226) % 5)),
        _ => None,
    }
}

/// Timer2-type timers (index 0 = Timer2, 1 = Timer4, 2 = Timer6): sub-code 0 TxTMR, 1 TxPR, 2 TxCON, 3 TxHLT, 4 TxCLKCON.
/// Index 0 keeps the first hook codes, the others use T2X0..=T2X0 + 9 (64..=73, after the ports).
const T2X0: u8 = PORT_END;
fn t2_hook(i: usize, sub: u8) -> u8 {
    if i == 0 { T2T + sub } else { T2X0 + 5 * (i as u8 - 1) + sub }
}

fn t2_dec(k: u8) -> Option<(usize, u8)> {
    match k {
        T2T..=T2CLK => Some((0, k - T2T)),
        T2X0..=73 => Some((1 + usize::from(k - T2X0) / 5, (k - T2X0) % 5)),
        _ => None,
    }
}

/// ADCON0 of the ADCC (basic mode).
const ADC0: u8 = 14;
/// EUSART n (0-based) uses hook codes UART0 + 5n + {TXREG 0, TXSTA 1, BAUDCON 2, RCREG 3, RCSTA 4}.
/// FVRCON: FVRRDY (read only) follows FVREN. `[A15]` at once; the data sheet gives only a typical start-up time of
/// 25 us (FVR04, DS40002043F p.707) and the models do not know Fosc. A note is added when it is first enabled.
const FVR0: u8 = 15;
const UART0: u8 = 16;
const UART_HOOKS: u8 = uart::HOOKS;
/// Q41 UART n (0-based) uses hook codes QUART0 + quart::HOOKS * n + sub-code (see `quart.rs`); at most 5 UARTs.
const QUART0: u8 = 112;
const QUART_END: u8 = QUART0 + 5 * quart::HOOKS;
/// T1GCON: GVAL (bit 2) is read only and follows the gate source.
const T1G: u8 = 40;
/// PORTx (index into `ports`) uses hook code PORT0 + index: a read gives the pin levels, a write goes to LATx.
const PORT0: u8 = 48;
/// At most 16 ports (48..=63).
const PORT_END: u8 = PORT0 + 16;
/// Q41 SPI n (0-based) uses hook codes SPI0 + spi::HOOKS * n + sub-code (see `spi.rs`); at most 2.
const SPI0: u8 = 160;
const SPI_END: u8 = SPI0 + 2 * spi::HOOKS;
/// Q10 MSSP n (0-based) uses hook codes MSSP0 + mssp::HOOKS * n + sub-code (see `mssp.rs`); at most 2.
const MSSP0: u8 = 190;
const MSSP_END: u8 = MSSP0 + 2 * mssp::HOOKS;
/// Q41 I2C n (0-based) uses hook codes I2CQ0 + i2cq::HOOKS * n + sub-code (see `i2cq.rs`); at most 2.
const I2CQ0: u8 = 202;
const I2CQ_END: u8 = I2CQ0 + 2 * i2cq::HOOKS;
/// Q10 CCP n (0-based) uses hook code CCP0 + n (CCPxCON only, see `ccp.rs`); at most 2.
const CCP0: u8 = 224;
/// The third to fifth CCP (CCP3..CCP5 of the PIC18(L)F2x/4xK22) use CCPK0 + n - 2 (free codes after the Timer2-type hooks).
const CCPK0: u8 = 74;
const CCPK_END: u8 = CCPK0 + 2;

/// The hook code of the CCP with index `i` (0-based).
fn ccp_hook(i: usize) -> u8 {
    if i < 2 { CCP0 + i as u8 } else { CCPK0 + i as u8 - 2 }
}
/// Q10 PWM3/PWM4 (PWMxCON only) use hook codes PWM0 + n.
const PWM0: u8 = 236;
/// Q10 CWG1 registers use hook codes CWG0 + cwg sub-code (see `cwg.rs`).
const CWG0: u8 = 41; // 41..=47 (T1G is 40, the ports start at 48)
const CWG_END: u8 = CWG0 + cwg::HOOKS - 1;
/// Q10 comparator n (0-based) uses hook codes CMP0 + cmp::HOOKS * n + sub-code (CMxCON0, CMxNCH, CMxPCH); CMOUT is CMOUT_HOOK.
const CMP0: u8 = 184;
const CMP_END: u8 = CMP0 + 2 * cmp::HOOKS;
const CMOUT_HOOK: u8 = 222;
/// Q10/Q41 WWDT registers (sub-codes of `wdt.rs`) use the codes that are left over: 157..=159 and 238, 239.
fn wdt_hook(sub: u8) -> u8 {
    [157, 158, 159, 238, 239][usize::from(sub)]
}

fn wdt_dec(k: u8) -> Option<u8> {
    match k {
        157..=159 => Some(k - 157),
        238 | 239 => Some(k - 238 + 3),
        _ => None,
    }
}
const NVM0: u8 = 240;
const NVM_LAST: u8 = NVM0 + nvm::CON2;
const NVMQ0: u8 = 242;
const NVMQ_LAST: u8 = NVMQ0 + nvmq::LOCK;

/// A repeatable request/reply device attached to a UART's completed TX characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UartReply {
    request: Vec<u8>,
    reply: Vec<u8>,
    delay: u64,
    bit: Option<u64>,
    gap: u64,
    seen: VecDeque<u8>,
}

impl UartReply {
    pub fn new(request: Vec<u8>, reply: Vec<u8>, delay: u64, bit: Option<u64>, gap: u64) -> Self {
        Self { request, reply, delay, bit, gap, seen: VecDeque::new() }
    }

    pub(crate) fn receive(&mut self, byte: u8, end: u64) -> Option<RxStream> {
        self.seen.push_back(byte);
        if self.seen.len() > self.request.len() { self.seen.pop_front(); }
        if self.request.is_empty() || !self.seen.iter().eq(self.request.iter()) { return None; }
        Some(RxStream { at: end.saturating_add(self.delay), words: self.reply.iter().copied().map(u16::from).collect(), bit: self.bit, gap: self.gap, nine: false, bad_stop: false })
    }
}

pub(crate) fn uart_replies(rules: &mut [UartReply], byte: u8, end: u64, queue: &mut VecDeque<RxStream>) {
    for rule in rules {
        if let Some(stream) = rule.receive(byte, end) {
            let i = queue.partition_point(|s| s.at <= stream.at);
            queue.insert(i, stream);
        }
    }
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

#[derive(Clone)]
struct Timer0 {
    l: usize,
    h: usize,
    c0: usize,
    c1: usize,
    flag: Flag,
    cnt: u16,
    pre: u64,
    post: u8,
    last: u64,
    /// T0CKIPPS, the pin of the clock sources 000 and 001.
    cki: Option<usize>,
    /// Set at SLEEP when the clock keeps this timer running in Sleep `[A102]`.
    sleep_run: bool,
    /// The older Timer0 of the PIC18(L)F2x/4xK22: one T0CON (here both `c0` and `c1`) = TMR0ON (7) | T08BIT (6) | T0CS (5) | T0SE (4) |
    /// PSA (3) | T0PS<2:0>; 8-bit mode counts to FFh (no period register), no postscaler, no T0OUT, prescaler 1:2..1:256 or none
    /// (DS40001412H Register 11-1 p.154, 11.2-11.5 p.155-156).
    classic: bool,
}

impl Timer0 {
    /// 16-bit mode: MD16 (T0CON0 bit 4), or T08BIT = 0 on the classic timer.
    fn wide(&self, m: &DataMem) -> bool {
        if self.classic { m.mem[self.c0] & 0x40 == 0 } else { m.mem[self.c0] & 0x10 != 0 }
    }
    /// Prescaler shift: T0CKPS (T0CON1 bits 3:0, 1:2^n), or on the classic timer 0 with PSA = 1 and T0PS + 1 otherwise.
    fn shift(&self, m: &DataMem) -> u32 {
        let c = m.mem[self.c1];
        if !self.classic {
            u32::from(c & 15)
        } else if c & 8 != 0 {
            0
        } else {
            u32::from(c & 7) + 1
        }
    }
    /// Postscaler count (T0OUTPS); the classic timer has none.
    fn outps(&self, m: &DataMem) -> u8 {
        if self.classic { 0 } else { m.mem[self.c0] & 15 }
    }
    /// The 8-bit count that ends a period: TMR0H, or FFh on the classic timer.
    fn top8(&self, m: &DataMem) -> u8 {
        if self.classic { 0xFF } else { m.mem[self.h] }
    }
}

/// What the Timer1 gate does over one window of cycles: the stretches in which the timer counts, the falling edges of the
/// gate (TMRxGIF) and the gate state at the end.
#[derive(Default)]
struct GatePlan {
    /// (from, to, counts): ticks on cycles from+1 ..= to.
    segs: Vec<(u64, u64, bool)>,
    gif: Vec<u64>,
    ff: bool,
    run: bool,
    ggo: bool,
    b: Option<bool>,
}

/// Where the gate of a Timer1-type timer comes from.
#[derive(Clone, Copy)]
enum GSrc {
    /// The pin of the TxGPPS register (address).
    Pin(usize),
    /// A PPS output source (code of Table 18-2): CCP, PWM, comparator and CLC outputs.
    Sig(u8),
    None,
}

#[derive(Clone)]
struct Timer1 {
    /// Data sheet number: 1, 3 or 5.
    n: u8,
    /// A compare with clearing makes the timer go to 0 on its next clock.
    clr: bool,
    gate: Option<usize>,
    gpps: Option<usize>,
    /// TMRxGIF, set on the falling edge of the gate (20.7.6).
    gif: Option<Flag>,
    /// Gate toggle flip-flop (GTM) and the single-pulse "counting enabled" state (GSPM).
    gff: bool,
    grun: bool,
    /// Gate level (after GPOL) at the last look, to see a change that software made to the pin in between.
    gb: Option<bool>,
    l: usize,
    h: usize,
    con: usize,
    clk: usize,
    gcon: usize,
    flag: Flag,
    cnt: u16,
    pre: u64,
    last: u64,
    /// TxCKIPPS, the pin of clock source 00000.
    cki: Option<usize>,
    /// Set at SLEEP when the clock keeps this timer running in Sleep `[A102]`.
    sleep_run: bool,
    /// The older Timer1/3/5 of the PIC18(L)F2x/4xK22: no TxCLK and no TxGATE; the clock is TMRxCS<1:0> of TxCON (bits 7:6, 00
    /// FOSC/4, 01 FOSC, 10 the TxCKI pin or with TxSOSCEN the secondary oscillator, 11 reserved), the gate source TxGSS<1:0> of
    /// TxGCON (00 the TxG pin, 01 the Timer2/4/6 match, 10 and 11 the comparators) (DS40001412H Registers 12-1, 12-2 p.166-167).
    /// `clk` and `gate` then hold TxCON and TxGCON.
    classic: bool,
}

#[derive(Clone)]
struct Timer2 {
    /// Data sheet number: 2, 4 or 6.
    n: u8,
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
    /// The older Timer2/4/6 of the PIC18(L)F2x/4xK22: TMRx/PRx, TxCON = TxOUTPS<3:0> (6:3) | TMRxON (2) | TxCKPS<1:0> (prescaler 1, 4,
    /// 16), FOSC/4 only, no TxHLT or TxCLKCON (`hlt` and `clk` hold TxCON) (DS40001412H Register 13-1 p.171, 13.1 p.170).
    classic: bool,
}

impl Timer2 {
    fn on(&self, m: &DataMem) -> bool {
        m.mem[self.con] & if self.classic { 0x04 } else { 0x80 } != 0
    }
    /// Prescaler shift: TxCKPS (6:4, 1:2^n), or on the classic timer 0, 2, 4 for 1, 4, 16 (1x is 16).
    fn shift(&self, m: &DataMem) -> u32 {
        let c = m.mem[self.con];
        if self.classic { [0, 2, 4, 4][usize::from(c & 3)] } else { u32::from((c >> 4) & 7) }
    }
    fn outps(&self, m: &DataMem) -> u8 {
        let c = m.mem[self.con];
        if self.classic { (c >> 3) & 15 } else { c & 15 }
    }
    /// Clock source: TxCLKCON CS (1 = FOSC/4, 2 = FOSC), FOSC/4 on the classic timer.
    fn cs(&self, m: &DataMem) -> u8 {
        if self.classic { 1 } else { m.mem[self.clk] & 0x1F }
    }
    /// Hardware-limit mode (TxHLT), none on the classic timer.
    fn mode(&self, m: &DataMem) -> u8 {
        if self.classic { 0 } else { m.mem[self.hlt] & 0x1F }
    }
}

/// A port with pins: PORTx reads the pins, a write goes to LATx (DS40002043F 17.1: the pin level is read, the
/// latch is written). Input pins with their ANSELx bit set read 0 (the digital input buffer is off, 17.2).
#[derive(Clone)]
struct Port {
    letter: u8,
    port: usize,
    lat: usize,
    tris: usize,
    ansel: Option<usize>,
    wpu: Option<usize>,
    odcon: Option<usize>,
    wpu_disable: Option<(usize, u8)>,
}

#[derive(Clone, PartialEq, Eq)]
struct PinState {
    output: u8, high: u8, od: u8, weak: u8, analog: u8, implemented: u8,
    codes: [u8; 8], uart: [u8; 2], quart: [u8; 5], inverted: u8,
}

#[derive(Clone)]
struct I2cPps {
    num: u8,
    scl_in: usize,
    sda_in: usize,
    scl_name: String,
    sda_name: String,
    scl_code: u8,
    sda_code: u8,
}

#[derive(Clone)]
pub struct Periph {
    q41: bool,
    pwmqs: Vec<pwmq::PwmQ>,
    pwm_en: Option<usize>,
    pwm_load: Option<usize>,
    clc_select: Option<usize>,
    ports: Vec<Port>,
    /// What the outside world drives on the input pins: (from cycle, port index, pin mask, levels), stable-sorted by cycle.
    drive: crate::drive::Drive,
    t0: Option<Timer0>,
    t1: Vec<Timer1>,
    t2: Vec<Timer2>,
    uarts: Vec<Uart>,
    uart_pins: Vec<(usize, u8)>,
    uart_scan: u64,
    pin_history: Vec<VecDeque<(u64, PinState)>>,
    quarts: Vec<QUart>,
    quart_pins: Vec<usize>,
    spis: Vec<Spi>,
    mssps: Vec<Mssp>,
    i2cqs: Vec<I2cQ>,
    adc: Option<adc::Adc>,
    nvm: Option<Nvm>,
    nvmq: Option<NvmQ>,
    /// Cycles the CPU is stalled by a program flash operation; the core takes them after the write (`take_stall`).
    stall: u64,
    fvr: Option<usize>,
    /// Bytes the transmitters started to send: (EUSART number 1-based, cycle it started, byte).
    pub tx: Log<(u8, u64, u8)>,
    /// Bytes the SPI hosts sent: (SPI number, cycle the byte started, byte).
    pub spi_tx: Log<(u8, u64, u8)>,
    /// What the I2C hosts put on the bus: (MSSP number, cycle, event).
    pub i2c_log: Log<(u8, u64, I2cEv)>,
    pub notes: BTreeSet<String>,
    ccps: Vec<Ccp>,
    pwms: Vec<Pwm>,
    cwg: Option<Cwg>,
    cmps: Vec<Cmp>,
    cmout: Option<usize>,
    /// Configurable logic cells, the CLCINxPPS registers, CLCDATA, "inside `clc_eval`" and the cycle stimulus was last looked at.
    clcs: Vec<Clc>,
    clcin: Vec<usize>,
    clcdata: Option<usize>,
    in_clc: bool,
    clc_scan: u64,
    /// CRC with memory scanner (Q10).
    crc: Option<Crc>,
    crcq: Option<crcq::CrcQ>,
    wdt: Option<Wdt>,
    /// A reset the watchdog or CLRWDT asks for; the core takes it after the instruction (`take_reset`).
    reset_req: Option<ResetKind>,
    /// The part is in Sleep (set by the core); a watchdog time-out then wakes it instead of resetting `[A93]`.
    sleeping: bool,
    /// Cycle the part went to sleep.
    sleep_at: u64,
    /// Fosc and Fosc/4 are off: Sleep, not Idle (DS40002043F 7.3-7.4 p.71).
    fosc_off: bool,
    wdt_wake: bool,
    /// Stimulus: (from cycle, port letter, pin, volts), stable-sorted by cycle.
    analog: Vec<(u64, u8, u8, f64)>,
    /// A compare match with TMR1 clearing happened on the last clock: TMR1 goes to 0 on the next one `[A61]`.
    ccptmrs: ccp::TmrSel,
    /// PIC18(L)F2x/4xK22: the part has ports D and E (the 40/44-pin parts), for the fixed CCP pins (`route_k22_ccp`).
    k22_ccp: Option<bool>,
    pub(crate) suppress_ioc_latch: bool,
    /// RxyPPS registers: (register, index into `ports`, pin).
    ppsout: Vec<(usize, usize, u8)>,
    /// `ppsout` per port index: (RxyPPS address, bit). Built once at the end of `bind`, like `ppsout`.
    pps_by_port: Vec<Vec<(usize, u8)>>,
    /// Bit n set: signal code n has a CCP, PWM, comparator or CLC source (those lists are fixed after `bind`).
    sig_src: u32,
    /// Hardware I2C hosts: the PPS input registers and the output codes of SCL and SDA.
    i2c_pps: Vec<I2cPps>,
    /// MSSP hosts with fixed I2C pins (no PPS, the PIC18(L)F2x/4xK22): (module number, SCL pin, SDA pin).
    i2c_fixed: Vec<(u8, (u8, u8), (u8, u8))>,
    /// The parts on the board (LEDs, OLED).
    pub board: Board,
    /// Pins with an external pull-up (bit-banged I2C lines) and pins the OLED holds low, per port.
    pullup: Vec<u8>,
    forced: Vec<u8>,
    /// Levels of the PPS output sources (bit = source code of Table 18-2).
    sig: u32,
    /// Changes of the PPS output sources: (time in Fosc ticks, code, new level). Kept back to the oldest timer look so that
    /// a Timer1 gate can be followed through a window (`gate_plan`).
    sig_log: Vec<(u64, u8, bool)>,
    /// Output source changes scheduled for a later time (CCP compare and PWM edges).
    sched: Vec<Sched>,
    /// Changes of the pins driven by a peripheral through RxyPPS: (time in Fosc ticks = 4 * cycle + quarter,
    /// port letter, pin, level). Includes serial signals and peripheral output-enable overrides.
    pub pin_log: Log<(u64, u8, u8, bool)>,
    /// Cycle of the latest flag set by `sync` since the core last looked (`take_event`).
    event: Option<u64>,
    ioc: Ioc,
    /// Buffers `sync_core` reuses from one call to the next, so that it does not allocate on every instruction.
    sb: SyncBufs,
    /// `tick` skips the end-of-instruction sync before this cycle: no model can change anything the CPU sees sooner
    /// (`next_due`). 0 = unknown, sync on the next tick.
    due: u64,
    /// The cycle of the last end-of-instruction sync that `tick` skipped; `catch_up` brings the models there.
    owed: Option<u64>,
}

/// The gate plan and clock edges of one Timer1-type timer for the span being synced (`t1_prepare`).
#[derive(Default)]
struct T1Prep {
    /// Filled in this span.
    on: bool,
    /// `gp` holds a gate plan (the gate source is modelled).
    plan: bool,
    gp: GatePlan,
    /// The clock, when the timer counts in this span.
    clk: Option<Clk>,
    /// Each stretch in which the gate lets the timer count, with its clock edges.
    wins: Vec<(u64, Edges)>,
}

/// Scratch space of `sync_core`. Its contents mean nothing between calls; a clone starts empty.
#[derive(Default)]
struct SyncBufs {
    prep: Vec<T1Prep>,
    times: Vec<u64>,
    ev: Vec<u64>,
    comps: Vec<(usize, u16, u8, u8)>,
    hits: Vec<Sched>,
    periods: Vec<(u8, u64, u8, u32)>,
}

impl Clone for SyncBufs {
    fn clone(&self) -> Self {
        SyncBufs::default()
    }
}

/// A change of a PPS output source at `tick` (Fosc ticks): `act` 0 clears, 1 sets, 2 toggles; `flag` is set with it.
/// `pwm` marks edges that stop when Timer2 is switched off.
#[derive(Clone)]
struct Sched {
    tick: u64,
    code: u8,
    act: u8,
    flag: Option<Flag>,
    pwm: bool,
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

    /// Cycles the CPU stalled for a program flash write or erase since the last call.
    pub fn take_stall(&mut self) -> u64 {
        std::mem::take(&mut self.stall)
    }

    /// The NVM flag of a stalling operation is set when the stall ends.
    fn flash_event(&mut self, now: u64) {
        if self.stall > 0 {
            self.raised(now + self.stall);
        }
    }

    /// Data memory address of the holding registers / buffer RAM that `TBLWT` writes, when the part has them.
    /// The fixed CCP output pins of the PIC18(L)F2x/4xK22, by the CONFIG3H CCP2MX and CCP3MX settings (1 = the default pin):
    /// replaces the fixed entries of the pin-output layer (DS40001412H Tables 2, 3 p.6-8, Register 24-4 p.348).
    pub fn route_k22_ccp(&mut self, ccp2mx: bool, ccp3mx: bool) {
        let Some(big) = self.k22_ccp else { return };
        self.ppsout.retain(|o| o.0 < FIXED_PIN || matches!(o.0 - FIXED_PIN, 30 | 31));
        let pins: [(u8, u8, u8); 5] = [
            (1, b'C', 2),
            (2, if ccp2mx { b'C' } else { b'B' }, if ccp2mx { 1 } else { 3 }),
            (3, if ccp3mx { b'B' } else if big { b'E' } else { b'C' }, if ccp3mx { 5 } else if big { 0 } else { 6 }),
            (4, if big { b'D' } else { b'B' }, if big { 1 } else { 0 }),
            (5, if big { b'E' } else { b'A' }, if big { 2 } else { 4 }),
        ];
        for (n, port, bit) in pins {
            if let (Some(c), Some(pi)) = (self.ccps.iter().find(|c| c.n == n), self.ports.iter().position(|p| p.letter == port)) {
                self.ppsout.push((FIXED_PIN + usize::from(c.sig), pi, bit));
            }
        }
        self.pps_by_port = vec![Vec::new(); self.ports.len()];
        for &(reg, pi, bit) in &self.ppsout {
            self.pps_by_port[pi].push((reg, bit));
        }
    }

    /// TBLWT into holding registers that are not RAM (the PIC18(L)F2x/4xK22, `Nvm::latch`): true when the part has them.
    pub fn tblwt_latch(&mut self, p: u32, v: u8) -> bool {
        self.nvm.as_mut().is_some_and(|n| n.latch(p, v))
    }

    pub(crate) fn dma_eeprom(&self, a: u32) -> u8 {
        self.nvmq.as_ref().and_then(|n| n.eeprom.get(a as usize)).copied().unwrap_or(0xFF)
    }

    pub fn hold_base(&self) -> Option<usize> {
        self.nvm.as_ref().and_then(|n| n.hold()).or_else(|| self.nvmq.as_ref().and_then(|n| n.hold()))
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
        let mut p = Periph { q41: addr(dev, "ADCPCON").is_some(), pwmqs: Vec::new(), pwm_en: addr(dev, "PWMEN"), pwm_load: addr(dev, "PWMLOAD"), clc_select: None, ports: Vec::new(), drive: Default::default(), t0: None, t1: Vec::new(), t2: Vec::new(), uarts: Vec::new(), uart_pins: Vec::new(), uart_scan: 0, pin_history: Vec::new(), quarts: Vec::new(), quart_pins: Vec::new(), spis: Vec::new(), mssps: Vec::new(), i2cqs: Vec::new(), adc: None, nvm: None, nvmq: None, stall: 0, fvr: None, tx: Log::new(), spi_tx: Log::new(), i2c_log: Log::new(), notes: BTreeSet::new(), ccps: Vec::new(), pwms: Vec::new(), cwg: None, cmps: Vec::new(), cmout: None, clcs: Vec::new(), clcin: Vec::new(), clcdata: None, in_clc: false, clc_scan: 0, crc: None, crcq: None, wdt: None, reset_req: None, sleeping: false, sleep_at: 0, fosc_off: false, wdt_wake: false, analog: Vec::new(), ccptmrs: ccp::TmrSel::Q10(None), k22_ccp: None, suppress_ioc_latch: false, ppsout: Vec::new(), pps_by_port: Vec::new(), sig_src: 0, i2c_pps: Vec::new(), i2c_fixed: Vec::new(), board: Board::default(), pullup: Vec::new(), forced: Vec::new(), sig: 0, sig_log: Vec::new(), sched: Vec::new(), pin_log: Log::new(), event: None, ioc: Ioc::default(), sb: SyncBufs::default(), due: 0, owed: None };
        let mut hook = |a: usize, k: u8| m.special[a] = Special::Hook(k);
        if let (Some(l), Some(h), Some(c0), Some(c1), Some(flag)) =
            (addr(dev, "TMR0L"), addr(dev, "TMR0H"), addr(dev, "T0CON0"), addr(dev, "T0CON1"), Flag::find(dev, "TMR0IF"))
        {
            for (a, k) in [(l, T0L), (h, T0H), (c0, T0C0), (c1, T0C1)] {
                hook(a, k);
            }
            p.t0 = Some(Timer0 { l, h, c0, c1, flag, cnt: 0, pre: 0, post: 0, last: 0, cki: addr(dev, "T0CKIPPS"), sleep_run: false, classic: false });
        } else if let (Some(l), Some(h), Some(c), Some(flag)) = (addr(dev, "TMR0L"), addr(dev, "TMR0H"), addr(dev, "T0CON"), Flag::find(dev, "TMR0IF")) {
            // PIC18(L)F2x/4xK22: one control register, T0CON (DS40001412H Register 11-1 p.154).
            for (a, k) in [(l, T0L), (h, T0H), (c, T0C0)] {
                hook(a, k);
            }
            p.t0 = Some(Timer0 { l, h, c0: c, c1: c, flag, cnt: 0, pre: 0, post: 0, last: 0, cki: None, sleep_run: false, classic: true });
        }
        for (i, n) in [1u8, 3, 5].into_iter().enumerate() {
            if let (Some(l), Some(h), Some(con), Some(clk), Some(gcon), Some(flag)) = (
                addr(dev, &format!("TMR{n}L")),
                addr(dev, &format!("TMR{n}H")),
                addr(dev, &format!("T{n}CON")),
                addr(dev, &format!("T{n}CLK")),
                addr(dev, &format!("T{n}GCON")),
                Flag::find(dev, &format!("TMR{n}IF")),
            ) {
                for (sub, a) in [l, h, con, clk, gcon].into_iter().enumerate() {
                    hook(a, t1_hook(i, sub as u8));
                }
                p.t1.push(Timer1 { n, clr: false, gate: addr(dev, &format!("T{n}GATE")), gpps: addr(dev, &format!("T{n}GPPS")), gif: Flag::find(dev, &format!("TMR{n}GIF")), gff: false, grun: false, gb: None, l, h, con, clk, gcon, flag, cnt: 0, pre: 0, last: 0, cki: addr(dev, &format!("T{n}CKIPPS")), sleep_run: false, classic: false });
            } else if let (Some(l), Some(h), Some(con), Some(gcon), Some(flag), None) = (
                addr(dev, &format!("TMR{n}L")),
                addr(dev, &format!("TMR{n}H")),
                addr(dev, &format!("T{n}CON")),
                addr(dev, &format!("T{n}GCON")),
                Flag::find(dev, &format!("TMR{n}IF")),
                addr(dev, &format!("T{n}CLK")),
            ) {
                // PIC18(L)F2x/4xK22 (see `Timer1::classic`). Timer1's clock pin is T1CKI = RC0 and its gate pin T1G = RB5; Timer3 and
                // Timer5 have pins moved by Config bits (T3CMX) and are given none here (their pin sources do not count).
                for (sub, a) in [(0u8, l), (1, h), (2, con), (4, gcon)] {
                    hook(a, t1_hook(i, sub));
                }
                let (cki, gpps) = if n == 1 { (Some(FIXED_PIN + (2 << 3)), Some(FIXED_PIN + (1 << 3 | 5))) } else { (None, None) };
                p.t1.push(Timer1 { n, clr: false, gate: Some(gcon), gpps, gif: Flag::find(dev, &format!("TMR{n}GIF")), gff: false, grun: false, gb: None, l, h, con, clk: con, gcon, flag, cnt: 0, pre: 0, last: 0, cki, sleep_run: false, classic: true });
            }
        }
        for (i, n) in [2u8, 4, 6].into_iter().enumerate() {
            if let (Some(tmr), Some(pr), Some(con), Some(hlt), Some(clk), Some(flag)) = (
                addr(dev, &format!("T{n}TMR")),
                addr(dev, &format!("T{n}PR")),
                addr(dev, &format!("T{n}CON")),
                addr(dev, &format!("T{n}HLT")),
                addr(dev, &format!("T{n}CLKCON")),
                Flag::find(dev, &format!("TMR{n}IF")),
            ) {
                for (sub, a) in [tmr, pr, con, hlt, clk].into_iter().enumerate() {
                    hook(a, t2_hook(i, sub as u8));
                }
                p.t2.push(Timer2 { n, tmr, pr, con, hlt, clk, flag, cnt: 0, pre: 0, post: 0, last: 0, classic: false });
            } else if let (Some(tmr), Some(pr), Some(con), Some(flag), None) =
                (addr(dev, &format!("TMR{n}")), addr(dev, &format!("PR{n}")), addr(dev, &format!("T{n}CON")), Flag::find(dev, &format!("TMR{n}IF")), addr(dev, &format!("T{n}HLT")))
            {
                // PIC18(L)F2x/4xK22 (see `Timer2::classic`).
                for (sub, a) in [(0u8, tmr), (1, pr), (2, con)] {
                    hook(a, t2_hook(i, sub));
                }
                p.t2.push(Timer2 { n, tmr, pr, con, hlt: con, clk: con, flag, cnt: 0, pre: 0, post: 0, last: 0, classic: true });
            }
        }
        if let Some(a) = adc::Adc::find(dev) {
            // The Q10 (DS40002043F ch.33), the Q41 (DS40002214G ch.40) and the PIC16 ADCC parts are sourced.
            hook(a.con, ADC0);
            p.adc = Some(a);
        } else if addr(dev, "ADCON0").is_some() && addr(dev, "ADCLK").is_some() {
            p.notes.insert("the ADC of this part is not modelled; ADGO never clears".into());
        }
        for letter in b'A'..=b'H' {
            let l = letter as char;
            if let (Some(port), Some(lat), Some(tris)) = (addr(dev, &format!("PORT{l}")), addr(dev, &format!("LAT{l}")), addr(dev, &format!("TRIS{l}"))) {
                hook(port, PORT0 + p.ports.len() as u8);
                let wpu_disable = if letter == b'B' {
                    dev.sfr.iter().find_map(|s| s.fields.iter().find(|f| f.name == "nRBPU" || f.name == "RBPU")
                        .map(|f| (s.addr as usize, 1u8 << f.pos)))
                } else { None };
                p.ports.push(Port { letter, port, lat, tris, ansel: addr(dev, &format!("ANSEL{l}")),
                    wpu: addr(dev, &format!("WPU{l}")), odcon: addr(dev, &format!("ODCON{l}")), wpu_disable });
            }
        }
        // The Q10's NVMCON registers, or the EECON1/EECON2 of the PIC18(L)F2x/4xK22 (`Nvm::find_k22`).
        if (addr(dev, "NVMCON2").is_some() || addr(dev, "EECON2").is_some()) && addr(dev, "NVMLOCK").is_none()
            && let Some(n) = Nvm::find(dev)
        {
            hook(n.con1, NVM0 + nvm::CON1);
            hook(n.con2, NVM0 + nvm::CON2);
            p.nvm = Some(n);
        }
        if addr(dev, "NVMLOCK").is_some()
            && let Some(n) = NvmQ::find(dev)
        {
            hook(n.con0, NVMQ0 + nvmq::CON0);
            hook(n.con1, NVMQ0 + nvmq::CON1);
            hook(n.lock, NVMQ0 + nvmq::LOCK);
            p.nvmq = Some(n);
        }
        if addr(dev, "PRODL").is_some() {
            p.ccptmrs = ccp::TmrSel::Q10(addr(dev, "CCPTMRS").or_else(|| addr(dev, "CCPTMRS0")));
            for n in 1..=2u8 {
                if let Some(c) = Ccp::find(dev, n) {
                    hook(c.con, CCP0 + p.ccps.len() as u8 * ccp::HOOKS);
                    p.ccps.push(c);
                }
            }
            // PIC18(L)F2x/4xK22: CCP1..CCP5, CCPTMRS0/1 (ccp.rs); CCP3..CCP5 take the hook codes CCPK0.. (`ccp_hook`).
            if let (Some(t0), Some(t1), true) = (addr(dev, "CCPTMRS0"), addr(dev, "CCPTMRS1"), addr(dev, "PSTR1CON").is_some()) {
                p.ccptmrs = ccp::TmrSel::K22(t0, t1);
                p.k22_ccp = Some(addr(dev, "LATD").is_some());
                for n in 1..=5u8 {
                    if let Some(c) = Ccp::find_k22(dev, n) {
                        hook(c.con, ccp_hook(p.ccps.len()));
                        p.ccps.push(c);
                    }
                }
            }
            for n in 1..=2u8 {
                if let Some(c) = Cmp::find(dev, n) {
                    let k = CMP0 + cmp::HOOKS * p.cmps.len() as u8;
                    for (reg, sub) in [(c.con0, cmp::CON0), (c.nch, cmp::NCH), (c.pch, cmp::PCH)] {
                        hook(reg, k + sub);
                    }
                    p.cmps.push(c);
                }
            }
            if !p.cmps.is_empty()
                && let Some(a) = addr(dev, "CMOUT")
            {
                hook(a, CMOUT_HOOK);
                p.cmout = Some(a);
            }
            if let Some(c) = Cwg::find(dev) {
                for (reg, sub) in [(c.con0, cwg::CON0), (c.con1, cwg::CON1), (c.as0, cwg::AS0), (c.as1, cwg::AS1), (c.str_, cwg::STR), (c.dbr, cwg::DBR), (c.dbf, cwg::DBF)] {
                    hook(reg, CWG0 + sub);
                }
                p.cwg = Some(c);
            }
            for n in 1..=8u8 {
                if let Some(c) = Clc::find(dev, n) {
                    p.clcs.push(c);
                }
            }
            if !p.clcs.is_empty() {
                p.clcin = (0..8).map(|k| addr(dev, &format!("CLCIN{k}PPS")).unwrap_or(usize::MAX)).collect();
                p.clcdata = addr(dev, "CLCDATA");
            }
            if p.q41 {
                p.clc_select = addr(dev, "CLCSELECT");
                for n in 1..=4 {
                    if let Some(c) = Clc::find_q41(dev, n) { p.clcs.push(c); }
                }
                p.clcin = (0..4).map(|k| addr(dev, &format!("CLCIN{k}PPS")).unwrap_or(usize::MAX)).collect();
                p.clcdata = addr(dev, "CLCDATA");
            }
            for n in 3..=4u8 {
                if let Some(w) = Pwm::find(dev, n) {
                    hook(w.con, PWM0 + p.pwms.len() as u8);
                    p.pwms.push(w);
                }
            }
        }
        // Q10 and Q41 share the windowed watchdog register layout.
        if let Some(w) = Wdt::find(dev).or_else(|| Wdt::find_k22(dev)) {
            for (reg, sub) in [(w.con0, wdt::CON0), (w.con1, wdt::CON1), (w.psl, wdt::PSL), (w.psh, wdt::PSH), (w.tmr, wdt::TMR)] {
                if reg != usize::MAX { hook(reg, wdt_hook(sub)); }
            }
            p.wdt = Some(w);
        }
        // Output codes of the bus pins: Q10 MSSPn SCL 0x0F + 2(n-1), SDA 0x10 + 2(n-1) (DS40002043F Table 18-2 p.287);
        // Q41 I2C1 SCL 0x21, SDA 0x22 (DS40002214G Table 21-2 p.323).
        for n in 1..=2u8 {
            let (sc, sd) = (format!("SSP{n}CLKPPS"), format!("SSP{n}DATPPS"));
            if let (Some(a), Some(b)) = (addr(dev, &sc), addr(dev, &sd)) {
                p.i2c_pps.push(I2cPps { num: n, scl_in: a, sda_in: b, scl_name: sc, sda_name: sd, scl_code: 0x0F + 2 * (n - 1), sda_code: 0x10 + 2 * (n - 1) });
            }
            let (sc, sd) = (format!("I2C{n}SCLPPS"), format!("I2C{n}SDAPPS"));
            if let (Some(a), Some(b)) = (addr(dev, &sc), addr(dev, &sd))
                && n == 1
            {
                p.i2c_pps.push(I2cPps { num: n, scl_in: a, sda_in: b, scl_name: sc, sda_name: sd, scl_code: 0x21, sda_code: 0x22 });
            }
        }
        // PIC18(L)F2x/4xK22: SCL1 = RC3, SDA1 = RC4; SCL2/SDA2 = RB1/RB2 on the 28-pin parts, RD0/RD1 on the 40/44-pin parts
        // (DS40001412H Tables 2, 3 p.6-7).
        if p.i2c_pps.is_empty() && addr(dev, "PSTR1CON").is_some() {
            let big = addr(dev, "LATD").is_some();
            for (n, scl, sda) in [(1u8, (b'C', 3), (b'C', 4)), (2, if big { (b'D', 0) } else { (b'B', 1) }, if big { (b'D', 1) } else { (b'B', 2) })] {
                if addr(dev, &format!("SSP{n}CON1")).is_some() {
                    p.i2c_fixed.push((n, scl, sda));
                }
            }
        }
        p.pullup = vec![0; p.ports.len()];
        p.forced = vec![0; p.ports.len()];
        for (i, port) in p.ports.iter().enumerate() {
            for bit in 0..8u8 {
                if let Some(a) = addr(dev, &format!("R{}{bit}PPS", port.letter as char)) {
                    p.ppsout.push((a, i, bit));
                }
            }
        }
        if let Some(a) = addr(dev, "FVRCON") {
            hook(a, FVR0);
            p.fvr = Some(a);
        }
        for n in 1..=2u8 {
            if let Some((u, regs)) = Uart::find(dev, n) {
                let k = UART0 + UART_HOOKS * p.uarts.len() as u8;
                for (o, a) in regs.into_iter().enumerate() {
                    hook(a, k + o as u8);
                }
                p.uarts.push(u);
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
        for n in 1..=2u8 {
            if let Some((q, regs)) = Spi::find(dev, n) {
                let k = SPI0 + spi::HOOKS * p.spis.len() as u8;
                for (i, a) in regs.into_iter().enumerate() {
                    hook(a, k + i as u8);
                }
                p.spis.push(q);
            }
        }
        for n in 1..=2u8 {
            if let Some((q, regs)) = Mssp::find(dev, n) {
                let k = MSSP0 + mssp::HOOKS * p.mssps.len() as u8;
                for (i, a) in regs.into_iter().enumerate() {
                    hook(a, k + i as u8);
                }
                p.mssps.push(q);
            }
        }
        for n in 1..=2u8 {
            if let Some((q, regs)) = I2cQ::find(dev, n) {
                let k = I2CQ0 + i2cq::HOOKS * p.i2cqs.len() as u8;
                for (i, a) in regs.into_iter().enumerate() {
                    hook(a, k + i as u8);
                }
                p.i2cqs.push(q);
            }
        }
        if let Some(c) = Crc::find(dev) {
            for (reg, sub) in c.regs() {
                m.special[reg] = Special::Crc(sub);
            }
            p.crc = Some(c);
        }
        if let Some(c) = crcq::CrcQ::find(dev) {
            for (i, &a) in c.regs.iter().enumerate() { m.special[a] = Special::CrcQ(i as u8); }
            for (i, &a) in c.scan_regs.iter().enumerate() { m.special[a] = Special::CrcQ(11 + i as u8); }
            p.crcq = Some(c);
        }
        if p.q41 {
            for n in 1..=3 {
                if let Some(w) = pwmq::PwmQ::find(dev, n) {
                    for (sub, &a) in w.regs.iter().enumerate() { m.special[a] = Special::PwmQ(n - 1, sub as u8); }
                    p.pwmqs.push(w);
                }
            }
            if let Some(a) = p.pwm_en { m.special[a] = Special::PwmQ(3, 0); }
            if let Some(a) = p.pwm_load { m.special[a] = Special::PwmQ(3, 1); }
        }
        if let Some(a) = p.clc_select {
            m.special[a] = Special::Clc(10);
            for (i, a) in p.clcs[0].regs().into_iter().enumerate() { m.special[a] = Special::Clc(i as u8); }
            if let Some(a) = p.clcdata { m.special[a] = Special::Clc(11); }
        }
        p.init_mem(m);
        for port in 0..p.ports.len() {
            let l = p.ports[port].letter as char;
            if let (Some(f), Some(n), Some(pp)) = (addr(dev, &format!("IOC{l}F")), addr(dev, &format!("IOC{l}N")), addr(dev, &format!("IOC{l}P"))) {
                p.ioc.ports.push((port, f, n, pp));
            }
        }
        p.ioc.iocif = Flag::find(dev, "IOCIF");
        for k in 0..3 {
            let edg = dev.sfr.iter().find_map(|s| s.fields.iter().chain(s.legacy.iter()).find(|f| f.name == format!("INT{k}EDG") || f.name == format!("INTEDG{k}")).map(|f| (s.addr as usize, 1u8 << f.pos)));
            if let (Some(pps), Some(flag), Some((ea, em))) = (addr(dev, &format!("INT{k}PPS")).or_else(|| p.k22_ccp.map(|_| FIXED_PIN + 8 + k)), Flag::find(dev, &format!("INT{k}IF")), edg) {
                p.ioc.ints.push((pps, flag, ea, em));
            }
        }
        if let (Some(pi), Some(en), Some(flag)) = (p.ports.iter().position(|p| p.letter == b'B'), addr(dev, "IOCB"), Flag::find(dev, "RBIF")) {
            let baseline = 0;
            p.ioc.k22 = Some((pi, en, flag, baseline));
            m.special[en] = Special::Sync;
        }
        // Registers that change what a pin or a gate source reads are looked at up to the write (`Special::Sync`).
        let mut regs: Vec<usize> = Vec::new();
        for t in &p.t1 {
            regs.extend(t.gate.into_iter().chain(t.gpps));
        }
        for port in &p.ports {
            regs.extend([port.lat, port.tris]);
            regs.extend(port.ansel);
            regs.extend(port.wpu);
            regs.extend(port.odcon);
            regs.extend(port.wpu_disable.map(|(a, _)| a));
        }
        regs.extend(p.ppsout.iter().map(|o| o.0));
        // the baud rate registers the EUSART model reads from the stored bytes (not unmodelled)
        regs.extend(p.uarts.iter().flat_map(|u| [u.brgl, u.brgh]));
        for a in regs.into_iter().filter(|&a| a < FIXED_PIN) {
            if m.special[a] == Special::None {
                m.special[a] = Special::Sync;
            }
        }
        if p.ppsout.is_empty() || p.k22_ccp.is_some() {
            for u in &p.uarts {
                let (letter, bit) = if u.num == 1 { (b'C', 6) } else if p.ports.iter().any(|p| p.letter == b'D') { (b'D', 6) } else { (b'B', 6) };
                if let Some(pi) = p.ports.iter().position(|p| p.letter == letter) {
                    p.ppsout.push((FIXED_PIN + usize::from(29 + u.num), pi, bit));
                }
            }
        }
        p.pps_by_port = vec![Vec::new(); p.ports.len()];
        for &(reg, pi, bit) in &p.ppsout {
            p.pps_by_port[pi].push((reg, bit));
        }
        for u in &p.uarts {
            let fixed = if u.num == 1 { 2 * 8 + 7 } else if p.ports.iter().any(|p| p.letter == b'D') { 3 * 8 + 7 } else { 1 * 8 + 7 };
            let rx = addr(dev, &format!("RX{}PPS", u.num));
            p.uart_pins.push((rx.unwrap_or(FIXED_PIN + fixed), if rx.is_some() { 0x09 + 2 * (u.num - 1) } else { 29 + u.num }));
        }
        p.quart_pins = p.quarts.iter().map(|q| addr(dev, &format!("U{}RXPPS", q.num)).unwrap()).collect();
        let sigs = p.ccps.iter().map(|c| c.sig).chain(p.pwms.iter().map(|c| c.sig)).chain(p.cmps.iter().map(|c| c.sig)).chain(p.clcs.iter().map(|c| c.sig));
        p.sig_src = sigs.fold(0, |a, c| a | 1u32.checked_shl(u32::from(c)).unwrap_or(0));
        if !p.pwmqs.is_empty() { p.sig_src |= 0xFC0; }
        p.pin_history = (0..p.ports.len()).map(|i| VecDeque::from([(0, p.pin_state(i, m, 0))])).collect();
        if let Some((pi, en, flag, _)) = p.ioc.k22 {
            p.ioc.k22 = Some((pi, en, flag, p.port_levels(pi, m, 0)));
        }
        p
    }

    pub(crate) fn crcq_read(&self, k: u8, m: &DataMem) -> u8 {
        self.crcq.as_ref().map_or(0, |c| c.read(k, m))
    }
    pub(crate) fn crcq_write(&mut self, k: u8, v: u8, now: u64, m: &mut DataMem) {
        if let Some(c) = &mut self.crcq { c.write(k, v, now, m); }
        self.invalidate();
    }

    fn scanner_trigger(&self, m: &DataMem, now: u64) -> Option<bool> {
        match self.crcq.as_ref()?.trigger_select(m) {
            0 => Some(self.board.time_ns(now) as u64 % 32_258 < 16_129),
            2 => self.t0.as_ref().map(|t| m.mem[t.c0] & 0x20 != 0),
            8..=11 => self.clcs.get((self.crcq.as_ref()?.trigger_select(m) - 8) as usize).map(|c| c.out),
            _ => None,
        }
    }
    pub(crate) fn scanner_active(&self, m: &DataMem) -> bool {
        self.crcq.as_ref().is_some_and(|c| c.scan_active(m))
    }
    pub(crate) fn scanner_request(&mut self, m: &mut DataMem, now: u64) -> Option<bool> {
        let trigger = self.scanner_trigger(m, now);
        self.crcq.as_mut()?.scan_request(m, trigger, &mut self.notes)
    }
    pub(crate) fn scanner_cycle(&mut self, m: &mut DataMem, prog: &[u8], now: u64) {
        let trigger = self.scanner_trigger(m, now);
        let fetch = self.crcq.as_ref().and_then(|c| c.scan_fetch(m, trigger));
        if let Some((address, eeprom)) = fetch {
            let value = if eeprom { self.dma_eeprom(address) } else { prog.get(address as usize).copied().unwrap_or(0xFF) };
            if let Some(c) = &mut self.crcq { c.scan_byte(value, m); }
        }
        self.invalidate();
    }

    fn pwmq_sync(&mut self, m: &mut DataMem, now: u64, asleep: bool) {
        if self.pwmqs.is_empty() { return; }
        let mut events = Vec::new();
        for w in &mut self.pwmqs { w.sync(m, now, asleep, &mut events, &mut self.notes); }
        events.sort_by_key(|e| e.0);
        for (tick, code, v) in events { self.set_sig(m, code, u8::from(v), tick); }
        if let Some(a) = self.pwm_en { m.mem[a] = self.pwmqs.iter().fold(0, |v, w| v | (m.mem[w.regs[9]] >> 7) << (w.n - 1)); }
        if let Some(a) = self.pwm_load { m.mem[a] = self.pwmqs.iter().fold(0, |v, w| v | ((m.mem[w.regs[9]] >> 2) & 1) << (w.n - 1)); }
    }
    pub(crate) fn pwmq_read(&self, i: u8, sub: u8, m: &DataMem) -> u8 {
        if i < 3 { m.mem[self.pwmqs[i as usize].regs[sub as usize]] }
        else { self.pwmqs.iter().fold(0, |v, w| v | ((m.mem[w.regs[9]] >> if sub == 0 { 7 } else { 2 }) & 1) << (w.n - 1)) }
    }
    pub(crate) fn pwmq_write(&mut self, i: u8, sub: u8, v: u8, m: &mut DataMem, now: u64) {
        self.sync(m, now);
        let mut events = Vec::new();
        if i < 3 { self.pwmqs[i as usize].write(sub, v, now, m, &mut events); }
        else {
            for w in &mut self.pwmqs {
                let bit = if sub == 0 { 0x80 } else { 4 };
                let value = m.mem[w.regs[9]] & !bit | if v & (1 << (w.n - 1)) != 0 { bit } else { 0 };
                w.write(9, value, now, m, &mut events);
            }
        }
        for (tick, code, v) in events { self.set_sig(m, code, u8::from(v), tick); }
        self.invalidate();
    }

    /// Translate the family's PPS codes to the signal numbers used by the shared models.
    fn output_code(&self, m: &DataMem, reg: usize) -> u8 {
        let code = out_code(m, reg);
        if !self.q41 || reg >= FIXED_PIN { return code; }
        match m.mem[reg] & 0x3F {
            1..=4 => 0x17 + (m.mem[reg] & 0x3F),
            5..=8 => (m.mem[reg] & 0x3F) - 4,
            9 => 5,
            n @ 10..=15 => n - 4,
            25 | 26 => (m.mem[reg] & 0x3F) - 12,
            _ => 0,
        }
    }

    pub(crate) fn clc_read(&self, k: u8, m: &DataMem) -> u8 {
        let selected = self.clc_select.map_or(0, |a| m.mem[a] & 3);
        if k == 11 { return self.clcs.iter().fold(0, |v, c| v | u8::from(c.out) << (c.n - 1)); }
        if k == 10 { selected } else { self.clcs.get(selected as usize).and_then(|c| c.bank).map_or(0, |b| b[k as usize]) }
    }

    pub(crate) fn clc_write(&mut self, k: u8, v: u8, m: &mut DataMem, now: u64) {
        self.sync(m, now);
        if k == 11 { return; }
        if k == 10 { m.mem[self.clc_select.unwrap()] = v & 3; return; }
        let selected = m.mem[self.clc_select.unwrap()] as usize & 3;
        if let Some(b) = &mut self.clcs[selected].bank {
            b[k as usize] = match k { 0 => v & 0x9F | b[0] & 0x20, 1 => v & 0x8F, 2..=5 => v & 0x7F, _ => v };
        }
        self.clc_eval(m, 4 * now);
        self.invalidate();
    }

    /// A Timer1-type timer is running or gated, so a write to a pin register must first bring the models up to date: the
    /// levels it changes are what the gate sees.
    pub fn watching(&self, m: &DataMem) -> bool {
        self.t1.iter().any(|t| m.mem[t.con] & 1 != 0 || m.mem[t.gcon] & 0x80 != 0)
    }

    /// After a write to register `a` that `Special::Sync` covers: a changed gate source or pin is not an edge of the gate.
    pub fn synced_write(&mut self, a: usize) {
        for t in &mut self.t1 {
            if t.gate == Some(a) || t.gpps == Some(a) {
                t.gb = None;
            }
        }
    }

    /// Set the byte-level client that answers SPI `num` (1-based). False when the part has no such SPI.
    pub fn spi_client(&mut self, num: u8, c: SpiClient) -> bool {
        if let Some(s) = self.spis.iter_mut().find(|s| s.num == num) {
            s.client = c;
            true
        } else if let Some(s) = self.mssps.iter_mut().find(|s| s.num == num) {
            s.spi = c;
            s.byte_client = true;
            true
        } else {
            false
        }
    }

    /// Add an I2C client to the bus of MSSP `num` (1-based). False when the part has no such MSSP.
    pub fn i2c_client(&mut self, num: u8, c: I2cClient) -> bool {
        if let Some(s) = self.mssps.iter_mut().find(|s| s.num == num) {
            s.i2c.push(c);
            true
        } else if let Some(s) = self.i2cqs.iter_mut().find(|s| s.num == num) {
            s.i2c.push(c);
            true
        } else {
            false
        }
    }

    /// Whether this part has a modelled UART or EUSART receiver.
    pub fn has_uart(&self, num: u8) -> bool {
        self.quarts.iter().any(|u| u.num == num) || self.uarts.iter().any(|u| u.num == num)
    }

    /// Attach a reactive request/reply rule; false when this UART is absent.
    pub fn uart_reply(&mut self, num: u8, rule: UartReply) -> bool {
        if let Some(u) = self.uarts.iter_mut().find(|u| u.num == num) { u.replies.push(rule); return true; }
        if let Some(u) = self.quarts.iter_mut().find(|u| u.num == num) { u.replies.push(rule); return true; }
        false
    }

    /// Queue characters for the receiver of EUSART `num` (1-based). False when the part has no such EUSART.
    pub fn rx_send(&mut self, num: u8, s: RxStream) -> bool {
        self.due = 0; // newly queued input invalidates the next peripheral deadline
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

    /// Bring every running timer to cycle `now`.
    /// Bring every model to cycle `now`. Capture edges on the CCPx pins inside the span are handled at their own cycle
    /// so that TMR1 is read at that cycle.
    pub fn sync(&mut self, m: &mut DataMem, now: u64) {
        // A sync to `now` covers a skipped one; what it changes may move the next due cycle.
        self.owed = None;
        self.due = 0;
        if self.fosc_off {
            return self.sync_asleep(m, now);
        }
        loop {
            let mut best: Option<(u64, usize)> = None;
            for (i, c) in self.ccps.iter().enumerate() {
                let Some(reg) = c.pps else { continue };
                if !c.en(m) || !(3..=7).contains(&c.mode(m)) || !self.t1.iter().any(|t| t.n == ccp::timer_of(self.ccptmrs, m, c.n, false)) {
                    continue;
                }
                if m.mem[c.cap] & 15 != 0 {
                    continue; // noted in `sync_with`
                }
                for at in self.pin_times(c.scan, now) {
                    if best.is_none_or(|b| (at, i) < b) && self.pps_pin(m, reg, at) != self.pps_pin(m, reg, at - 1) {
                        best = Some((at, i));
                        break;
                    }
                }
            }
            let Some((e, i)) = best else { break };
            self.sync_core(m, e, false);
            self.capture(m, i, e);
        }
        self.clc_scan(m, now);
        self.ioc_scan(m, now);
        self.sync_core(m, now, false);
        self.clc_eval(m, 4 * now);
        self.publish_pins(m, now);
        self.wdt_tick(m, now);
        if let Some(c) = &mut self.crc {
            c.sync(m, now);
        }
        if let Some(c) = &mut self.crcq { c.sync(m, now); }
        for c in &mut self.ccps {
            c.scan = now;
        }
    }

    /// The end-of-instruction sync of a run (`Pic18::lazy`): skipped while `now` is before the next cycle at which a
    /// model can change a register, a flag, a pin or a log (`next_due`). The CPU calls `catch_up` before it touches a
    /// peripheral register and `invalidate` after it changed one, so a skipped span never sees a register value it
    /// did not have. `PICSIM_CHECK_LAZY=1` checks every skip against a full sync of a copy and panics on a difference.
    pub fn tick(&mut self, m: &mut DataMem, now: u64) {
        if !self.fosc_off && now < self.due {
            if check_lazy() {
                self.check_skip(m, now);
            }
            self.owed = Some(now);
            return;
        }
        self.sync(m, now);
        self.due = self.next_due(m, now);
    }

    /// Bring the models to the last skipped end-of-instruction sync, if there is one.
    pub fn catch_up(&mut self, m: &mut DataMem) {
        if let Some(t) = self.owed.take() {
            let (e, due) = (self.event, self.due);
            self.sync(m, t);
            (self.event, self.due) = (e, due); // flags raised up to `t` belonged to earlier instructions
        }
    }

    /// A register the models read has changed (or a model's state was touched): sync on the next tick.
    pub fn invalidate(&mut self) {
        self.due = 0;
    }

    /// The first cycle after `now` at which some model may change what the CPU or the outside sees: a timer
    /// overflow, compare or period match, a stimulus pin or analog change, a scheduled output edge, a watchdog
    /// time-out. Early is always safe (it only costs a sync); `now + 1` when a model is mid-way through something
    /// it follows cycle by cycle (a serial transfer, a conversion, a NVM write, the CRC, a CLC, the CWG, a board).
    fn next_due(&self, m: &DataMem, now: u64) -> u64 {
        let soon = now + 1;
        if self.sleeping
            || self.reset_req.is_some()
            || !self.board.is_empty()
            || self.pwmqs.iter().any(|p| p.enabled(m))
            || self.clc_active(m)
            || self.cwg.as_ref().is_some_and(|c| c.en(m))
            || self.uarts.iter().any(Uart::busy)
            || self.quarts.iter().any(QUart::busy)
            || self.spis.iter().any(Spi::busy)
            || self.mssps.iter().any(Mssp::busy)
            || self.i2cqs.iter().any(I2cQ::busy)
            || self.adc.as_ref().is_some_and(adc::Adc::busy)
            || self.nvm.as_ref().is_some_and(Nvm::busy)
            || self.nvmq.as_ref().is_some_and(NvmQ::busy)
            || self.crc.as_ref().is_some_and(Crc::busy)
            || self.crcq.as_ref().is_some_and(crcq::CrcQ::busy)
        {
            return soon;
        }
        let mut due = u64::MAX;
        let mut at = |c: u64| due = due.min(c.max(soon));
        if let Some(t) = self.pin_history.iter().flat_map(|h| h.iter().map(|e| e.0)).chain(self.sig_log.iter().map(|e| (e.0 + 3) / 4 + 1)).filter(|&t| t > now).min() { at(t); }
        if let Some(t) = self.next_drive(now) {
            at(t);
        }
        if let Some(t) = self.next_analog(now) {
            at(t);
        }
        for s in &self.sched {
            at(s.tick / 4);
        }
        if let Some(w) = &self.wdt {
            let (c0, c1) = (m.mem[w.con0], w.con1_value(&m.mem));
            if w.on(c0) {
                at(if w.was_on { w.deadline(c0, c1) } else { soon });
            }
        }
        // The cycle of the `j`-th prescaled clock of a timer clocked by `clk` (a pin clock: its edges are stimulus
        // or output changes, already counted above).
        let clock_at = |clk: Clk, cki: Option<usize>, last: u64, pre: u64, sh: u32, j: u64| match clk {
            Clk::Cyc(_) | Clk::Hz(_) => Some(self.clock_edges(m, clk, cki, last, last).at(last, pre, sh, j)),
            Clk::Pin(_) | Clk::Other => None,
        };
        if let (Some(t), Some(clk)) = (&self.t0, self.t0_clock(m)) {
            if m.mem[t.c0] & 0x80 != 0 {
                let d = if t.wide(m) { 65536 - u64::from(t.cnt) } else { u64::from(t.top8(m).wrapping_sub(t.cnt as u8)) + 1 };
                if let Some(c) = clock_at(clk, t.cki, t.last, t.pre, t.shift(m), d) {
                    at(c);
                }
            }
        }
        for (i, t) in self.t1.iter().enumerate() {
            let con = m.mem[t.con];
            if con & 1 == 0 {
                continue;
            }
            let mut d = 65536 - u64::from(t.cnt);
            if t.clr {
                d = 1;
            }
            for c in &self.ccps {
                if c.en(m) && !c.k22 && matches!(c.mode(m), 1 | 2 | 8 | 9 | 10 | 11) && ccp::timer_of(self.ccptmrs, m, c.n, false) == t.n {
                    let k = u64::from(c.ccpr(m).wrapping_sub(t.cnt));
                    d = d.min(if k == 0 { 65536 } else { k });
                }
            }
            if let Some(c) = clock_at(self.t1_clock(m, i), t.cki, t.last, t.pre, u32::from((con >> 4) & 3), d) {
                at(c);
            }
        }
        for t in &self.t2 {
            let cs = t.cs(m);
            if !t.on(m) || (cs != 1 && cs != 2) {
                continue;
            }
            let d = u64::from(m.mem[t.pr].wrapping_sub(t.cnt)) + 1;
            at(self::at(t.last, t.pre, t.shift(m), d, if cs == 2 { 4 } else { 1 }));
        }
        due
    }

    /// `PICSIM_CHECK_LAZY`: a skipped sync must change nothing. A copy of the part is synced to `now` and compared
    /// with the part (registers the CPU reads through a model, which compute their value at the read, are left out).
    fn check_skip(&self, m: &DataMem, now: u64) {
        let (mut p, mut mc) = (self.clone(), m.clone());
        p.catch_up(&mut mc);
        let before = (mc.mem.clone(), p.pin_log.total(), p.tx.total(), p.spi_tx.total(), p.i2c_log.total(), p.sched.len());
        p.event = None;
        p.sync(&mut mc, now);
        let live = |a: usize| matches!(mc.special[a], Special::Hook(k) if !Periph::is_stored(k));
        for a in 0..mc.mem.len() {
            if before.0[a] != mc.mem[a] && !live(a) {
                panic!("lazy sync: skipping to cycle {now} (due {}) missed a change of {} ({a:#05X}): {:#04X} -> {:#04X}", self.due, mc.names[a].as_deref().unwrap_or("?"), before.0[a], mc.mem[a]);
            }
        }
        let after = (p.pin_log.total(), p.tx.total(), p.spi_tx.total(), p.i2c_log.total(), p.sched.len());
        assert_eq!((before.1, before.2, before.3, before.4, before.5), after, "lazy sync: skipping to cycle {now} (due {}) missed a pin, serial or schedule change", self.due);
        assert!(p.event.is_none() && p.reset_req == self.reset_req, "lazy sync: skipping to cycle {now} (due {}) missed a flag event or a reset", self.due);
    }

    /// Edges of the pins on the interrupt-on-change registers and the INTx pins, from `ioc.scan` to `now`: the flags are set
    /// (also in Sleep), IOCIF follows the IOCxF registers `[A101]`.
    fn ioc_scan(&mut self, m: &mut DataMem, now: u64) {
        if self.ioc.ports.is_empty() && self.ioc.ints.is_empty() && self.ioc.k22.is_none() {
            return;
        }
        let from = self.ioc.scan;
        self.ioc.scan = now;
        let mut times = self.pin_times(from, now);
        times.sort_unstable();
        times.dedup();
        for t in times {
            if let Some((pi, en, flag, baseline)) = self.ioc.k22 {
                let value = self.port_levels_at(pi, m, t);
                if (value ^ baseline) & m.mem[en] & m.mem[self.ports[pi].tris] != 0 {
                    flag.set(m); self.raised(t);
                }
            }
            for k in 0..self.ioc.ports.len() {
                let (pi, f, n, pp) = self.ioc.ports[k];
                let (old, new) = (self.port_levels_at(pi, m, t - 1), self.port_levels_at(pi, m, t));
                let hit = (old ^ new) & (new & m.mem[pp] | !new & m.mem[n]);
                if hit != 0 {
                    m.mem[f] |= hit & m.implemented[f];
                    if let Some(flag) = self.ioc.iocif { flag.set(m); }
                    self.raised(t);
                }
            }
            for k in 0..self.ioc.ints.len() {
                let (pps, flag, ea, em) = self.ioc.ints[k];
                let (old, new) = (self.pps_pin(m, pps, t - 1), self.pps_pin(m, pps, t));
                if old != new && new == (m.mem[ea] & em != 0) {
                    flag.set(m);
                    self.raised(t);
                }
            }
        }
        if let Some((pi, en, flag, baseline)) = self.ioc.k22 {
            if (self.port_levels(pi, m, now) ^ baseline) & m.mem[en] & m.mem[self.ports[pi].tris] != 0 {
                flag.set_in(&mut m.mem);
            }
        }
        if let Some(fl) = self.ioc.iocif {
            if self.ioc.ports.iter().any(|&(_, f, ..)| m.mem[f] != 0) {
                fl.set_in(&mut m.mem); // status summary, not another DMA request pulse
            } else {
                fl.clear(m);
            }
        }
    }

    /// Edges before `now` are not looked at (a reset: the pins are not watched meanwhile).
    pub fn ioc_rebase(&mut self, now: u64) {
        self.ioc.scan = now;
        self.uart_scan = now;
    }

    /// The next time after `after` that the stimulus changes a pin.
    pub fn next_drive(&self, after: u64) -> Option<u64> {
        self.drive.next_after(after)
    }

    /// Capture event of CCP `i` at the input edge on cycle `e` (the capture timer is already at cycle `e`).
    fn capture(&mut self, m: &mut DataMem, i: usize, e: u64) {
        let c = &self.ccps[i];
        let reg = c.pps.unwrap();
        let rising = self.pps_pin(m, reg, e);
        let mode = c.mode(m);
        let tn = ccp::timer_of(self.ccptmrs, m, c.n, false);
        let c = &mut self.ccps[i];
        c.scan = e;
        let hit = match mode {
            3 => true,
            4 => !rising,
            5 => rising,
            _ if rising => {
                c.edges += 1;
                if c.edges == if mode == 6 { 4 } else { 16 } {
                    c.edges = 0;
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if hit && let Some(t) = self.t1.iter().find(|t| t.n == tn) {
            m.mem[c.rl] = t.cnt as u8;
            m.mem[c.rh] = (t.cnt >> 8) as u8;
            c.flag.set(m);
            self.raised(e);
        }
    }

    /// Change the PPS output source `code` (`act` 0 clear, 1 set, 2 toggle) at `tick` Fosc ticks, log the pins it drives
    /// and update the OUT bit of the CCP that owns the source.
    fn set_sig(&mut self, m: &mut DataMem, code: u8, act: u8, tick: u64) {
        let cur = self.sig >> code & 1 != 0;
        let new = match act {
            0 => false,
            1 => true,
            _ => !cur,
        };
        if new == cur {
            return;
        }
        self.sig ^= 1 << code;
        self.sig_log.push((tick, code, new));
        for c in self.ccps.iter().filter(|c| c.sig == code && !c.k22) {
            if new { m.mem[c.con] |= 0x20 } else { m.mem[c.con] &= !0x20 }
        }
        for w in self.pwms.iter().filter(|w| w.sig == code) {
            if new { m.mem[w.con] |= 0x20 } else { m.mem[w.con] &= !0x20 }
        }
        for &(reg, pi, bit) in &self.ppsout {
            if self.output_code(m, reg) == code && m.mem[self.ports[pi].tris] >> bit & 1 == 0 {
                self.pin_log.push((tick, self.ports[pi].letter, bit, new));
            }
        }
        if self.cwg.as_ref().is_some_and(|c| c.source(m) == Some(code)) {
            self.cwg_edge(m, new, tick);
        }
        self.clc_eval(m, tick);
    }

    /// Set the watchdog's configuration (WDTE, WDTCPS, WDTCWS, WDTCCS fields of the configuration words).
    /// CONFIG4.SCANE: the scanner exists.
    pub fn scan_config(&mut self, scane: bool) {
        if let Some(c) = &mut self.crc {
            c.scane = scane;
        }
    }

    /// A write to a CRC or scanner register; the CPU stalls for a Burst scan.
    pub fn crc_write(&mut self, sub: u8, v: u8, now: u64, m: &mut DataMem, prog: &[u8]) {
        if let Some(c) = &mut self.crc {
            self.stall += c.write(sub, v, now, m, prog, &mut self.notes);
            if self.stall > 0 {
                self.flash_event(now);
            }
        }
    }

    /// Scanner work after an instruction (Concurrent, Triggered, Peek).
    pub fn crc_scan(&mut self, m: &mut DataMem, prog: &[u8], now: u64, dead: u64, in_isr: bool) {
        let Some(c) = &mut self.crc else { return };
        let trig = match c.tsel(m) {
            t @ 16..=23 => self.clcs.get(usize::from(t - 16)).map(|x| x.out),
            _ => None,
        };
        if c.waiting(m) && trig.is_none() {
            self.notes.insert("scanner trigger source (SCANTRIG) is not modelled except CLC1-CLC8: the scan never starts".into());
        }
        self.stall += c.scan(m, prog, now, dead, trig, in_isr);
        if self.stall > 0 {
            self.flash_event(now);
        }
        if self.stall > 0 || self.crc.as_ref().is_some_and(Crc::busy)
            || self.crcq.as_ref().is_some_and(crcq::CrcQ::busy) {
            self.due = 0; // the scanner fed the CRC: `tick` must follow it
        }
    }

    pub fn wdt_config(&mut self, wdte: u8, cps: u8, cws: u8, ccs: u8) -> bool {
        let Some(w) = &mut self.wdt else { return false };
        (w.wdte, w.ps_fixed, w.win_fixed, w.cs_fixed) = (wdte, cps != 0x1F, cws != 7, ccs != 7);
        true
    }

    pub(crate) fn k22_wdt_clock_clear(&mut self, now: u64) {
        if let Some(w) = &mut self.wdt {
            if w.k22_ps.is_some() { w.start = now; }
        }
    }

    pub fn wdt_config_k22(&mut self, enable: u8, ps: u8) {
        if let Some(w) = &mut self.wdt {
            if w.k22_ps.is_some() {
                w.k22_ps = Some(ps & 15);
                // K22 01 = awake-only, 10 = software; shared WWDT uses the reverse.
                w.wdte = [0, 2, 1, 3][usize::from(enable & 3)];
            }
        }
    }

    /// After a reset that rebuilt the peripherals: what the run has observed so far (logs, notes) and the board stay.
    pub fn carry_over(&mut self, old: Periph, keep_ioc: bool) {
        if keep_ioc {
            if let (Some(new), Some(previous)) = (&mut self.ioc.k22, old.ioc.k22) { new.3 = previous.3; }
        }
        self.tx = old.tx;
        self.spi_tx = old.spi_tx;
        self.i2c_log = old.i2c_log;
        self.pin_log = old.pin_log;
        self.notes.extend(old.notes);
        // The data EEPROM is non-volatile: only a write in progress is lost (B9).
        if let (Some(n), Some(o)) = (&mut self.nvm, old.nvm) {
            n.eeprom = o.eeprom;
        }
        if let (Some(n), Some(o)) = (&mut self.nvmq, old.nvmq) {
            n.eeprom = o.eeprom;
        }
        // The `[[spi]]` and `[[i2c]]` clients are outside devices too: they go on from the byte they had reached (B17).
        // The board's I2C clients (tag not 0) are put back fresh by `board_wiring`; their state is in `board`.
        for o in old.spis {
            if let Some(s) = self.spis.iter_mut().find(|s| s.num == o.num) {
                s.client = o.client;
            }
        }
        for o in old.mssps {
            if let Some(s) = self.mssps.iter_mut().find(|s| s.num == o.num) {
                s.spi = o.spi;
                s.byte_client = o.byte_client;
                s.i2c = o.i2c;
            }
        }
        for o in old.i2cqs {
            if let Some(s) = self.i2cqs.iter_mut().find(|s| s.num == o.num) {
                s.i2c = o.i2c;
            }
        }
        // The board is outside the part: its LED history and OLED contents and state survive the reset (B11).
        let pos = old.board.log_pos;
        self.board = old.board;
        self.board_wiring();
        self.board.log_pos = pos;
    }

    /// The register bits `bind` sets in data memory (beyond `special`); a warm reset that reuses a bound copy calls it.
    pub fn init_mem(&self, m: &mut DataMem) {
        if let Some(c) = &self.crc {
            c.init(m);
        }
    }

    /// The data EEPROM array, if the part has one modelled.
    pub fn eeprom(&self) -> Option<&[u8]> {
        self.nvm.as_ref().map(|n| &n.eeprom[..]).or(self.nvmq.as_ref().map(|n| &n.eeprom[..]))
    }

    /// Data EEPROM contents from a hex image (`get` gives the byte at an address); FFh where the image has none.
    pub fn load_eeprom(&mut self, get: impl Fn(u32) -> Option<u8>) {
        let fill = |start: u32, ee: &mut Vec<u8>| {
            for (i, v) in ee.iter_mut().enumerate() {
                *v = get(start + i as u32).unwrap_or(0xFF);
            }
        };
        if let Some(n) = &mut self.nvm {
            fill(n.start, &mut n.eeprom);
        }
        if let Some(n) = &mut self.nvmq {
            fill(n.start, &mut n.eeprom);
        }
    }

    /// The reset the watchdog asked for, if any.
    pub fn take_reset(&mut self) -> Option<ResetKind> {
        self.reset_req.take()
    }

    pub(crate) fn wdt_fosc(&mut self, now: u64) {
        let f = self.board.effective_fosc_mhz();
        if let Some(w) = &mut self.wdt {
            w.set_fosc(f, now);
        }
    }

    /// The watchdog at cycle `now`: held clear while off, a time-out asks for a reset.
    fn wdt_tick(&mut self, m: &DataMem, now: u64) {
        self.wdt_fosc(now);
        let sleeping = self.sleeping;
        let Some(w) = &mut self.wdt else { return };
        let (c0, c1) = (m.mem[w.con0], w.con1_value(&m.mem));
        let on = if sleeping { w.on_sleep(c0) } else { w.on(c0) };
        if !on || !w.was_on {
            (w.start, w.armed) = (now, false);
        }
        w.was_on = on;
        if on && now >= w.deadline(c0, c1) {
            if sleeping {
                self.wdt_wake = true;
            } else {
                self.reset_req.get_or_insert(ResetKind::Wdt);
            }
        }
    }

    fn clear_sleep_run(&mut self) {
        if let Some(t) = &mut self.t0 {
            t.sleep_run = false;
        }
        self.t1.iter_mut().for_each(|t| t.sleep_run = false);
    }

    /// SLEEP at cycle `now`: the watchdog is cleared and counts on only in the modes that run in Sleep.
    pub fn enter_sleep(&mut self, m: &mut DataMem, now: u64, idle: bool) {
        self.sync(m, now);
        if self.reset_req.is_some() {
            return; // a reset fell due on the SLEEP cycle: the part resets instead of sleeping
        }
        self.sleeping = true;
        self.fosc_off = !idle;
        self.sleep_at = now;
        self.clear_sleep_run();
        self.wdt_wake = false;
        if let Some(w) = &mut self.wdt {
            (w.start, w.armed, w.was_on) = (now, false, w.on_sleep(m.mem[w.con0]));
        }
        if idle {
            return;
        }
        // Which timers go on counting: their `last` is kept up to date in Sleep and `thaw` leaves it alone.
        let t0_ok = self.t0_sleep_ok(m);
        if let Some(t) = &mut self.t0 {
            t.sleep_run = t0_ok;
        }
        for i in 0..self.t1.len() {
            self.t1[i].sleep_run = self.t1_sleep_ok(m, i);
        }
        for u in &mut self.uarts {
            u.freeze_pins(now);
            u.sleep(true, 0);
        }
        for q in &mut self.mssps { q.freeze_pins(now); }
        for q in &mut self.quarts {
            q.freeze_pins(now);
            q.sleep(true);
        }
    }

    /// One look at the models that keep running in Sleep, up to cycle `now`: the watchdog, the comparators, the CLCs, the CWG
    /// dead band and the data EEPROM. Everything clocked by Fosc or Fosc/4 has no clock (DS40002043F 7.4 p.71) and waits for `thaw`; Timer0 and
    /// Timer1-type timers on a pin, LFINTOSC or SOSC count on (`sync_core` with `asleep`) `[A102]`.
    /// Characters the stimulus sends to a UART meanwhile are lost `[A94]`.
    fn sync_asleep(&mut self, m: &mut DataMem, now: u64) {
        self.uart_scan = now;
        let mut ev: Vec<u64> = Vec::new();
        self.ioc_scan(m, now);
        self.clc_scan(m, now);
        self.cmp_scan(m, now);
        self.apply_sched(m, now, &mut ev);
        for u in &mut self.uarts {
            u.rx(&mut m.mem, now, &mut ev);
        }
        for q in &mut self.quarts {
            q.sync_asleep(m, now, &mut ev);
        }
        if let Some(n) = &mut self.nvm
            && let Some(e) = n.sync(m, now)
        {
            ev.push(e);
        }
        if let Some(n) = &mut self.nvmq
            && let Some(e) = n.sync(m, now)
        {
            ev.push(e);
        }
        self.sync_core(m, now, true);
        self.clc_eval(m, 4 * now);
        self.publish_pins(m, now);
        self.wdt_tick(m, now);
        for e in ev {
            self.raised(e);
        }
    }

    /// The part slept for `d` cycles: what has no clock in Sleep carries on from where it stopped, so every time it
    /// has in hand moves `d` cycles later.
    fn thaw(&mut self, d: u64) {
        if let Some(c) = &mut self.crcq { c.thaw(d); }
        if let Some(c) = &mut self.crc {
            c.thaw(d);
        }
        if let Some(t) = self.t0.as_mut().filter(|t| !t.sleep_run) {
            t.last += d;
        }
        for t in self.t1.iter_mut().filter(|t| !t.sleep_run) {
            t.last += d;
        }
        for t in &mut self.t2 {
            t.last += d;
        }
        for s in self.sched.iter_mut().filter(|s| s.pwm) {
            s.tick += 4 * d;
        }
        for c in &mut self.ccps {
            c.scan += d;
        }
        for u in &mut self.uarts {
            u.sleep(false, d);
        }
        for q in &mut self.quarts {
            q.sleep(false);
            q.shift(d);
        }
        self.spis.iter_mut().for_each(|q| q.shift(d));
        self.mssps.iter_mut().for_each(|q| q.shift(d));
        self.i2cqs.iter_mut().for_each(|q| q.shift(d));
        if let Some(a) = self.adc.as_mut() {
            a.shift(d);
        }
    }

    /// Wake-up: the watchdog is cleared again (DS40002043F 7.2.1 p.69).
    pub fn exit_sleep(&mut self, now: u64) {
        let d = now.saturating_sub(self.sleep_at);
        self.sleeping = false;
        self.wdt_wake = false;
        if std::mem::take(&mut self.fosc_off) {
            self.thaw(d);
        }
        self.clear_sleep_run();
        if let Some(w) = &mut self.wdt {
            (w.start, w.armed, w.was_on) = (now, false, false);
        }
    }

    /// A watchdog time-out happened in Sleep.
    pub fn wdt_woke(&self) -> bool {
        self.wdt_wake
    }

    /// Cycle of the watchdog time-out while asleep, if it counts in Sleep.
    pub fn wdt_sleep_deadline(&self, m: &DataMem) -> Option<u64> {
        let w = self.wdt.as_ref()?;
        let (c0, c1) = (m.mem[w.con0], w.con1_value(&m.mem));
        w.on_sleep(c0).then(|| w.deadline(c0, c1))
    }

    /// CLRWDT at cycle `now`: a valid one clears the watchdog, one in a closed window (or unarmed in windowed mode) asks for a reset.
    pub fn clrwdt(&mut self, m: &DataMem, now: u64) {
        self.wdt_fosc(now);
        let Some(w) = &mut self.wdt else { return };
        let (c0, c1) = (m.mem[w.con0], w.con1_value(&m.mem));
        if !w.on(c0) {
            return;
        }
        if w.violation(now, c0, c1) {
            self.reset_req.get_or_insert(ResetKind::WdtWindow);
        } else {
            (w.start, w.armed) = (now, false);
        }
    }

    /// Value of WDTPSL / WDTPSH / WDTTMR (`sub`) at cycle `now`.
    fn wdt_value(&self, sub: u8, m: &DataMem, now: u64) -> u8 {
        let w = self.wdt.as_ref().unwrap();
        let (c0, c1) = (m.mem[w.con0], w.con1_value(&m.mem));
        let psc = (w.clocks(now, c1) & ((1u64 << Wdt::n(c0)) - 1)) as u32;
        match sub {
            wdt::PSL => psc as u8,
            wdt::PSH => (psc >> 8) as u8,
            _ => w.window_value(now, c0, c1) << 3 | u8::from(w.armed) << 2 | (psc >> 16) as u8 & 3,
        }
    }

    /// A write to WDTCON0 / WDTCON1 (the other watchdog registers are read only): read-only fields keep their value, the WWDT is cleared.
    fn wdt_write(&mut self, sub: u8, v: u8, now: u64, m: &mut DataMem) {
        let w = self.wdt.as_mut().unwrap();
        match sub {
            wdt::CON0 => {
                let keep = if w.ps_fixed { 0x3E } else { 0 };
                m.mem[w.con0] = v & !keep | m.mem[w.con0] & keep;
            }
            wdt::CON1 => {
                let keep = if w.cs_fixed { 0x70 } else { 0 } | if w.win_fixed { 0x07 } else { 0 };
                m.mem[w.con1] = v & !keep | w.con1_value(&m.mem) & keep;
                if w.con1_value(&m.mem) >> 4 & 7 > 1 {
                    self.notes.insert("WDTCS reserved value: the watchdog runs from LFINTOSC".to_string());
                }
            }
            _ => return,
        }
        (w.start, w.armed) = (now, false);
    }

    /// Level of CLC data input `src` (DyS code, DS40002043F p.409) of cell `n` at cycle `now`.
    fn clc_input(&mut self, m: &DataMem, src: u8, now: u64, n: u8) -> bool {
        if self.q41 {
            let code = match src {
                0..=3 => return self.clcin.get(src as usize).is_some_and(|&r| r != usize::MAX && self.pps_pin(m, r, now)),
                20 => 5,
                21..=26 => src - 15,
                28 | 29 => src - 15,
                34..=37 => src - 10,
                49 | 50 => src - 48,
                _ => {
                    self.notes.insert(format!("CLC{n} data input {src} is not modelled; it reads low"));
                    return false;
                }
            };
            return self.sig >> code & 1 != 0;
        }
        let code = match src {
            0..=7 => return self.clcin.get(usize::from(src)).is_some_and(|&r| r != usize::MAX && self.pps_pin(m, r, now)),
            24..=27 => src - 19, // CCP1, CCP2, PWM3, PWM4 = source codes 5..8
            28 | 29 => src - 15, // CMP1 = 0x0D, CMP2 = 0x0E
            33..=40 => 0x18 + src - 33,
            49 | 50 => src - 48,
            _ => {
                self.notes.insert(format!("CLC{n} data input {src} (clock, timer, serial, DSM, ZCD or IOC source) is not modelled; it reads low"));
                return false;
            }
        };
        self.sig >> code & 1 != 0
    }

    /// One evaluation of cell `i` at `tick` Fosc ticks; true when its output changed.
    fn clc_step(&mut self, m: &mut DataMem, i: usize, tick: u64) -> bool {
        let now = tick / 4;
        let c = &self.clcs[i];
        let (con, flag, sig, n) = (c.con, c.flag, c.sig, c.n);
        let cfg = c.values(m);
        let (was_on, q0, clk0, out0) = (c.on, c.q, c.clk, c.out);
        let on = cfg[0] & 0x80 != 0 && !pmd_off(m, c.md);
        let (mut q, mut clk, mut out) = (false, false, false);
        if on {
            let pol = cfg[1];
            let used = |k: usize| cfg[6..10].iter().any(|&g| g >> (2 * k) & 3 != 0);
            let mut d = [false; 4];
            for (k, dk) in d.iter_mut().enumerate() {
                if used(k) {
                    *dk = self.clc_input(m, cfg[2 + k] & if self.q41 { 127 } else { 63 }, now, n);
                }
            }
            let mut g = [false; 4];
            for (y, gy) in g.iter_mut().enumerate() {
                *gy = clc::gate(cfg[6 + y], pol >> y & 1 != 0, d);
            }
            clk = g[0];
            q = clc::logic(cfg[0] & 7, g, was_on && q0, if was_on { clk0 } else { g[0] });
            out = q != (pol & 0x80 != 0);
        }
        let c = &mut self.clcs[i];
        (c.q, c.clk, c.on) = (q, clk, on);
        if out != out0 {
            c.out = out;
        }
        if let Some(b) = &mut c.bank { b[0] = cfg[0] & !0x20 | u8::from(out) << 5; }
        else { m.mem[con] = cfg[0] & !0x20 | u8::from(out) << 5; }
        if let Some(a) = self.clcdata {
            let bit = 1u8 << (n - 1);
            if out { m.mem[a] |= bit } else { m.mem[a] &= !bit }
        }
        if out == out0 {
            return false;
        }
        self.set_sig(m, sig, u8::from(out), tick);
        if cfg[0] & (if out { 0x10 } else { 0x08 }) != 0 {
            flag.set(m);
            self.raised(now);
        }
        true
    }

    /// Some cell is enabled or still shows a high output (so that disabling it is seen).
    fn clc_active(&self, m: &DataMem) -> bool {
        self.clcs.iter().any(|c| c.on || c.out || c.values(m)[0] & 0x80 != 0)
    }

    /// Re-evaluate all the cells at `tick` until the outputs settle (cells feed each other).
    fn clc_eval(&mut self, m: &mut DataMem, tick: u64) {
        if self.in_clc || !self.clc_active(m) {
            return;
        }
        self.in_clc = true;
        let mut settled = false;
        for _ in 0..8 {
            let mut changed = false;
            for i in 0..self.clcs.len() {
                changed |= self.clc_step(m, i, tick);
            }
            if !changed {
                settled = true;
                break;
            }
        }
        if !settled {
            self.notes.insert("CLC: a loop of cells did not settle within 8 passes (ring oscillator); left as is".to_string());
        }
        self.in_clc = false;
    }

    /// Pin stimulus changes between the last look and `now` are evaluated in order (the state at `now` follows in `sync`).
    fn clc_scan(&mut self, m: &mut DataMem, now: u64) {
        let from = self.clc_scan;
        if !self.clc_active(m) {
            self.clc_scan = now;
            return;
        }
        let mut at: Vec<u64> = self.drive.times(from, now.saturating_sub(1)).collect();
        at.dedup();
        for t in at {
            self.clc_eval(m, 4 * t);
        }
        self.clc_scan = now;
    }

    /// Voltage of a comparator input source at cycle `now`.
    fn cmp_volts(&self, m: &DataMem, src: cmp::Src, now: u64) -> f64 {
        match src {
            cmp::Src::Pin(l, b) => self.analog[..self.analog.partition_point(|e| e.0 <= now)].iter().rev().find(|e| e.1 == l && e.2 == b).map_or(0.0, |e| e.3),
            cmp::Src::Fvr => self.fvr.map_or(0.0, |a| cmp::fvr_volts(m.mem[a])),
            _ => 0.0,
        }
    }

    /// The output level of comparator `i` at cycle `now`, after POL.
    fn cmp_level(&self, m: &DataMem, i: usize, now: u64) -> bool {
        let c = &self.cmps[i];
        let con = m.mem[c.con0];
        let core = con & 0x80 != 0 && !pmd_off(m, c.md) && {
            let (p, n) = (c.positive(m.mem[c.pch]), c.negative(m.mem[c.nch]));
            self.cmp_volts(m, p, now) > self.cmp_volts(m, n, now)
        };
        core != (con & 0x10 != 0)
    }

    /// Re-evaluate comparator `i` at cycle `at`: on a change update OUT and MCxOUT, drive the PPS source, set CxIF per INTP/INTN
    /// and let the CWG see a shutdown source change.
    fn cmp_update(&mut self, m: &mut DataMem, i: usize, at: u64) {
        let new = self.cmp_level(m, i, at);
        let c = &self.cmps[i];
        let (con0, con1, flag, sig, n) = (c.con0, c.con1, c.flag, c.sig, c.n);
        let pch = self.cmps[i].positive(m.mem[c.pch]);
        if m.mem[con0] & 0x80 != 0 && pch == cmp::Src::Dac {
            self.notes.insert(format!("C{n} DAC input is not modelled; it reads 0 V"));
        }
        if new == c.out {
            return;
        }
        self.cmps[i].out = new;
        if new { m.mem[con0] |= 0x40 } else { m.mem[con0] &= !0x40 }
        if let Some(cm) = self.cmout {
            let bit = 1u8 << (n - 1);
            if new { m.mem[cm] |= bit } else { m.mem[cm] &= !bit }
        }
        self.set_sig(m, sig, u8::from(new), 4 * at);
        if m.mem[con1] & (if new { 2 } else { 1 }) != 0 {
            flag.set(m);
            self.raised(at);
        }
        if let Some(a0) = self.cwg.as_ref().map(|c| c.as0) {
            let before = m.mem[a0] & 0x80;
            self.cwg_sources(m, at);
            if m.mem[a0] & 0x80 != before {
                let ch = self.cwg.as_ref().unwrap().now(m);
                self.cwg_apply(m, 4 * at, ch);
            }
        }
    }

    /// Stimulus changes of the analog pins between the last look and `now`: each distinct cycle is evaluated in order.
    fn cmp_scan(&mut self, m: &mut DataMem, now: u64) {
        for i in 0..self.cmps.len() {
            let from = self.cmps[i].scan;
            let (a, b) = (self.analog.partition_point(|e| e.0 <= from), self.analog.partition_point(|e| e.0 <= now));
            let mut at: Vec<u64> = self.analog[a..b.max(a)].iter().map(|e| e.0).collect();
            at.dedup();
            for t in at {
                self.cmp_update(m, i, t);
            }
            self.cmps[i].scan = now;
        }
    }

    /// Stimulus: pin `bit` of port `letter` is `volts` from cycle `at` on.
    pub fn analog_feed(&mut self, letter: u8, bit: u8, at: u64, volts: f64) -> bool {
        if !self.ports.iter().any(|p| p.letter == letter) {
            return false;
        }
        crate::drive::insert_sorted(&mut self.analog, (at, letter, bit, volts), |e| e.0);
        true
    }

    /// Apply the outputs the CWG asks for at `tick`; delayed changes (dead band) go to the schedule. Pending delayed
    /// changes are dropped first: an input that turned round before the dead band ended shows no pulse (25.8).
    fn cwg_apply(&mut self, m: &mut DataMem, tick: u64, ch: [cwg::Change; 4]) {
        self.sched.retain(|s| !(1..=4).contains(&s.code));
        for (y, c) in ch.into_iter().enumerate() {
            let code = y as u8 + 1;
            let bit = 1u8 << y;
            let Some(l) = c.level else {
                self.cwg.as_mut().unwrap().hiz |= bit;
                continue;
            };
            self.cwg.as_mut().unwrap().hiz &= !bit;
            if c.delay == 0 {
                self.set_sig(m, code, u8::from(l), tick);
            } else {
                self.sched.push(Sched { tick: tick + c.delay, code, act: u8::from(l), flag: None, pwm: false });
            }
        }
    }

    /// Shutdown sources: the AS0E pin (low) is looked at; the others are not modelled `[A76]`. Sets SHUTDOWN and CWG1IF when a source is
    /// active, clears SHUTDOWN by hardware when the sources are gone and REN = 1. Returns true when a source is active.
    fn cwg_sources(&mut self, m: &mut DataMem, now: u64) -> bool {
        let Some(c) = &self.cwg else { return false };
        let (as0, as1, pps) = (c.as0, m.mem[c.as1], c.pps);
        if as1 & if self.q41 { 0xE6 } else { 0xCE } != 0 {
            self.notes.insert("CWG1 selected auto-shutdown source is not modelled (the pin and comparators are)".to_string());
        }
        let cmp_low = |n: usize| self.cmps.get(n).is_some_and(|c| !c.out);
        let (c1, c2) = if self.q41 { (0x08, 0x10) } else { (0x10, 0x20) };
        let active = as1 & 1 != 0 && pps.is_some_and(|r| !self.pps_pin(m, r, now)) || as1 & c1 != 0 && cmp_low(0) || as1 & c2 != 0 && cmp_low(1);
        let c = self.cwg.as_mut().unwrap();
        if active && c.en(m) && m.mem[as0] & 0x80 == 0 {
            m.mem[as0] |= 0x80;
            c.flag.set(m);
            c.reset_seq();
            self.raised(now);
        } else if !active && m.mem[as0] & 0xC0 == 0xC0 {
            m.mem[as0] &= !0x80; // REN = 1: hardware restart
            c.wait_rise_set();
        }
        active
    }

    /// The CWG data input went to `new` at `tick` (Fosc ticks).
    fn cwg_edge(&mut self, m: &mut DataMem, new: bool, tick: u64) {
        self.cwg_sources(m, tick / 4);
        let ch = self.cwg.as_mut().unwrap().edge(m, new);
        self.cwg_apply(m, tick, ch);
    }

    /// A write to a CWG1 register (hook sub-code `k - CWG0`).
    fn cwg_write(&mut self, k: u8, v: u8, now: u64, m: &mut DataMem) {
        let tick = 4 * now;
        let c = self.cwg.as_mut().unwrap();
        let sub = k - CWG0;
        match sub {
            cwg::CON0 => {
                let old = m.mem[c.con0];
                m.mem[c.con0] = v & 0xC7;
                if v & 0x80 == 0 {
                    c.disable();
                } else if old & 0x80 == 0 {
                    let lvl = c.source(m).is_some_and(|src| self.sig >> src & 1 != 0);
                    c.enable(m, lvl);
                } else {
                    if v & 0x40 != 0 && old & 0x40 == 0 {
                        c.ld_set();
                    }
                    if (v ^ old) & 7 == 1 && v & 6 == 2 {
                        c.dir_change();
                    }
                }
                if c.source(m).is_none() {
                    self.notes.insert(if self.q41 { "CWG1 selected data input is not modelled; the CWG sees it low" } else { "CWG1 data input other than CCP1, CCP2, PWM3, PWM4, CMP1 and CMP2 is not modelled; the CWG sees it low" }.to_string());
                }
                if m.mem[self.cwg.as_ref().unwrap().clk] & 1 != 0 {
                    self.notes.insert("CWG1 clock HFINTOSC is counted as Fosc".to_string());
                }
            }
            cwg::CON1 => {
                m.mem[c.con1] = v & 0x0F | m.mem[c.con1] & 0x20;
            }
            cwg::AS0 => {
                let old = m.mem[c.as0];
                let keep = if v & 0x80 != 0 { 0x80 } else { 0 };
                m.mem[c.as0] = v & 0x7F | keep;
                if v & 0x80 == 0 && old & 0x80 != 0 {
                    // Software clear: refused while a source is active; outputs wait for the next rising edge.
                    let active = self.cwg_sources(m, now);
                    let c = self.cwg.as_mut().unwrap();
                    if active {
                        m.mem[c.as0] |= 0x80;
                    } else {
                        c.wait_rise_set();
                    }
                } else if v & 0x80 != 0 && old & 0x80 == 0 {
                    c.flag.set(m);
                    c.reset_seq();
                    self.raised(now);
                    if v & 0x40 != 0 && !self.cwg_sources(m, now) {
                        // Auto-restart: SHUTDOWN clears itself, the outputs wait for the next rising edge.
                        let c = self.cwg.as_mut().unwrap();
                        m.mem[c.as0] &= !0x80;
                        c.wait_rise_set();
                    }
                }
            }
            cwg::AS1 => {
                m.mem[c.as1] = v;
            }
            cwg::STR => {
                m.mem[c.str_] = v; // asynchronous steering takes effect at the end of this cycle (below)
            }
            cwg::DBR | cwg::DBF => {
                let reg = if sub == cwg::DBR { c.dbr } else { c.dbf };
                m.mem[reg] = v & 63;
                if m.mem[c.con0] & 0x80 == 0 {
                    c.load_buffers(m);
                }
            }
            _ => unreachable!(),
        }
        if matches!(sub, cwg::CON0 | cwg::AS0 | cwg::AS1 | cwg::CON1 | cwg::STR) {
            if matches!(sub, cwg::CON0 | cwg::AS1) {
                self.cwg_sources(m, now);
            }
            let ch = self.cwg.as_ref().unwrap().now(m);
            self.cwg_apply(m, tick + 3, ch);
        }
    }

    /// Apply the scheduled output changes that are due by cycle `now`, in time order.
    fn apply_sched(&mut self, m: &mut DataMem, now: u64, ev: &mut Vec<u64>) {
        // A change can schedule another one (CWG dead band) that is also due: go on until nothing is.
        loop {
            // Edges that Timer2-type time bases make (`pwm`) wait while the part sleeps.
            let asleep = self.fosc_off;
            let (mut due, rest): (Vec<Sched>, Vec<Sched>) = std::mem::take(&mut self.sched).into_iter().partition(|s| s.tick / 4 <= now && !(asleep && s.pwm));
            self.sched = rest;
            if due.is_empty() {
                break;
            }
            due.sort_by_key(|s| s.tick);
            for s in due {
                self.set_sig(m, s.code, s.act, s.tick);
                if let Some(f) = s.flag {
                    f.set(m);
                    ev.push(s.tick / 4);
                }
            }
        }
    }

    /// Effective Fosc in Hz, from firmware unless the board supplies an override.
    fn fosc_hz(&self) -> u64 {
        (self.board.effective_fosc_mhz() * 1e6) as u64
    }

    /// The Q41 family (its timers select SOSC with other codes).
    fn q41(&self) -> bool {
        self.nvmq.is_some()
    }

    /// Clock of Timer0 now.
    fn t0_clock(&self, m: &DataMem) -> Option<Clk> {
        // Classic T0CON: T0CS = 0 is the instruction clock FOSC/4; the T0CKI pin (RA4) is not modelled there (DS40001412H Register 11-1).
        self.t0.as_ref().map(|t| if t.classic { if m.mem[t.c0] & 0x20 == 0 { Clk::Cyc(1) } else { Clk::Other } } else { t0_clk(self.q41(), m.mem[t.c1] >> 5) })
    }

    /// Clock of Timer1-type timer `i` now.
    fn t1_clock(&self, m: &DataMem, i: usize) -> Clk {
        let t = &self.t1[i];
        if t.classic {
            let con = m.mem[t.con];
            return match con >> 6 {
                0 => Clk::Cyc(1),
                1 => Clk::Cyc(4),
                2 if con & 8 != 0 => Clk::Hz(SOSC_HZ),
                2 => Clk::Pin(false),
                _ => Clk::Other,
            };
        }
        t1_clk(self.q41(), m.mem[t.clk] & 0x1F)
    }

    /// Timer0 counts without Fosc/4: ON, T0ASYNC = 1 and a pin or an oscillator that runs in Sleep `[A102]`.
    fn t0_sleep_ok(&self, m: &DataMem) -> bool {
        let Some(t) = &self.t0 else { return false };
        // The classic Timer0 is shut down in Sleep (DS40001412H 11.5 p.156).
        !t.classic && m.mem[t.c0] & 0x80 != 0 && m.mem[t.c1] & 0x10 != 0 && matches!(self.t0_clock(m), Some(Clk::Pin(_) | Clk::Hz(_)))
    }

    /// Timer1-type timer `i` counts without Fosc/4: ON, SYNC = 1 and a pin or an oscillator that runs in Sleep `[A102]`.
    fn t1_sleep_ok(&self, m: &DataMem, i: usize) -> bool {
        let con = m.mem[self.t1[i].con];
        con & 1 != 0 && con & 4 != 0 && matches!(self.t1_clock(m, i), Clk::Pin(_) | Clk::Hz(_))
    }

    /// The ticks a clock makes in cycles `from+1 ..= to`. A pin edge is a change of its level (`cki` is the PPS register).
    fn clock_edges(&self, m: &DataMem, clk: Clk, cki: Option<usize>, from: u64, to: u64) -> Edges {
        match clk {
            Clk::Cyc(r) => Edges { n: (to - from) * r, how: How::Ratio(r) },
            Clk::Hz(hz) => {
                let fosc = self.fosc_hz();
                let ns = self.board.time_ns(from);
                let e0 = (ns * hz as f64 / 1e9 + 1e-9).floor() as u64;
                let e1 = (self.board.time_ns(to) * hz as f64 / 1e9 + 1e-9).floor() as u64;
                Edges { n: e1.saturating_sub(e0), how: How::Time { hz, fosc, e0, base: from, ns } }
            }
            Clk::Pin(inv) => {
                let mut l: Vec<u64> = Vec::new();
                if let Some(reg) = cki {
                    let mut times = self.pin_times(from, to);
                    times.sort_unstable();
                    times.dedup();
                    for t in times {
                        let (old, new) = (self.pps_pin(m, reg, t - 1), self.pps_pin(m, reg, t));
                        if old != new && new != inv {
                            l.push(t);
                        }
                    }
                }
                Edges { n: l.len() as u64, how: How::List(l) }
            }
            Clk::Other => Edges { n: 0, how: How::Ratio(0) },
        }
    }

    /// The part is in Sleep with Fosc and Fosc/4 off (not Idle): only the models of `sync_asleep` move.
    pub fn fosc_stopped(&self) -> bool {
        self.fosc_off
    }

    /// Some timer counts in this Sleep (a pin, LFINTOSC or SOSC clock).
    pub fn timer_in_sleep(&self) -> bool {
        self.t0.as_ref().is_some_and(|t| t.sleep_run) || self.t1.iter().any(|t| t.sleep_run)
    }

    /// The next cycle after `after` on which an oscillator that keeps a timer running in Sleep makes an edge. The part
    /// is looked at on each one so that the timer's flag wakes it on the cycle it is set `[A102]`.
    pub fn next_osc_edge(&self, m: &DataMem, after: u64) -> Option<u64> {
        let fosc = self.fosc_hz();
        let mut hzs: Vec<u64> = Vec::new();
        if self.t0.as_ref().is_some_and(|t| t.sleep_run)
            && let Some(Clk::Hz(h)) = self.t0_clock(m)
        {
            hzs.push(h);
        }
        for i in 0..self.t1.len() {
            if self.t1[i].sleep_run
                && let Clk::Hz(h) = self.t1_clock(m, i)
            {
                hzs.push(h);
            }
        }
        hzs.into_iter().map(|hz| {
            let ns = self.board.time_ns(after);
            let edge = (ns * hz as f64 / 1e9 + 1e-9).floor() + 1.0;
            after + (((edge * 1e9 / hz as f64 - ns) * fosc as f64 / 4e9) - 1e-9).ceil().max(1.0) as u64
        }).min()
    }

    /// The next cycle after `after` that a stimulus analog level changes.
    pub fn next_analog(&self, after: u64) -> Option<u64> {
        self.analog.iter().map(|e| e.0).filter(|&t| t > after).min()
    }

    /// The next cycle after `after` that a scheduled pin change (not a Timer2-type edge, which waits in Sleep) or the end of a
    /// data EEPROM write falls due.
    pub fn next_pending(&self, after: u64) -> Option<u64> {
        let s = self.sched.iter().filter(|s| !s.pwm).map(|s| s.tick / 4);
        let n = self.nvm.iter().filter_map(|n| n.end()).chain(self.nvmq.iter().filter_map(|n| n.end()));
        s.chain(n).filter(|&t| t > after).min()
    }

    /// The timers gated by a CCP, PWM, comparator or CLC output, now that the edges of this span are known.
    fn t1_late(&mut self, m: &mut DataMem, now: u64, asleep: bool, late: u64, sb: &mut SyncBufs) {
        if late == 0 {
            return;
        }
        let SyncBufs { prep, times, ev, comps, hits, .. } = sb;
        hits.clear();
        for ti in 0..self.t1.len() {
            if late >> ti & 1 != 0 {
                self.t1_prepare(m, ti, now, asleep, &mut prep[ti], times);
                self.t1_step(m, ti, now, comps, &prep[ti], ev, hits);
            }
        }
        self.sched.append(hits);
        self.apply_sched(m, now, ev);
    }

    /// Gate plan and clock edges of Timer1-type timer `i` over the span from its last look to `now`.
    fn t1_prepare(&mut self, m: &DataMem, i: usize, now: u64, asleep: bool, p: &mut T1Prep, times: &mut Vec<u64>) {
        let last = self.t1[i].last;
        p.on = true;
        p.plan = now > last && self.gate_plan(m, i, last, now, &mut p.gp, times);
        let t = &self.t1[i];
        let clk = self.t1_clock(m, i);
        let go = (!asleep || self.t1_sleep_ok(m, i)) && m.mem[t.con] & 1 != 0 && now > last;
        p.clk = go.then_some(clk);
        p.wins.clear();
        if go {
            if p.plan {
                for &(a, b, counts) in &p.gp.segs {
                    if counts {
                        p.wins.push((a, self.clock_edges(m, clk, t.cki, a, b)));
                    }
                }
            } else {
                p.wins.push((last, self.clock_edges(m, clk, t.cki, last, now)));
            }
        }
    }

    /// Bring Timer1-type timer `ti` to `now` with its gate plan and clock edges; compare matches go to `hits`.
    #[allow(clippy::too_many_arguments)]
    fn t1_step(&mut self, m: &mut DataMem, ti: usize, now: u64, comps_all: &[(usize, u16, u8, u8)], p: &T1Prep, ev: &mut Vec<u64>, hits: &mut Vec<Sched>) {
        let t = &mut self.t1[ti];
        let tn = t.n;
        let comps = || comps_all.iter().filter(move |c| c.3 == tn).map(|&(i, r, mo, _)| (i, r, mo));
        t.last = now;
        if p.plan {
            let g = &p.gp;
            (t.gff, t.grun, t.gb) = (g.ff, g.run, g.b);
            if m.mem[t.gcon] & 8 != 0 && !g.ggo {
                m.mem[t.gcon] &= !8; // GGO/DONE: the pulse is done
            }
            for &c in &g.gif {
                if let Some(f) = t.gif {
                    f.set(m);
                }
                ev.push(c);
            }
        }
        let con = m.mem[t.con];
        let clk = p.clk.unwrap_or(Clk::Other);
        let wins: &[(u64, Edges)] = if p.clk.is_some() { &p.wins } else { &[] };
        for &(base, ref ed) in wins {
            let pre0 = t.pre;
            let cs = if t.classic { m.mem[t.con] >> 6 } else { m.mem[t.clk] & 0x1F };
            if clk == Clk::Other {
                self.notes.insert(format!("Timer{} clock source CS={cs} is not modelled (Fosc/4, Fosc, the TxCKIPPS pin, LFINTOSC and SOSC are); it does not count", t.n));
            } else {
                let clocks = prescale(&mut t.pre, u32::from((con >> 4) & 3), ed.n);
                let sh = u32::from((con >> 4) & 3);
                if comps().next().is_none() {
                    if std::mem::take(&mut t.clr) && clocks > 0 {
                        t.cnt = 0;
                    }
                    let total = u64::from(t.cnt) + clocks;
                    if total >= 65536 {
                        t.flag.set(m);
                        let last_wrap = (total / 65536) * 65536 - u64::from(t.cnt);
                        ev.push(ed.at(base, pre0, sh, last_wrap));
                    }
                    t.cnt = total as u16;
                } else {
                    // Step from event to event: the next compare match or the rollover, whichever comes first.
                    let (mut left, mut done) = (clocks, 0u64);
                    while left > 0 {
                        if t.clr {
                            t.clr = false;
                            t.cnt = 0;
                            left -= 1;
                            done += 1;
                            continue;
                        }
                        let to_wrap = 65536 - u64::from(t.cnt);
                        let to_match = comps()
                            .map(|(_, r, _)| {
                                let d = u64::from(r.wrapping_sub(t.cnt));
                                if d == 0 { 65536 } else { d }
                            })
                            .min()
                            .unwrap();
                        let step = left.min(to_wrap).min(to_match);
                        let wrapped = step == to_wrap;
                        t.cnt = t.cnt.wrapping_add(step as u16);
                        done += step;
                        left -= step;
                        let cyc = ed.at(base, pre0, sh, done);
                        if wrapped {
                            t.flag.set(m);
                            ev.push(cyc);
                        }
                        let mut clear = false;
                        for (i, r, mode) in comps() {
                            if r == t.cnt {
                                // [A61]
                                let act = match mode {
                                    8 => 1,
                                    9 => 0,
                                    1 | 2 => 2,
                                    _ => 3, // pulse modes: flag only [A63]
                                };
                                if act == 3 {
                                    self.notes.insert("CCP compare pulse output (MODE 1010/1011) does not drive the pin".into());
                                }
                                hits.push(Sched { tick: 4 * cyc, code: self.ccps[i].sig, act, flag: Some(self.ccps[i].flag), pwm: false });
                                clear |= matches!(mode, 1 | 11);
                            }
                        }
                        t.clr = clear;
                    }
                }
            }
        }
        if m.mem[t.con] & 2 == 0 {
            m.mem[t.h] = (t.cnt >> 8) as u8;
        }
        m.mem[t.l] = t.cnt as u8;
    }

    fn sync_core(&mut self, m: &mut DataMem, now: u64, asleep: bool) {
        // A nested call (none is expected) would find the buffers taken and simply allocate its own.
        let mut sb = std::mem::take(&mut self.sb);
        sb.ev.clear();
        sb.hits.clear();
        sb.periods.clear();
        sb.comps.clear();
        self.sync_with(m, now, asleep, &mut sb);
        self.sb = sb;
    }

    fn sync_with(&mut self, m: &mut DataMem, now: u64, asleep: bool, sb: &mut SyncBufs) {
        self.pwmq_sync(m, now, asleep);
        // Compare CCPs with the timer they use: (index, CCPR, MODE, timer number).
        let mut comps = std::mem::take(&mut sb.comps);
        comps.extend(self
            .ccps
            .iter()
            .enumerate()
            .filter(|(_, c)| c.en(m) && !c.k22 && matches!(c.mode(m), 1 | 2 | 8 | 9 | 10 | 11))
            .filter_map(|(i, c)| {
                let n = ccp::timer_of(self.ccptmrs, m, c.n, false);
                if self.t1.iter().any(|t| t.n == n) {
                    Some((i, c.ccpr(m), c.mode(m), n))
                } else {
                    self.notes.insert(format!("CCP{} selects a capture/compare timer this part does not have (CCPTMRS); it is not modelled", c.n));
                    None
                }
            }));
        sb.comps = comps;
        // Capture from a source other than the CCPxPPS pin (CTS not 0) is not modelled, as on the PIC16 core.
        for c in &self.ccps {
            let cap = m.mem[c.cap] & 15;
            if cap != 0 && c.en(m) && (3..=7).contains(&c.mode(m)) {
                self.notes.insert(format!("CCP{} capture source CTS={cap} is not modelled (only the CCPxPPS pin); no capture", c.n));
            }
        }
        // Timer2-type period matches (timer number, cycle, PR, prescaler shift) go to `sb.periods`.
        // Each running timer's clock edges are looked up first: they read the pins and the other tables of `self`.
        // In Sleep a timer is run if it was set up to count at SLEEP (`sleep_run`, the same fact `thaw` goes by); it counts
        // only while its registers still say so (a debugger can write them meanwhile).
        let t0_run = !asleep || self.t0.as_ref().is_some_and(|t| t.sleep_run);
        let t0_in = match (&self.t0, self.t0_clock(m)) {
            (Some(t), Some(clk)) if t0_run && (!asleep || self.t0_sleep_ok(m)) && m.mem[t.c0] & 0x80 != 0 && now > t.last => Some((clk, self.clock_edges(m, clk, t.cki, t.last, now))),
            _ => None,
        };
        // A timer gated by a CCP, PWM, comparator or CLC output waits until those have made their edges of this span (bit i
        // of `late`).
        assert!(self.t1.len() <= 64);
        let mut late = 0u64;
        sb.prep.resize_with(self.t1.len(), T1Prep::default);
        for i in 0..self.t1.len() {
            sb.prep[i].on = false;
            if !asleep || self.t1[i].sleep_run {
                if matches!(self.gate_src(m, i), GSrc::Sig(_)) {
                    late |= 1 << i;
                } else {
                    self.t1_prepare(m, i, now, asleep, &mut sb.prep[i], &mut sb.times);
                }
            }
        }
        let ev = &mut sb.ev;
        if let Some(t) = self.t0.as_mut().filter(|_| t0_run) {
            let (base, pre0) = (t.last, t.pre);
            t.last = now;
            let c1 = m.mem[t.c1];
            if let Some((clk, ed)) = t0_in {
                if clk == Clk::Other {
                    if t.classic {
                        self.notes.insert("Timer0 clock T0CS = 1 (the T0CKI pin) is not modelled on this part (FOSC/4 is); it does not count".into());
                    } else {
                        self.notes.insert(format!("Timer0 clock source T0CS={} is not modelled (Fosc/4, the T0CKIPPS pin, LFINTOSC and SOSC are); it does not count", c1 >> 5));
                    }
                } else {
                    let sh = t.shift(m);
                    let clocks = prescale(&mut t.pre, sh, ed.n);
                    let outps = t.outps(m);
                    let wide = t.wide(m);
                    let top = t.top8(m);
                    let classic = t.classic;
                    for j in 1..=clocks {
                        let wrap = if wide {
                            t.cnt = t.cnt.wrapping_add(1);
                            t.cnt == 0
                        } else if t.cnt as u8 == top {
                            t.cnt = 0;
                            true
                        } else {
                            // 8-bit mode counts in TMR0L only; a high byte left from 16-bit mode is dropped (B4)
                            t.cnt = u16::from((t.cnt as u8).wrapping_add(1));
                            false
                        };
                        if wrap {
                            if t.post >= outps {
                                t.post = 0;
                                if !classic {
                                    m.mem[t.c0] ^= 0x20; // T0OUT toggles
                                }
                                t.flag.set(m);
                                ev.push(ed.at(base, pre0, sh, j));
                            } else {
                                t.post += 1;
                            }
                        }
                    }
                }
            }
            m.mem[t.l] = t.cnt as u8;
        }
        for ti in 0..self.t1.len() {
            if sb.prep[ti].on {
                self.t1_step(m, ti, now, &sb.comps, &sb.prep[ti], &mut sb.ev, &mut sb.hits);
            }
        }
        if asleep {
            // Only the timers on a clock that runs in Sleep were looked at; what they make is done here, the rest waits for `thaw`.
            self.sched.append(&mut sb.hits);
            self.apply_sched(m, now, &mut sb.ev);
            self.t1_late(m, now, true, late, sb);
            self.sig_prune_to(4 * self.t1.iter().map(|t| t.last).min().unwrap_or(now));
            for k in 0..sb.ev.len() {
                self.raised(sb.ev[k]);
            }
            return;
        }
        let (ev, hits, periods) = (&mut sb.ev, &mut sb.hits, &mut sb.periods);
        for t in &mut self.t2 {
            let (base, pre0) = (t.last, t.pre);
            let dt = now - t.last;
            t.last = now;
            if t.on(m) && dt > 0 {
                let cs = t.cs(m);
                let mode = t.mode(m);
                if cs != 1 && cs != 2 {
                    self.notes.insert(format!("Timer{} clock source CS={cs} is not modelled (only FOSC/4 and FOSC); it does not count", t.n));
                } else {
                    if mode != 0 && mode != 8 {
                        self.notes.insert(format!("Timer{} MODE={mode:#04x} needs an external reset/gate source (S4); run as the software mode", t.n));
                    }
                    let one_shot = mode >> 3 == 1;
                    if cs == 2 {
                        // The PWM edges below are placed from period ends on Fosc/4 cycles; on Fosc the period ends
                        // are not collected, so a CCP in PWM mode or a PWM module on this timer drives nothing.
                        let tn = t.n;
                        let ccp = self.ccps.iter().find(|c| c.en(m) && c.mode(m) >= 12 && ccp::timer_of(self.ccptmrs, m, c.n, true) == tn).map(|c| ("CCP", c.n));
                        let pwm = || self.pwms.iter().find(|w| w.en(m) && ccp::timer_of(self.ccptmrs, m, w.n, true) == tn).map(|w| ("PWM", w.n));
                        if let Some((kind, n)) = ccp.or_else(pwm) {
                            self.notes.insert(format!("{kind}{n} PWM on Timer{tn} clocked by FOSC (T{tn}CLK CS=2) is not modelled: it gives no output edges (Timer{tn} counts and sets its flag)"));
                        }
                    }
                    let ticks = if cs == 2 { dt * 4 } else { dt };
                    let sh = t.shift(m);
                    let clocks = prescale(&mut t.pre, sh, ticks);
                    let outps = t.outps(m);
                    let pr = m.mem[t.pr];
                    let mut left = clocks;
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
                            if cs == 1 {
                                periods.push((t.n, at(base, pre0, sh, j, ratio), pr, sh));
                            }
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
        // PWM: the pin sets at each Timer2 period match and clears when the 10-bit time base reaches the duty.
        for &(tn, c, t2_pr, t2_sh) in periods.iter() {
            for ccp in &self.ccps {
                if !ccp.en(m) || ccp.mode(m) < 12 {
                    continue;
                }
                let want = ccp::timer_of(self.ccptmrs, m, ccp.n, true);
                if !self.t2.iter().any(|t| t.n == want) {
                    self.notes.insert(format!("CCP{} PWM selects a timer this part does not have (CCPTMRS); it is not modelled", ccp.n));
                    continue;
                }
                if want != tn {
                    continue;
                }
                let duty = u64::from(ccp.duty(m));
                let pulse = duty << t2_sh; // Tosc ticks: CCPR * TOSC * prescale
                let period = (u64::from(t2_pr) + 1) * 4 << t2_sh;
                if duty == 0 {
                    continue;
                }
                let (on, off) = if ccp.inverted(m) { (0, 1) } else { (1, 0) };
                hits.push(Sched { tick: 4 * c, code: ccp.sig, act: on, flag: None, pwm: true });
                if pulse < period {
                    hits.push(Sched { tick: 4 * c + pulse, code: ccp.sig, act: off, flag: Some(ccp.flag), pwm: true }); // [A62]
                }
            }
        }
        for &(tn, c, t2_pr, t2_sh) in periods.iter() {
            for w in &self.pwms {
                if !w.en(m) || ccp::timer_of(self.ccptmrs, m, w.n, true) != tn {
                    continue;
                }
                let (on, off) = if w.pol(m) { (0, 1) } else { (1, 0) };
                let pulse = u64::from(w.duty(m)) << t2_sh;
                let period = (u64::from(t2_pr) + 1) * 4 << t2_sh;
                if pulse == 0 {
                    continue;
                }
                hits.push(Sched { tick: 4 * c, code: w.sig, act: on, flag: None, pwm: true });
                if pulse < period {
                    hits.push(Sched { tick: 4 * c + pulse, code: w.sig, act: off, flag: None, pwm: true });
                }
            }
        }
        self.sched.append(hits);
        self.apply_sched(m, now, ev);
        self.cmp_scan(m, now);
        self.apply_sched(m, now, ev); // comparator edges can schedule CWG dead-band changes
        self.t1_late(m, now, false, late, sb);
        let ev = &mut sb.ev;
        self.sig_prune_to(4 * self.t1.iter().map(|t| t.last).min().unwrap_or(now));
        if let Some(a) = &mut self.adc
            && a.sync(&mut m.mem, now, ev)
        {
            a.flag.set(m);
        }
        for u in &mut self.uarts {
            u.sync(&mut m.mem, now, ev, &mut self.tx, &mut self.notes);
        }
        for i in 0..self.uarts.len() {
            if m.mem[self.uarts[i].rcsta] & 0x90 != 0x90 { continue; }
            for t in self.uart_scan.saturating_add(1)..=now {
                let high = self.pps_pin(m, self.uart_pins[i].0, t);
                let len = self.uarts[i].fifo.len();
                self.uarts[i].rx_pin(&mut m.mem, t, high);
                if self.uarts[i].fifo.len() > len { ev.push(t); }
            }
            self.uarts[i].bits(&mut m.mem);
        }
        for q in &mut self.quarts {
            q.sync(m, now, ev, &mut self.tx, &mut self.notes);
        }
        for i in 0..self.quarts.len() {
            if !self.quarts[i].rx_enabled(m) { continue; }
            for t in self.uart_scan.saturating_add(1)..=now {
                let high = self.pps_pin(m, self.quart_pins[i], t);
                self.quarts[i].rx_pin(m, t, high, ev);
            }
        }
        self.serial_edges(m, self.uart_scan, now);
        self.uart_scan = now;
        for q in &mut self.spis {
            q.sync(m, now, ev, &mut self.spi_tx, &mut self.notes);
        }
        for i in 0..self.mssps.len() {
            let samples = self.mssps[i].take_samples(now);
            if !self.mssps[i].byte_client {
                let reg = self.i2c_pps.iter().find(|p| p.num == self.mssps[i].num).map(|p| p.sda_in);
                for t in samples {
                    let reg = reg.or_else(|| self.i2c_fixed.iter().find(|f| f.0 == self.mssps[i].num).map(|f| FIXED_PIN + usize::from((f.2.0 - b'A') * 8 + f.2.1)));
                    let high = reg.is_some_and(|reg| self.pps_pin_tick(m, reg, 4 * t - 1));
                    self.mssps[i].pin_received = self.mssps[i].pin_received << 1 | u8::from(high);
                }
            }
        }
        for q in &mut self.mssps {
            q.sync(&mut m.mem, now, ev, &mut self.spi_tx, &mut self.i2c_log, &mut self.notes);
        }
        for q in &mut self.i2cqs {
            q.sync(m, now, ev, &mut self.i2c_log, &mut self.notes);
        }
        if let Some(n) = &mut self.nvm
            && let Some(e) = n.sync(m, now)
        {
            ev.push(e);
        }
        if let Some(n) = &mut self.nvmq
            && let Some(e) = n.sync(m, now)
        {
            ev.push(e);
        }
        for k in 0..ev.len() {
            self.raised(ev[k]);
        }
    }

    pub fn read(&mut self, k: u8, m: &mut DataMem, now: u64) -> u8 {
        match k {
            PORT0..PORT_END => {
                let v = self.port_levels(usize::from(k - PORT0), m, now);
                if !self.suppress_ioc_latch {
                    if let Some((pi, _, _, baseline)) = &mut self.ioc.k22 {
                        if *pi == usize::from(k - PORT0) { *baseline = v; }
                    }
                }
                m.mem[self.ports[usize::from(k - PORT0)].port] = v;
                v
            }
            k if wdt_dec(k).is_some() => {
                let sub = wdt_dec(k).unwrap();
                if sub == wdt::CON0 {
                    self.wdt.as_mut().unwrap().armed = true;
                    m.mem[self.wdt.as_ref().unwrap().con0]
                } else {
                    self.wdt_value(sub, m, now)
                }
            }
            k if matches!(t1_dec(k), Some((_, 4))) => {
                let i = t1_dec(k).unwrap().0;
                let g = m.mem[self.t1[i].gcon] & !4;
                g | if self.gate_value(i, m, now) { 4 } else { 0 }
            }
            T0L => {
                let t = self.t0.as_ref().unwrap();
                if t.wide(m) {
                    m.mem[t.h] = (t.cnt >> 8) as u8;
                }
                t.cnt as u8
            }
            k if matches!(t1_dec(k), Some((_, 0))) => {
                let t = &self.t1[t1_dec(k).unwrap().0];
                if m.mem[t.con] & 2 != 0 {
                    m.mem[t.h] = (t.cnt >> 8) as u8;
                }
                t.cnt as u8
            }
            k if matches!(t1_dec(k), Some((_, 1))) => {
                let t = &self.t1[t1_dec(k).unwrap().0];
                if m.mem[t.con] & 2 != 0 { m.mem[t.h] } else { (t.cnt >> 8) as u8 }
            }
            k if matches!(t2_dec(k), Some((_, 0))) => self.t2[t2_dec(k).unwrap().0].cnt,
            k if k == NVM0 + nvm::CON2 || k == NVMQ0 + nvmq::LOCK => 0,
            QUART0..QUART_END => {
                let i = usize::from(k - QUART0) / usize::from(quart::HOOKS);
                self.quarts[i].read_rxb(m)
            }
            SPI0..SPI_END => {
                let i = usize::from(k - SPI0) / usize::from(spi::HOOKS);
                self.spis[i].read_rxb(m, now)
            }
            MSSP0..MSSP_END => {
                let i = usize::from(k - MSSP0) / usize::from(mssp::HOOKS);
                self.mssps[i].read_buf(&mut m.mem)
            }
            I2CQ0..I2CQ_END => {
                let i = usize::from(k - I2CQ0) / usize::from(i2cq::HOOKS);
                self.i2cqs[i].read_rxb(m, now)
            }
            UART0..=39 => {
                let u = &mut self.uarts[usize::from(k - UART0) / usize::from(UART_HOOKS)];
                u.read_rc(&mut m.mem)
            }
            _ => unreachable!("read of hook {k} has no model"),
        }
    }

    /// Plain registers (TMR0H, T0CON0/1, T1CON, ...) read the stored byte.
    /// The value a register shows right now, for a display: the registers the CPU computes on a read (port pin levels)
    /// without any side effect. `None` for the rest (their stored byte is current, or a read would pop a FIFO).
    pub fn peek_hook(&self, k: u8, m: &DataMem, now: u64) -> Option<u8> {
        match k {
            PORT0..PORT_END => Some(self.port_levels(usize::from(k - PORT0), m, now)),
            k if wdt_dec(k).is_some_and(|s| s >= wdt::PSL) && self.wdt.is_some() => Some(self.wdt_value(wdt_dec(k).unwrap(), m, now)),
            _ => None,
        }
    }

    pub fn is_stored(k: u8) -> bool {
        !(k == NVM0 + nvm::CON2 || k == NVMQ0 + nvmq::LOCK || (matches!(k, T0L | PORT0..PORT_END) || matches!(t1_dec(k), Some((_, 0 | 1 | 4))) || matches!(t2_dec(k), Some((_, 0)))) || (UART0..40).contains(&k) && (k - UART0) % UART_HOOKS == 3
            || (QUART0..QUART_END).contains(&k) && (k - QUART0) % quart::HOOKS == quart::RXB
            || (SPI0..SPI_END).contains(&k) && (k - SPI0) % spi::HOOKS == spi::RXB
            || (I2CQ0..I2CQ_END).contains(&k) && (k - I2CQ0) % i2cq::HOOKS == i2cq::RXB
            || (MSSP0..MSSP_END).contains(&k) && (k - MSSP0) % mssp::HOOKS == mssp::BUF
            || wdt_dec(k).is_some_and(|s| s != wdt::CON1))
    }

    /// A write: `v` is already masked to the implemented bits. The caller has synced to the writing cycle.
    pub fn write(&mut self, k: u8, v: u8, now: u64, m: &mut DataMem, prog: &mut [u8]) {
        let serial = (UART0..=39).contains(&k) || (QUART0..QUART_END).contains(&k) || (MSSP0..MSSP_END).contains(&k);
        let before: Vec<_> = if serial { (0..self.ports.len()).map(|i| self.port_physical(i, m, now).1).collect() } else { Vec::new() };
        match k {
            NVMQ0..=NVMQ_LAST => {
                let f = self.fosc_hz() as f64 / 1e6;
                let n = self.nvmq.as_mut().unwrap();
                n.fosc_mhz = f;
                self.stall += n.write(k - NVMQ0, v, now, m, prog, &mut self.notes);
                self.flash_event(now);
            }
            NVM0..=NVM_LAST => {
                let f = self.fosc_hz() as f64 / 1e6;
                let n = self.nvm.as_mut().unwrap();
                n.fosc_mhz = f;
                self.stall += n.write(k - NVM0, v, now, m, prog, &mut self.notes);
                self.flash_event(now);
            }
            k if wdt_dec(k).is_some() => self.wdt_write(wdt_dec(k).unwrap(), v, now, m),
            UART0..=39 => {
                let (i, sub) = (usize::from(k - UART0) / usize::from(UART_HOOKS), (k - UART0) % UART_HOOKS);
                self.uarts[i].write(sub, v, now, &mut m.mem, &mut self.tx);
            }
            SPI0..SPI_END => {
                let i = usize::from(k - SPI0) / usize::from(spi::HOOKS);
                self.spis[i].write((k - SPI0) % spi::HOOKS, v, now, m);
            }
            MSSP0..MSSP_END => {
                let i = usize::from(k - MSSP0) / usize::from(mssp::HOOKS);
                self.mssps[i].write((k - MSSP0) % mssp::HOOKS, v, now, &mut m.mem, &mut self.notes);
            }
            I2CQ0..I2CQ_END => {
                let i = usize::from(k - I2CQ0) / usize::from(i2cq::HOOKS);
                self.i2cqs[i].write((k - I2CQ0) % i2cq::HOOKS, v, now, m, &mut self.i2c_log);
            }
            QUART0..QUART_END => {
                let i = usize::from(k - QUART0) / usize::from(quart::HOOKS);
                self.quarts[i].write((k - QUART0) % quart::HOOKS, v, now, m, &mut self.tx);
            }
            k if matches!(t1_dec(k), Some((_, 4))) => {
                let t = &mut self.t1[t1_dec(k).unwrap().0];
                let mut v = v;
                if v & 0x10 == 0 {
                    v &= !8; // GGO/DONE is cleared with GSPM (20.14.2)
                }
                if v & 0x20 == 0 {
                    t.gff = false; // the toggle flip-flop is held clear without GTM
                }
                if v & 8 == 0 || m.mem[t.gcon] & 8 == 0 || v & 0x10 == 0 {
                    t.grun = false; // a new acquisition waits for the next rising edge
                }
                t.gb = None; // a polarity change is not an edge
                m.mem[t.gcon] = v & !4 | m.mem[t.gcon] & 4; // GVAL is read only
            }
            PORT0..PORT_END => {
                let p = &self.ports[usize::from(k - PORT0)];
                m.mem[p.lat] = v;
                m.mem[p.port] = v;
                if !self.suppress_ioc_latch {
                    let pins = self.port_levels(usize::from(k - PORT0), m, now);
                    if let Some((pi, _, _, baseline)) = &mut self.ioc.k22 {
                        if *pi == usize::from(k - PORT0) { *baseline = pins; }
                    }
                }
            }
            FVR0 => {
                let a = self.fvr.unwrap();
                if v & 0x80 != 0 {
                    self.notes.insert("FVR start-up time (25 us typ.) is not modelled; FVRRDY is set at once".into());
                }
                m.mem[a] = v & !0x40 | (v >> 1) & 0x40;
                for i in 0..self.cmps.len() {
                    self.cmp_update(m, i, now);
                }
            }
            ADC0 => {
                let a = self.adc.as_mut().unwrap();
                a.write_con(&mut m.mem, v, now, &mut self.notes);
            }
            T0L => {
                let t = self.t0.as_mut().unwrap();
                t.pre = 0;
                t.post = 0;
                if t.wide(m) {
                    t.cnt = u16::from(m.mem[t.h]) << 8 | u16::from(v);
                } else {
                    t.cnt = u16::from(v);
                }
                m.mem[t.l] = v;
            }
            T0H => {
                let t = self.t0.as_mut().unwrap();
                m.mem[t.h] = v;
            }
            T0C0 => {
                let t = self.t0.as_mut().unwrap();
                if t.classic {
                    m.mem[t.c0] = v;
                } else {
                    let out = m.mem[t.c0] & 0x20; // T0OUT is read only
                    m.mem[t.c0] = v & !0x20 | out;
                }
                t.pre = 0;
                t.post = 0;
            }
            T0C1 => {
                let t = self.t0.as_mut().unwrap();
                m.mem[t.c1] = v;
                t.pre = 0;
                t.post = 0;
            }
            k if matches!(t1_dec(k), Some((_, 0))) => {
                let t = &mut self.t1[t1_dec(k).unwrap().0];
                t.pre = 0;
                if m.mem[t.con] & 2 != 0 {
                    t.cnt = u16::from(m.mem[t.h]) << 8 | u16::from(v);
                } else {
                    t.cnt = t.cnt & 0xFF00 | u16::from(v);
                }
                m.mem[t.l] = v;
            }
            k if matches!(t1_dec(k), Some((_, 1))) => {
                let t = &mut self.t1[t1_dec(k).unwrap().0];
                t.pre = 0;
                if m.mem[t.con] & 2 == 0 {
                    t.cnt = t.cnt & 0xFF | u16::from(v) << 8;
                }
                m.mem[t.h] = v;
            }
            k if matches!(t1_dec(k), Some((_, 2))) => {
                let t = &self.t1[t1_dec(k).unwrap().0];
                m.mem[t.con] = v;
            }
            k if matches!(t1_dec(k), Some((_, 3))) || matches!(t2_dec(k), Some((_, 1 | 3 | 4))) => {
                let a = self.addr_of(k);
                m.mem[a] = v;
            }
            k if matches!(t2_dec(k), Some((_, 0))) => {
                let t = &mut self.t2[t2_dec(k).unwrap().0];
                t.cnt = v;
                t.pre = 0;
                t.post = 0;
                m.mem[t.tmr] = v;
            }
            CCP0..=225 | CCPK0..=CCPK_END => {
                let i = if k >= CCP0 { usize::from(k - CCP0) / usize::from(ccp::HOOKS) } else { 2 + usize::from(k - CCPK0) };
                let (con, sig, k22) = (self.ccps[i].con, self.ccps[i].sig, self.ccps[i].k22);
                let old = m.mem[con];
                // Q10: OUT (bit 5) is read only. K22: bit 5 is DCxB1 and there is no EN, CCPxM = 0 is off (ccp.rs).
                m.mem[con] = if k22 { v } else { v & 0xDF | old & 0x20 };
                self.ccps[i].edges = 0;
                if k22 && v & 15 != 0 && v & 12 != 12 {
                    self.notes.insert(format!("CCP{} capture and compare (CCP{}M = {:04b}) are not modelled on this part; only PWM is", self.ccps[i].n, self.ccps[i].n, v & 15));
                }
                if k22 && v & 0xC0 != 0 && v & 12 == 12 && self.ccps[i].ecc {
                    self.notes.insert(format!("CCP{} half- or full-bridge PWM (P{}M = {}) is not modelled: only P{}A is driven", self.ccps[i].n, self.ccps[i].n, v >> 6, self.ccps[i].n));
                }
                if (!k22 && v & 0x80 == 0) || v & 15 == 0 {
                    // Clearing the module forces the output latch low (22.3.1).
                    self.sched.retain(|s| s.code != sig);
                    self.set_sig(m, sig, 0, 4 * now);
                } else if (v ^ old) & 15 != 0 {
                    self.sched.retain(|s| s.code != sig);
                }
            }
            CWG0..=CWG_END => self.cwg_write(k, v, now, m),
            CMOUT_HOOK => {} // read only
            CMP0..CMP_END => {
                let i = usize::from(k - CMP0) / usize::from(cmp::HOOKS);
                let c = &self.cmps[i];
                match (k - CMP0) % cmp::HOOKS {
                    cmp::CON0 => m.mem[c.con0] = v & 0x93 | m.mem[c.con0] & 0x40, // OUT is read only
                    cmp::NCH => m.mem[c.nch] = v & 7,
                    _ => m.mem[c.pch] = v & 7,
                }
                if v & 1 != 0 && (k - CMP0) % cmp::HOOKS == cmp::CON0 {
                    self.notes.insert(format!("C{} SYNC (output latched on the Timer1 clock) is not modelled; the output is asynchronous", i + 1));
                }
                self.cmp_update(m, i, now);
            }
            PWM0..=237 => {
                let i = usize::from(k - PWM0);
                let (con, sig) = (self.pwms[i].con, self.pwms[i].sig);
                let old = m.mem[con];
                m.mem[con] = v & 0x90 | old & 0x20; // only EN and POL are writable; OUT is read only
                self.sched.retain(|s| s.code != sig);
                // Idle level: inactive (POL) while enabled, low while disabled [A64][A65].
                let idle = u8::from(v & 0x80 != 0 && v & 0x10 != 0);
                self.set_sig(m, sig, idle, 4 * now);
            }
            k if matches!(t2_dec(k), Some((_, 2))) => {
                let ti = t2_dec(k).unwrap().0;
                if v & if self.t2[ti].classic { 0x04 } else { 0x80 } == 0 {
                    // A stopped Timer2/4/6 freezes the PWM pins of the CCPs that use it.
                    let n = self.t2[ti].n;
                    let mut off: Vec<u8> = self.ccps.iter().filter(|c| ccp::timer_of(self.ccptmrs, m, c.n, true) == n).map(|c| c.sig).collect();
                    off.extend(self.pwms.iter().filter(|w| ccp::timer_of(self.ccptmrs, m, w.n, true) == n).map(|w| w.sig));
                    self.sched.retain(|s| !(s.pwm && off.contains(&s.code)));
                }
                let t = &mut self.t2[ti];
                m.mem[t.con] = v;
                t.pre = 0;
                t.post = 0;
            }
            _ => unreachable!("write of hook {k} has no model"),
        }
        for (i, old) in before.into_iter().enumerate() {
            let (driven, new) = self.port_physical(i, m, now);
            for bit in 0..8 {
                if (old ^ new) & (1 << bit) != 0 && (driven & (1 << bit) != 0 || self.pin_state(i, m, now).output & (1 << bit) != 0) {
                    self.pin_log.push((4 * now, self.ports[i].letter, bit, new & (1 << bit) != 0));
                }
            }
        }
    }

    fn pin_state(&self, i: usize, m: &DataMem, now: u64) -> PinState {
        let p = &self.ports[i];
        let mut state = PinState { output: !m.mem[p.tris] & m.implemented[p.tris] & m.implemented[p.port],
            high: m.mem[p.lat], od: p.odcon.map_or(0, |a| m.mem[a]), weak: 0,
            analog: p.ansel.map_or(0, |a| m.mem[a]), implemented: m.implemented[p.port], codes: [0; 8], uart: [0; 2], quart: [0; 5], inverted: 0 };
        for &(reg, bit) in &self.pps_by_port[i] {
            let code = self.output_code(m, reg);
            if self.sig_src >> code & 1 != 0 || ((1..=4).contains(&code) && self.cwg.as_ref().is_some_and(|c| c.hiz >> (code - 1) & 1 == 0)) { state.codes[usize::from(bit)] = code; }
            for q in &self.mssps {
                if q.enabled(&m.mem) && (code == 0x0F + 2 * (q.num - 1) || code == 0x10 + 2 * (q.num - 1)) {
                    if q.i2c_mode(&m.mem) {
                        let selected = self.i2c_pps.iter().find(|pp| pp.num == q.num).map(|pp| m.mem[if code == pp.scl_code { pp.scl_in } else { pp.sda_in }]);
                        let pin = ((p.letter - b'A') << 3) | bit;
                        if selected != Some(pin) { continue; }
                    }
                    state.codes[usize::from(bit)] = code;
                    if q.i2c_mode(&m.mem) {
                        // MSSP owns the output enable in I2C mode; both TRIS bits must be inputs.
                        if m.mem[p.tris] & (1 << bit) != 0 { state.output |= 1 << bit; state.od |= 1 << bit; }
                    }
                }
            }
            for (k, u) in self.uarts.iter().enumerate() {
                if self.uart_pins[k].1 == code && u.tx_level(&m.mem, now).is_some() {
                    state.uart[k] |= 1 << bit;
                    if m.mem[u.baud] & 0x10 != 0 { state.inverted |= 1 << bit; }
                }
            }
        }
        for &(reg, bit) in &self.pps_by_port[i] {
            let code = out_code(m, reg);
            for (k, q) in self.quarts.iter().enumerate() {
                if code == 0x10 + 3 * (q.num - 1) && q.tx_enabled(m) {
                    state.quart[k] |= 1 << bit;
                    if q.tx_inverted(m) { state.inverted |= 1 << bit; }
                }
            }
        }
        for &(num, scl, sda) in &self.i2c_fixed {
            let Some(q) = self.mssps.iter().find(|q| q.num == num && q.enabled(&m.mem)) else { continue; };
            let i2c = q.i2c_mode(&m.mem);
            let data = if i2c { sda } else if num == 1 { (b'C', 5) } else if scl.0 == b'D' { (b'D', 4) } else { (b'B', 3) };
            for (pin, code) in [(scl, 0x0F + 2 * (num - 1)), (data, 0x10 + 2 * (num - 1))] {
                if pin.0 != p.letter { continue; }
                state.codes[usize::from(pin.1)] = code;
                if i2c && m.mem[p.tris] & (1 << pin.1) != 0 { state.output |= 1 << pin.1; state.od |= 1 << pin.1; }
            }
        }
        state.output |= state.uart.iter().fold(0, |a, b| a | b);
        let weak = if p.wpu_disable.is_some_and(|(a, mask)| m.mem[a] & mask != 0) { 0 } else { p.wpu.map_or(0, |a| m.mem[a]) & !state.output };
        state.weak = weak | self.pullup.get(i).copied().unwrap_or(0);
        state
    }

    pub fn publish_pins(&mut self, m: &DataMem, now: u64) {
        for i in 0..self.ports.len() {
            let state = self.pin_state(i, m, now);
            let h = &mut self.pin_history[i];
            let at = now + 1;
            if h.back().is_none_or(|e| e.1 != state) {
                if h.back().is_some_and(|e| e.0 == at) { h.pop_back(); }
                h.push_back((at, state));
            }
            while h.len() > 2 && h[1].0 < now.saturating_sub(4) { h.pop_front(); }
        }
    }

    fn pin_times(&self, from: u64, to: u64) -> Vec<u64> {
        let mut times: Vec<_> = self.drive.times(from, to).collect();
        times.extend(self.pin_history.iter().flat_map(|h| h.iter().map(|e| e.0)).filter(|&t| from < t && t <= to));
        times.extend(self.mssps.iter().flat_map(|q| q.pin_times(from, to)));
        times.extend(self.sig_log.iter().map(|e| (e.0 + 3) / 4 + 1).filter(|&t| from < t && t <= to));
        times.extend(self.quarts.iter().flat_map(|u| u.tx_times(from.saturating_sub(1), to.saturating_sub(1))).map(|t| t + 1));
        times.extend(self.uarts.iter().flat_map(|u| u.tx_times(from.saturating_sub(1), to.saturating_sub(1))).map(|t| t + 1));
        times.sort_unstable(); times.dedup(); times
    }

    fn serial_edges(&mut self, m: &DataMem, from: u64, now: u64) {
        let mut times: Vec<_> = self.uarts.iter().flat_map(|u| u.tx_times(from, now)).map(|t| 4 * t).collect();
        times.extend(self.quarts.iter().flat_map(|q| q.tx_times(from, now)).map(|t| 4 * t));
        times.extend(self.mssps.iter().flat_map(|q| q.wire_times(4 * from, 4 * now)));
        times.sort_unstable(); times.dedup();
        for tick in times {
            for i in 0..self.ports.len() {
                let state = self.pin_state(i, m, tick / 4);
                let serial = state.uart.iter().chain(state.quart.iter()).fold(0, |a, b| a | b)
                    | state.codes.iter().enumerate().fold(0, |a, (bit, &code)| a | if (0x0F..=0x12).contains(&code) { 1 << bit } else { 0 });
                let old = self.resolve_pin_state(i, tick / 4, &state, tick.saturating_sub(1)).1;
                let new = self.resolve_pin_state(i, tick / 4, &state, tick).1;
                let changed = (old ^ new) & serial & state.output;
                for bit in 0..8 { if changed & (1 << bit) != 0 { self.pin_log.push((tick, self.ports[i].letter, bit, new & (1 << bit) != 0)); } }
            }
        }
    }

    // `now` selects external stimulus in instruction cycles; `source` selects internal Fosc ticks.
    fn resolve_pin_state(&self, i: usize, now: u64, state: &PinState, source: u64) -> (u8, u8) {
        let mut high = state.high;
        for (bit, &code) in state.codes.iter().enumerate() {
            if code != 0 {
                let level = self.mssps.iter().find_map(|q| {
                    let base = 0x0F + 2 * (q.num - 1);
                    if code == base { Some(q.pin_level(source).0) }
                    else if code == base + 1 { Some(q.pin_level(source).1) } else { None }
                }).unwrap_or_else(|| self.sig_level(code, source));
                high = high & !(1 << bit) | u8::from(level) << bit;
            }
        }
        for (k, &mask) in state.uart.iter().enumerate() {
            if mask != 0 { high = high & !mask | if self.uarts[k].tx_signal(source / 4) { mask } else { 0 }; }
        }
        for (k, &mask) in state.quart.iter().enumerate() {
            if mask != 0 { high = high & !mask | if self.quarts[k].tx_signal(source / 4) { mask } else { 0 }; }
        }
        high ^= state.inverted;
        let driven = state.output & !(state.od & high);
        let input = self.drive.level(i, now) | state.weak & !self.drive.mask(i, now);
        (driven, (high & driven | input & !driven) & !self.forced.get(i).copied().unwrap_or(0) & state.implemented)
    }

    fn port_physical(&self, i: usize, m: &DataMem, now: u64) -> (u8, u8) {
        self.resolve_pin_state(i, now, &self.pin_state(i, m, now), 4 * now)
    }

    fn port_levels(&self, i: usize, m: &DataMem, now: u64) -> u8 {
        self.port_physical(i, m, now).1 & !self.ports[i].ansel.map_or(0, |a| m.mem[a])
    }

    fn port_levels_at(&self, i: usize, m: &DataMem, now: u64) -> u8 {
        let state = self.pin_history.get(i).and_then(|h| h.iter().rev().find(|e| e.0 <= now)).map(|e| e.1.clone()).unwrap_or_else(|| self.pin_state(i, m, now));
        self.resolve_pin_state(i, now, &state, 4 * now.saturating_sub(1)).1 & !state.analog
    }

    fn pps_pin_tick(&self, m: &DataMem, reg: usize, tick: u64) -> bool {
        let pin = if reg >= FIXED_PIN { (reg - FIXED_PIN) as u8 } else { m.mem[reg] };
        self.ports.iter().position(|p| p.letter == b'A' + (pin >> 3 & 7)).is_some_and(|i| {
            let state = self.pin_state(i, m, tick / 4);
            self.resolve_pin_state(i, tick / 4, &state, tick).1 & !state.analog & (1 << (pin & 7)) != 0
        })
    }

    /// Level of the pin a PPS input register selects (PORT field at bits 5:3, PIN at 2:0).
    fn pps_pin(&self, m: &DataMem, reg: usize, now: u64) -> bool {
        let v = if reg >= FIXED_PIN { (reg - FIXED_PIN) as u8 } else { m.mem[reg] };
        let letter = b'A' + (v >> 3 & 7);
        self.ports.iter().position(|p| p.letter == letter).is_some_and(|i| self.port_levels_at(i, m, now) >> (v & 7) & 1 != 0)
    }

    /// Timer1 gate value (GVAL): the gate source after the polarity bit. `[A17]` MDB (probed with every GE/GPOL/GTM/GSPM
    /// combination on a low pin) reads GVAL = 0 while GE = 0, although the data sheet says GVAL is unaffected by GE
    /// (DS40002043F 20.14.2 p.318); the model follows MDB. Single-pulse and toggle mode do not change it either.
    /// The sources are the T1GPPS pin and the CCP, PWM, comparator and CLC outputs (`gate_src`); others read as a low signal.
    fn gate_value(&mut self, i: usize, m: &DataMem, now: u64) -> bool {
        let tn = self.t1[i].n;
        let gcon = self.t1[i].gcon;
        if m.mem[gcon] & 0x80 == 0 {
            return false;
        }
        let src = self.gate_src(m, i);
        if matches!(src, GSrc::None) {
            self.notes.insert(format!("Timer{tn} gate source is not modelled (the T{tn}GPPS pin and the CCP, PWM, comparator and CLC outputs are); GVAL sees it low"));
        }
        self.gate_raw(m, src, now) == (m.mem[gcon] & 0x40 != 0)
    }

    /// Level of the PPS output source `code` at Fosc tick `tick`: the last change at or before it.
    fn sig_level(&self, code: u8, tick: u64) -> bool {
        self.sig_log.iter().filter(|e| e.1 == code && e.0 <= tick).max_by_key(|e| e.0).is_some_and(|e| e.2)
    }

    /// Drop the source changes older than `cutoff` ticks, keeping the last one of each source as the starting level.
    fn sig_prune_to(&mut self, cutoff: u64) {
        if self.sig_log.len() > 64 {
            self.sig_prune(cutoff);
        }
    }

    fn sig_prune(&mut self, cutoff: u64) {
        let mut keep = [usize::MAX; 32];
        for (k, e) in self.sig_log.iter().enumerate() {
            let c = usize::from(e.1) & 31;
            if e.0 < cutoff && (keep[c] == usize::MAX || self.sig_log[keep[c]].0 <= e.0) {
                keep[c] = k;
            }
        }
        let mut k = 0;
        self.sig_log.retain(|e| {
            k += 1;
            e.0 >= cutoff || keep[usize::from(e.1) & 31] == k - 1
        });
    }

    /// The gate source of Timer1-type timer `i` by GSS (Q10: DS40002043F Table 20-4 p.310; Q41: DS40002214G Table 25-5 p.378).
    /// Timer overflows and post-scaled outputs, ZCD, NCO, SMT and the Q41 PWM are not modelled.
    fn gate_src(&self, m: &DataMem, i: usize) -> GSrc {
        let t = &self.t1[i];
        let Some(g) = t.gate else { return GSrc::None };
        if t.classic {
            // TxGSS: 00 the TxG pin; the Timer2 match and the comparators are not modelled on this part.
            return if m.mem[g] & 3 == 0 { t.gpps.map_or(GSrc::None, GSrc::Pin) } else { GSrc::None };
        }
        let gss = m.mem[g] & 31;
        if gss == 0 {
            return t.gpps.map_or(GSrc::None, GSrc::Pin);
        }
        let ccp = |n: u8| self.ccps.iter().find(|c| c.n == n).map(|c| c.sig);
        let pwm = |n: u8| self.pwms.iter().find(|c| c.n == n).map(|c| c.sig);
        let cmp = |n: u8| self.cmps.iter().find(|c| c.n == n).map(|c| c.sig);
        let clc = |n: u8| self.clcs.iter().find(|c| c.n == n).map(|c| c.sig);
        let code = if self.q41() {
            match gss {
                7 => ccp(1),
                15 => cmp(1),
                16 => cmp(2),
                18..=21 => clc(gss - 17),
                _ => None,
            }
        } else {
            match gss {
                8 => ccp(1),
                9 => ccp(2),
                10 => pwm(3),
                11 => pwm(4),
                12 => cmp(1),
                13 => cmp(2),
                16..=23 => clc(gss - 15),
                _ => None,
            }
        };
        code.map_or(GSrc::None, GSrc::Sig)
    }

    /// Level of the gate source before GPOL at the start of cycle `c`.
    fn gate_raw(&self, m: &DataMem, src: GSrc, c: u64) -> bool {
        match src {
            GSrc::Pin(r) => self.pps_pin(m, r, c),
            GSrc::Sig(code) => self.sig_level(code, 4 * c),
            GSrc::None => false,
        }
    }

    /// The Timer1-type timer `i`'s gate over cycles `from+1 ..= to`, `None` for a source that is not modelled (the timer
    /// then counts regardless, with a note when GE is set). `[A103]` A change of the source on cycle t (a stimulus step,
    /// or a source edge in the tick of that cycle) is seen by the clock ticks on cycle t and later; a change that software
    /// made to the pin since the last look (LATx, TRISx, ANSELx, RxyPPS are synced before they are written) is seen on
    /// the first cycle of the window. The gate signal is the source after GPOL; the toggle flip-flop changes on its
    /// rising edge (GTM); with GSPM the timer counts from the first rising edge after GGO is set to the next falling edge,
    /// which clears GGO. TMRxGIF is set on a falling edge of the gate after the toggle flip-flop (in single-pulse mode
    /// only for the pulse that was counted), also while GE = 0 (20.7.1-20.7.6, Figures 20-4 to 20-7). The prescaler holds
    /// while the gate is closed (not stated in the data sheet).
    fn gate_plan(&mut self, m: &DataMem, i: usize, from: u64, to: u64, p: &mut GatePlan, times: &mut Vec<u64>) -> bool {
        let src = self.gate_src(m, i);
        let t = &self.t1[i];
        let (tn, gcon, gff, grun, gb, gss) = (t.n, t.gcon, t.gff, t.grun, t.gb, t.gate.map_or(0, |g| m.mem[g] & 31));
        let con = m.mem[gcon];
        let (ge, gpol, gtm, gspm) = (con & 0x80 != 0, con & 0x40 != 0, con & 0x20 != 0, con & 0x10 != 0);
        if matches!(src, GSrc::None) {
            self.t1[i].gb = None;
            if ge {
                self.notes.insert(format!("Timer{tn} gate source GSS={gss} is not modelled (the T{tn}GPPS pin and the CCP, PWM, comparator and CLC outputs are); the timer counts regardless of the gate"));
            }
            return false;
        }
        let active = |c: u64| self.gate_raw(m, src, c) == gpol;
        let (mut ff, mut run, mut ggo) = (gff && gtm, grun && gspm, con & 8 != 0 && gspm);
        let counts = |ff: bool, run: bool, b: bool| !ge || if gspm { run && if gtm { ff } else { b } } else if gtm { ff } else { b };
        times.clear();
        match src {
            GSrc::Pin(_) => times.extend(self.pin_times(from, to)),
            GSrc::Sig(code) => times.extend(self.sig_log.iter().filter(|e| e.1 == code).map(|e| e.0.div_ceil(4)).filter(|&c| c > from && c <= to)),
            GSrc::None => {}
        }
        let a0 = active(from);
        let mut b = gb.unwrap_or(a0);
        if a0 != b {
            times.push(from + 1);
        }
        times.sort_unstable();
        times.dedup();
        let (segs, gif) = (&mut p.segs, &mut p.gif);
        segs.clear();
        gif.clear();
        let mut start = from;
        let mut cur = counts(ff, run, b);
        for &c in times.iter() {
            let nb = active(c);
            if nb == b {
                continue;
            }
            if c - 1 > start {
                segs.push((start, c - 1, cur));
                start = c - 1;
            }
            let s_old = if gtm { ff } else { b };
            if gtm && nb {
                ff = !ff;
            }
            let s_new = if gtm { ff } else { nb };
            if s_new && !s_old && gspm && ggo {
                run = true;
            }
            if !s_new && s_old {
                if !gspm || run {
                    gif.push(c);
                }
                if gspm && run {
                    (run, ggo) = (false, false);
                }
            }
            b = nb;
            cur = counts(ff, run, b);
        }
        segs.push((start, to, cur));
        (p.ff, p.run, p.ggo, p.b) = (ff, run, ggo, Some(b));
        true
    }

    /// Attach a periodic input clock to a port.
    pub fn clock_feed(&mut self, clock: crate::drive::Clock) -> bool {
        let Some(i) = self.ports.iter().position(|p| p.letter == clock.pin.0) else { return false; };
        self.drive.clock(i, clock);
        self.due = 0;
        true
    }

    /// Stimulus: the pins `mask` of port `letter` are driven to `val` from cycle `at` on.
    pub fn pin_feed(&mut self, letter: u8, at: u64, mask: u8, val: u8) -> bool {
        match self.ports.iter().position(|p| p.letter == letter) {
            Some(i) => {
                self.drive.push(at, i, mask, val);
                true
            }
            None => false,
        }
    }

    /// Stimulus: the input of ADC channel `ch` is code `v` (full scale = the part's resolution) from cycle `at` on.
    pub fn adc_feed(&mut self, ch: u8, at: u64, v: u16) -> bool {
        match &mut self.adc {
            Some(a) => {
                a.push_feed(ch, at, v);
                true
            }
            None => false,
        }
    }

    fn addr_of(&self, k: u8) -> usize {
        if let Some((i, _)) = t1_dec(k) {
            return self.t1[i].clk;
        }
        let (i, sub) = t2_dec(k).unwrap();
        let t = &self.t2[i];
        match sub {
            1 => t.pr,
            3 => t.hlt,
            _ => t.clk,
        }
    }
}
