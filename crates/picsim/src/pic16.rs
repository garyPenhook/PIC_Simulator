//! PIC16 enhanced mid-range core (`pic16e`): the standard instruction set of DS40002343F Table 41-3 (pp.592-594) and
//! the instruction descriptions on pp.595-607, executed from the 14-bit words pic-as wrote. Written for the
//! PIC16F17126/46; the PIC16F131xx, 152xx and 180x6 share it. The PIC16F183xx (DS40001839E) is the same core with a
//! 5-bit BSR: 32 banks, MOVLB `00 0000 001k kkkk` (Table 34-3 p.399, p.406), a shorter linear region and the EEPROM
//! readable through the FSRs (4.5 pp.60-62); its Timer1/3/5 and Timer2/4/6 are the older register layout (ch.27-28).
//!
//! Cycle counts are the data sheet's (p.591): 1; 2 for CALL, CALLW, RETURN, RETLW, RETFIE, GOTO, BRA, BRW, a taken
//! skip and a write to PCL; +1 when an instruction reaches program flash through an INDFn register.
//! Of the peripherals only the timers (0, 1/3, 2/4/6; milestone S3/S4) exist: the other registers are plain storage and `unmodelled` lists
//! the ones a program touched. The facts the core relies on, with their pages, are in `docs/FACTS-SIM.md` (PIC16 core).

use crate::config::Config;
use crate::periph::adc::Adc;
use crate::periph::nvm16::Nvm16;
use crate::periph::uart::{self, Uart};
use crate::periph::wdt::{self, Wdt};
use crate::periph::mssp::{self, I2cEv, Mssp};
use crate::periph::{pin_name, Board, Bus, Pin, PinView, RxStream};
use devdesc::Device;
use std::collections::{BTreeMap, BTreeSet};

pub const C: u8 = 1;
pub const DC: u8 = 2;
pub const Z: u8 = 4;
pub const NPD: u8 = 8;
pub const NTO: u8 = 16;

/// Size of the data address space the model keeps: banks 0-63 of 128 bytes (DS40002343F 9.2).
const MEM: usize = 0x2000;
/// An empty stack: STKPTR resets to 1Fh (DS40002343F Fig. 9-14 p.53).
const EMPTY: u8 = 0x1F;

/// One I/O port: PORTx, LATx, TRISx and ANSELx (DS40002343F ch.16).
struct Port16 {
    letter: u8,
    port: usize,
    lat: usize,
    tris: usize,
    ansel: Option<usize>,
}

/// Timer0 (DS40002343F ch.22 p.244-249; same behaviour as the PIC18 model in `periph.rs`): Fosc/4 clock only.
/// 8-bit mode compares TMR0L with the live TMR0H and goes to 0 on the next clock; 16-bit mode rolls over at FFFFh and
/// buffers TMR0H; prescaler and postscaler are cleared by a write to TMR0L, T0CON0 or T0CON1 `[A106]`.
struct Timer0 {
    /// T0CKIPPS, the pin of the clock sources 000 (true) and 001 (inverted).
    cki: Option<usize>,
    l: usize,
    h: usize,
    c0: usize,
    c1: usize,
    flag: (usize, u8),
    cnt: u16,
    pre: u64,
    post: u8,
    last: u64,
}

/// Timer1/Timer3 (DS40002343F ch.23 p.252-267): 16-bit up counter, prescaler 1:1-1:8, TMRxIF on the roll-over from FFFFh to
/// 0000h. RD16 buffers TMRxH like Timer0; only a write to TMRxL clears the prescaler (23.6). Fosc/4 and Fosc clocks `[A107]`.
struct Timer1 {
    n: u8,
    /// TxCKIPPS (the clock pin), TxGPPS (the gate pin) and TxGATE (the gate source, newer layout only).
    cki: Option<usize>,
    gpps: Option<usize>,
    gate: Option<usize>,
    l: usize,
    h: usize,
    con: usize,
    /// TxCLK; `None` on the PIC16F183xx, where TMRxCS<1:0> is T1CON<7:6> (00 Fosc/4, 01 Fosc; DS40001839E Register
    /// 27-1 p.293) and a write to TMRxH or TMRxL clears the prescaler (27.3 p.286).
    clk: Option<usize>,
    gcon: usize,
    flag: (usize, u8),
    cnt: u16,
    pre: u64,
    last: u64,
}

/// Timer2/4/6 (DS40002343F ch.24 p.268-287): T2TMR counts up, matches T2PR and goes to 0 on the next clock; the postscaler
/// counts matches and TMRxIF is set when it reaches OUTPS+1. Prescaler and postscaler are cleared by a write to T2TMR or
/// T2CON (T2TMR is not cleared by T2CON, 24.1). Software-gate and software-start one-shot modes only `[A107]`.
struct Timer2 {
    n: u8,
    tmr: usize,
    pr: usize,
    con: usize,
    /// TxHLT and TxCLKCON; `None` on the PIC16F183xx: Fosc/4 only, T2CON = -/OUTPS<3:0>/ON/CKPS<1:0> with prescaler
    /// 1, 4, 16, 64 (DS40001839E 28.1 p.298, Register 28-1 p.299).
    hlt: Option<usize>,
    clk: Option<usize>,
    flag: (usize, u8),
    cnt: u8,
    pre: u64,
    post: u8,
    last: u64,
}

/// The clock of a Timer1-type timer.
enum Src {
    /// Fosc/4 ticks per instruction cycle: 1 for Fosc/4, 4 for Fosc.
    Cycles(u64),
    /// The rising edges of the pin an input selection register (TxCKIPPS) selects.
    Pin(usize),
    /// A source the simulator does not model.
    None,
}

