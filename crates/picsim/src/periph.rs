//! Peripheral models that live on the instruction clock: Timer0, Timers 1-6 and the interrupt flag
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

mod board;
mod ccp;
mod clc;
mod cmp;
mod cwg;
mod flash;
mod nvm;
mod nvmq;
mod i2cq;
mod mssp;
mod quart;
mod spi;
pub use board::{parse_pin, pin_name, Board, Bus, Led, Oled, Pin};
use ccp::{Ccp, Pwm};
use clc::Clc;
use cmp::Cmp;
use cwg::Cwg;
use i2cq::I2cQ;
use mssp::Mssp;
use nvm::Nvm;
use nvmq::NvmQ;
use quart::QUart;
use spi::Spi;
pub use mssp::{I2cClient, I2cEv};
pub use spi::SpiClient;

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
/// Index 0 keeps the first hook codes, the others use 244..=253.
fn t2_hook(i: usize, sub: u8) -> u8 {
    if i == 0 { T2T + sub } else { 244 + 5 * (i as u8 - 1) + sub }
}

fn t2_dec(k: u8) -> Option<(usize, u8)> {
    match k {
        T2T..=T2CLK => Some((0, k - T2T)),
        244..=253 => Some((1 + usize::from(k - 244) / 5, (k - 244) % 5)),
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
const UART_HOOKS: u8 = 5;
/// Q41 UART n (0-based) uses hook codes QUART0 + quart::HOOKS * n + sub-code (see `quart.rs`); at most 5 UARTs.
const QUART0: u8 = 112;
const QUART_END: u8 = QUART0 + 5 * quart::HOOKS;
/// T1GCON: GVAL (bit 2) is read only and follows the gate source.
const T1G: u8 = 40;
/// PORTx (index into `ports`) uses hook code PORT0 + index: a read gives the pin levels, a write goes to LATx.
const PORT0: u8 = 48;
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
/// Q10 PWM3/PWM4 (PWMxCON only) use hook codes PWM0 + n.
const PWM0: u8 = 236;
/// Q10 CWG1 registers use hook codes CWG0 + cwg sub-code (see `cwg.rs`).
const CWG0: u8 = 41; // 41..=47 (T1G is 40, the ports start at 48)
const CWG_END: u8 = CWG0 + cwg::HOOKS - 1;
/// Q10 comparator n (0-based) uses hook codes CMP0 + cmp::HOOKS * n + sub-code (CMxCON0, CMxNCH, CMxPCH); CMOUT is CMOUT_HOOK.
const CMP0: u8 = 184;
const CMP_END: u8 = CMP0 + 2 * cmp::HOOKS;
const CMOUT_HOOK: u8 = 222;
const NVM0: u8 = 240;
const NVMQ0: u8 = 242;

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
    /// High bytes of ADPRE and ADACQ (Q41: 13-bit values counted in Fosc clocks).
    q41: Option<(usize, usize)>,
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
    /// Data sheet number: 1, 3 or 5.
    n: u8,
    /// A compare with clearing makes the timer go to 0 on its next clock.
    clr: bool,
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

struct I2cPps {
    num: u8,
    scl_in: usize,
    sda_in: usize,
    scl_name: String,
    sda_name: String,
    scl_code: u8,
    sda_code: u8,
}

pub struct Periph {
    ports: Vec<Port>,
    /// What the outside world drives on the input pins: (from cycle, port index, pin mask, levels), stable-sorted by cycle.
    drive: Vec<(u64, usize, u8, u8)>,
    t0: Option<Timer0>,
    t1: Vec<Timer1>,
    t2: Vec<Timer2>,
    uarts: Vec<Uart>,
    quarts: Vec<QUart>,
    spis: Vec<Spi>,
    mssps: Vec<Mssp>,
    i2cqs: Vec<I2cQ>,
    adc: Option<Adc>,
    nvm: Option<Nvm>,
    nvmq: Option<NvmQ>,
    /// Cycles the CPU is stalled by a program flash operation; the core takes them after the write (`take_stall`).
    stall: u64,
    fvr: Option<usize>,
    /// Bytes the transmitters started to send: (EUSART number 1-based, cycle it started, byte).
    pub tx: Vec<(u8, u64, u8)>,
    /// Bytes the SPI hosts sent: (SPI number, cycle the byte started, byte).
    pub spi_tx: Vec<(u8, u64, u8)>,
    /// What the I2C hosts put on the bus: (MSSP number, cycle, event).
    pub i2c_log: Vec<(u8, u64, I2cEv)>,
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
    /// Stimulus: (from cycle, port letter, pin, volts), stable-sorted by cycle.
    analog: Vec<(u64, u8, u8, f64)>,
    /// A compare match with TMR1 clearing happened on the last clock: TMR1 goes to 0 on the next one `[A61]`.
    ccptmrs: Option<usize>,
    /// RxyPPS registers: (register, index into `ports`, pin).
    ppsout: Vec<(usize, usize, u8)>,
    /// Hardware I2C hosts: the PPS input registers and the output codes of SCL and SDA.
    i2c_pps: Vec<I2cPps>,
    /// The parts on the board (LEDs, OLED).
    pub board: Board,
    /// Pins with an external pull-up (bit-banged I2C lines) and pins the OLED holds low, per port.
    pullup: Vec<u8>,
    forced: Vec<u8>,
    /// Levels of the PPS output sources (bit = source code of Table 18-2).
    sig: u32,
    /// Output source changes scheduled for a later time (CCP compare and PWM edges).
    sched: Vec<Sched>,
    /// Changes of the pins driven by a peripheral through RxyPPS: (time in Fosc ticks = 4 * cycle + quarter,
    /// port letter, pin, level). Only pins whose TRIS bit is clear at the time of the change are logged.
    pub pin_log: Vec<(u64, u8, u8, bool)>,
    /// Cycle of the latest flag set by `sync` since the core last looked (`take_event`).
    event: Option<u64>,
}

/// A change of a PPS output source at `tick` (Fosc ticks): `act` 0 clears, 1 sets, 2 toggles; `flag` is set with it.
/// `pwm` marks edges that stop when Timer2 is switched off.
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
        let mut p = Periph { ports: Vec::new(), drive: Vec::new(), t0: None, t1: Vec::new(), t2: Vec::new(), uarts: Vec::new(), quarts: Vec::new(), spis: Vec::new(), mssps: Vec::new(), i2cqs: Vec::new(), adc: None, nvm: None, nvmq: None, stall: 0, fvr: None, tx: Vec::new(), spi_tx: Vec::new(), i2c_log: Vec::new(), notes: BTreeSet::new(), ccps: Vec::new(), pwms: Vec::new(), cwg: None, cmps: Vec::new(), cmout: None, clcs: Vec::new(), clcin: Vec::new(), clcdata: None, in_clc: false, clc_scan: 0, analog: Vec::new(), ccptmrs: None, ppsout: Vec::new(), i2c_pps: Vec::new(), board: Board::default(), pullup: Vec::new(), forced: Vec::new(), sig: 0, sched: Vec::new(), pin_log: Vec::new(), event: None };
        let mut hook = |a: usize, k: u8| m.special[a] = Special::Hook(k);
        if let (Some(l), Some(h), Some(c0), Some(c1), Some(flag)) =
            (addr(dev, "TMR0L"), addr(dev, "TMR0H"), addr(dev, "T0CON0"), addr(dev, "T0CON1"), Flag::find(dev, "TMR0IF"))
        {
            for (a, k) in [(l, T0L), (h, T0H), (c0, T0C0), (c1, T0C1)] {
                hook(a, k);
            }
            p.t0 = Some(Timer0 { l, h, c0, c1, flag, cnt: 0, cmp: 0xFF, pre: 0, post: 0, last: 0 });
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
                p.t1.push(Timer1 { n, clr: false, gate: addr(dev, &format!("T{n}GATE")), gpps: addr(dev, &format!("T{n}GPPS")), l, h, con, clk, gcon, flag, cnt: 0, pre: 0, last: 0 });
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
                p.t2.push(Timer2 { n, tmr, pr, con, hlt, clk, flag, cnt: 0, pre: 0, post: 0, last: 0 });
            }
        }
        let q41 = addr(dev, "ADCPCON").is_some();
        if let (Some(con), Some(clk), Some(pre), Some(acq), Some(pch), Some(resl), Some(resh), Some(flag)) = (
            addr(dev, "ADCON0"),
            addr(dev, "ADCLK"),
            addr(dev, if q41 { "ADPREL" } else { "ADPRE" }),
            addr(dev, if q41 { "ADACQL" } else { "ADACQ" }),
            addr(dev, "ADPCH"),
            addr(dev, "ADRESL"),
            addr(dev, "ADRESH"),
            Flag::find(dev, "ADIF"),
        ) {
            // The Q10 (DS40002043F ch.33) and the Q41 (DS40002214G ch.40) are sourced; the PIC16 converters differ.
            if q41 {
                let hi = (addr(dev, "ADPREH"), addr(dev, "ADACQH"));
                if let (Some(ph), Some(ah)) = hi {
                    hook(con, ADC0);
                    p.adc = Some(Adc { con, clk, pre, acq, q41: Some((ph, ah)), pch, resl, resh, flag, bits: 12, end: None, feed: Vec::new() });
                } else {
                    p.notes.insert("the ADC of this part is not modelled; ADGO never clears".into());
                }
            } else if addr(dev, "PRODL").is_some() {
                hook(con, ADC0);
                p.adc = Some(Adc { con, clk, pre, acq, q41: None, pch, resl, resh, flag, bits: 10, end: None, feed: Vec::new() });
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
        if addr(dev, "NVMCON2").is_some() && addr(dev, "NVMLOCK").is_none()
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
        if addr(dev, "ADCPCON").is_none() && addr(dev, "PRODL").is_some() {
            p.ccptmrs = addr(dev, "CCPTMRS");
            for n in 1..=2u8 {
                if let Some(c) = Ccp::find(dev, n) {
                    hook(c.con, CCP0 + p.ccps.len() as u8 * ccp::HOOKS);
                    p.ccps.push(c);
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
            for n in 3..=4u8 {
                if let Some(w) = Pwm::find(dev, n) {
                    hook(w.con, PWM0 + p.pwms.len() as u8);
                    p.pwms.push(w);
                }
            }
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
        p
    }

    /// Set the byte-level client that answers SPI `num` (1-based). False when the part has no such SPI.
    pub fn spi_client(&mut self, num: u8, c: SpiClient) -> bool {
        if let Some(s) = self.spis.iter_mut().find(|s| s.num == num) {
            s.client = c;
            true
        } else if let Some(s) = self.mssps.iter_mut().find(|s| s.num == num) {
            s.spi = c;
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
    /// Bring every model to cycle `now`. Capture edges on the CCPx pins inside the span are handled at their own cycle
    /// so that TMR1 is read at that cycle.
    pub fn sync(&mut self, m: &mut DataMem, now: u64) {
        loop {
            let mut best: Option<(u64, usize)> = None;
            for (i, c) in self.ccps.iter().enumerate() {
                let Some(reg) = c.pps else { continue };
                if !c.en(m) || !(3..=7).contains(&c.mode(m)) || !self.t1.iter().any(|t| t.n == ccp::timer_of(self.ccptmrs, m, c.n, false)) || m.mem[c.cap] & 15 != 0 {
                    continue;
                }
                for &(at, ..) in &self.drive {
                    if at > c.scan && at <= now && best.is_none_or(|b| (at, i) < b) && self.pps_pin(m, reg, at) != self.pps_pin(m, reg, at - 1) {
                        best = Some((at, i));
                        break;
                    }
                }
            }
            let Some((e, i)) = best else { break };
            self.sync_core(m, e);
            self.capture(m, i, e);
        }
        self.clc_scan(m, now);
        self.sync_core(m, now);
        self.clc_eval(m, 4 * now);
        for c in &mut self.ccps {
            c.scan = now;
        }
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
        for c in self.ccps.iter().filter(|c| c.sig == code) {
            if new { m.mem[c.con] |= 0x20 } else { m.mem[c.con] &= !0x20 }
        }
        for w in self.pwms.iter().filter(|w| w.sig == code) {
            if new { m.mem[w.con] |= 0x20 } else { m.mem[w.con] &= !0x20 }
        }
        for &(reg, pi, bit) in &self.ppsout {
            if m.mem[reg] & 31 == code && m.mem[self.ports[pi].tris] >> bit & 1 == 0 {
                self.pin_log.push((tick, self.ports[pi].letter, bit, new));
            }
        }
        if self.cwg.as_ref().is_some_and(|c| c.source(m) == Some(code)) {
            self.cwg_edge(m, new, tick);
        }
        self.clc_eval(m, tick);
    }

    /// Level of CLC data input `src` (DyS code, DS40002043F p.409) of cell `n` at cycle `now`.
    fn clc_input(&mut self, m: &DataMem, src: u8, now: u64, n: u8) -> bool {
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
        let (con, polr, sel, gls, flag, sig, n) = (c.con, c.pol, c.sel, c.gls, c.flag, c.sig, c.n);
        let (was_on, q0, clk0, out0) = (c.on, c.q, c.clk, c.out);
        let on = m.mem[con] & 0x80 != 0;
        let (mut q, mut clk, mut out) = (false, false, false);
        if on {
            let pol = m.mem[polr];
            let used = |k: usize| gls.iter().any(|&g| m.mem[g] >> (2 * k) & 3 != 0);
            let mut d = [false; 4];
            for (k, dk) in d.iter_mut().enumerate() {
                if used(k) {
                    *dk = self.clc_input(m, m.mem[sel[k]] & 63, now, n);
                }
            }
            let mut g = [false; 4];
            for (y, gy) in g.iter_mut().enumerate() {
                *gy = clc::gate(m.mem[gls[y]], pol >> y & 1 != 0, d);
            }
            clk = g[0];
            q = clc::logic(m.mem[con] & 7, g, was_on && q0, if was_on { clk0 } else { g[0] });
            out = q != (pol & 0x80 != 0);
        }
        let c = &mut self.clcs[i];
        (c.q, c.clk, c.on) = (q, clk, on);
        if out != out0 {
            c.out = out;
        }
        m.mem[con] = m.mem[con] & !0x20 | u8::from(out) << 5;
        if let Some(a) = self.clcdata {
            let bit = 1u8 << (n - 1);
            if out { m.mem[a] |= bit } else { m.mem[a] &= !bit }
        }
        if out == out0 {
            return false;
        }
        self.set_sig(m, sig, u8::from(out), tick);
        if m.mem[con] & (if out { 0x10 } else { 0x08 }) != 0 {
            flag.set(m);
            self.raised(now);
        }
        true
    }

    /// Some cell is enabled or still shows a high output (so that disabling it is seen).
    fn clc_active(&self, m: &DataMem) -> bool {
        self.clcs.iter().any(|c| c.on || c.out || m.mem[c.con] & 0x80 != 0)
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
        let mut at: Vec<u64> = self.drive.iter().map(|e| e.0).filter(|&t| t > from && t < now).collect();
        at.dedup();
        for t in at {
            self.clc_eval(m, 4 * t);
        }
        self.clc_scan = now;
    }

    /// Voltage of a comparator input source at cycle `now`.
    fn cmp_volts(&self, m: &DataMem, src: cmp::Src, now: u64) -> f64 {
        match src {
            cmp::Src::Pin(l, b) => self.analog.iter().filter(|e| e.0 <= now && e.1 == l && e.2 == b).last().map_or(0.0, |e| e.3),
            cmp::Src::Fvr => self.fvr.map_or(0.0, |a| cmp::fvr_volts(m.mem[a])),
            _ => 0.0,
        }
    }

    /// The output level of comparator `i` at cycle `now`, after POL.
    fn cmp_level(&self, m: &DataMem, i: usize, now: u64) -> bool {
        let c = &self.cmps[i];
        let con = m.mem[c.con0];
        let core = con & 0x80 != 0 && {
            let (p, n) = (cmp::pos(c.n, m.mem[c.pch]), cmp::neg(m.mem[c.nch]));
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
        let pch = cmp::pos(n, m.mem[c.pch]);
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
            let mut at: Vec<u64> = self.analog.iter().map(|e| e.0).filter(|&t| t > from && t <= now).collect();
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
        self.analog.push((at, letter, bit, volts));
        self.analog.sort_by_key(|e| e.0);
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
        if as1 & 0xCE != 0 {
            self.notes.insert("CWG1 auto-shutdown sources other than the CWG1PPS pin (AS0E) and the comparators (AS4E, AS5E) are not modelled".to_string());
        }
        let cmp_low = |n: usize| self.cmps.get(n).is_some_and(|c| !c.out);
        let active = as1 & 1 != 0 && pps.is_some_and(|r| !self.pps_pin(m, r, now)) || as1 & 0x10 != 0 && cmp_low(0) || as1 & 0x20 != 0 && cmp_low(1);
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
                if m.mem[c.ism] & 15 == 0 || m.mem[c.ism] & 15 > 6 {
                    self.notes.insert("CWG1 data input other than CCP1, CCP2, PWM3, PWM4, CMP1 and CMP2 is not modelled; the CWG sees it low".to_string());
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
            self.sched.sort_by_key(|s| s.tick);
            let due = self.sched.iter().take_while(|s| s.tick / 4 <= now).count();
            if due == 0 {
                break;
            }
            for s in self.sched.drain(..due).collect::<Vec<_>>() {
                self.set_sig(m, s.code, s.act, s.tick);
                if let Some(f) = s.flag {
                    f.set(m);
                    ev.push(s.tick / 4);
                }
            }
        }
    }

    fn sync_core(&mut self, m: &mut DataMem, now: u64) {
        let mut ev: Vec<u64> = Vec::new();
        // Compare CCPs with the timer they use: (index, CCPR, MODE, timer number).
        let comps: Vec<(usize, u16, u8, u8)> = self
            .ccps
            .iter()
            .enumerate()
            .filter(|(_, c)| c.en(m) && matches!(c.mode(m), 1 | 2 | 8 | 9 | 10 | 11))
            .filter_map(|(i, c)| {
                let n = ccp::timer_of(self.ccptmrs, m, c.n, false);
                if self.t1.iter().any(|t| t.n == n) {
                    Some((i, c.ccpr(m), c.mode(m), n))
                } else {
                    self.notes.insert(format!("CCP{} selects a capture/compare timer this part does not have (CCPTMRS); it is not modelled", c.n));
                    None
                }
            })
            .collect();
        let mut hits: Vec<Sched> = Vec::new();
        // Timer2-type period matches: (timer number, cycle, PR, prescaler shift).
        let mut periods: Vec<(u8, u64, u8, u32)> = Vec::new();
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
        for t in &mut self.t1 {
            let comps: Vec<(usize, u16, u8)> = comps.iter().filter(|c| c.3 == t.n).map(|&(i, r, mo, _)| (i, r, mo)).collect();
            let (base, pre0) = (t.last, t.pre);
            let dt = now - t.last;
            t.last = now;
            let con = m.mem[t.con];
            if con & 1 != 0 && dt > 0 {
                let cs = m.mem[t.clk] & 0x1F;
                if cs != 1 && cs != 2 {
                    self.notes.insert(format!("Timer{} clock source CS={cs} is not modelled (only FOSC/4 and FOSC); it does not count", t.n));
                } else {
                    let ticks = if cs == 2 { dt * 4 } else { dt };
                    let clocks = prescale(&mut t.pre, u32::from((con >> 4) & 3), ticks);
                    let sh = u32::from((con >> 4) & 3);
                    let ratio = if cs == 2 { 4 } else { 1 };
                    if comps.is_empty() {
                        if std::mem::take(&mut t.clr) && clocks > 0 {
                            t.cnt = 0;
                        }
                        let total = u64::from(t.cnt) + clocks;
                        if total >= 65536 {
                            t.flag.set(m);
                            let last_wrap = (total / 65536) * 65536 - u64::from(t.cnt);
                            ev.push(at(base, pre0, sh, last_wrap, ratio));
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
                            let to_match = comps
                                .iter()
                                .map(|&(_, r, _)| {
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
                            let cyc = at(base, pre0, sh, done, ratio);
                            if wrapped {
                                t.flag.set(m);
                                ev.push(cyc);
                            }
                            let mut clear = false;
                            for &(i, r, mode) in &comps {
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
        for t in &mut self.t2 {
            let (base, pre0) = (t.last, t.pre);
            let dt = now - t.last;
            t.last = now;
            let con = m.mem[t.con];
            if con & 0x80 != 0 && dt > 0 {
                let cs = m.mem[t.clk] & 0x1F;
                let mode = m.mem[t.hlt] & 0x1F;
                if cs != 1 && cs != 2 {
                    self.notes.insert(format!("Timer{} clock source CS={cs} is not modelled (only FOSC/4 and FOSC); it does not count", t.n));
                } else {
                    if mode != 0 && mode != 8 {
                        self.notes.insert(format!("Timer{} MODE={mode:#04x} needs an external reset/gate source (S4); run as the software mode", t.n));
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
        for &(tn, c, t2_pr, t2_sh) in &periods {
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
                hits.push(Sched { tick: 4 * c, code: ccp.sig, act: 1, flag: None, pwm: true });
                if pulse < period {
                    hits.push(Sched { tick: 4 * c + pulse, code: ccp.sig, act: 0, flag: Some(ccp.flag), pwm: true }); // [A62]
                }
            }
        }
        for &(tn, c, t2_pr, t2_sh) in &periods {
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
        self.sched.extend(hits);
        self.apply_sched(m, now, &mut ev);
        self.cmp_scan(m, now);
        self.apply_sched(m, now, &mut ev); // comparator edges can schedule CWG dead-band changes
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
        for q in &mut self.spis {
            q.sync(m, now, &mut ev, &mut self.spi_tx, &mut self.notes);
        }
        for q in &mut self.mssps {
            q.sync(m, now, &mut ev, &mut self.spi_tx, &mut self.i2c_log, &mut self.notes);
        }
        for q in &mut self.i2cqs {
            q.sync(m, now, &mut ev, &mut self.i2c_log, &mut self.notes);
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
            k if matches!(t1_dec(k), Some((_, 4))) => {
                let i = t1_dec(k).unwrap().0;
                let g = m.mem[self.t1[i].gcon] & !4;
                g | if self.gate_value(i, m, now) { 4 } else { 0 }
            }
            T0L => {
                let t = self.t0.as_ref().unwrap();
                if m.mem[t.c0] & 0x10 != 0 {
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
                self.mssps[i].read_buf(m)
            }
            I2CQ0..I2CQ_END => {
                let i = usize::from(k - I2CQ0) / usize::from(i2cq::HOOKS);
                self.i2cqs[i].read_rxb(m, now)
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
    /// The value a register shows right now, for a display: the registers the CPU computes on a read (port pin levels)
    /// without any side effect. `None` for the rest (their stored byte is current, or a read would pop a FIFO).
    pub fn peek_hook(&self, k: u8, m: &DataMem, now: u64) -> Option<u8> {
        match k {
            PORT0..=0x6F => Some(self.port_levels(usize::from(k - PORT0), m, now)),
            _ => None,
        }
    }

    pub fn is_stored(k: u8) -> bool {
        !(k == NVM0 + nvm::CON2 || k == NVMQ0 + nvmq::LOCK || (matches!(k, T0L | PORT0..=0x6F) || matches!(t1_dec(k), Some((_, 0 | 1 | 4))) || matches!(t2_dec(k), Some((_, 0)))) || (UART0..40).contains(&k) && (k - UART0) % UART_HOOKS == 3
            || (QUART0..QUART_END).contains(&k) && (k - QUART0) % quart::HOOKS == quart::RXB
            || (SPI0..SPI_END).contains(&k) && (k - SPI0) % spi::HOOKS == spi::RXB
            || (I2CQ0..I2CQ_END).contains(&k) && (k - I2CQ0) % i2cq::HOOKS == i2cq::RXB
            || (MSSP0..MSSP_END).contains(&k) && (k - MSSP0) % mssp::HOOKS == mssp::BUF)
    }

    /// A write: `v` is already masked to the implemented bits. The caller has synced to the writing cycle.
    pub fn write(&mut self, k: u8, v: u8, now: u64, m: &mut DataMem, prog: &mut [u8]) {
        match k {
            NVMQ0..=244 => {
                let n = self.nvmq.as_mut().unwrap();
                self.stall += n.write(k - NVMQ0, v, now, m, prog, &mut self.notes);
                self.flash_event(now);
            }
            NVM0..=241 => {
                let n = self.nvm.as_mut().unwrap();
                self.stall += n.write(k - NVM0, v, now, m, prog, &mut self.notes);
                self.flash_event(now);
            }
            UART0..=39 => self.uart_write(k, v, now, m),
            SPI0..SPI_END => {
                let i = usize::from(k - SPI0) / usize::from(spi::HOOKS);
                self.spis[i].write((k - SPI0) % spi::HOOKS, v, now, m);
            }
            MSSP0..MSSP_END => {
                let i = usize::from(k - MSSP0) / usize::from(mssp::HOOKS);
                self.mssps[i].write((k - MSSP0) % mssp::HOOKS, v, now, m, &mut self.notes);
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
                let a = self.t1[t1_dec(k).unwrap().0].gcon;
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
                for i in 0..self.cmps.len() {
                    self.cmp_update(m, i, now);
                }
            }
            ADC0 => {
                let a = self.adc.as_mut().unwrap();
                // Q41: GO is not set by the instruction that turns the ADC on (DS40002214G 40.2.1 p.720). `[A20]`
                let was_on = m.mem[a.con] & 0x80 != 0;
                let (on, go) = (v & 0x80 != 0, v & 1 != 0 && (a.q41.is_none() || was_on));
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
                if v & 1 != 0 && m.mem[t.gcon] & 0x80 != 0 {
                    self.notes.insert(format!("Timer{} gate (GE) is not modelled; the timer counts regardless of the gate", t.n));
                }
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
            CCP0..=225 => {
                let i = usize::from(k - CCP0) / usize::from(ccp::HOOKS);
                let (con, sig) = (self.ccps[i].con, self.ccps[i].sig);
                let old = m.mem[con];
                m.mem[con] = v & 0xDF | old & 0x20; // OUT is read only
                self.ccps[i].edges = 0;
                if v & 0x80 == 0 || v & 15 == 0 {
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
                if v & 0x80 == 0 {
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
        let (sample, end);
        if let Some((ph, ah)) = a.q41 {
            // Q41: precharge and acquisition are counted in Fosc clocks (ADACQ = 0 with ADPRE != 0 is 8192),
            // the conversion is 14 TAD + 2 TCY (DS40002214G 40.7.9, 40.7.10, AD21 p.903). `[A18]`
            let pre = u64::from(m.mem[ph] & 0x1F) << 8 | u64::from(m.mem[a.pre]);
            let mut acq = u64::from(m.mem[ah] & 0x1F) << 8 | u64::from(m.mem[a.acq]);
            if acq == 0 && pre != 0 {
                acq = 8192;
            }
            sample = t + (pre + acq).div_ceil(4);
            end = sample + (2 * n * 14).div_ceil(4) + 2;
        } else {
            let lead = u64::from(m.mem[a.pre]) + u64::from(m.mem[a.acq]);
            sample = t + (2 * n * lead).div_ceil(4);
            end = t + (n * (2 * lead + 23)).div_ceil(4);
        }
        let ch = if a.q41.is_some() { m.mem[a.pch] } else { m.mem[a.pch] & 0x3F };
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
        let mut v = m.mem[p.lat] & !tris | ext & tris & !ana;
        for &(reg, pi, bit) in &self.ppsout {
            // A pin driven by a modelled peripheral source (RxyPPS, TRIS clear) shows the source level, not the latch.
            let code = m.mem[reg] & 31;
            if pi == i && tris >> bit & 1 == 0 && (self.ccps.iter().any(|c| c.sig == code) || self.pwms.iter().any(|c| c.sig == code) || self.cmps.iter().any(|c| c.sig == code) || self.clcs.iter().any(|c| c.sig == code) || ((1..=4).contains(&code) && self.cwg.as_ref().is_some_and(|c| c.hiz >> (code - 1) & 1 == 0))) {
                v = v & !(1 << bit) | u8::from(self.sig >> code & 1 != 0) << bit;
            }
        }
        // The OLED's bus lines: external pull-up on a released pin, held low while the OLED acknowledges or answers.
        v |= self.pullup.get(i).copied().unwrap_or(0) & tris;
        v &= !self.forced.get(i).copied().unwrap_or(0);
        v & m.implemented[p.port]
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
    fn gate_value(&mut self, i: usize, m: &DataMem, now: u64) -> bool {
        let t = &self.t1[i];
        let tn = t.n;
        let (gate, gpps, gcon) = (t.gate, t.gpps, t.gcon);
        if m.mem[gcon] & 0x80 == 0 {
            return false;
        }
        let src = match (gate.map(|g| m.mem[g] & 31), gpps) {
            (Some(0), Some(r)) => self.pps_pin(m, r, now),
            _ => {
                self.notes.insert(format!("Timer{tn} gate source other than the T{tn}GPPS pin is not modelled; GVAL sees it low"));
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
