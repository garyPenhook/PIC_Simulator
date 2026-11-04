//! CWG1 of the PIC18F27Q10 (DS40002043F ch.25 p.377-405): steering, full-bridge, half-bridge and push-pull modes, dead
//! band, auto-shutdown.
//!
//! Rules relied on:
//! * Data input (CWG1ISM, Table 25-1 p.389): 0001 CCP1, 0010 CCP2, 0011 PWM3, 0100 PWM4. The other sources (pin, comparators,
//!   DSM, CLC) are not modelled; the module then sees a low input and a note says so.
//! * Outputs CWG1A..D are PPS output codes 0x01..0x04 (Table 18-2 p.287).
//! * MODE (p.397): 000 asynchronous steering, 001 synchronous steering (STRy changes take effect on the next rising edge of the
//!   input, OVRy do not), 010 forward full bridge (A active, B and C inactive, D modulated), 011 reverse (C active, A and D
//!   inactive, B modulated, 25.2.3 p.382-385), 100 half bridge (A = input, B = inverted input, C copies A, D copies B), 101
//!   push-pull (the first input pulse goes to A, the next to B, ...; C copies A, D copies B), 11x reserved.
//! * Steering (25.2.4 p.386-388, STR p.401): STRy = 1 the pin shows the input with POLy, STRy = 0 it shows OVRy (no polarity).
//! * POLy = 1 inverts the output (CWGxCON1, p.398).
//! * Dead band (25.7-25.9 p.389-391): half bridge only (and the modulated output of a full bridge after a direction change):
//!   the turn-on of A (and C) is delayed by DBR, that of B (and D) by DBF, counted in CWG clock periods; an input that turns
//!   round before the count ends shows no pulse. The counts are double buffered: written with EN = 0 they go to the buffer at
//!   once; with EN = 1 the buffer is loaded at the rising edge after the first falling edge following LD = 1 (LD clears).
//! * Auto-shutdown (25.11 p.391-394, AS0 p.402): SHUTDOWN set by software or by an enabled source, flag CWG1IF set on the 0 -> 1
//!   change. Override levels: LSAC for A and C, LSBD for B and D (11 high, 10 low, 01 tri-state, 00 the inactive level with
//!   polarity). In steering modes only pins with STRy = 1 are affected. SHUTDOWN cannot be cleared while a source is active.
//!   After the clear (by software, or by hardware with REN = 1) the outputs stay overridden until the next rising edge of the input.
//! Assumptions `[A#]`:
//! * `[A73]` Dead band is exactly DBx CWG clock periods (the data sheet allows DBx to DBx + 1, Equation 25-1) and the CWG clock
//!   is Fosc (CS = 1, HFINTOSC, is counted as Fosc too and a note is added).
//! * `[A74]` After EN goes 1 the modes with a modulated output (half bridge, push-pull, full bridge) hold the inactive level
//!   until the first rising edge of the input; the steering modes follow the input at once.
//! * `[A75]` With EN = 0 the module drives its four outputs low (like `[A65]`).
//! * `[A76]` The AS0E pin source is looked at when a CWG register is written and at each edge of the input, not between those.
//!   The other shutdown sources (comparators, CLC, Timer2/4/6 postscaled) are not modelled; enabling them adds a note.
//! * `[A77]` Polarity: the register description (POLy = 1 inverts) is followed, not the text of 25.6.2 which says the opposite.
//! * `[A78]` A direction change of the full bridge (MODE[0] written while EN = 1) switches the static outputs at the next rising
//!   edge and delays the turn-on of the new modulated output by DBR (forward to reverse) or DBF (reverse to forward).

use super::{addr, Flag};
use crate::mem::DataMem;
use devdesc::Device;

/// Hook sub-codes (CWG0 + sub-code).
pub const CON0: u8 = 0;
pub const CON1: u8 = 1;
pub const AS0: u8 = 2;
pub const AS1: u8 = 3;
pub const STR: u8 = 4;
pub const DBR: u8 = 5;
pub const DBF: u8 = 6;
pub const HOOKS: u8 = 7;