/// Where an access made through a file-register operand or an FSR ends up.
#[derive(Clone, Copy, Debug)]
enum Tgt {
    /// A data memory address (0..MEM).
    Mem(usize),
    /// A program flash word (readable through INDF: its low eight bits; writes are ignored), DS40002343F 9.6.3 p.57.
    Flash(u32),
    /// A data EEPROM byte at FSR 7000h-70FFh on the PIC16F183xx (read only, one extra cycle; DS40001839E 4.5.3 p.62).
    Eeprom(u8),
    /// Reads 0, writes are ignored: an INDF that points at an INDF (9.6 p.55) or the reserved FSR range.
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reset {
    /// The RESET instruction (clears PCON0.nRI).
    Software,
    StackOverflow,
    StackUnderflow,
    /// Watchdog time-out (TO = 0, PCON0.RWDT = 0).
    Wdt,
    /// CLRWDT in the closed window of the windowed watchdog, or unarmed (PCON0.WDTWV = 0; TO = 0).
    WdtWindow,
}

pub struct Pic16 {
    pub mem: Vec<u8>,
    /// Bits that exist, per address.
    pub implemented: Vec<u8>,
    is_sfr: Vec<bool>,
    names: Vec<Option<String>>,
    /// Addresses the core handles itself; an SFR outside this set is storage only (listed in `unmodelled` when used).
    core: Vec<bool>,
    /// SFRs read or written that nothing models.
    pub unmodelled: BTreeSet<String>,
    /// Data addresses outside every RAM and SFR range that were accessed.
    pub stray: BTreeSet<u32>,
    /// Program memory as the bytes of the hex file (word `w` is at `2w`, `2w + 1`); erased = FFh.
    pub prog: Vec<u8>,
    /// Bytes outside program memory (user ID, configuration words, ...), by byte address.
    pub aux: BTreeMap<u32, u8>,
    /// Program counter in words: the address of the next instruction to fetch.
    pub pc: u32,
    pub cycles: u64,
    pub icount: u64,
    stack: [u16; 16],
    pub cfg: Config,
    cfg_regs: Vec<devdesc::ConfigRegister>,
    /// Messages about things the model does not do, deduplicated.
    pub notes: BTreeSet<String>,
    words: u32,
    /// The PIC16F183xx: BSR is 5 bits wide (32 banks, the MOVLB encoding and FSR map of DS40001839E).
    pub bsr5: bool,
    /// End of the linear data region (exclusive): 2FF0h (banks 0-50, DS40002343F 9.6.2) or 29B0h (banks 0-30,
    /// DS40001839E 4.5.2 p.62).
    lin_end: u32,
    /// Byte address in `aux` of EEPROM byte 0 (word `memory.eeprom.start` of the hex image), for FSR reads.
    ee_base: Option<u32>,
    vector: u32,
    sleeping: bool,
    /// The instruction after SLEEP always runs before an interrupt is taken (12.7 p.113).
    no_irq: bool,
    /// A peripheral set a flag that made a request (PIE & PIR, PEIE for the other groups) pending since the previous
    /// step began (`raise_flag`): the request arrived while the program was running (`[A108]`). A request made pending
    /// by software (a PIE, PIR or PEIE write) does not set it and is taken at once (`[A104]`).
    irq_new: bool,
    /// The flags whose rise set `irq_new` during the current step: a software write that clears all of them again
    /// withdraws the fresh request (`[A110]`), so a request the same write makes is taken at once (`[A104]`).
    irq_new_by: Vec<(usize, u8)>,
    stk_fault: Option<Reset>,
    /// A reset requested by the instruction that is executing, done at its end.
    reset_req: Option<Reset>,
    extra: u64,
    /// An access through INDFn reached program flash during this instruction (one extra cycle in all, however many accesses).
    flash_cycle: bool,
    // register addresses
    a_status: usize,
    a_wreg: usize,
    a_bsr: usize,
    a_pclath: usize,
    a_intcon: usize,
    a_stkptr: usize,
    a_tos: usize,
    a_pcon0: Option<usize>,
    /// FSRnL addresses (FSRnH is +1) and the matching shadow registers (STATUS, WREG, BSR, PCLATH, FSR0L/H, FSR1L/H).
    a_fsr: [usize; 2],
    shad: [usize; 8],
    a_tris: [Option<usize>; 3],
    /// (PIRn, PIEn) addresses; group 0 does not depend on PEIE.
    irq: Vec<(usize, usize)>,
    /// MCLR-column reset values: register address, bits set, bits cleared.
    mclr: Vec<(usize, u8, u8)>,
    reset_vector: u32,
    t0: Option<Timer0>,
    t1: Vec<Timer1>,
    t2: Vec<Timer2>,
    /// Registers of any timer: an access first brings the timers to the current cycle.
    tmr_reg: Vec<bool>,
    /// The PIRn registers: a read or write first brings Timer1/3/5 and the PIC16F183xx's Timer2/4/6 to the accessing
    /// cycle, so a read sees a flag they set in that cycle, as a timer register read sees its count (`[A2]`), and a
    /// clear in that cycle wins. Timer0 and the newer Timer2/4/6 (with TxHLT) set their flags after the access: a read in
    /// that cycle misses it and a clear leaves it set (MDB, `tests/probe/t1_poll.S`, `t2_f183.S`, `t2_poll.S`,
    /// `t0_poll.S`, `[A110]`).
    flag_reg: Vec<bool>,
    /// EUSARTs (`periph/uart.rs`, the module of the Q10) and, per address, which register of which one it is
    /// (index, hook sub-code 0 TXREG .. 4 RCSTA). `tx` logs the bytes they start to send: (EUSART number, cycle, byte).
    uarts: Vec<Uart>,
    uart_reg: Vec<Option<(u8, u8)>>,
    pub tx: Vec<(u8, u64, u8)>,
    /// MSSPs (`periph/mssp.rs`, the module of the Q10; I2C host mode drives the OLED on a hardware bus) and, per address,
    /// which register of which one it is (index, sub-code `mssp::BUF` ..). `i2c_log` is what the hosts put on the bus.
    mssps: Vec<Mssp>,
    mssp_reg: Vec<Option<(u8, u8)>>,
    i2c_log: Vec<(u8, u64, I2cEv)>,
    spi_log: Vec<(u8, u64, u8)>,
    /// Per MSSP: (number, SSPxCLKPPS, SSPxDATPPS, SCL1 and SDA1 output codes of RxyPPS); `None` codes when the family's
    /// table is not known to the simulator.
    i2c_pps: Vec<(u8, usize, usize, Option<(u8, u8)>)>,
    /// Cycle SLEEP was executed (the EUSARTs owe the cycles of Sleep when the part wakes).
    sleep_at: u64,
    /// The ADC (`periph/adc.rs`, the ADCC of the Q41) and which addresses are its ADCON0 or ADRES (a read of them sees
    /// the conversion that has just ended).
    adc: Option<Adc>,
    adc_reg: Vec<bool>,
    /// FVRCON: FVRRDY (bit 6, read only) follows FVREN (bit 7) `[A15]`; on the PIC16F152xx it is always 1 (DS40002195E
    /// 26.3.1 p.316 note 2; MDB agrees). The PIC16F183xx data sheet says the same (Register 16-1 p.180 note 1) but also
    /// lists the bit as "R-q/q", and MDB follows FVREN there, so the model does too (D-SIM-7).
    a_fvr: Option<usize>,
    fvr_always: bool,
    /// The data EEPROM (`periph/nvm16.rs`) and which addresses are its NVMCON1 / NVMCON2 (hook sub-code).
    nvm: Option<Nvm16>,
    nvm_reg: Vec<Option<u8>>,
    /// Unimplemented bits that read 1 (NVMADRH bit 7 on the PIC16F183xx, DS40001839E Register 11-4 p.137: "U-1").
    ones: Vec<u8>,
    /// The I/O ports and, per address, which port's PORTx (`true`) or LATx (`false`) it is.
    ports: Vec<Port16>,
    port_reg: Vec<Option<(u8, bool)>>,
    /// What the outside world drives on the input pins: (from cycle, port index, pin mask, levels), stable-sorted by cycle.
    drive: Vec<(u64, usize, u8, u8)>,
    /// PPSLOCK, how far the 55h, AAh sequence has got (0 none, 1 after 55h, 2 after AAh), and which addresses are PPS
    /// selection registers (xxxPPS, RxyPPS): writes to them are ignored while PPSLOCKED is set.
    pps_lock: Option<usize>,
    pps_seq: u8,
    pps_reg: Vec<bool>,
    /// RxyPPS: (register, index into `ports`, pin).
    pps_out: Vec<(usize, usize, u8)>,
    /// The MCLR pin (index into `ports`, mask): input only, no analog function, and it reads 1 while MCLRE = 1.
    mclr_pin: Option<(usize, u8)>,
    /// The parts on the board (LEDs, a bit-banged OLED) and, per port, the pins their external pull-ups raise and the
    /// pins an OLED holds low (see `periph/board.rs`).
    pub board: Board,
    pullup: Vec<u8>,
    forced: Vec<u8>,
    /// Interrupt-on-change `[A125]`: (index into `ports`, IOCxF, IOCxN, IOCxP), the IOCIF flag, and the INT pin: (INTPPS,
    /// INTF flag, INTEDG register and mask). `ioc_at` is the cycle the pins have been looked at up to.
    ioc: Vec<(usize, usize, usize, usize)>,
    iocif: Option<(usize, u8)>,
    int_pin: Option<(usize, (usize, u8), usize, u8)>,
    ioc_at: u64,
    /// The watchdog (`periph/wdt.rs`, the timing model of the Q10; `[A126]`): the windowed WWDT of the PIC16F131xx and
    /// PIC16F171xx (WDTCON0/1) or the single-register WDT (WDTCON). `wdt_reg` marks its registers.
    wdt: Option<Wdt>,
    wdt_simple: bool,
    wdt_reg: Vec<bool>,
}

fn por_bits(s: &str) -> u8 {
    let mut v = 0u8;
    for (i, c) in s.chars().rev().enumerate() {
        if c == '1' && i < 8 {
            v |= 1 << i;
        }
    }
    v
}

/// a + b + cin with the C, DC and Z flags (C is the carry out; for a subtraction `a + !b + 1` it means "no borrow").
#[inline]
fn add8(a: u8, b: u8, cin: u8) -> (u8, u8) {
    let t = u16::from(a) + u16::from(b) + u16::from(cin);
    let r = t as u8;
    let mut f = 0;
    if t > 0xFF {
        f |= C;
    }
    if (a & 15) + (b & 15) + cin > 15 {
        f |= DC;
    }
    (r, f | z(r))
}

#[inline]
fn z(r: u8) -> u8 {
    if r == 0 { Z } else { 0 }
}

/// Sign-extend the low `bits` bits of `v`.
#[inline]
fn sext(v: u32, bits: u32) -> i32 {
    ((v << (32 - bits)) as i32) >> (32 - bits)
}

impl Pic16 {
    pub fn new(dev: &Device) -> Result<Pic16, String> {
        if dev.device.family != "pic16e" {
            return Err(format!("{}: family {} is not a pic16e part", dev.device.name, dev.device.family));
        }
        let d = &dev.memory.data;
        let mut p = Pic16 {
            mem: vec![0; MEM],
            implemented: vec![0; MEM],
            is_sfr: vec![false; MEM],
            names: vec![None; MEM],
            core: vec![false; MEM],
            unmodelled: BTreeSet::new(),
            stray: BTreeSet::new(),
            prog: vec![0xFF; 2 * dev.memory.program.size as usize],
            aux: BTreeMap::new(),
            pc: dev.vectors.reset,
            cycles: 0,
            icount: 0,
            stack: [0; 16],
            cfg: Config::decode_words(&dev.config, &BTreeMap::new()),
            cfg_regs: dev.config.clone(),
            notes: BTreeSet::new(),
            words: dev.memory.program.size,
            bsr5: false,
            lin_end: 0x2FF0,
            ee_base: None,
            vector: dev.vectors.interrupt_high,
            sleeping: false,
            no_irq: false,
            irq_new: false,
            irq_new_by: Vec::new(),
            stk_fault: None,
            reset_req: None,
            extra: 0,
            flash_cycle: false,
            a_status: 0,
            a_wreg: 0,
            a_bsr: 0,
            a_pclath: 0,
            a_intcon: 0,
            a_stkptr: 0,
            a_tos: 0,
            a_pcon0: None,
            a_fsr: [0; 2],
            shad: [0; 8],
            a_tris: [None; 3],
            irq: Vec::new(),
            mclr: Vec::new(),
            reset_vector: dev.vectors.reset,
            t0: None,
            t1: Vec::new(),
            t2: Vec::new(),
            tmr_reg: vec![false; MEM],
            flag_reg: vec![false; MEM],
            uarts: Vec::new(),
            uart_reg: vec![None; MEM],
            tx: Vec::new(),
            mssps: Vec::new(),
            mssp_reg: vec![None; MEM],
            i2c_log: Vec::new(),
            spi_log: Vec::new(),
            i2c_pps: Vec::new(),
            sleep_at: 0,
            adc: None,
            adc_reg: vec![false; MEM],
            a_fvr: None,
            fvr_always: false,
            nvm: None,
            nvm_reg: vec![None; MEM],
            ones: vec![0; MEM],
            ports: Vec::new(),
            port_reg: vec![None; MEM],
            drive: Vec::new(),
            pps_lock: None,
            pps_seq: 0,
            pps_reg: vec![false; MEM],
            pps_out: Vec::new(),
            mclr_pin: None,
            board: Board::default(),
            pullup: Vec::new(),
            forced: Vec::new(),
            ioc: Vec::new(),
            iocif: None,
            int_pin: None,
            ioc_at: 0,
            wdt: None,
            wdt_simple: false,
            wdt_reg: vec![false; MEM],
        };
        for g in &d.gpr {
            for a in g.start..g.end {
                if (a as usize) < MEM {
                    p.implemented[a as usize] = 0xFF;
                }
            }
        }
        for r in &d.sfr {
            for a in r.start..r.end {
                if (a as usize) < MEM {
                    p.is_sfr[a as usize] = true;
                    p.implemented[a as usize] = 0xFF;
                }
            }
        }
        for s in &dev.sfr {
            let a = s.addr as usize;
            if s.width != 8 || a >= MEM {
                continue;
            }
            p.names[a] = Some(s.name.clone());
            p.implemented[a] = s.implemented.map_or(0xFF, |v| v as u8);
            p.mem[a] = por_bits(&s.por) & p.implemented[a];
        }
        let addr = |name: &str| -> Result<usize, String> {
            dev.find_sfr(name).map(|s| s.addr as usize).ok_or_else(|| format!("{}: no SFR {name}", dev.device.name))
        };
        // The core registers sit at fixed offsets in every bank (DS40002343F 9.2.2); check the description agrees.
        for (n, want) in [("INDF0", 0), ("INDF1", 1), ("PCL", 2), ("STATUS", 3), ("FSR0L", 4), ("FSR0H", 5), ("FSR1L", 6), ("FSR1H", 7), ("BSR", 8), ("WREG", 9), ("PCLATH", 10), ("INTCON", 11)] {
            if addr(n)? != want {
                return Err(format!("{}: {n} is not at {want:#x}", dev.device.name));
            }
            p.core[want] = true;
        }
        p.a_status = 3;
        p.a_wreg = 9;
        p.a_bsr = 8;
        p.a_pclath = 10;
        p.a_intcon = 11;
        p.a_fsr = [4, 6];
        p.a_stkptr = addr("STKPTR")?;
        // The stack is empty after any reset (DS40001839E 4.4 Fig. 4-4 p.57); the PIC16F183xx packs give STKPTR's POR
        // value as unknown (Table 4-4 p.55 likewise), which would read as one entry deep.
        p.mem[p.a_stkptr] = EMPTY & p.implemented[p.a_stkptr];
        p.a_tos = addr("TOSL")?;
        if addr("TOSH")? != p.a_tos + 1 {
            return Err(format!("{}: TOSH does not follow TOSL", dev.device.name));
        }
        for (i, n) in ["STATUS_SHAD", "WREG_SHAD", "BSR_SHAD", "PCLATH_SHAD", "FSR0L_SHAD", "FSR0H_SHAD", "FSR1L_SHAD", "FSR1H_SHAD"].iter().enumerate() {
            p.shad[i] = addr(n)?;
            p.core[p.shad[i]] = true;
        }
        for a in [p.a_stkptr, p.a_tos, p.a_tos + 1] {
            p.core[a] = true;
        }
        p.wdt = Wdt::find(dev).or_else(|| {
            p.wdt_simple = true;
            Wdt::find_simple(dev)
        });
        if let Some(w) = &p.wdt {
            let regs: Vec<usize> = if p.wdt_simple { vec![w.con0] } else { vec![w.con0, w.con1, w.psl, w.psh, w.tmr] };
            if regs.iter().all(|&r| r < MEM) {
                for r in regs {
                    (p.core[r], p.wdt_reg[r]) = (true, true);
                }
            } else {
                p.wdt = None;
            }
        }
        p.a_pcon0 = dev.find_sfr("PCON0").map(|s| s.addr as usize);
        if let Some(a) = p.a_pcon0 {
            p.core[a] = true;
        }
        for (i, n) in ["TRISA", "TRISB", "TRISC"].iter().enumerate() {
            p.a_tris[i] = dev.find_sfr(n).map(|s| s.addr as usize);
        }
        for n in 0..8 {
            if let (Some(r), Some(e)) = (dev.find_sfr(&format!("PIR{n}")), dev.find_sfr(&format!("PIE{n}"))) {
                p.irq.push((r.addr as usize, e.addr as usize));
                p.flag_reg[r.addr as usize] = true;
                p.core[r.addr as usize] = true;
                p.core[e.addr as usize] = true;
            }
        }
        let flag_of = |name: &str| {
            dev.sfr
                .iter()
                .filter(|s| s.name.starts_with("PIR"))
                .find_map(|s| s.fields.iter().chain(s.legacy.iter()).find(|f| f.name == name).map(|f| (s.addr as usize, 1u8 << f.pos)))
        };
        let addr = |name: &str| dev.find_sfr(name).map(|s| s.addr as usize);
        let has_field = |reg: &str, field: &str| dev.find_sfr(reg).is_some_and(|s| s.fields.iter().any(|f| f.name == field));
        if let (Some(l), Some(h), Some(c0), Some(c1), Some(flag)) = (addr("TMR0L"), addr("TMR0H"), addr("T0CON0"), addr("T0CON1"), flag_of("TMR0IF")) {
            for a in [l, h, c0, c1] {
                p.core[a] = true;
                p.tmr_reg[a] = true;
            }
            if let Some(a) = addr("T0CKIPPS") {
                p.core[a] = true;
            }
            p.t0 = Some(Timer0 { cki: addr("T0CKIPPS"), l, h, c0, c1, flag, cnt: 0, pre: 0, post: 0, last: 0 });
        }
        // Timer1/3/5: the newer parts select the clock in TxCLK; the PIC16F183xx has no TxCLK (clock in TxCON).
        for n in [1u8, 3, 5] {
            if let (Some(l), Some(h), Some(con), Some(gcon), Some(flag)) = (
                addr(&format!("TMR{n}L")),
                addr(&format!("TMR{n}H")),
                addr(&format!("T{n}CON")),
                addr(&format!("T{n}GCON")),
                flag_of(&format!("TMR{n}IF")),
            ) {
                // The layout is the one the TxCON fields name (TMRxCS: clock select in TxCON, DS40001839E Register 27-1;
                // RD16: DS40002343F 23.13.1 p.261, clock in TxCLK); TxCLK must be there exactly for the newer one.
                let clk = addr(&format!("T{n}CLK"));
                let con_name = format!("T{n}CON");
                let old = has_field(&con_name, &format!("TMR{n}CS"));
                if old == has_field(&con_name, "RD16") || old == clk.is_some() {
                    p.notes.insert(format!("Timer{n}: T{n}CON fields and T{n}CLK do not agree on a register layout; it is not modelled"));
                    continue;
                }
                for a in [l, h, con, gcon].into_iter().chain(clk) {
                    p.core[a] = true;
                    p.tmr_reg[a] = true;
                }
                for a in [addr(&format!("T{n}CKIPPS")), addr(&format!("T{n}GPPS")), addr(&format!("T{n}GATE"))].into_iter().flatten() {
                    p.core[a] = true; // read by the model (the clock and gate pins)
                }
                p.t1.push(Timer1 { n, cki: addr(&format!("T{n}CKIPPS")), gpps: addr(&format!("T{n}GPPS")), gate: addr(&format!("T{n}GATE")), l, h, con, clk, gcon, flag, cnt: 0, pre: 0, last: 0 });
            }
        }
        // Timer2/4/6: T2TMR/T2PR with T2HLT and T2CLKCON, or the PIC16F183xx's TMR2/PR2 with neither.
        for n in [2u8, 4, 6] {
            let tmr = addr(&format!("T{n}TMR")).or_else(|| addr(&format!("TMR{n}")));
            let pr = addr(&format!("T{n}PR")).or_else(|| addr(&format!("PR{n}")));
            let (hlt, clk) = (addr(&format!("T{n}HLT")), addr(&format!("T{n}CLKCON")));
            if hlt.is_some() != clk.is_some() {
                p.notes.insert(format!("Timer{n} has only one of T{n}HLT and T{n}CLKCON in the description; it is not modelled"));
                continue;
            }
            let con_name = format!("T{n}CON");
            if let (Some(tmr), Some(pr), Some(con), Some(flag)) = (tmr, pr, addr(&con_name), flag_of(&format!("TMR{n}IF"))) {
                // TMRxON in TxCON is the older layout (DS40001839E Register 28-1), ON the newer (DS40002343F 24.10.3
                // p.284); TxHLT and TxCLKCON must be there exactly for the newer one.
                let old = has_field(&con_name, &format!("TMR{n}ON"));
                if old == has_field(&con_name, "ON") || old == hlt.is_some() {
                    p.notes.insert(format!("Timer{n}: T{n}CON fields and T{n}HLT/T{n}CLKCON do not agree on a register layout; it is not modelled"));
                    continue;
                }
                for a in [tmr, pr, con].into_iter().chain(hlt).chain(clk) {
                    p.core[a] = true;
                    p.tmr_reg[a] = true;
                }
                p.t2.push(Timer2 { n, tmr, pr, con, hlt, clk, flag, cnt: 0, pre: 0, post: 0, last: 0 });
            }
        }
        for n in 1..=2u8 {
            if let Some((u, regs)) = Uart::find(dev, n) {
                if regs.iter().any(|&a| a >= MEM) {
                    continue;
                }
                let i = p.uarts.len() as u8;
                for (sub, a) in regs.into_iter().enumerate() {
                    p.core[a] = true;
                    p.uart_reg[a] = Some((i, sub as u8));
                }
                p.uarts.push(u);
            }
        }
        // RxyPPS output codes of SCL1 and SDA1 (Table 18-2: DS40002343F p.202 and DS40002519D p.188 for the PIC16F171xx and
        // PIC16F131xx; the other families as recorded in lib/i2c.bas from their data sheets).
        let name = dev.device.name.as_str();
        let codes = if name.starts_with("PIC16F171") || name.starts_with("PIC16F131") {
            Some((0x1B, 0x1C))
        } else if name.starts_with("PIC16F152") {
            Some((0x07, 0x08))
        } else if name.starts_with("PIC16F180") {
            Some((0x15, 0x16))
        } else if name.starts_with("PIC16F183") {
            Some((0x18, 0x19))
        } else {
            None
        };
        for n in 1..=2u8 {
            if let Some((q, regs)) = Mssp::find(dev, n) {
                if regs.iter().any(|&a| a >= MEM) {
                    continue;
                }
                let i = p.mssps.len() as u8;
                for (sub, a) in regs.into_iter().enumerate() {
                    p.core[a] = true;
                    p.mssp_reg[a] = Some((i, sub as u8));
                }
                if let (Some(c), Some(d)) = (addr(&format!("SSP{n}CLKPPS")), addr(&format!("SSP{n}DATPPS"))) {
                    p.i2c_pps.push((n, c, d, codes));
                }
                p.mssps.push(q);
            }
        }
        if let Some(a) = dev.find_sfr("FVRCON").map(|s| s.addr as usize).filter(|&a| a < MEM) {
            p.core[a] = true;
            p.a_fvr = Some(a);
            let n = dev.device.name.as_str();
            p.fvr_always = n.starts_with("PIC16F152");
            if p.fvr_always {
                p.mem[a] |= 0x40;
            }
        }
        if let Some(a) = Adc::find(dev) {
            let regs = a.regs();
            if regs.iter().all(|&r| r < MEM) {
                for r in regs {
                    p.core[r] = true;
                }
                for r in [a.con, a.resl, a.resh] {
                    p.adc_reg[r] = true;
                }
                p.adc = Some(a);
            } else {
                p.notes.insert("the ADC of this part is not modelled (a register is outside the data memory); GO never clears".into());
            }
        } else if dev.find_sfr("ADCON0").is_some() {
            p.notes.insert("the ADC of this part is not modelled; GO never clears".into());
        }
        if p.irq.is_empty() {
            return Err(format!("{}: no PIR0/PIE0 interrupt registers", dev.device.name));
        }
        p.ee_base = (dev.memory.eeprom.size > 0).then_some(2 * dev.memory.eeprom.start);
        match p.implemented[p.a_bsr] {
            0x3F => {}
            0x1F => {
                p.bsr5 = true;
                p.lin_end = 0x29B0;
            }
            m => return Err(format!("{}: BSR implemented mask {m:#04x}: only 5-bit (PIC16F183xx) and 6-bit BSRs are modelled", dev.device.name)),
        }
        p.nvm = Nvm16::find(dev, p.bsr5).filter(|n| n.addrs().iter().all(|&r| r < MEM));
        if let Some(n) = &p.nvm {
            let adrh = n.adrh();
            p.ones[adrh] = por_bits(&dev.find_sfr("NVMADRH").map_or(String::new(), |s| s.por.clone())) & !p.implemented[adrh];
            p.mem[adrh] |= p.ones[adrh];
            for (k, r) in n.regs().into_iter().enumerate() {
                p.core[r] = true;
                p.nvm_reg[r] = Some(k as u8);
            }
        }
        // Ports: PORTx reads the pins and writes LATx; the other registers are storage.
        for letter in b'A'..=b'H' {
            let l = char::from(letter);
            let (Some(port), Some(lat), Some(tris)) = (addr(&format!("PORT{l}")), addr(&format!("LAT{l}")), addr(&format!("TRIS{l}"))) else { continue };
            let ansel = addr(&format!("ANSEL{l}"));
            if [port, lat, tris].into_iter().chain(ansel).any(|a| a >= MEM) {
                continue;
            }
            let i = p.ports.len() as u8;
            for a in [tris].into_iter().chain(ansel) {
                p.core[a] = true;
            }
            (p.core[port], p.core[lat]) = (true, true);
            (p.port_reg[port], p.port_reg[lat]) = (Some((i, true)), Some((i, false)));
            p.ports.push(Port16 { letter, port, lat, tris, ansel });
        }
        // The MCLR pin is RA3, or RE3 where the part has PORTE (PIC16F180x6). Its TRIS bit, where there is one, is read only
        // and reads 1 (DS40002195E 16.14.3 p.133; DS40001839E Register 12-2 p.143 "U-1"): the reset value says whether it does.
        p.mclr_pin = b"EA".iter().copied().find_map(|l| {
            let i = p.ports.iter().position(|q| q.letter == l)?;
            (p.implemented[p.ports[i].port] & 8 != 0).then_some((i, 8u8))
        });
        if let Some((i, m)) = p.mclr_pin {
            let t = p.ports[i].tris;
            if por_bits(&dev.find_sfr(&format!("TRIS{}", char::from(p.ports[i].letter))).map_or(String::new(), |s| s.por.clone())) & m != 0 {
                p.ones[t] |= m;
                p.mem[t] |= m;
            }
        }
        for i in 0..p.ports.len() {
            let l = char::from(p.ports[i].letter);
            if let (Some(f), Some(n), Some(pp)) = (addr(&format!("IOC{l}F")), addr(&format!("IOC{l}N")), addr(&format!("IOC{l}P"))) {
                p.ioc.push((i, f, n, pp));
            }
        }
        p.iocif = flag_of("IOCIF");
        let edg = dev.sfr.iter().find_map(|s| s.fields.iter().find(|f| f.name == "INTEDG").map(|f| (s.addr as usize, 1u8 << f.pos)));
        if let (Some(pps), Some(flag), Some((ea, em))) = (addr("INTPPS"), flag_of("INTF"), edg) {
            p.int_pin = Some((pps, flag, ea, em));
        }
        // PPS: the selection registers are xxxPPS and RxyPPS; PPSLOCK is the lock.
        p.pps_lock = addr("PPSLOCK").filter(|&a| a < MEM);
        if let Some(a) = p.pps_lock {
            p.core[a] = true;
            for s in &dev.sfr {
                let a = s.addr as usize;
                if a < MEM && s.width == 8 && s.name.ends_with("PPS") {
                    p.pps_reg[a] = true;
                    p.core[a] = true;
                    let b = s.name.as_bytes();
                    if b.len() == 6 && b[0] == b'R' && (b'0'..=b'7').contains(&b[2])
                        && let Some(i) = p.ports.iter().position(|q| q.letter == b[1])
                    {
                        p.pps_out.push((a, i, b[2] - b'0'));
                    }
                }
            }
        }
        p.mclr = dev
            .sfr
            .iter()
            .filter(|s| s.width == 8 && s.mclr.len() == 8 && (s.addr as usize) < MEM)
            .map(|s| {
                let (mut set, mut clr) = (0u8, 0u8);
                for (i, c) in s.mclr.chars().enumerate() {
                    match c {
                        '1' => set |= 0x80 >> i,
                        '0' => clr |= 0x80 >> i,
                        _ => {}
                    }
                }
                (s.addr as usize, set, clr)
            })
            .filter(|&(a, set, clr)| set | clr != 0 && a != 3)
            .collect();
        Ok(p)
    }

