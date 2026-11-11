//! The Configurable Logic Block (CLB) of the PIC16F131xx (DS40002519D ch.29 p.317-336) and the PIC16F132xx (DS40002686B ch.29
//! p.359-383): 32 basic logic elements (BLEs; a 4-input LUT, a flop, a mux between them), 16 input selections with edge
//! detectors, a 3-bit counter, 8 PPS outputs, 4 interrupts, the software input and output registers.
//!
//! **The configuration.** On the part everything but the SFRs is set by a bitstream made by the Microchip CLB Synthesizer; the
//! NVM scanner copies it from program memory into the CLB latches (29.9). Its layout is not in either data sheet, so the
//! simulator cannot read it: the netlist comes from the `[clb]` section of the stimulus file instead (`stimulus.rs`; one
//! table per latch group, numbered as the data sheet's figures number them). The SFR side (CLBCON, CLBSWIN*, CLBCLK,
//! CLBSWOUT*, CLBDEBUG0, CLBPPSCON*, the PPS outputs, PIR7/PIE7) is modelled as the data sheets describe it.
//!
//! **Timing.** The CLB works in quarter instruction cycles (Fosc periods; 4 to a cycle, `q`). CLBCLK is FOSC or a pin
//! (CLBIN0-3PPS, CLBCLKPPS); the other sources need clock frequencies the model does not carry and stop the block with a
//! note. BLE_clk is CLBCLK divided by `divider` (the Synthesizer's clock divider latches). A *tick* (a BLE_clk rising edge)
//! loads every BLE flop from its LUT, steps the counter, copies the software inputs in and moves the input synchronisers on.
//! Between ticks the combinational paths follow the inputs as they change (the pins change at cycle boundaries).
//!
//! Assumptions `[A133]`-`[A140]` in `docs/FACTS-SIM.md`.

use std::collections::BTreeSet;

/// The two register layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// PIC16F131xx: CLBPPSCON1-4 pick the output enables; no CLBSWOUT, no CLBDEBUG0.
    F131,
    /// PIC16F132xx: CLBSWOUTH/L, CLBDEBUG0, TRISxyPPS pick the output enables.
    F132,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Edge {
    /// Bypassed: the level goes through.
    #[default]
    None,
    Pos,
    Neg,
}

/// One of the 16 CLB input selections (Table 29-1, Fig. 29-3/29-4..9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputCfg {
    /// The Table 29-1 value of the signal.
    pub source: u8,
    pub edge: Edge,
    /// Synchronised to BLE_clk (an edge pulse then lasts one BLE_clk period).
    pub sync: bool,
}

