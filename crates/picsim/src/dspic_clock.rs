//! Oscillator and reset of the dsPIC33CK: `OSCCON` with its unlock sequences and clock switching, the FRC divider and the
//! PLL (DS70005399D section 9), `RCON` and the watchdog timer. Sources: Registers 9-1 to 9-5 and Equations 9-1, 9-2.

use crate::dspic::*;

const OSCCON: usize = 0xF84;
const CLKDIV: usize = 0xF86;
const PLLFBD: usize = 0xF88;
const PLLDIV: usize = 0xF8A;

/// Instruction cycles a clock switch takes before COSC follows NOSC.
const SWITCH_CYCLES: u64 = 64;

#[derive(Default, Clone)]
pub struct Clock {
    /// 0 idle, 1 saw the first key byte, 2 armed (the next write goes through): high and low byte of OSCCON
    pub hi: u8,
    pub lo: u8,
    pub switch_at: Option<u64>,
    /// the oscillator's own frequency in MHz when it is a crystal or an external clock (not modelled: 8)
    pub wdt_run: bool,
}

impl Dspic {
    fn rd(&self, a: usize) -> u16 {
        u16::from(self.mem[a]) | u16::from(self.mem[a + 1]) << 8
    }

    fn wr(&mut self, a: usize, v: u16) {
        self.mem[a] = v as u8;
        self.mem[a + 1] = (v >> 8) as u8;
    }

    /// COSC/NOSC from `FNOSC` and LOCK after a Reset.
    pub fn clock_reset(&mut self) {
        self.clock = Clock::default();
        let n = self.config_field("FNOSC") as u16 & 7;
        let lock = if n == 1 || n == 3 { 0x20 } else { 0 };
        let v = (self.rd(OSCCON) & !0x7700 & !0x20) | n << 12 | n << 8 | lock;
        self.wr(OSCCON, v);
        self.p.wdt_count = 0;
        // WDTCONL: SLPDIV, CLKSEL and RUNDIV follow the configuration words after a Reset
        let w = (self.rd(self.p.a.wdtconl) & 0x8001)
            | (self.config_field("SWDTPS") as u16 & 0x1F) << 1
            | (self.config_field("RCLKSEL") as u16 & 3) << 6
            | (self.config_field("RWDTPS") as u16 & 0x1F) << 8;
        if self.p.a.wdtconl != 0 {
            self.wr(self.p.a.wdtconl, w);
        }
    }

    /// The system oscillator frequency in MHz.
    pub fn fosc_mhz(&self) -> f64 {
        if let Some(f) = self.p.fosc_override {
            return f;
        }
        let con = self.rd(OSCCON);
        let cosc = (con >> 12) & 7;
        match cosc {
            0 | 4 | 6 => 8.0,
            7 => 8.0 / [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 256.0][((self.rd(CLKDIV) >> 8) & 7) as usize],
            5 => 0.032,
            1 | 3 => {
                let n1 = f64::from(self.rd(CLKDIV) & 0xF).max(1.0);
                let m = f64::from(self.rd(PLLFBD) & 0xFF);
                let d = self.rd(PLLDIV);
                let n2 = f64::from((d >> 4) & 7).max(1.0);
                let n3 = f64::from(d & 7).max(1.0);
                8.0 / n1 * m / (2.0 * n2 * n3)
            }
            _ => 8.0,
        }
    }

    /// A write to OSCCON; `mask` says which bytes. Returns true when it was handled.
    pub(crate) fn osccon_write(&mut self, v: u16, mask: u16) -> bool {
        let hi_write = mask & 0xFF00 != 0;
        let lo_write = mask & 0x00FF != 0;
        let mut old = self.rd(OSCCON);
        if hi_write {
            let b = (v >> 8) as u8;
            match (self.clock.hi, b) {
                (0, 0x78) => self.clock.hi = 1,
                (1, 0x9A) => self.clock.hi = 2,
                (2, _) => {
                    // the instruction after the sequence: NOSC is writable
                    old = (old & !0x0700) | (v & 0x0700);
                    self.clock.hi = 0;
                }
                _ => self.clock.hi = 0,
            }
        }
        if lo_write {
            let b = v as u8;
            match (self.clock.lo, b) {
                (0, 0x46) => self.clock.lo = 1,
                (1, 0x57) => self.clock.lo = 2,
                (2, _) => {
                    // CLKLOCK (7) and OSWEN (0) are writable, CF (3) clearable
                    old = (old & !0x0089) | (v & 0x0081) | (old & 0x8 & v);
                    self.clock.lo = 0;
                    if v & 1 != 0 {
                        self.clock.switch_at = Some(self.cycles + SWITCH_CYCLES);
                    }
                }
                _ => self.clock.lo = 0,
            }
        }
        self.wr(OSCCON, old);
        true
    }

    /// Completes a clock switch when its time has come; sets the watchdog's timing too.
    pub(crate) fn clock_advance(&mut self) {
        if let Some(t) = self.clock.switch_at
            && self.cycles >= t
        {
            self.clock.switch_at = None;
            let con = self.rd(OSCCON);
            let nosc = (con >> 8) & 7;
            let lock = if nosc == 1 || nosc == 3 { 0x20 } else { 0 };
            self.wr(OSCCON, (con & !0x7021) | nosc << 12 | lock);
        }
    }

    // ---- reset and watchdog ----

    /// A Reset that keeps program and RAM: the cause bits in RCON are set as given.
    pub fn reset_with(&mut self, rcon_bits: u16) {
        let keep = self.rd(0xF80) & !0x0003;
        self.apply_por();
        // RCON: the flags accumulate across resets; POR/BOR clear only on a power-on
        let v = (self.rd(0xF80) & !0xC0DF) | (keep & 0xC0DF) | rcon_bits;
        self.wr(0xF80, v);
    }

    fn wdt_enabled(&self) -> bool {
        self.config_field("FWDTEN") == 1 || self.rd(self.p.a.wdtconl) & 0x8000 != 0
    }

    /// Watchdog period in instruction cycles: the LPRC (32 kHz) divided by 32 gives 1 ms steps; running 2^RWDTPS ms,
    /// asleep 2^SLPDIV ms [DS70005250 3.1, Table 3-1; DS70005399D Register 28-19].
    fn wdt_period_cycles(&self) -> u64 {
        let ms = if self.sleeping.is_some() {
            let ps = if self.config_field("FWDTEN") == 1 { self.config_field("SWDTPS") } else { u32::from((self.rd(self.p.a.wdtconl) >> 1) & 0x1F) };
            1u64 << ps.min(31)
        } else {
            let ps = if self.config_field("FWDTEN") == 1 { self.config_field("RWDTPS") } else { u32::from((self.rd(self.p.a.wdtconl) >> 8) & 0x1F) };
            1u64 << ps.min(31)
        };
        (ms as f64 * self.fosc_mhz() * 1000.0 / 2.0) as u64
    }

    pub(crate) fn wdt_advance(&mut self, dc: u64) {
        if !self.wdt_enabled() {
            return;
        }
        self.p.wdt_count += dc;
        let period = self.wdt_period_cycles().max(1);
        if self.p.wdt_count >= period {
            self.p.wdt_count = 0;
            let rcon = 0xF80;
            self.mem[rcon] |= 0x10; // WDTO
            if self.sleeping.is_some() {
                self.p.wdt_woke = true;
            } else {
                self.reset_with(0x0010);
            }
        }
    }
}