    /// Put a hex image into program memory (and the auxiliary regions), then decode the configuration words.
    pub fn load(&mut self, img: &crate::hex::Image) {
        for (&a, &b) in &img.bytes {
            if (a as usize) < self.prog.len() {
                self.prog[a as usize] = b;
            } else {
                self.aux.insert(a, b);
            }
        }
        self.cfg = Config::decode_words(&self.cfg_regs, &self.aux);
        self.wdt_reset_regs();
        if let (Some(n), Some(b)) = (self.nvm.as_mut(), self.ee_base) {
            for (i, v) in n.eeprom.iter_mut().enumerate() {
                *v = self.aux.get(&(b + 2 * i as u32)).copied().unwrap_or(0xFF);
            }
        }
    }

    // ----- registers -------------------------------------------------------------------------------------------

    #[inline]
    fn w(&self) -> u8 {
        self.mem[self.a_wreg]
    }

    #[inline]
    fn set_w(&mut self, v: u8) {
        self.mem[self.a_wreg] = v;
    }

    /// Set the STATUS flags in `mask` to the matching bits of `v`.
    #[inline]
    fn flags(&mut self, mask: u8, v: u8) {
        self.mem[self.a_status] = self.mem[self.a_status] & !mask | v & mask;
    }

    #[inline]
    fn carry(&self) -> u8 {
        self.mem[self.a_status] & C
    }

    fn fsr(&self, n: usize) -> u16 {
        u16::from(self.mem[self.a_fsr[n] + 1]) << 8 | u16::from(self.mem[self.a_fsr[n]])
    }

    fn set_fsr(&mut self, n: usize, v: u16) {
        self.mem[self.a_fsr[n]] = v as u8;
        self.mem[self.a_fsr[n] + 1] = (v >> 8) as u8 & self.implemented[self.a_fsr[n] + 1];
    }

    // ----- data memory -----------------------------------------------------------------------------------------

    /// The data address behind a banked address: the core registers (offset 00h-0Bh) and the common RAM (70h-7Fh)
    /// are the same bytes in every bank (DS40002343F 9.2).
    #[inline]
    fn phys(a: u32) -> usize {
        let a = (a & 0x1FFF) as usize;
        let o = a & 0x7F;
        if o < 0x0C || o >= 0x70 { o } else { a }
    }

    /// Where an FSR value points: banked memory, linear memory (the 80-byte GPR blocks of banks 0-50 laid end to end)
    /// or program flash (DS40002343F 9.6 pp.55-57). On the PIC16F183xx (Fig. 4-8 p.60, 4.5.2-4.5.3 p.62): banked
    /// 0000h-0FFFh, 1000h-1FFFh reserved, linear 2000h-29AFh (banks 0-30), EEPROM 7000h-70FFh; reserved space reads 0.
    fn target_of(&self, p: u16) -> Tgt {
        let p = u32::from(p);
        let banked_end = if self.bsr5 { 0x1000 } else { 0x2000 };
        if p & 0x8000 != 0 {
            Tgt::Flash(p & 0x7FFF)
        } else if p < banked_end {
            let a = Pic16::phys(p);
            if a < 2 { Tgt::Null } else { Tgt::Mem(a) }
        } else if (0x2000..self.lin_end).contains(&p) {
            let n = p - 0x2000;
            Tgt::Mem(((n / 80) << 7) as usize + 0x20 + (n % 80) as usize)
        } else if self.nvm.as_ref().is_some_and(|n| !n.eeprom.is_empty()) && p >> 8 == 0x70 {
            Tgt::Eeprom(p as u8)
        } else {
            Tgt::Null
        }
    }

    /// An EEPROM byte: the hex image's, or what the program wrote (erased = FFh).
    fn ee_byte(&self, n: u8) -> u8 {
        self.nvm.as_ref().map_or(0xFF, |x| x.eeprom[usize::from(n)])
    }

    /// The operand of a file-register instruction: direct through BSR, or through FSRn for INDFn.
    fn file(&mut self, f: u32) -> Tgt {
        let a = Pic16::phys(u32::from(self.mem[self.a_bsr] & 0x3F) << 7 | f);
        if a < 2 { self.target_of(self.fsr(a)) } else { Tgt::Mem(a) }
    }

    fn flash_word(&self, w: u32) -> u8 {
        let a = 2 * (w % self.words) as usize;
        self.prog[a]
    }

    fn rd(&mut self, t: Tgt) -> u8 {
        match t {
            Tgt::Null => 0,
            Tgt::Flash(w) => {
                self.flash_cycle = true;
                self.flash_word(w)
            }
            Tgt::Eeprom(n) => {
                self.flash_cycle = true; // "one additional instruction cycle" (DS40001839E 4.5.3 p.62)
                self.ee_byte(n)
            }
            Tgt::Mem(a) => {
                self.note(a);
                if self.tmr_reg[a] {
                    return self.read_tmr(a);
                }
                if self.flag_reg[a] {
                    self.sync_flag_timers();
                }
                if self.adc_reg[a] {
                    self.sync_adc();
                }
                if self.nvm_reg[a].is_some() {
                    self.sync_nvm();
                }
                if let Some((i, true)) = self.port_reg[a] {
                    let v = self.port_levels(usize::from(i));
                    self.mem[a] = v;
                    return v;
                }
                if let Some((i, sub)) = self.mssp_reg[a] {
                    self.sync_mssps();
                    if sub == mssp::BUF {
                        return self.mssps[usize::from(i)].read_buf(&mut self.mem);
                    }
                }
                if let Some((i, sub)) = self.uart_reg[a] {
                    self.sync_uarts();
                    if sub == uart::RCREG {
                        let before = self.flags_of(i);
                        let v = self.uarts[usize::from(i)].read_rc(&mut self.mem);
                        self.flags_after(i, before);
                        return v;
                    }
                }
                if self.wdt_reg[a] {
                    let w = self.wdt.as_ref().unwrap();
                    return if a == w.con0 {
                        self.wdt.as_mut().unwrap().armed = true; // a read of WDTCON0 arms the WWDT in every mode (`[A90]`)
                        self.mem[a]
                    } else if a == w.con1 {
                        self.mem[a]
                    } else {
                        self.wdt_value(if a == w.psl { wdt::PSL } else if a == w.psh { wdt::PSH } else { wdt::TMR })
                    };
                }
                if a == 2 {
                    self.pc as u8
                } else if a == self.a_tos || a == self.a_tos + 1 {
                    self.tos_byte(a - self.a_tos)
                } else {
                    self.mem[a]
                }
            }
        }
    }