/// One BLE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BleCfg {
    /// The 5-bit selections of LUT inputs A, B, C, D (Fig. 29-10 / 29-3); 31 selects nothing and reads 0.
    pub sel: [u8; 4],
    /// Bit `A + 2B + 4C + 8D` is the output `[A133]`.
    pub lut: u16,
    /// The output is the flop, not the LUT.
    pub flop: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CounterCfg {
    pub enable: bool,
    /// BLE whose output, while high, holds the counter / clears it.
    pub stop: Option<u8>,
    pub reset: Option<u8>,
    /// Counter[A1], [A2], [B1], [B2], [C1], [C2], [D1], [D2]: high while the count equals this value (Fig. 29-2, `[A135]`).
    pub sel: [Option<u8>; 8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cfg {
    /// BLE_clk = CLBCLK / divider.
    pub divider: u32,
    pub inputs: [InputCfg; 16],
    pub ble: [BleCfg; 32],
    pub counter: CounterCfg,
    /// CLBPPSOUTn is BLE[4n + outsel[n]] (Fig. 29-11).
    pub outsel: [u8; 8],
    /// CLB1IFn is set by a rising edge of this BLE (Fig. 29-13).
    pub irq: [Option<u8>; 4],
    /// A netlist was given (otherwise the block is unconfigured and a note says so).
    pub given: bool,
}

impl Default for Cfg {
    fn default() -> Cfg {
        Cfg {
            divider: 1,
            inputs: [InputCfg { source: 63, edge: Edge::None, sync: false }; 16],
            ble: [BleCfg { sel: [31; 4], lut: 0, flop: false }; 32],
            counter: CounterCfg::default(),
            outsel: [0; 8],
            irq: [None; 4],
            given: false,
        }
    }
}

/// What an input selection or the clock can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Src {
    Low,
    /// CLBIN0-3PPS (0-3), CLBCLKPPS (4).
    Pin(u8),
    Busy,
    Ccp(u8),
    Iocif,
    Fosc,
    /// A source the model has no value for; reads low.
    Other(&'static str),
}

/// Table 29-1 (DS40002519D p.320-321 for the 131xx, DS40002686B p.362-363 for the 132xx).
pub fn input_src(f: Family, v: u8) -> Src {
    use Src::*;
    match v {
        0..=3 => Pin(v),
        4 => Fosc,
        5 => Other("HFINTOSC"),
        6 => Other("LFINTOSC"),
        7 | 8 => Other("MFINTOSC"),
        9 => Other("EXTOSC"),
        10 => Other("ADCRC"),
        11 => Other("Timer0 overflow"),
        12 => Other("Timer1 overflow"),
        13 => Other("Timer2 output"),
        14 => Ccp(1),
        15 => Ccp(2),
        16 | 17 => Other("PWM"),
        18 => Iocif,
        19..=22 => Other("CLC output"),
        23 => Other("TX1"),
        24 => Other("SDO1"),
        25 => Other("SCK1"),
        26 => Busy,
        27 | 28 => Other("comparator output"),
        31 if f == Family::F132 => Other("HLVD"),
        32 if f == Family::F132 => Other("TX2"),
        33 if f == Family::F132 => Other("SOSC"),
        34 if f == Family::F132 => Other("Timer3 overflow"),
        35 if f == Family::F132 => Other("Timer4 output"),
        36 | 37 if f == Family::F132 => Other("PWM"),
        38 if f == Family::F132 => Other("CLKR"),
        39 if f == Family::F132 => Low, // VSS
        _ => Low,                       // reserved
    }
}

/// What CLBCLK selects (Table 29-4 DS40002519D p.335, Table 29-3 DS40002686B p.379).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clk {
    None,
    Fosc,
    Pin(u8),
    Other(&'static str),
}

pub fn clk_src(f: Family, v: u8) -> Clk {
    match (f, v) {
        (Family::F131, 0) => Clk::None,
        (Family::F132, 0) => Clk::Pin(4),
        (_, 1..=4) => Clk::Pin(v - 1),
        (_, 5) => Clk::Fosc,
        (_, 6) => Clk::Other("HFINTOSC"),
        (_, 7) => Clk::Other("LFINTOSC"),
        (_, 8 | 9) => Clk::Other("MFINTOSC"),
        (_, 10) => Clk::Other("EXTOSC"),
        (_, 11) => Clk::Other("ADCRC"),
        (_, 12) => Clk::Other("Timer0 overflow"),
        (_, 13) => Clk::Other("Timer1 overflow"),
        (_, 14) => Clk::Other("Timer2 output"),
        (Family::F132, 15) => Clk::Other("Timer3 overflow"),
        (Family::F132, 16) => Clk::Other("Timer4 output"),
        (Family::F132, 17) => Clk::Other("SOSC"),
        (Family::F132, 18) => Clk::Other("CLKR"),
        _ => Clk::None, // reserved
    }
}

/// The SFR values the engine needs, read by the caller before every advance.
#[derive(Clone, Copy, Debug, Default)]
pub struct Regs {
    pub en: bool,
    /// The CLBMD bit of PMD4: the module is held in Reset (29.2).
    pub md: bool,
    pub clk: u8,
}

/// The rest of the part, as the CLB sees it.
pub trait Ctx {
    /// The level of pin input `n` (CLBIN0-3PPS, 4 = CLBCLKPPS) in instruction cycle `cycle`.
    fn pin(&self, n: u8, cycle: u64) -> bool;
    fn ccp(&self, n: u8) -> bool;
    fn iocif(&self) -> bool;
    /// Cycles in `(from, to]` at which a pin may change.
    fn pin_events(&self, from: u64, to: u64) -> Vec<u64>;
}

#[derive(Debug)]
pub struct Clb {
    pub family: Family,
    pub cfg: Cfg,
    /// Quarter cycles processed up to.
    last: u64,
    pub flop: [bool; 32],
    /// The BLE outputs (flop or LUT, zero while the block is held in Reset).
    pub out: [bool; 32],
    lut: [bool; 32],
    raw: [bool; 16],
    latch: [bool; 16],
    sreg: [bool; 16],
    pv: [bool; 16],
    primed: bool,
    pub counter: u8,
    /// CLBSWIN as the logic sees it, the holding latches and BUSY.
    swin: u32,
    hold: u32,
    pub busy: bool,
    irq_prev: [bool; 4],
    quiet: bool,
    quiet_clk: u8,
    /// Edges of a pin clock seen, for the divider.
    edges: u32,
    clk_prev: bool,
    /// CLBSWOUT as read: frozen while RDHOLD = 1.
    pub frozen: Option<u16>,
    pub debug: u8,
}

impl Clb {
    pub fn new(family: Family, now_q: u64) -> Clb {
        Clb {
            family,
            cfg: Cfg::default(),
            last: now_q,
            flop: [false; 32],
            out: [false; 32],
            lut: [false; 32],
            raw: [false; 16],
            latch: [false; 16],
            sreg: [false; 16],
            pv: [false; 16],
            primed: false,
            counter: 0,
            swin: 0,
            hold: 0,
            busy: false,
            irq_prev: [false; 4],
            quiet: false,
            quiet_clk: 0,
            edges: 0,
            clk_prev: false,
            frozen: None,
            debug: 0,
        }
    }

    /// BLE outputs as bits, for the pin logic of the core.
    pub fn out_bits(&self) -> u32 {
        self.out.iter().enumerate().fold(0, |a, (i, &b)| a | u32::from(b) << i)
    }

    /// CLBSWOUT[15:0] as read: BLE[15:0] `[A136]`.
    pub fn swout(&self) -> u16 {
        self.frozen.unwrap_or_else(|| self.live())
    }

    fn live(&self) -> u16 {
        (0..16).fold(0u16, |a, i| a | u16::from(self.out[i]) << i)
    }

    /// RDHOLD written: 1 captures the outputs and keeps them, 0 lets them follow again.
    pub fn set_rdhold(&mut self, on: bool) {
        match (on, self.frozen) {
            (true, None) => self.frozen = Some(self.live()),
            (false, Some(_)) => self.frozen = None,
            _ => {}
        }
    }

    /// The Reset state: flops, counter, synchronisers and the interrupt edge detectors cleared (29.2: EN = 0, CLBMD, a device
    /// Reset). The holding latches and the SWIN registers' values stay.
    pub fn reset(&mut self, now_q: u64) {
        self.flop = [false; 32];
        self.out = [false; 32];
        self.lut = [false; 32];
        self.latch = [false; 16];
        self.sreg = [false; 16];
        self.pv = [false; 16];
        self.primed = false;
        self.counter = 0;
        self.irq_prev = [false; 4];
        self.quiet = false;
        self.edges = 0;
        self.clk_prev = false;
        if self.frozen.is_some() {
            self.frozen = Some(0); // RDHOLD stays set; the outputs it holds are zero while the block is in Reset
        }
        self.busy = false; // the synchroniser is cleared too, so the CLBSWIN registers are not locked for good
        self.last = now_q;
    }

    /// A new netlist: nothing is settled any more.
    pub fn set_cfg(&mut self, cfg: Cfg) {
        self.cfg = cfg;
        self.quiet = false;
    }

    /// CLBSWINL written: the four registers are copied into the holding latches and BUSY is set until the next BLE_clk tick
    /// (29.4.2). `regs` is U, H, M, L as they are after the write.
    pub fn swin_written(&mut self, regs: [u8; 4]) {
        self.hold = u32::from_le_bytes([regs[3], regs[2], regs[1], regs[0]]);
        self.busy = true;
        self.quiet = false;
    }

    fn level(&self, ctx: &dyn Ctx, src: Src, cycle: u64) -> bool {
        match src {
            Src::Pin(n) => ctx.pin(n, cycle),
            Src::Busy => self.busy,
            Src::Ccp(n) => ctx.ccp(n),
            Src::Iocif => ctx.iocif(),
            _ => false,
        }
    }

    /// The value of BLE input selection `code` of LUT input `x` (0-3 = A-D) of BLE `n` (Fig. 29-10).
    fn ble_in(&self, x: usize, code: u8) -> bool {
        let k = usize::from(code);
        match code {
            0..=7 => self.out[8 * x + k],
            8..=11 => self.pv[4 * x + k - 8],
            12..=19 => self.swin >> (8 * x + k - 12) & 1 != 0,
            20 | 21 => {
                let sel = self.cfg.counter.sel[2 * x + k - 20];
                sel.is_some_and(|s| self.counter == s)
            }
            _ => false,
        }
    }

    /// Settle the combinational network: LUT outputs and BLE outputs follow the flops, the synchronised inputs, CLBSWIN and the
    /// counter. A loop that does not settle is left where it is after 40 passes, with a note.
    fn settle(&mut self, notes: &mut BTreeSet<String>) {
        for pass in 0..40 {
            let mut changed = false;
            for n in 0..32 {
                let b = self.cfg.ble[n];
                // an all-zero LUT (an unused BLE) is 0 whatever its inputs are
                let l = b.lut != 0 && {
                    let idx = (0..4).fold(0usize, |a, x| a | usize::from(self.ble_in(x, b.sel[x])) << x);
                    b.lut >> idx & 1 != 0
                };
                let o = if b.flop { self.flop[n] } else { l };
                if l != self.lut[n] || o != self.out[n] {
                    changed = true;
                }
                self.lut[n] = l;
                self.out[n] = o;
            }
            if !changed {
                return;
            }
            if pass == 39 {
                notes.insert("a combinational loop of the CLB does not settle (a BLE without its flop feeds itself through an odd number of inversions); its outputs are left as they are".into());
            }
        }
    }

    fn edge_hit(e: Edge, was: bool, now: bool) -> bool {
        match e {
            Edge::None => false,
            Edge::Pos => !was && now,
            Edge::Neg => was && !now,
        }
    }

    fn presented(&mut self) {
        for i in 0..16 {
            let c = self.cfg.inputs[i];
            self.pv[i] = match (c.edge, c.sync) {
                (Edge::None, false) => self.raw[i],
                (_, true) => self.sreg[i],
                (_, false) => self.latch[i],
            };
        }
    }

    fn signature(&self) -> (u32, u32, u8, [bool; 16], [bool; 16], bool, u32) {
        let f = self.flop.iter().enumerate().fold(0u32, |a, (i, &b)| a | u32::from(b) << i);
        let l = self.latch.iter().enumerate().fold(0u32, |a, (i, &b)| a | u32::from(b) << i);
        (f, l, self.counter, self.sreg, self.pv, self.busy, self.swin)
    }

    /// One instant: an input change, a BLE_clk tick, or both. Returns the CLB1IFn that rose.
    fn instant(&mut self, ctx: &dyn Ctx, q: u64, tick: bool, notes: &mut BTreeSet<String>, raised: &mut Vec<u8>) {
        let cyc = q / 4;
        for i in 0..16 {
            let src = input_src(self.family, self.cfg.inputs[i].source);
            if let Src::Other(n) = src
                && self.cfg.given
            {
                notes.insert(format!("CLB input selection {i}: {n} is not modelled; it reads low"));
            }
            let now = self.level(ctx, src, cyc);
            if self.primed && Self::edge_hit(self.cfg.inputs[i].edge, self.raw[i], now) {
                self.latch[i] = true;
            }
            self.raw[i] = now;
        }
        self.primed = true;
        self.presented();
        self.settle(notes);
        if tick {
            let d = self.lut;
            // the counter sees the outputs of the period that ends
            let c = self.cfg.counter;
            if c.reset.is_some_and(|b| self.out[usize::from(b)]) {
                self.counter = 0;
            } else if c.enable && !c.stop.is_some_and(|b| self.out[usize::from(b)]) {
                self.counter = (self.counter + 1) & 7;
            }
            if self.busy {
                self.swin = self.hold;
                self.busy = false;
            }
            for i in 0..16 {
                let c = self.cfg.inputs[i];
                self.sreg[i] = if c.edge == Edge::None { self.raw[i] } else { self.latch[i] };
                if c.edge != Edge::None {
                    self.latch[i] = false;
                }
            }
            self.flop = d;
            self.presented();
            self.settle(notes);
        }
        for k in 0..4 {
            if let Some(b) = self.cfg.irq[k] {
                let v = self.out[usize::from(b) % 32];
                if v && !self.irq_prev[k] {
                    raised.push(k as u8);
                }
                self.irq_prev[k] = v;
            }
        }
    }

    /// Bring the block to quarter cycle `now_q`. Returns the interrupt flags (0-3) that were set.
    pub fn advance(&mut self, ctx: &dyn Ctx, regs: Regs, now_q: u64, notes: &mut BTreeSet<String>) -> Vec<u8> {
        let mut raised = Vec::new();
        if now_q <= self.last {
            return raised;
        }
        if !regs.en || regs.md {
            self.reset(now_q);
            return raised;
        }
        if self.quiet {
            // nothing moved the block since the last tick, but firmware can change a pin, a CCP or IOCIF between two steps: look
            let cyc = now_q / 4;
            let moved = (0..16).any(|i| self.level(ctx, input_src(self.family, self.cfg.inputs[i].source), cyc) != self.raw[i]);
            if moved || regs.clk != self.quiet_clk {
                self.quiet = false;
            }
        }
        self.quiet_clk = regs.clk;
        let clk = clk_src(self.family, regs.clk);
        if let Clk::Other(n) = clk {
            notes.insert(format!("CLBCLK = {n} is not modelled (the model has no frequency for it); the CLB is not clocked"));
        }
        let div = u64::from(self.cfg.divider.max(1));
        let events: Vec<u64> = ctx.pin_events(self.last / 4, now_q / 4).into_iter().map(|c| c * 4).filter(|&q| q > self.last && q <= now_q).collect();
        let mut ev = events.into_iter().peekable();
        let mut q = self.last;
        loop {
            let tick_q = match clk {
                Clk::Fosc if !self.quiet => Some((q / div + 1) * div),
                _ => None,
            };
            let next_ev = ev.peek().copied();
            let t = match (tick_q, next_ev) {
                (Some(a), Some(b)) => a.min(b),
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (None, None) => break,
            };
            if t > now_q {
                break;
            }
            let is_event = next_ev == Some(t);
            if is_event {
                ev.next();
                self.quiet = false;
            }
            let mut tick = tick_q == Some(t);
            if let Clk::Pin(n) = clk
                && is_event
            {
                let lvl = ctx.pin(n, t / 4);
                if lvl && !self.clk_prev {
                    self.edges += 1;
                    if u64::from(self.edges) >= div {
                        self.edges = 0;
                        tick = true;
                    }
                }
                self.clk_prev = lvl;
            }
            let before = self.signature();
            self.instant(ctx, t, tick, notes, &mut raised);
            if tick && !is_event && self.signature() == before {
                self.quiet = true; // nothing changes until an input does
            }
            q = t;
        }
        self.last = now_q;
        raised
    }

    /// Start the clock reference at `now_q` (a Reset, wake-up from Sleep: FOSC did not run).
    pub fn park(&mut self, now_q: u64) {
        self.last = self.last.max(now_q);
    }
}