pub struct Cwg {
    pub con0: usize,
    pub con1: usize,
    pub as0: usize,
    pub as1: usize,
    pub str_: usize,
    pub dbr: usize,
    pub dbf: usize,
    pub clk: usize,
    pub ism: usize,
    pub pps: Option<usize>,
    pub flag: Flag,
    /// The data input level.
    pub inp: bool,
    /// Push-pull sequencer: false = next pulse goes to A.
    seq: bool,
    /// A rising edge came since EN was set `[A74]`.
    started: bool,
    /// Outputs stay in the shutdown state until the next rising edge of the input.
    wait_rise: bool,
    /// STR bits in force (synchronous steering takes the register at a rising edge).
    str_eff: u8,
    /// Dead band count buffers.
    dbr_buf: u8,
    dbf_buf: u8,
    /// LD seen set; the first falling edge came since.
    ld_fall: bool,
    /// MODE[0] changed with EN = 1: the next rising edge is a direction change `[A78]`.
    dir_chg: bool,
    /// Outputs that are tri-stated (bit y = CWG1A..D).
    pub hiz: u8,
}

/// What `Cwg` wants the outputs to do: the new level (`None` = tri-state) and the delay in Fosc ticks before it applies.
pub struct Change {
    pub level: Option<bool>,
    pub delay: u64,
}

impl Cwg {
    pub fn find(dev: &Device) -> Option<Cwg> {
        Some(Cwg {
            con0: addr(dev, "CWG1CON0")?,
            con1: addr(dev, "CWG1CON1")?,
            as0: addr(dev, "CWG1AS0")?,
            as1: addr(dev, "CWG1AS1")?,
            str_: addr(dev, "CWG1STR")?,
            dbr: addr(dev, "CWG1DBR")?,
            dbf: addr(dev, "CWG1DBF")?,
            clk: addr(dev, "CWG1CLK")?,
            ism: addr(dev, "CWG1ISM")?,
            pps: addr(dev, "CWG1PPS"),
            flag: Flag::find(dev, "CWG1IF")?,
            inp: false,
            seq: false,
            started: false,
            wait_rise: false,
            str_eff: 0,
            dbr_buf: 0,
            dbf_buf: 0,
            ld_fall: false,
            dir_chg: false,
            hiz: 0,
        })
    }

    pub fn en(&self, m: &DataMem) -> bool {
        m.mem[self.con0] & 0x80 != 0
    }

    fn mode(&self, m: &DataMem) -> u8 {
        m.mem[self.con0] & 7
    }

    fn pol(&self, m: &DataMem, y: usize) -> bool {
        m.mem[self.con1] >> y & 1 != 0
    }

    /// PPS output code of the PPS source the data input comes from (CCP1 = 5, CCP2 = 6, PWM3 = 7, PWM4 = 8), if modelled.
    pub fn source(&self, m: &DataMem) -> Option<u8> {
        match m.mem[self.ism] & 15 {
            n @ 1..=4 => Some(4 + n),
            _ => None,
        }
    }

    pub fn shutdown(&self, m: &DataMem) -> bool {
        m.mem[self.as0] & 0x80 != 0
    }

    fn steering(&self, m: &DataMem) -> bool {
        self.mode(m) < 2
    }

    /// The ideal (pre-polarity) data of A..D in the steady state of the present input.
    fn data(&self, m: &DataMem) -> [bool; 4] {
        let i = self.inp;
        let go = i && self.started;
        match self.mode(m) {
            0 | 1 => [i; 4],
            2 => [true, false, false, go],
            3 => [false, go, true, false],
            4 => {
                let (a, b) = (go, self.started && !i);
                [a, b, a, b]
            }
            5 => {
                let (a, b) = (i && !self.seq, i && self.seq);
                [a, b, a, b]
            }
            _ => [false; 4],
        }
    }

    /// The level each output pin shows for `data` (`None` = tri-state), with steering, polarity and shutdown.
    fn finals(&self, m: &DataMem, data: [bool; 4]) -> [Option<bool>; 4] {
        if !self.en(m) {
            return [Some(false); 4]; // [A75]
        }
        let stage = self.steering(m);
        let shut = self.shutdown(m) || self.wait_rise;
        let as0 = m.mem[self.as0];
        let strv = if self.mode(m) == 1 { self.str_eff } else { m.mem[self.str_] };
        let mut out = [None; 4];
        for (y, o) in out.iter_mut().enumerate() {
            let steered = strv >> y & 1 != 0;
            let pol = self.pol(m, y);
            *o = if stage && !steered {
                Some(m.mem[self.str_] >> (4 + y) & 1 != 0) // OVRy, no polarity
            } else if shut {
                match as0 >> (if y % 2 == 0 { 2 } else { 4 }) & 3 {
                    3 => Some(true),
                    2 => Some(false),
                    1 => None,
                    _ => Some(pol),
                }
            } else {
                Some(data[y] ^ pol)
            };
        }
        out
    }