    /// Write `v`; `alu` is true when the instruction also sets Z, DC and C, which then disables those three bits
    /// of STATUS (DS40002343F 9.3 p.50). TO and PD cannot be written.
    fn wr(&mut self, t: Tgt, v: u8, alu: bool) {
        match t {
            Tgt::Null => {}
            Tgt::Flash(_) => self.flash_cycle = true, // the write is ignored but the access still takes the cycle
            Tgt::Eeprom(_) => {
                // ignored; only reads are said to take the extra cycle (4.5.3 p.62) `[A110]`
                self.notes.insert("a write through an FSR to the EEPROM (7000h-70FFh) is ignored".into());
            }
            Tgt::Mem(a) => {
                self.note(a);
                if a == 2 {
                    self.mem[2] = v;
                    self.pc = (u32::from(self.mem[self.a_pclath] & 0x7F) << 8 | u32::from(v)) % self.words;
                    self.extra += 1;
                } else if a == self.a_status {
                    if !alu {
                        self.flags(C | DC | Z, v);
                    }
                } else if a == self.a_tos || a == self.a_tos + 1 {
                    let sp = self.mem[self.a_stkptr] & 0x1F;
                    if sp != EMPTY {
                        let i = usize::from(sp & 15);
                        let s = 8 * (a - self.a_tos);
                        self.stack[i] = (self.stack[i] & !(0xFF << s) | u16::from(v) << s) & 0x7FFF;
                    }
                } else if let Some((i, _)) = self.port_reg[a] {
                    // a write to PORTx is a write to LATx (DS40002343F 16.2 p.179, 16.14.1 p.184)
                    let p = &self.ports[usize::from(i)];
                    let v = v & self.implemented[p.lat]; // RA3 has no latch bit where it is input only
                    (self.mem[p.lat], self.mem[p.port]) = (v, v & self.implemented[p.port]);
                } else if self.wdt_reg[a] {
                    self.wdt_write(a, v);
                } else if self.pps_lock == Some(a) {
                    self.write_ppslock(v);
                } else if self.pps_reg[a] {
                    self.write_pps(a, v);
                } else if self.tmr_reg[a] {
                    self.write_tmr(a, v & self.implemented[a]);
                } else if self.a_fvr == Some(a) {
                    // RDY is read only and follows EN at once; the data sheets give no start-up time (17146: 37.2 p.569)
                    let v = v & self.implemented[a];
                    if v & 0x80 != 0 && !self.fvr_always {
                        self.notes.insert("FVR start-up time is not modelled; FVRRDY is set at once".into());
                    }
                    self.mem[a] = v & !0x40 | if self.fvr_always { 0x40 } else { (v >> 1) & 0x40 };
                } else if self.adc.as_ref().is_some_and(|x| x.con == a) {
                    self.sync_adc();
                    let (now, v) = (self.cycles, v & self.implemented[a]);
                    if let Some(x) = self.adc.as_mut() {
                        x.write_con(&mut self.mem, v, now, &mut self.notes);
                    }
                } else if let Some(k) = self.nvm_reg[a] {
                    self.sync_nvm();
                    let (now, v) = (self.cycles, v & self.implemented[a]);
                    let wrtd = self.cfg.get("WRTD") == Some(0);
                    if let Some(n) = self.nvm.as_mut() {
                        self.extra += n.write(k, v, now, &mut self.mem, &mut self.prog, wrtd, &mut self.notes);
                    }
                    self.sync_nvm();
                } else if let Some((i, sub)) = self.mssp_reg[a] {
                    self.sync_mssps();
                    let (now, v) = (self.cycles, v & self.implemented[a]);
                    let fl = self.mssps[usize::from(i)].sspif();
                    let before = self.mem[fl.0] & fl.1 != 0;
                    self.mssps[usize::from(i)].write(sub, v, now, &mut self.mem, &mut self.notes);
                    self.mssp_flag_after(fl, before);
                } else if let Some((i, sub)) = self.uart_reg[a] {
                    self.sync_uarts();
                    let before = self.flags_of(i);
                    let (now, v) = (self.cycles, v & self.implemented[a]);
                    self.uarts[usize::from(i)].write(sub, v, now, &mut self.mem, &mut self.tx);
                    self.flags_after(i, before);
                } else {
                    if self.flag_reg[a] {
                        // a flag Timer1 or the older Timer2 sets in this cycle is set before the write, so a write that
                        // clears it wins, also for CLRF and MOVWF; Timer0's and the newer Timer2's come after it (`[A110]`)
                        self.sync_flag_timers();
                    }
                    self.mem[a] = v & self.implemented[a] | self.ones[a];
                    if self.flag_reg[a] {
                        // TXIF and RCIF follow the EUSART's state and cannot be cleared by software (DS40002343F 32.1.1.4
                        // p.456): they are back at once, not as a new request
                        for u in &self.uarts {
                            if u.flag.addr == a || u.rcflag.addr == a {
                                u.bits(&mut self.mem);
                            }
                        }
                    }
                    if self.flag_reg[a] && self.irq_new && self.irq_new_by.iter().all(|&(r, b)| self.mem[r] & b == 0) {
                        self.irq_new = false;
                    }
                }
            }
        }
    }

    // ----- ports and PPS ---------------------------------------------------------------------------------------

    /// Levels of the pins of port `i` now. An output pin shows its latch, an input pin what the outside world drives (0
    /// until a stimulus says otherwise `[A16]`) and 0 when ANSELx makes it analog (DS40002343F 16.5: the digital input is
    /// off). A pin whose RxyPPS selects a source other than LATxy still shows its latch `[A120]`.
    fn port_levels(&self, i: usize) -> u8 {
        self.port_levels_at(i, self.cycles)
    }

    /// `port_levels` with the outside world as it was driving at cycle `t` (the registers are the current ones).
    fn port_levels_at(&self, i: usize, t: u64) -> u8 {
        let p = &self.ports[i];
        let mut ext = 0u8;
        for &(at, pi, mask, val) in &self.drive {
            if at <= t && pi == i {
                ext = ext & !mask | val & mask;
            }
        }
        // the MCLR pin has no analog function
        let m = self.mclr_pin.filter(|&(k, _)| k == i).map_or(0, |(_, m)| m);
        let tris = self.tris_of(i);
        let ana = p.ansel.map_or(0, |a| self.mem[a]) & !m;
        let mclr_high = if self.cfg.get("MCLRE") == Some(1) { m } else { 0 }; // reads 1 while MCLR is enabled (DS40002343F 16.13 p.182)
        let v = self.mem[p.lat] & !tris | (ext | mclr_high) & tris & !ana;
        // The OLED's bus lines: an external pull-up on a released pin, held low while the OLED acknowledges.
        let v = v | self.pullup.get(i).copied().unwrap_or(0) & tris;
        (v & !self.forced.get(i).copied().unwrap_or(0)) & self.implemented[p.port]
    }

    /// TRISx of port `i` as the pins see it: a pin without a TRISx bit and the MCLR pin are always inputs.
    fn tris_of(&self, i: usize) -> u8 {
        let p = &self.ports[i];
        let m = self.mclr_pin.filter(|&(k, _)| k == i).map_or(0, |(_, m)| m);
        self.mem[p.tris] | self.implemented[p.port] & !self.implemented[p.tris] | m
    }

    /// Every pin of the part, for a pin chooser.
    pub fn pins(&self) -> Vec<Pin> {
        self.ports.iter().flat_map(|q| (0..8u8).filter(move |b| self.implemented[q.port] >> b & 1 != 0).map(move |b| (q.letter, b))).collect()
    }

    /// Replace the parts on the board (their states start at power-on); an OLED on a hardware I2C bus is registered on
    /// that MSSP as a client.
    pub fn set_board(&mut self, b: Board) {
        self.board = b.fresh();
        self.board_wiring();
    }

    /// Call after the pins or addresses in `board` were edited.
    pub fn board_wiring(&mut self) {
        for s in &mut self.mssps {
            s.i2c.retain(|c| c.tag == 0);
        }
        self.pullup = vec![0; self.ports.len()];
        self.forced = vec![0; self.ports.len()];
        for (k, o) in self.board.oleds.iter().enumerate() {
            match o.bus {
                Bus::I2c(n) => {
                    let c = crate::periph::I2cClient { address: o.address, fill: 0xFF, tag: k as u8 + 1, ..Default::default() }; // a read finds SDA undriven: 0xFF
                    if let Some(s) = self.mssps.iter_mut().find(|s| s.num == n) {
                        s.i2c.push(c);
                    }
                }
                Bus::Gpio => {
                    for p in [o.scl, o.sda] {
                        if let Some(i) = self.ports.iter().position(|q| q.letter == p.0) {
                            self.pullup[i] |= 1 << p.1;
                        }
                    }
                }
            }
        }
        self.board.set_log_pos(self.i2c_log.len());
    }

    /// Is the OLED `o` on MSSP `num` wired as the code expects: the input selections name its pins and its pins' output
    /// selections are SCL1 and SDA1? The reason when not.
    fn hw_wiring(&self, o: &crate::periph::Oled, num: u8) -> Result<(), String> {
        let Some(&(_, scl_in, sda_in, codes)) = self.i2c_pps.iter().find(|p| p.0 == num) else {
            return Err(format!("the part has no I2C host {num}"));
        };
        let Some((scl_code, sda_code)) = codes else {
            return Err("the simulator does not know this family's RxyPPS output codes for the MSSP".into());
        };
        for (name, inreg, code, pin) in [("SCL", scl_in, scl_code, o.scl), ("SDA", sda_in, sda_code, o.sda)] {
            let v = self.mem[inreg];
            let sel: Pin = (b'A' + (v >> 3 & 7), v & 7);
            if sel != pin {
                return Err(format!("SSP{num}{}PPS selects {} but the OLED {name} is on {}", if name == "SCL" { "CLK" } else { "DAT" }, pin_name(sel), pin_name(pin)));
            }
            let out = self.pps_out.iter().find(|&&(_, pi, bit)| self.ports[pi].letter == pin.0 && bit == pin.1);
            match out {
                Some(&(reg, ..)) if self.mem[reg] & 0x3F == code => {}
                Some(&(reg, ..)) => return Err(format!("{}PPS is {:#04x}, the {name} output code is {code:#04x}", pin_name(pin), self.mem[reg] & 0x3F)),
                None => return Err(format!("{} has no output selection (RxyPPS)", pin_name(pin))),
            }
        }
        Ok(())
    }

    /// Update the parts after an instruction ended at the current cycle.
    fn board_poll(&mut self) {
        if self.board.is_empty() {
            return;
        }
        self.sync_mssps();
        let now = self.cycles;
        let mut b = std::mem::take(&mut self.board);
        let mut notes = BTreeSet::new();
        let held = b.poll_pins(&*self, now, &mut notes);
        let mut hw = Vec::new();
        for (k, o) in b.oleds.iter_mut().enumerate() {
            if let Bus::I2c(n) = o.bus {
                let w = self.hw_wiring(o, n);
                o.status = w.clone().err().unwrap_or_default();
                hw.push((k, n, w.is_ok()));
            }
        }
        if !b.oleds.is_empty() {
            let mut forced = vec![0u8; self.ports.len()];
            for pin in held {
                if let Some(i) = self.ports.iter().position(|q| q.letter == pin.0) {
                    forced[i] |= 1 << pin.1;
                }
            }
            self.forced = forced;
        }
        for s in &mut self.mssps {
            for c in s.i2c.iter_mut().filter(|c| c.tag != 0) {
                c.absent = !hw.iter().any(|&(k, n, ok)| k + 1 == usize::from(c.tag) && n == s.num && ok);
            }
        }
        b.feed_log(&self.i2c_log, &mut notes);
        self.notes.extend(notes);
        self.board = b;
    }

    /// The pin level an input selection register (PORT field at bits 5:3, PIN at 2:0; the PIC16F183xx has 5 bits and the
    /// same layout, DS40002343F Register 18-1, DS40001839E Register 13-1) selects. `None` when it names no pin of the part.
    pub fn pps_input(&self, reg: usize) -> Option<bool> {
        self.pps_input_at(reg, self.cycles)
    }

    fn pps_input_at(&self, reg: usize, t: u64) -> Option<bool> {
        let v = self.mem[reg];
        let i = self.ports.iter().position(|p| p.letter == b'A' + (v >> 3 & 7))?;
        Some(self.port_levels_at(i, t) >> (v & 7) & 1 != 0)
    }

    /// Interrupt-on-change and the INT pin `[A125]`: the changes the stimulus made to the pins since the last look (up to
    /// the current cycle, also while asleep) set the IOCxF bits whose edge is enabled, and INTF on the INTEDG edge of the
    /// INTPPS pin, whatever the enable bits say; IOCIF follows the IOCxF registers (read only).
    fn ioc_scan(&mut self) {
        let (from, now) = (self.ioc_at, self.cycles);
        self.ioc_at = now;
        if self.ioc.is_empty() && self.int_pin.is_none() {
            return;
        }
        let mut times: Vec<u64> = self.drive.iter().map(|d| d.0).filter(|&t| t > from && t <= now).collect();
        times.sort_unstable();
        times.dedup();
        // With MCLRE = 1 or LVP = 1 the MCLR pin has no port function and no IOC (Register 17.7.1 Important)
        let mclr_off = self.cfg.get("MCLRE") == Some(1) || self.cfg.get("LVP") == Some(1);
        for t in times {
            for k in 0..self.ioc.len() {
                let (pi, f, n, pp) = self.ioc[k];
                let off = self.mclr_pin.filter(|&(q, _)| q == pi && mclr_off).map_or(0, |(_, m)| m);
                let (old, new) = (self.port_levels_at(pi, t - 1), self.port_levels_at(pi, t));
                let hit = (old ^ new) & (new & self.mem[pp] | !new & self.mem[n]) & !off & self.implemented[f];
                if hit != 0 {
                    self.mem[f] |= hit;
                }
            }
            if let Some((pps, flag, ea, em)) = self.int_pin {
                if let (Some(old), Some(new)) = (self.pps_input_at(pps, t - 1), self.pps_input_at(pps, t)) {
                    if old != new && new == (self.mem[ea] & em != 0) {
                        self.raise_flag(flag);
                    }
                }
            }
        }
        if let Some((r, b)) = self.iocif {
            if self.ioc.iter().any(|&(_, f, ..)| self.mem[f] != 0) {
                self.raise_flag((r, b));
            } else {
                self.mem[r] &= !b;
            }
        }
    }

    /// A write to PPSLOCK. 55h then AAh (consecutive writes to PPSLOCK) arm the next write, whose bit 0 sets or clears
    /// PPSLOCKED; any other write is ignored and starts over `[A121]`. With PPS1WAY set the bit cannot be cleared again
    /// (DS40002343F 18.5.1 p.204).
    fn write_ppslock(&mut self, v: u8) {
        let a = self.pps_lock.unwrap();
        self.pps_seq = match (self.pps_seq, v) {
            (0, 0x55) => 1,
            (1, 0xAA) => 2,
            (2, 0x55) => 1, // not a bit write: the sequence starts over
            (2, _) => {
                let one_way = self.cfg.get("PPS1WAY") == Some(1);
                if v & 1 != 0 || !(one_way && self.mem[a] & 1 != 0) {
                    self.mem[a] = v & 1;
                } else {
                    self.notes.insert("PPS1WAY is set: PPSLOCKED cannot be cleared again until a Reset; the write is ignored".into());
                }
                0
            }
            (_, 0x55) => 1,
            _ => 0,
        };
    }

    /// A write to a PPS selection register: ignored while PPSLOCKED is set (Register 18-3: "Writes to any PPS register are
    /// ignored"). An output source other than LATxy is not modelled on the PIC16 parts yet.
    fn write_pps(&mut self, a: usize, v: u8) {
        if self.pps_lock.is_some_and(|l| self.mem[l] & 1 != 0) {
            self.notes.insert("a write to a PPS selection register while PPSLOCKED is set is ignored".into());
            return;
        }
        let v = v & self.implemented[a];
        self.mem[a] = v;
        if v != 0 && self.pps_out.iter().any(|o| o.0 == a) {
            self.notes.insert("a RxyPPS output source other than LATxy is not modelled on PIC16 parts; the pin shows its latch".into());
        }
    }

    pub fn has_port(&self, letter: u8) -> bool {
        self.ports.iter().any(|p| p.letter == letter)
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

    /// Bring Timer0 to the current cycle: the clock ticks of cycles `last+1 ..= cycles`.
    fn sync_t0(&mut self) {
        let now = self.cycles;
        let Some(t0) = self.t0.as_ref() else { return };
        let (last, cki) = (t0.last, t0.cki);
        let (c0, c1) = (self.mem[t0.c0], self.mem[t0.c1]);
        let dt = now.saturating_sub(last);
        self.t0.as_mut().unwrap().last = now;
        if c0 & 0x80 == 0 || dt == 0 {
            return;
        }
        // the clock ticks of cycles last+1 ..= now: Fosc/4 cycles, or the edges of the T0CKIPPS pin that the stimulus makes (CS = 000
        // counts rising edges, 001 the pin inverted: falling ones; the synchroniser of ASYNC = 0 adds no delay `[A127]`)
        let ticks = match c1 >> 5 {
            2 => dt,
            cs @ (0 | 1) => match cki {
                Some(r) => self.pin_edges(r, last, now, cs == 0),
                None => 0,
            },
            cs => {
                self.notes.insert(format!("Timer0 clock source CS={cs} is not modelled (Fosc/4 and the T0CKIPPS pin are); it does not count"));
                return;
            }
        };
        let t = self.t0.as_mut().unwrap();
        let sh = u32::from(c1 & 15);
        let mut raised = false;
        let total = t.pre + ticks;
        t.pre = total & ((1u64 << sh) - 1);
        for _ in 0..total >> sh {
            let wrap = if c0 & 0x10 != 0 {
                t.cnt = t.cnt.wrapping_add(1);
                t.cnt == 0
            } else if t.cnt as u8 == self.mem[t.h] {
                t.cnt = 0;
                true
            } else {
                t.cnt += 1;
                false
            };
            if wrap {
                if t.post >= c0 & 15 {
                    t.post = 0;
                    self.mem[t.c0] ^= 0x20; // T0OUT toggles
                    raised = true;
                } else {
                    t.post += 1;
                }
            }
        }
        self.mem[t.l] = t.cnt as u8;
        let f = t.flag;
        if raised {
            self.raise_flag(f);
        }
    }

    /// Bring every timer to the current cycle.
    fn sync_timers(&mut self) {
        self.sync_uarts();
        self.sync_mssps();
        self.sync_adc();
        self.sync_nvm();
        self.sync_t0();
        for i in 0..self.t1.len() {
            self.sync_t1(i);
        }
        for i in 0..self.t2.len() {
            self.sync_t2(i);
        }
    }

    /// Bring the timers whose flags land before a PIRn access in the same cycle to the current cycle: Timer1/3/5 and
    /// the older Timer2/4/6 (`flag_reg`, `[A110]`).
    fn sync_flag_timers(&mut self) {
        self.sync_uarts();
        self.sync_mssps();
        self.sync_adc();
        self.sync_nvm();
        for i in 0..self.t1.len() {
            self.sync_t1(i);
        }
        for i in 0..self.t2.len() {
            if self.t2[i].clk.is_none() {
                self.sync_t2(i);
            }
        }
    }

    /// The TXIF and RCIF bits of EUSART `i` as they are now.
    fn flags_of(&self, i: u8) -> [bool; 2] {
        let u = &self.uarts[usize::from(i)];
        [self.mem[u.flag.addr] & u.flag.mask != 0, self.mem[u.rcflag.addr] & u.rcflag.mask != 0]
    }

    /// A flag of EUSART `i` that rose since `before` is raised through `raise_flag`, so that it counts as a request
    /// that arrived while the program was running.
    fn flags_after(&mut self, i: u8, before: [bool; 2]) {
        let u = &self.uarts[usize::from(i)];
        let fl = [(u.flag.addr, u.flag.mask), (u.rcflag.addr, u.rcflag.mask)];
        for (k, &(r, b)) in fl.iter().enumerate() {
            if !before[k] && self.mem[r] & b != 0 {
                self.mem[r] &= !b;
                self.raise_flag((r, b));
            }
        }
    }

    /// Bring the EUSARTs to the current cycle: the transmitter shifts out, the receiver takes in the characters of the
    /// stimulus, TXIF, TRMT, RCIF and the error bits follow. A part in Sleep has no clock for them: characters that
    /// arrive then are lost (`[A94]`).
    fn sync_uarts(&mut self) {
        let mut ev = Vec::new();
        for i in 0..self.uarts.len() {
            let before = self.flags_of(i as u8);
            self.uarts[i].sync(&mut self.mem, self.cycles, &mut ev, &mut self.tx, &mut self.notes);
            self.flags_after(i as u8, before);
        }
    }

    /// Bring the MSSPs to the current cycle (steps of an I2C or SPI operation that are due).
    fn sync_mssps(&mut self) {
        let mut ev = Vec::new();
        for i in 0..self.mssps.len() {
            let fl = self.mssps[i].sspif();
            let before = self.mem[fl.0] & fl.1 != 0;
            self.mssps[i].sync(&mut self.mem, self.cycles, &mut ev, &mut self.spi_log, &mut self.i2c_log, &mut self.notes);
            self.mssp_flag_after(fl, before);
        }
    }

    /// SSPxIF that rose since `before` is raised through `raise_flag`, so that it counts as a request that arrived
    /// while the program was running.
    fn mssp_flag_after(&mut self, fl: (usize, u8), before: bool) {
        if !before && self.mem[fl.0] & fl.1 != 0 {
            self.mem[fl.0] &= !fl.1;
            self.raise_flag(fl);
        }
    }

    /// Bring the ADC to the current cycle: a conversion that has ended loads ADRES, clears GO and raises ADIF (through
    /// `raise_flag`, so it is a request arriving while the program runs, `[A108]`).
    fn sync_adc(&mut self) {
        let Some(a) = self.adc.as_mut() else { return };
        let (flag, mut ev) = ((a.flag.addr, a.flag.mask), Vec::new());
        if a.sync(&mut self.mem, self.cycles, &mut ev) {
            self.raise_flag(flag);
        }
    }

    /// The data EEPROM as it is now (empty on a part without one).
    pub fn eeprom(&self) -> &[u8] {
        self.nvm.as_ref().map_or(&[], |n| &n.eeprom)
    }

    /// Bring the data EEPROM to the current cycle: a write that has ended clears WR and raises NVMIF.
    fn sync_nvm(&mut self) {
        let Some(n) = self.nvm.as_mut() else { return };
        let flag = (n.flag.addr, n.flag.mask);
        if n.sync(&mut self.mem, self.cycles) {
            self.raise_flag(flag);
        }
    }

    /// The part has an ADC the simulator models.
    pub fn has_adc(&self) -> bool {
        self.adc.is_some()
    }

    /// Stimulus: the input of ADC channel `ch` is code `v` (full scale = the part's resolution) from cycle `at` on;
    /// false when the part has no ADC in the simulator.
    pub fn adc_feed(&mut self, ch: u8, at: u64, v: u16) -> bool {
        match self.adc.as_mut() {
            Some(a) => {
                a.push_feed(ch, at, v);
                true
            }
            None => false,
        }
    }

    /// The part has MSSP `num` (its I2C host mode is modelled).
    pub fn has_mssp(&self, num: u8) -> bool {
        self.mssps.iter().any(|q| q.num == num)
    }

    /// A register read as the program sees it (for tests and tools): the models are brought to the current cycle first.
    pub fn read(&mut self, a: u32) -> u8 {
        self.rd(Tgt::Mem(a as usize))
    }

    /// A register written as the program would (for tests and tools).
    pub fn write(&mut self, a: u32, v: u8) {
        self.wr(Tgt::Mem(a as usize), v, false);
    }

    /// The part has EUSART `num`.
    pub fn has_uart(&self, num: u8) -> bool {
        self.uarts.iter().any(|u| u.num == num)
    }

    /// Queue characters for the receiver of EUSART `num`; false when the part has no such EUSART.
    pub fn rx_send(&mut self, num: u8, s: RxStream) -> bool {
        match self.uarts.iter_mut().find(|u| u.num == num) {
            Some(u) => {
                u.rx_in.push_back(s);
                true
            }
            None => false,
        }
    }

    /// A peripheral sets interrupt flag `(register, bit)`. Every peripheral sets its flag through here: a flag that
    /// makes a request pending is a request arriving while the program runs (`irq_new`, `[A108]`), also when software
    /// cleared it earlier in the same step.
    fn raise_flag(&mut self, (r, b): (usize, u8)) {
        if self.mem[r] & b != 0 {
            return;
        }
        let before = self.enabled_flag();
        self.mem[r] |= b;
        if !before && self.enabled_flag() {
            self.irq_new = true;
            self.irq_new_by.push((r, b));
        }
    }

    /// Sleep: Fosc/4 and Fosc stop, so no timer counts and none owes ticks when the part wakes.
    fn park_timers(&mut self) {
        let now = self.cycles;
        // Timer0 with ASYNC = 1 and Timer1 with SYNC = 1 on their clock pins go on in Sleep and wake the part (DS40002343F 22.4
        // p.246, 23.5 p.255, `[A127]`); the others have no clock
        if self.t0_pin_async() {
            self.sync_t0();
        } else if let Some(t) = self.t0.as_mut() {
            t.last = now;
        }
        // `max` keeps the one-cycle start delay of a timer switched on just before SLEEP (`[A110]`)
        for i in 0..self.t1.len() {
            if self.t1_pin_async(i) {
                self.sync_t1(i);
            } else {
                self.t1[i].last = self.t1[i].last.max(now);
            }
        }
        for t in &mut self.t2 {
            t.last = t.last.max(now);
        }
    }

    /// Timer0 is on, takes its clock from the T0CKIPPS pin and is not synchronised to Fosc/4 (ASYNC = 1).
    fn t0_pin_async(&self) -> bool {
        self.t0.as_ref().is_some_and(|t| self.mem[t.c0] & 0x80 != 0 && self.mem[t.c1] >> 5 < 2 && self.mem[t.c1] & 0x10 != 0 && t.cki.is_some())
    }

    /// Timer1-type timer `i` is on, takes its clock from the TxCKIPPS pin and is not synchronised (SYNC = 1).
    fn t1_pin_async(&self, i: usize) -> bool {
        let t = &self.t1[i];
        let con = self.mem[t.con];
        let pin = match t.clk {
            Some(c) => self.mem[c] & 0x1F == 0,
            None => con >> 6 == 2 && con & 8 == 0,
        };
        con & 1 != 0 && con & 4 != 0 && pin && t.cki.is_some()
    }

    /// Timer1-type timer `i`: the Fosc/4 (CS = 1) or Fosc (CS = 2, four ticks a cycle) clock ticks of cycles `last+1 ..= cycles`,
    /// or the rising edges of the TxCKIPPS pin (CS = 0; on the PIC16F183xx TMRxCS = 10 with TxSOSC = 0), counted only while the
    /// gate lets the timer count when TMRxGE = 1 and the gate source is the TxGPPS pin `[A127]`.
    fn sync_t1(&mut self, i: usize) {
        let now = self.cycles;
        let (con_a, clk_a, gcon_a, gate_a, cki, gpps, last) = {
            let t = &self.t1[i];
            (t.con, t.clk, t.gcon, t.gate, t.cki, t.gpps, t.last)
        };
        let dt = now.saturating_sub(last);
        self.t1[i].last = last.max(now); // `last` is one cycle ahead just after TMRxON is set (`[A110]`)
        let con = self.mem[con_a];
        // the clock: Fosc/4 ticks per cycle (1 for Fosc/4, 4 for Fosc), a pin, or a source that is not modelled
        let (cs, src) = match clk_a {
            Some(c) => {
                let cs = self.mem[c] & 0x1F;
                (cs, match cs {
                    1 => Src::Cycles(1),
                    2 => Src::Cycles(4),
                    0 => cki.map_or(Src::None, Src::Pin),
                    _ => Src::None,
                })
            }
            None => (con >> 6, match con >> 6 {
                0 => Src::Cycles(1),
                1 => Src::Cycles(4),
                2 if con & 8 == 0 => cki.map_or(Src::None, Src::Pin),
                _ => Src::None,
            }),
        };
        let g = self.mem[gcon_a];
        let n = self.t1[i].n;
        let mut raised = false;
        'run: {
            if con & 1 == 0 || dt == 0 {
                break 'run;
            }
            let mut gate = None;
            if g & 0x80 != 0 {
                // gate enable mode on the gate pin; toggle, single-pulse and the other gate sources hold the count (noted)
                let pin_src = gate_a.map_or(g & 3 == 0, |r| self.mem[r] & 0x1F == 0);
                match gpps {
                    Some(r) if pin_src && g & 0x30 == 0 => gate = Some((r, g & 0x40 != 0)),
                    _ => {
                        self.notes.insert(format!("Timer{n} gate: only the gate enable mode on the TxGPPS pin is modelled (not toggle, single pulse, or another source); the timer holds its count"));
                        break 'run;
                    }
                }
            }
            if matches!(src, Src::None) {
                self.notes.insert(format!("Timer{n} clock source CS={cs} is not modelled (Fosc/4, Fosc and the TxCKIPPS pin are); it does not count"));
                break 'run;
            }
            let ticks = self.t1_ticks(&src, gate, last, now);
            let t = &mut self.t1[i];
            let sh = u32::from((con >> 4) & 3);
            let total = t.pre + ticks;
            t.pre = total & ((1u64 << sh) - 1);
            let sum = u64::from(t.cnt) + (total >> sh);
            raised = sum >= 0x10000;
            t.cnt = sum as u16;
        }
        let t = &self.t1[i];
        let cnt = t.cnt;
        let (h, l, f) = (t.h, t.l, t.flag);
        if con & 2 == 0 {
            self.mem[h] = (cnt >> 8) as u8;
        }
        self.mem[l] = cnt as u8;
        if raised {
            self.raise_flag(f);
        }
    }

    /// Changes of the level of the pin the input selection register `reg` selects that the stimulus makes in cycles `from+1 ..= to`:
    /// (cycle, new level). A pin change made by the program or a peripheral is not seen (`[A125]`).
    fn pin_changes(&self, reg: usize, from: u64, to: u64) -> Vec<(u64, bool)> {
        let mut times: Vec<u64> = self.drive.iter().map(|d| d.0).filter(|&t| t > from && t <= to).collect();
        times.sort_unstable();
        times.dedup();
        times
            .into_iter()
            .filter_map(|t| {
                let (old, new) = (self.pps_input_at(reg, t - 1)?, self.pps_input_at(reg, t)?);
                (old != new).then_some((t, new))
            })
            .collect()
    }

    /// Rising (or falling) edges of the pin of `reg` in cycles `from+1 ..= to`.
    fn pin_edges(&self, reg: usize, from: u64, to: u64, rising: bool) -> u64 {
        self.pin_changes(reg, from, to).iter().filter(|c| c.1 == rising).count() as u64
    }