    /// Present outputs without any edge (after a register write, EN, shutdown change).
    pub fn now(&self, m: &DataMem) -> [Change; 4] {
        let f = self.finals(m, self.data(m));
        f.map(|level| Change { level, delay: 0 })
    }

    /// The input source changed to `new`. Returns the output changes and whether a pending (delayed) change must be dropped first.
    pub fn edge(&mut self, m: &mut DataMem, new: bool) -> [Change; 4] {
        m.mem[self.con1] = m.mem[self.con1] & !0x20 | u8::from(new) << 5;
        let old = self.data(m);
        let was = self.inp;
        self.inp = new;
        let mode = self.mode(m);
        let mut dly = [0u64; 4];
        if self.en(m) && new != was {
            if new {
                if self.wait_rise {
                    self.wait_rise = false;
                }
                if mode == 1 {
                    self.str_eff = m.mem[self.str_] & 15; // only STR is synchronised
                }
                if self.ld_fall {
                    // The buffers load at the rising edge that follows the first falling edge after LD was set.
                    self.dbr_buf = m.mem[self.dbr] & 63;
                    self.dbf_buf = m.mem[self.dbf] & 63;
                    self.ld_fall = false;
                    m.mem[self.con0] &= !0x40;
                }
                self.started = true;
            } else {
                if m.mem[self.con0] & 0x40 != 0 {
                    self.ld_fall = true;
                }
                if mode == 5 {
                    self.seq = !self.seq; // the next pulse goes to the other output, from the falling edge on
                }
            }
        }
        let new_data = self.data(m);
        let (r, f) = (u64::from(self.dbr_buf), u64::from(self.dbf_buf));
        if mode == 4 {
            dly = [r, f, r, f];
        } else if (mode == 2 || mode == 3) && self.dir_chg && new {
            dly = if mode == 3 { [0, r, 0, 0] } else { [0, 0, 0, f] }; // [A78]
            self.dir_chg = false;
        }
        let fin = self.finals(m, new_data);
        let mut i = 0;
        fin.map(|level| {
            let y = i;
            i += 1;
            let turn_on = !old[y] && new_data[y];
            Change { level, delay: if turn_on { dly[y] } else { 0 } }
        })
    }

    /// EN went 0 -> 1 with the data input at `lvl` `[A74]`.
    pub fn enable(&mut self, m: &mut DataMem, lvl: bool) {
        self.inp = lvl;
        m.mem[self.con1] = m.mem[self.con1] & !0x20 | u8::from(lvl) << 5;
        self.seq = false;
        self.started = false;
        self.wait_rise = false;
        self.ld_fall = false;
        self.dir_chg = false;
        self.str_eff = m.mem[self.str_] & 15;
        m.mem[self.con0] &= !0x40; // LD cannot be set in the cycle that sets EN
        self.hiz = 0;
    }

    pub fn disable(&mut self) {
        self.seq = false;
        self.started = false;
        self.wait_rise = false;
        self.ld_fall = false;
        self.dir_chg = false;
        self.hiz = 0;
    }

    /// LD set while enabled: the buffers load at the rising edge after the next falling edge.
    pub fn ld_set(&mut self) {
        self.ld_fall = false;
    }

    pub fn dir_change(&mut self) {
        self.dir_chg = true;
    }

    /// After a shutdown clear the outputs stay overridden until the next rising edge of the input.
    pub fn wait_rise_set(&mut self) {
        self.wait_rise = true;
    }

    /// A shutdown event resets the push-pull sequencer (25.2.2).
    pub fn reset_seq(&mut self) {
        self.seq = false;
    }

    /// DBR/DBF written with EN = 0: the buffers take the registers.
    pub fn load_buffers(&mut self, m: &DataMem) {
        self.dbr_buf = m.mem[self.dbr] & 63;
        self.dbf_buf = m.mem[self.dbf] & 63;
    }
}