    /// The clock ticks `src` makes in cycles `from+1 ..= to` while the gate (the level of the pin of the register, active high or
    /// low) is open; no gate: all of them.
    fn t1_ticks(&self, src: &Src, gate: Option<(usize, bool)>, from: u64, to: u64) -> u64 {
        // the gate pin's changes cut the interval into stretches of one level: stretch (a, b] ends one cycle before a change
        let mut cuts = vec![from];
        if let Some((r, _)) = gate {
            cuts.extend(self.pin_changes(r, from, to).into_iter().map(|c| c.0 - 1));
        }
        cuts.push(to);
        let mut total = 0;
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b <= a || gate.is_some_and(|(r, hi)| self.pps_input_at(r, b) != Some(hi)) {
                continue;
            }
            total += match *src {
                Src::Cycles(m) => m * (b - a),
                Src::Pin(r) => self.pin_edges(r, a, b, true),
                Src::None => 0,
            };
        }
        total
    }

    /// Timer2-type timer `i`: the clock ticks of cycles `last+1 ..= cycles`.
    fn sync_t2(&mut self, i: usize) {
        let now = self.cycles;
        let t = &mut self.t2[i];
        let dt = now.saturating_sub(t.last);
        t.last = t.last.max(now); // one cycle ahead just after TMR2ON is set (`[A110]`)
        let con = self.mem[t.con];
        let mut raised = false;
        // (on, clock source, prescaler shift, OUTPS): T2CON of the newer parts, or the PIC16F183xx's (Fosc/4 only)
        let (on, cs, sh, outps) = match t.clk {
            Some(c) => (con & 0x80 != 0, self.mem[c] & 15, u32::from((con >> 4) & 7), con & 15),
            None => (con & 4 != 0, 1, 2 * u32::from(con & 3), (con >> 3) & 15),
        };
        if on && dt > 0 {
            let mode = t.hlt.map_or(0, |h| self.mem[h] & 0x1F);
            if cs != 1 && cs != 2 {
                self.notes.insert(format!("Timer{} clock source CS={cs} is not modelled (only Fosc/4 and Fosc); it does not count", t.n));
            } else {
                if mode != 0 && mode != 8 {
                    self.notes.insert(format!("Timer{} MODE={mode:#04x} needs an external reset/gate source; run as the software mode", t.n));
                }
                let one_shot = mode >> 3 == 1;
                let total = t.pre + if cs == 2 { 4 * dt } else { dt };
                t.pre = total & ((1u64 << sh) - 1);
                let mut left = total >> sh;
                let pr = self.mem[t.pr];
                while left > 0 {
                    // Jump straight to the next period end when no event can happen before it.
                    if t.cnt < pr && left > 1 {
                        let step = u64::from(pr - t.cnt).min(left - 1);
                        t.cnt += step as u8;
                        left -= step;
                        continue;
                    }
                    left -= 1;
                    if t.cnt == pr {
                        t.cnt = 0;
                        if one_shot {
                            self.mem[t.con] &= if t.clk.is_some() { !0x80 } else { !0x04 }; // the match clears ON (24.1.2)
                            t.pre = 0;
                            t.post = 0;
                            raised = true;
                            break;
                        }
                        if t.post >= outps {
                            t.post = 0;
                            raised = true;
                        } else {
                            t.post += 1;
                        }
                    } else {
                        t.cnt = t.cnt.wrapping_add(1);
                    }
                }
            }
        }
        self.mem[t.tmr] = t.cnt;
        let f = t.flag;
        if raised {
            self.raise_flag(f);
        }
    }

    /// A read of a timer register: the timers are brought to the reading cycle first (`[A2]`). Reading TMRxL loads the
    /// TMRxH buffer when RD16 is set (23.6).
    fn read_tmr(&mut self, a: usize) -> u8 {
        self.sync_timers();
        if let Some(t) = &self.t0
            && a == t.l
        {
            if self.mem[t.c0] & 0x10 != 0 {
                self.mem[t.h] = (t.cnt >> 8) as u8;
            }
            return t.cnt as u8;
        }
        for t in &self.t1 {
            if a == t.l {
                if self.mem[t.con] & 2 != 0 {
                    self.mem[t.h] = (t.cnt >> 8) as u8;
                }
                return t.cnt as u8;
            }
            if a == t.h && self.mem[t.con] & 2 == 0 {
                return (t.cnt >> 8) as u8;
            }
        }
        self.mem[a]
    }

    /// A write to a timer register; `v` is already masked to the implemented bits.
    fn write_tmr(&mut self, a: usize, v: u8) {
        self.sync_timers();
        if self.t0.as_ref().is_some_and(|t| [t.l, t.h, t.c0, t.c1].contains(&a)) {
            return self.write_t0(a, v);
        }
        let now = self.cycles;
        for t in &mut self.t1 {
            if a == t.l {
                t.pre = 0;
                t.cnt = if self.mem[t.con] & 2 != 0 { u16::from(self.mem[t.h]) << 8 | u16::from(v) } else { t.cnt & 0xFF00 | u16::from(v) };
            } else if a == t.h && self.mem[t.con] & 2 == 0 {
                // A write to TMRx clears the prescaler (DS40002343F 23.3 p.255, DS40001839E 27.3 p.286); with RD16 set
                // the write goes to the buffer and only a TMRxL write does (23.6) `[A110]`
                t.cnt = t.cnt & 0xFF | u16::from(v) << 8;
                t.pre = 0;
            } else if a == t.con && self.mem[a] & 1 == 0 && v & 1 != 0 {
                // TMRxON 0 -> 1: the first clock is counted one cycle later than for a running timer (MDB, `[A110]`)
                t.last = now + 1;
            }
            if a == t.gcon {
                self.mem[a] = v & !4 | self.mem[a] & 4; // GVAL is read only
                return;
            }
        }
        for t in &mut self.t2 {
            if a == t.tmr {
                (t.cnt, t.pre, t.post) = (v, 0, 0);
            } else if a == t.con {
                (t.pre, t.post) = (0, 0);
                // the PIC16F183xx's TMR2ON 0 -> 1: the first clock is one cycle later, as for Timer1 (MDB, `[A110]`);
                // not measured on the newer Timer2
                if t.clk.is_none() && self.mem[a] & 4 == 0 && v & 4 != 0 {
                    t.last = now + 1;
                }
            }
        }
        self.mem[a] = v;
    }

    /// A write to a Timer0 register takes effect after the cycle of the writing instruction `[A2]`.
    fn write_t0(&mut self, a: usize, v: u8) {
        self.sync_t0();
        let t = self.t0.as_mut().unwrap();
        if a == t.l {
            t.pre = 0;
            t.post = 0;
            t.cnt = if self.mem[t.c0] & 0x10 != 0 { u16::from(self.mem[t.h]) << 8 | u16::from(v) } else { u16::from(v) };
            self.mem[a] = v;
        } else if a == t.h {
            self.mem[a] = v;
        } else if a == t.c0 {
            t.pre = 0;
            t.post = 0;
            // T0OUT is read only; it reads 0 while EN = 0 (MDB, `[A109]`; the data sheet does not say)
            self.mem[a] = if v & 0x80 == 0 { v & !0x20 } else { v & !0x20 | self.mem[a] & 0x20 };
        } else {
            t.pre = 0;
            t.post = 0;
            self.mem[a] = v;
        }
    }

    fn note(&mut self, a: usize) {
        if self.implemented[a] == 0 && !self.is_sfr[a] {
            self.stray.insert(a as u32);
        } else if self.is_sfr[a] && !self.core[a]
            && let Some(n) = &self.names[a]
        {
            self.unmodelled.insert(n.clone());
        }
    }

    /// A register as a display shows it: what the CPU would read, without side effects.
    pub fn peek(&self, a: u32) -> u8 {
        let i = Pic16::phys(a);
        if i == 2 {
            self.pc as u8
        } else if i == self.a_tos || i == self.a_tos + 1 {
            self.tos_byte(i - self.a_tos)
        } else if i < 2 {
            match self.target_of(self.fsr(i)) {
                Tgt::Mem(m) => self.mem[m],
                Tgt::Flash(w) => self.flash_word(w),
                Tgt::Eeprom(n) => self.ee_byte(n),
                Tgt::Null => 0,
            }
        } else if let Some((p, true)) = self.port_reg[i] {
            self.port_levels(usize::from(p))
        } else {
            self.mem[i]
        }
    }

    // ----- hardware stack --------------------------------------------------------------------------------------

    fn stvren(&self) -> bool {
        self.cfg.get("STVREN") != Some(0)
    }

    /// TOSL (k = 0) or TOSH (k = 1): the top entry; with an empty stack 0 if the stack Reset is enabled and the
    /// contents of entry 0Fh if not (DS40002343F Fig. 9-14 p.53).
    fn tos_byte(&self, k: usize) -> u8 {
        let sp = self.mem[self.a_stkptr] & 0x1F;
        let v = if sp == EMPTY {
            if self.stvren() { 0 } else { self.stack[15] }
        } else {
            self.stack[usize::from(sp & 15)]
        };
        (v >> (8 * k)) as u8
    }

    /// Stack depth for the front ends: 0 when empty.
    pub fn depth(&self) -> u32 {
        let sp = self.mem[self.a_stkptr] & 0x1F;
        if sp == EMPTY { 0 } else { u32::from(sp) + 1 }
    }

    pub fn tos(&self) -> u32 {
        u32::from(self.tos_byte(1)) << 8 | u32::from(self.tos_byte(0))
    }

    /// Entry `i` of the hardware return stack (0 is the oldest; `depth()` entries are in use), for the front ends.
    pub fn stack_entry(&self, i: usize) -> u32 {
        u32::from(self.stack[i & 15])
    }

    fn set_pcon(&mut self, bits: u8) {
        if let Some(a) = self.a_pcon0 {
            self.mem[a] |= bits;
        }
    }

    /// CALL, CALLW and an interrupt: the stack pointer goes up, then the entry is written. The 17th push (pointer 10h)
    /// sets STKOVF and wraps onto entry 0, or resets the part when STVREN = 1 (9.5 p.52, Fig. 9-17 p.55).
    fn push(&mut self, v: u32) {
        let sp = self.mem[self.a_stkptr] & 0x1F;
        if (0x0F..EMPTY).contains(&sp) {
            self.set_pcon(0x80);
            if self.stvren() {
                self.stk_fault = Some(Reset::StackOverflow);
                return;
            }
        }
        if (0x0F..EMPTY).contains(&sp) {
            self.notes.insert("stack overflow with STVREN = 0: the data sheet describes only the 17th push; further pushes keep counting STKPTR, which reaches the empty marker 1Fh".into());
        }
        let n = (sp + 1) & 0x1F;
        self.stack[usize::from(n & 15)] = (v & 0x7FFF) as u16;
        self.mem[self.a_stkptr] = n;
    }

    /// RETURN, RETLW and RETFIE. A pop of an empty stack sets STKUNF and resets the part when STVREN = 1; otherwise
    /// the model returns entry 0Fh and leaves the pointer alone (the data sheet does not say what the pointer does).
    fn pop(&mut self) -> u32 {
        let sp = self.mem[self.a_stkptr] & 0x1F;
        if sp == EMPTY {
            self.set_pcon(0x40);
            if self.stvren() {
                self.stk_fault = Some(Reset::StackUnderflow);
                return 0;
            }
            self.notes.insert("stack underflow with STVREN = 0: the stack pointer after it is not documented".into());
            return u32::from(self.stack[15]);
        }
        self.mem[self.a_stkptr] = (sp.wrapping_sub(1)) & 0x1F;
        u32::from(self.stack[usize::from(sp & 15)])
    }

    // ----- resets ----------------------------------------------------------------------------------------------

    /// A reset other than power-on: SFRs take their MCLR-column value (bits marked u, q, x or - are left alone), the
    /// PC is cleared and the stack is empty. Peripherals do not exist yet.
    fn do_reset(&mut self, kind: Reset) {
        self.sync_nvm(); // a write that ended before the Reset is done, not lost
        if kind == Reset::Software
            && let Some(a) = self.a_pcon0
        {
            self.mem[a] &= !0x04; // nRI = 0 (DS40002343F RESET p.603)
        }
        for i in 0..self.mclr.len() {
            let (a, set, clr) = self.mclr[i];
            self.mem[a] = (self.mem[a] & !clr | set) & self.implemented[a] | self.ones[a];
        }
        // Table 10-4 (DS40002343F p.76): a watchdog time-out clears TO and RWDT; as on the Q10 the table, not the prose, decides
        // (`[A91]`): a window violation only clears WDTWV.
        match kind {
            Reset::Wdt => {
                self.flags(NTO, 0);
                if let Some(a) = self.a_pcon0 {
                    self.mem[a] &= !0x10;
                }
            }
            Reset::WdtWindow => {
                if let Some(a) = self.a_pcon0 {
                    self.mem[a] &= !0x20;
                }
            }
            _ => {}
        }
        self.wdt_reset_regs();
        self.mem[self.a_stkptr] = EMPTY;
        if let Some(t) = self.t0.as_mut() {
            (t.cnt, t.pre, t.post, t.last) = (0, 0, 0, self.cycles);
            self.mem[t.l] = 0;
        }
        // Timer1 keeps its count in every reset but POR (TMRxL/H MCLR column u); Timer2/4/6 clear T2TMR (MCLR column 0).
        for t in &mut self.t1 {
            (t.pre, t.last) = (0, self.cycles);
        }
        for t in &mut self.t2 {
            (t.cnt, t.pre, t.post, t.last) = (0, 0, 0, self.cycles);
        }
        if let Some(a) = self.a_fvr {
            // the MCLR column leaves FVRRDY (q) alone: it follows FVREN, or stays 1
            self.mem[a] = self.mem[a] & !0x40 | if self.fvr_always { 0x40 } else { (self.mem[a] >> 1) & 0x40 };
        }
        if let Some(n) = self.nvm.as_mut() {
            n.reset(&mut self.mem);
        }
        if let Some(a) = self.adc.as_mut() {
            a.end = None; // a conversion in flight is lost; ADRES is a register with a reset value
        }
        for u in &mut self.uarts {
            (u.tsr_end, u.pending, u.rx_cur, u.asleep) = (None, None, None, false);
            u.fifo.clear();
            u.bits(&mut self.mem);
        }
        self.mssps.iter_mut().for_each(Mssp::abort);
        self.pps_seq = 0;
        self.ioc_at = self.cycles; // the pins are not watched during a Reset
        self.pc = self.reset_vector;
        self.sleeping = false;
        self.no_irq = false;
        self.irq_new = false;
        self.stk_fault = None;
        self.reset_req = None;
        if let Some(w) = self.wdt.as_mut() {
            (w.start, w.armed, w.was_on) = (self.cycles, false, false); // any Reset clears the watchdog (14.5)
        }
    }

    // ----- watchdog --------------------------------------------------------------------------------------------

    /// WDTE, WDTCPS, WDTCWS and WDTCCS from the configuration words, and the registers' values after a Reset `[A126]`.
    /// Windowed (DS40002343F 14.7.1-14.7.2 p.148-149): PS = WDTCPS (01011 when WDTCPS = 11111), WINDOW = WDTCWS, CS = WDTCCS
    /// (000 when 111); SEN = 0; a field the configuration fixes is read only. Single register (DS40001839E Register 10-1
    /// p.121, DS40002195E 14.6.1 p.105): PS = 01011 (2 s), SEN = 0, CS = 0.
    fn wdt_reset_regs(&mut self) {
        let Some(w) = self.wdt.as_mut() else { return };
        let (cps, cws, ccs) = (self.cfg.get("WDTCPS").unwrap_or(0x1F), self.cfg.get("WDTCWS").unwrap_or(7), self.cfg.get("WDTCCS").unwrap_or(7));
        (w.wdte, w.ps_fixed, w.win_fixed, w.cs_fixed) = (self.cfg.get("WDTE").unwrap_or(0) as u8, cps != 0x1F, cws != 7, ccs != 7);
        if self.wdt_simple {
            self.mem[w.con0] = 0x16 & self.implemented[w.con0];
        } else {
            let ps = if cps == 0x1F { 0x0B } else { cps as u8 };
            self.mem[w.con0] = ps << 1 & self.implemented[w.con0];
            let cs = if ccs == 7 { 0 } else { ccs as u8 };
            self.mem[w.con1] = (cs << 4 | cws as u8) & self.implemented[w.con1];
        }
    }

    /// WDTCON0 or WDTCON1 as `Wdt` expects them (the single-register WDT keeps CS in bit 7 of WDTCON: 1 is MFINTOSC).
    fn wdt_cons(&self) -> (u8, u8) {
        let w = self.wdt.as_ref().unwrap();
        let c0 = self.mem[w.con0];
        (c0, if self.wdt_simple { (c0 >> 7) << 4 } else { self.mem[w.con1] })
    }

    fn wdt_fosc(&mut self) {
        let f = if self.board.fosc_mhz > 0.0 { self.board.fosc_mhz } else { 64.0 };
        if let Some(w) = self.wdt.as_mut() {
            w.fosc_mhz = f;
        }
    }

    /// Awake: the watchdog is held clear while it is off, and a time-out resets the part.
    fn wdt_tick(&mut self) {
        if self.wdt.is_none() {
            return;
        }
        self.wdt_fosc();
        let (c0, c1) = self.wdt_cons();
        let now = self.cycles;
        let w = self.wdt.as_mut().unwrap();
        let on = w.on(c0);
        if !on || !w.was_on {
            (w.start, w.armed) = (now, false);
        }
        w.was_on = on;
        if on && now >= w.deadline(c0, c1) {
            self.reset_req.get_or_insert(Reset::Wdt);
        }
    }

    /// The cycle the watchdog times out while the part sleeps, when it counts in Sleep.
    fn wdt_sleep_deadline(&mut self) -> Option<u64> {
        self.wdt.as_ref()?;
        self.wdt_fosc();
        let (c0, c1) = self.wdt_cons();
        let w = self.wdt.as_ref().unwrap();
        w.on_sleep(c0).then(|| w.deadline(c0, c1))
    }

    /// Value of WDTPSL, WDTPSH or WDTTMR (`sub`) now (DS40002343F 14.7.3-14.7.5 p.150-152).
    fn wdt_value(&mut self, sub: u8) -> u8 {
        self.wdt_fosc();
        let (c0, c1) = self.wdt_cons();
        let (w, now) = (self.wdt.as_ref().unwrap(), self.cycles);
        let psc = (w.clocks(now, c1) & ((1u64 << Wdt::n(c0)) - 1)) as u32;
        match sub {
            wdt::PSL => psc as u8,
            wdt::PSH => (psc >> 8) as u8,
            _ => w.window_value(now, c0, c1) << 3 | u8::from(w.armed) << 2 | (psc >> 16) as u8 & 3,
        }
    }

    /// A write to WDTCON0 / WDTCON1 / WDTCON: read-only fields keep their value and the watchdog is cleared (14.5). The other
    /// watchdog registers are read only.
    fn wdt_write(&mut self, a: usize, v: u8) {
        let Some(w) = self.wdt.as_mut() else { return };
        let v = v & self.implemented[a];
        if a == w.con0 {
            let keep = if w.ps_fixed { 0x3E } else { 0 };
            self.mem[a] = v & !keep | self.mem[a] & keep;
        } else if a == w.con1 {
            let keep = if w.cs_fixed { 0x70 } else { 0 } | if w.win_fixed { 0x07 } else { 0 };
            self.mem[a] = v & !keep | self.mem[a] & keep;
            if self.mem[a] >> 4 & 7 > 1 {
                self.notes.insert("WDTCS selects a clock the simulator does not model (EXTOSC, SOSC or reserved): the watchdog runs from LFINTOSC".into());
            }
        } else {
            return;
        }
        (w.start, w.armed) = (self.cycles, false);
    }

    /// CLRWDT. A valid one clears the watchdog; in the windowed WWDT one in the closed window, or not armed by a read of
    /// WDTCON0, is a window violation and resets the part (14.4, 14.5.1 p.146-147). Nothing happens while the watchdog is off.
    fn wdt_clear(&mut self) {
        if self.wdt.is_none() {
            return;
        }
        self.wdt_fosc();
        let (c0, c1) = self.wdt_cons();
        let now = self.cycles;
        let w = self.wdt.as_mut().unwrap();
        if !w.on(c0) {
            return;
        }
        if !self.wdt_simple && w.violation(now, c0, c1) {
            self.reset_req.get_or_insert(Reset::WdtWindow);
        } else {
            (w.start, w.armed) = (now, false);
        }
    }

    // ----- program memory --------------------------------------------------------------------------------------

    /// The 14-bit word at word address `at`.
    #[inline]
    pub fn fetch(&self, at: u32) -> u16 {
        let a = 2 * (at % self.words) as usize;
        (u16::from(self.prog[a]) | u16::from(self.prog[a + 1]) << 8) & 0x3FFF
    }

    #[inline]
    fn inc_pc(&self, pc: u32) -> u32 {
        (pc + 1) % self.words
    }

    // ----- interrupts ------------------------------------------------------------------------------------------

    /// An enabled flag in PIE0/PIR0, or in another group with PEIE set (Fig. 12-1 p.110).
    fn enabled_flag(&self) -> bool {
        let peie = self.mem[self.a_intcon] & 0x40 != 0;
        self.irq.iter().enumerate().any(|(n, &(r, e))| (n == 0 || peie) && self.mem[r] & self.mem[e] != 0)
    }

    /// Take the interrupt if one is pending and GIE is set: GIE cleared, PC pushed, context saved in the shadow
    /// registers, PC = the vector (12.5 p.110-111, 12.9 p.113). The two cycles are the forced NOPs of Fig. 12-2 p.112.
    /// `cost` is 2, or 1 after a 2-cycle instruction of a request that arrived while running (`[A108]`).
    fn interrupt(&mut self, cost: u64) -> bool {
        if self.mem[self.a_intcon] & 0x80 == 0 || self.no_irq || !self.enabled_flag() {
            return false;
        }
        self.mem[self.a_intcon] &= !0x80;
        self.push(self.pc);
        let st = self.a_status;
        let src = [st, self.a_wreg, self.a_bsr, self.a_pclath, 4, 5, 6, 7];
        for (i, &s) in src.iter().enumerate() {
            let v = self.mem[s];
            self.mem[self.shad[i]] = if i == 0 { v & 0x07 } else { v } & self.implemented[self.shad[i]];
        }
        self.pc = self.vector;
        self.cycles += cost;
        true
    }

    /// RETFIE restores what the interrupt saved; TO and PD keep their current values.
    fn restore_context(&mut self) {
        let st = self.a_status;
        let dst = [st, self.a_wreg, self.a_bsr, self.a_pclath, 4, 5, 6, 7];
        for (i, &d) in dst.iter().enumerate() {
            let v = self.mem[self.shad[i]];
            self.mem[d] = if i == 0 { self.mem[st] & 0x18 | v & 0x07 } else { v & self.implemented[d] };
        }
    }

    /// Asleep with nothing that can wake the part (no watchdog or peripheral is modelled yet).
    pub fn sleep_dead(&self) -> bool {
        // a pin the stimulus has yet to change can raise an IOC or INT flag
        self.sleeping && !self.enabled_flag_any() && !self.drive.iter().any(|d| d.0 > self.cycles) && !self.wdt_counts_in_sleep()
    }

    /// The watchdog is on in Sleep (WDTE = 3, or 1 with SEN): it will wake the part.
    fn wdt_counts_in_sleep(&self) -> bool {
        self.wdt.as_ref().is_some_and(|w| w.on_sleep(self.mem[w.con0]))
    }

    /// A PIE & PIR bit pair is set (the wake-up condition; GIE and PEIE do not matter, 12.7 p.113).
    fn enabled_flag_any(&self) -> bool {
        self.irq.iter().any(|&(r, e)| self.mem[r] & self.mem[e] != 0)
    }

    // ----- execution -------------------------------------------------------------------------------------------

    /// Execute one instruction (or take one interrupt); returns the cycles it took (also added to `self.cycles`).
    pub fn step(&mut self) -> u32 {
        let n = self.step_core();
        self.ioc_scan();
        self.board_poll();
        n
    }

    fn step_core(&mut self) -> u32 {
        let start = self.cycles;
        if self.sleeping {
            let wdt_wake = self.wdt_sleep_deadline().is_some_and(|d| self.cycles >= d);
            if self.enabled_flag_any() || wdt_wake {
                self.sleeping = false;
                if let Some(w) = self.wdt.as_mut() {
                    (w.start, w.armed) = (self.cycles, false); // leaving Sleep clears the watchdog again
                }
                if wdt_wake && !self.enabled_flag_any() {
                    // a time-out in Sleep is no Reset: the part runs on with TO = PD = 0 (14.6 p.147); the single-register WDT also
                    // clears RWDT (DS40002195E 14.5 p.104), the windowed one leaves PCON0 alone (Table 10-4) `[A126]`
                    self.flags(NPD | NTO, 0);
                    if self.wdt_simple && let Some(a) = self.a_pcon0 {
                        self.mem[a] &= !0x10;
                    }
                }
                let slept = self.cycles - self.sleep_at;
                if let Some(a) = self.adc.as_mut() {
                    a.shift(slept); // only the ADCRC clock runs in Sleep and it is not modelled: the conversion waits
                }
                self.mssps.iter_mut().for_each(|q| q.shift(slept)); // the module's clock stopped
                for u in &mut self.uarts {
                    u.sleep(false, slept);
                    u.bits(&mut self.mem); // RCIDL, TRMT and the rest are current at once
                }
                self.no_irq = true; // the instruction after SLEEP runs first
            } else {
                // With the watchdog counting, jump to its time-out or the next change the stimulus makes (a UART still receiving
                // is stepped cycle by cycle) instead of stepping every cycle of a long Sleep.
                let mut adv = 1;
                if let Some(d) = self.wdt_sleep_deadline()
                    && !self.uarts.iter().any(|u| !u.rx_in.is_empty() || u.rx_cur.is_some())
                {
                    let next = self.drive.iter().map(|x| x.0).filter(|&t| t > self.cycles).min().unwrap_or(u64::MAX);
                    adv = d.min(next).saturating_sub(self.cycles).max(1);
                }
                self.cycles += adv;
                self.park_timers(); // Fosc/4 does not run in Sleep
                self.sync_nvm(); // the EEPROM write has its own timer `[A117]`
                let mut ev = Vec::new();
                for u in &mut self.uarts {
                    u.rx(&mut self.mem, self.cycles, &mut ev); // the stimulus goes on; the receiver is off
                }
                return 1;
            }
        }
        // [A108] A request that a peripheral made pending while the program was running lets the instruction already
        // fetched at the return address finish; the PC stacked is the one after it, and the vector is reached 3 cycles
        // after that instruction started (MDB; Fig. 12-2 p.112 shows the fetched instruction executing before the two
        // NOPs). MDB also steps over the two as one step: STEPI from that instruction stops at the vector, and a
        // breakpoint at the stacked PC is not hit first. A request that was already pending (GIE just set by BSF or
        // RETFIE) or that software made pending is taken at once (`[A104]`, MDB: `tests/probe/sw_pending.S`).
        let fresh = std::mem::take(&mut self.irq_new) && self.enabled_flag();
        self.irq_new_by.clear();
        if fresh && self.mem[self.a_intcon] & 0x80 != 0 && !self.no_irq {
            self.run_one();
            self.sync_timers();
            // 2 cycles of entry after a 1-cycle instruction, 1 after a 2-cycle one (a longer one is counted as 2).
            // The instruction may have dropped the request (GIE, PIE or the flag cleared) or asked for a reset.
            let cost = if self.cycles - start >= 2 { 1 } else { 2 };
            if self.reset_req.is_none() && self.stk_fault.is_none() {
                self.interrupt(cost);
            }
        } else if !self.interrupt(2) {
            self.run_one();
        }
        self.sync_timers();
        self.wdt_tick();
        if let Some(k) = self.stk_fault.or(self.reset_req) {
            self.do_reset(k);
        }
        (self.cycles - start) as u32
    }

    /// Fetch and execute one instruction.
    fn run_one(&mut self) {
        self.no_irq = false;
        let w = self.fetch(self.pc);
        self.pc = self.inc_pc(self.pc);
        self.cycles += 1;
        self.icount += 1;
        self.extra = 0;
        self.flash_cycle = false;
        self.exec(w);
        self.cycles += self.extra + u64::from(self.flash_cycle);
    }

    fn skip(&mut self) {
        self.pc = self.inc_pc(self.pc);
        self.extra += 1;
    }

    fn jump(&mut self, pc: u32) {
        self.pc = pc % self.words;
        self.extra += 1;
    }

    /// Store a byte-oriented result: W when d = 0, else the file register.
    fn put(&mut self, t: Tgt, d: bool, v: u8, alu: bool) {
        if d { self.wr(t, v, alu) } else { self.set_w(v) }
    }

    /// An instruction with a literal and/or W operand that returns through the shared flag logic.
    fn exec(&mut self, w: u16) {
        let wv = u32::from(w);
        let d = w & 0x80 != 0;
        let f = wv & 0x7F;
        match w >> 12 {
            0 => match w >> 8 {
                0x00 => self.misc(w),
                0x01 => {
                    match (w >> 6) & 3 {
                        0 => {
                            self.set_w(0); // CLRW
                            self.flags(Z, Z);
                        }
                        1 if !self.bsr5 => self.mem[self.a_bsr] = (w & 0x3F) as u8 & self.implemented[self.a_bsr], // MOVLB
                        1 => {
                            self.notes.insert(format!("undefined opcode {w:#06x} (MOVLB of the 6-bit-BSR parts)"));
                        }
                        _ => {
                            let t = self.file(f); // CLRF
                            self.wr(t, 0, true);
                            self.flags(Z, Z);
                        }
                    }
                }
                op => {
                    let t = self.file(f);
                    let a = self.rd(t);
                    let wr = self.w();
                    match op {
                        0x02 => {
                            let (r, fl) = add8(a, !wr, 1); // SUBWF: f - W
                            self.put(t, d, r, true);
                            self.flags(C | DC | Z, fl);
                        }
                        0x03 | 0x0A => {
                            let r = if op == 0x03 { a.wrapping_sub(1) } else { a.wrapping_add(1) }; // DECF, INCF
                            self.put(t, d, r, true);
                            self.flags(Z, z(r));
                        }
                        0x04 | 0x05 | 0x06 => {
                            let r = match op {
                                0x04 => a | wr,
                                0x05 => a & wr,
                                _ => a ^ wr,
                            };
                            self.put(t, d, r, true);
                            self.flags(Z, z(r));
                        }
                        0x07 => {
                            let (r, fl) = add8(a, wr, 0); // ADDWF
                            self.put(t, d, r, true);
                            self.flags(C | DC | Z, fl);
                        }
                        0x08 => {
                            self.put(t, d, a, true); // MOVF
                            self.flags(Z, z(a));
                        }
                        0x09 => {
                            let r = !a; // COMF
                            self.put(t, d, r, true);
                            self.flags(Z, z(r));
                        }
                        0x0B | 0x0F => {
                            let r = if op == 0x0B { a.wrapping_sub(1) } else { a.wrapping_add(1) }; // DECFSZ, INCFSZ
                            self.put(t, d, r, false);
                            if r == 0 {
                                self.skip();
                            }
                        }
                        0x0C => {
                            let r = a >> 1 | self.carry() << 7; // RRF
                            self.put(t, d, r, true);
                            self.flags(C, a & 1);
                        }
                        0x0D => {
                            let r = a << 1 | self.carry(); // RLF
                            self.put(t, d, r, true);
                            self.flags(C, a >> 7);
                        }
                        _ => {
                            let r = a.rotate_left(4); // 0x0E SWAPF
                            self.put(t, d, r, false);
                        }
                    }
                }
            },
            1 => {
                // BCF, BSF, BTFSC, BTFSS
                let t = self.file(f);
                let b = ((wv >> 7) & 7) as u8;
                let a = self.rd(t);
                match (w >> 10) & 3 {
                    0 => self.wr(t, a & !(1 << b), false),
                    1 => self.wr(t, a | 1 << b, false),
                    2 => {
                        if a >> b & 1 == 0 {
                            self.skip();
                        }
                    }
                    _ => {
                        if a >> b & 1 != 0 {
                            self.skip();
                        }
                    }
                }
            }
            2 => {
                // CALL (bit 11 = 0), GOTO (bit 11 = 1): PC[10:0] = k, PC[14:11] = PCLATH[6:3] (pp.597, 599)
                let to = u32::from(self.mem[self.a_pclath] & 0x78) << 8 | (wv & 0x7FF);
                if w & 0x0800 == 0 {
                    self.push(self.pc);
                }
                self.jump(to);
            }
            _ => self.lit(w),
        }
    }

    /// The group with opcode bits 13:12 = 11: literal operations, shifts, BRA, RETLW, ADDFSR, MOVLP, MOVIW/MOVWI k[n].
    fn lit(&mut self, w: u16) {
        let wv = u32::from(w);
        let d = w & 0x80 != 0;
        let k = (w & 0xFF) as u8;
        let wr = self.w();
        match (w >> 8) & 0xF {
            0x0 => self.set_w(k), // MOVLW
            0x1 => {
                if w & 0x80 == 0 {
                    // ADDFSR n, k
                    let n = usize::from(w >> 6 & 1);
                    let v = self.fsr(n).wrapping_add(sext(wv & 0x3F, 6) as u16);
                    self.set_fsr(n, v);
                } else {
                    self.mem[self.a_pclath] = (w & 0x7F) as u8 & self.implemented[self.a_pclath]; // MOVLP
                }
            }
            0x2 | 0x3 => {
                let to = (self.pc as i32 + sext(wv & 0x1FF, 9)) as u32; // BRA: PC + 1 + k, PC is already PC + 1
                self.jump(to);
            }
            0x4 => {
                let to = self.pop(); // RETLW
                self.set_w(k);
                self.jump(to);
            }
            0x5 | 0x6 | 0x7 => {
                // LSLF, LSRF, ASRF: C and Z only (Table 41-3 p.592)
                let t = self.file(wv & 0x7F);
                let a = self.rd(t);
                let (r, c) = match (w >> 8) & 0xF {
                    0x5 => (a << 1, a >> 7),
                    0x6 => (a >> 1, a & 1),
                    _ => ((a >> 1) | (a & 0x80), a & 1),
                };
                self.put(t, d, r, true);
                self.flags(C | Z, c | z(r));
            }
            0xB | 0xD => {
                // SUBWFB (f - W - borrow), ADDWFC
                let t = self.file(wv & 0x7F);
                let a = self.rd(t);
                let (r, fl) = if (w >> 8) & 0xF == 0xB { add8(a, !wr, self.carry()) } else { add8(a, wr, self.carry()) };
                self.put(t, d, r, true);
                self.flags(C | DC | Z, fl);
            }
            0x8 | 0x9 | 0xA => {
                let r = match (w >> 8) & 0xF {
                    0x8 => wr | k,
                    0x9 => wr & k,
                    _ => wr ^ k,
                };
                self.set_w(r);
                self.flags(Z, z(r));
            }
            0xC => {
                let (r, fl) = add8(k, !wr, 1); // SUBLW: k - W
                self.set_w(r);
                self.flags(C | DC | Z, fl);
            }
            0xE => {
                let (r, fl) = add8(wr, k, 0); // ADDLW
                self.set_w(r);
                self.flags(C | DC | Z, fl);
            }
            _ => {
                // 0xF: MOVIW k[n] (bit 7 = 0), MOVWI k[n] (bit 7 = 1)
                let n = usize::from(w >> 6 & 1);
                let t = self.target_of(self.fsr(n).wrapping_add(sext(wv & 0x3F, 6) as u16));
                if w & 0x80 == 0 {
                    let v = self.rd(t);
                    self.set_w(v);
                    self.flags(Z, z(v));
                } else {
                    self.wr(t, wr, false);
                }
            }
        }
    }

    /// The instructions with opcode bits 13:8 = 000000 (the byte at 0x00..=0xFF).
    fn misc(&mut self, w: u16) {
        if w & 0x80 != 0 {
            let t = self.file(u32::from(w & 0x7F)); // MOVWF
            let v = self.w();
            self.wr(t, v, false);
            return;
        }
        match w {
            0x0000 => {}                                 // NOP
            0x0001 => self.reset_req = Some(Reset::Software), // RESET
            0x0008 | 0x0009 => {
                let to = self.pop(); // RETURN, RETFIE
                if w == 0x0009 {
                    self.mem[self.a_intcon] |= 0x80;
                    self.restore_context();
                }
                self.jump(to);
            }
            0x000A => {
                let to = u32::from(self.mem[self.a_pclath] & 0x7F) << 8 | u32::from(self.w()); // CALLW
                self.push(self.pc);
                self.jump(to);
            }
            0x000B => {
                let to = self.pc + u32::from(self.w()); // BRW: PC + 1 + W
                self.jump(to);
            }
            0x0020..=0x003F if self.bsr5 => self.mem[self.a_bsr] = (w & 0x1F) as u8, // MOVLB k (DS40001839E p.406)
            0x0010..=0x001F => {
                // MOVIW (0x10-0x17) / MOVWI (0x18-0x1F) ++FSRn, --FSRn, FSRn++, FSRn--
                let n = usize::from(w >> 2 & 1);
                let mm = w & 3;
                let v = self.fsr(n);
                let (at, after) = match mm {
                    0 => (v.wrapping_add(1), v.wrapping_add(1)),
                    1 => (v.wrapping_sub(1), v.wrapping_sub(1)),
                    2 => (v, v.wrapping_add(1)),
                    _ => (v, v.wrapping_sub(1)),
                };
                let t = self.target_of(at);
                if w & 0x08 == 0 {
                    let x = self.rd(t);
                    self.set_w(x);
                    self.flags(Z, z(x));
                } else {
                    let x = self.w();
                    self.wr(t, x, false);
                }
                self.set_fsr(n, after);
            }
            0x0063 => {
                // An enabled flag already set: SLEEP executes as a NOP, TO and PD unchanged (13.2.2 p.139; MDB agrees
                // when the request arrived just before SLEEP with GIE set: vector 3 cycles after SLEEP started, PD = 1).
                if self.enabled_flag_any() {
                    return;
                }
                // SLEEP: PD = 0, TO = 1
                self.flags(NPD | NTO, NTO);
                self.sleeping = true;
                self.sleep_at = self.cycles;
                self.sync_adc(); // a conversion that ended in this cycle is done, not interrupted
                if let Some(a) = self.adc.as_mut() {
                    a.enter_sleep(&mut self.mem, &mut self.notes);
                }
                for u in &mut self.uarts {
                    u.sleep(true, 0);
                }
                if let Some(w) = self.wdt.as_mut() {
                    (w.start, w.armed) = (self.cycles, false); // entering Sleep clears the watchdog (14.6)
                }
            }
            0x0064 => {
                self.flags(NPD | NTO, NPD | NTO); // CLRWDT: TO = PD = 1
                self.wdt_clear();
            }
            0x0065..=0x0067 => {
                if let Some(a) = self.a_tris[usize::from(w - 0x65)] {
                    let v = self.w();
                    self.wr(Tgt::Mem(a), v, false);
                }
            }
            _ => {
                self.notes.insert(format!("undefined opcode {w:#06x}"));
            }
        }
    }
}

impl Pic16 {
    /// Disassembly of one word for this part (the MOVLB encoding depends on the BSR width).
    pub fn disasm(&self, w: u16) -> String {
        disasm_for(w, self.bsr5)
    }
}

/// Disassembly of one word in the syntax of Table 41-3; `bsr5` selects the PIC16F183xx MOVLB encoding (DS40001839E
/// Table 34-3 p.399). `Pic16::disasm` picks it for the loaded part.
pub fn disasm_for(w: u16, bsr5: bool) -> String {
    let f = w & 0x7F;
    let d = if w & 0x80 != 0 { "f" } else { "w" };
    let fd = |m: &str| format!("{m:<7} {f:#04x},{d}");
    let fb = |m: &str| format!("{m:<7} {f:#04x},{}", (w >> 7) & 7);
    let k8 = w & 0xFF;
    let s6 = sext(u32::from(w) & 0x3F, 6);
    let n = w >> 6 & 1;
    match w >> 12 {
        0 => match w >> 8 {
            0 => match w {
                0x0000 => "nop".into(),
                0x0001 => "reset".into(),
                0x0008 => "return".into(),
                0x0009 => "retfie".into(),
                0x000A => "callw".into(),
                0x000B => "brw".into(),
                0x0010..=0x001F => {
                    let (m, i) = if w & 8 == 0 { ("moviw", w) } else { ("movwi", w) };
                    let n = i >> 2 & 1;
                    let s = match i & 3 {
                        0 => format!("++FSR{n}"),
                        1 => format!("--FSR{n}"),
                        2 => format!("FSR{n}++"),
                        _ => format!("FSR{n}--"),
                    };
                    format!("{m:<7} {s}")
                }
                0x0020..=0x003F if bsr5 => format!("movlb   {}", w & 0x1F),
                0x0063 => "sleep".into(),
                0x0064 => "clrwdt".into(),
                0x0065..=0x0067 => format!("tris    {}", w - 0x60),
                _ if w & 0x80 != 0 => format!("movwf   {f:#04x}"),
                _ => format!("dw      {w:#06x}"),
            },
            1 => match (w >> 6) & 3 {
                0 => "clrw".into(),
                1 if !bsr5 => format!("movlb   {}", w & 0x3F),
                1 => format!("dw      {w:#06x}"),
                _ => format!("clrf    {f:#04x}"),
            },
            op => fd(match op {
                2 => "subwf",
                3 => "decf",
                4 => "iorwf",
                5 => "andwf",
                6 => "xorwf",
                7 => "addwf",
                8 => "movf",
                9 => "comf",
                0xA => "incf",
                0xB => "decfsz",
                0xC => "rrf",
                0xD => "rlf",
                0xE => "swapf",
                _ => "incfsz",
            }),
        },
        1 => fb(["bcf", "bsf", "btfsc", "btfss"][usize::from(w >> 10 & 3)]),
        2 => format!("{:<7} {:#06x}", if w & 0x800 == 0 { "call" } else { "goto" }, w & 0x7FF),
        _ => match (w >> 8) & 0xF {
            0 => format!("movlw   {k8:#04x}"),
            1 if w & 0x80 == 0 => format!("addfsr  {n},{s6}"),
            1 => format!("movlp   {:#04x}", w & 0x7F),
            2 | 3 => format!("bra     {:+}", sext(u32::from(w) & 0x1FF, 9)),
            4 => format!("retlw   {k8:#04x}"),
            5 => fd("lslf"),
            6 => fd("lsrf"),
            7 => fd("asrf"),
            8 => format!("iorlw   {k8:#04x}"),
            9 => format!("andlw   {k8:#04x}"),
            0xA => format!("xorlw   {k8:#04x}"),
            0xB => fd("subwfb"),
            0xC => format!("sublw   {k8:#04x}"),
            0xD => fd("addwfc"),
            0xE => format!("addlw   {k8:#04x}"),
            _ => format!("{:<7} {s6}[FSR{n}]", if w & 0x80 == 0 { "moviw" } else { "movwi" }),
        },
    }
}

impl PinView for Pic16 {
    fn pin(&self, p: Pin) -> Option<(bool, bool)> {
        let i = self.ports.iter().position(|q| q.letter == p.0)?;
        Some((self.tris_of(i) >> p.1 & 1 == 0, self.port_levels(i) >> p.1 & 1 != 0))
    }

    fn open_drain(&self, p: Pin) -> bool {
        match self.ports.iter().position(|q| q.letter == p.0) {
            Some(i) => !(self.tris_of(i) >> p.1 & 1 == 0 && self.mem[self.ports[i].lat] >> p.1 & 1 == 0),
            None => true,
        }
    }
}
