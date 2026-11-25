//! The 12-bit multiple-SAR ADC of the dsPIC33CK (DS70005399D chapter 12, DS70005213): the shared core with the individual
//! channel software trigger (`ADCON3L.CNVRTCH`), results in `ADCBUFn`, the `ANnRDY` flags of `ADSTATL`/`ADSTATH` (cleared by
//! reading `ADCBUFn`) and the channel interrupt requests. Input voltages come from `[[adc]]` entries of a stimulus file as
//! conversion codes. Triggers from other modules, scan, comparators and the oversampling filters are not modelled.

use crate::dspic::*;

const ADCON1L: usize = 0xB00;
const ADCON2H: usize = 0xB06;
const ADCON3L: usize = 0xB08;
const ADCON3H: usize = 0xB0A;
const ADIEL: usize = 0xB20;
const ADIEH: usize = 0xB22;
const ADSTATL: usize = 0xB30;
const ADSTATH: usize = 0xB32;
const ADCON5L: usize = 0xC00;
const ADCBUF0: usize = 0xC0C;
const CHANNELS: usize = 18;
/// the channel interrupt request of AN0; ANn is `ADCAN0_IRQ + n`
const ADCAN0_IRQ: usize = 91;

#[derive(Clone, Default)]
pub struct AdcState {
    /// the conversion under way: (cycle it ends, channel)
    pub conv: Option<(u64, usize)>,
}

impl Dspic {
    fn a16(&self, a: usize) -> u16 {
        u16::from(self.mem[a]) | u16::from(self.mem[a + 1]) << 8
    }

    fn a_set(&mut self, a: usize, v: u16) {
        self.mem[a] = v as u8;
        self.mem[a + 1] = (v >> 8) as u8;
    }

    pub(crate) fn adc_owns(a: usize) -> bool {
        (0xB00..0xB40).contains(&a) || (0xC00..0xC30).contains(&a)
    }

    /// The code a conversion of `ch` reads: the last `[[adc]]` entry of the channel that has started, else 0.
    fn adc_code(&self, ch: usize) -> u16 {
        let now = self.cycles;
        let given = self.p.adc_in.iter().filter(|e| usize::from(e.0) == ch && e.1 <= now).max_by_key(|e| e.1).map(|e| e.2);
        given.unwrap_or(match ch {
            17 => 1489, // the 1.2 V band gap against AVDD = 3.3 V
            16 => 1100, // the temperature diode, about 0.9 V
            _ => 0,
        }) & 0xFFF
    }

    pub(crate) fn adc_read(&mut self, a: usize) -> Option<u16> {
        match a {
            ADCON5L => {
                let v = self.a16(a);
                let on = self.a16(ADCON1L) & 0x8000 != 0 && v & 0x80 != 0;
                Some((v & !0x8000) | if on { 0x8000 } else { 0 })
            }
            ADCON3L => Some(self.a16(a)),
            x if (ADCBUF0..ADCBUF0 + 2 * CHANNELS).contains(&x) => {
                let n = (x - ADCBUF0) / 2;
                // reading the result clears the data-ready flag
                if n < 16 {
                    let v = self.a16(ADSTATL) & !(1 << n);
                    self.a_set(ADSTATL, v);
                } else {
                    let v = self.a16(ADSTATH) & !(1 << (n - 16));
                    self.a_set(ADSTATH, v);
                }
                None
            }
            _ => None,
        }
    }

    pub(crate) fn adc_write(&mut self, a: usize, v: u16, mask: u16) {
        let old = self.a16(a);
        let new = (old & !mask) | (v & mask);
        match a {
            ADCON3L => {
                // CNVRTCH starts one conversion of channel CNVCHSEL when the module and the shared core are on
                let on = self.a16(ADCON1L) & 0x8000 != 0 && self.a16(ADCON3H) & 0x80 != 0;
                let ch = usize::from(new & 0x3F);
                let mut stored = new & !0x0100;
                if new & 0x0100 != 0 && on && self.p.adc.conv.is_none() && ch < CHANNELS {
                    let samc = u64::from(self.a16(ADCON2H) & 0x3FF);
                    // sampling (SHRSAMC + 2 TADCORE) and 14 TADCORE for the conversion; TADCORE is two instruction cycles
                    // at the clock selected by the set-up of AdcInit (SHRADCS = 1)
                    let cyc = (samc + 2 + 14) * 2;
                    self.p.adc.conv = Some((self.cycles + cyc, ch));
                    stored |= 0x0100;
                }
                self.a_set(a, stored);
            }
            ADSTATL | ADSTATH => {} // read only
            ADCON5L => self.a_set(a, new & !0x8000),
            _ => self.a_set(a, new),
        }
    }

    pub(crate) fn adc_advance(&mut self) {
        let Some((end, ch)) = self.p.adc.conv else { return };
        if self.cycles < end {
            return;
        }
        self.p.adc.conv = None;
        let code = self.adc_code(ch);
        self.a_set(ADCBUF0 + 2 * ch, code);
        if ch < 16 {
            let v = self.a16(ADSTATL) | 1 << ch;
            self.a_set(ADSTATL, v);
        } else {
            let v = self.a16(ADSTATH) | 1 << (ch - 16);
            self.a_set(ADSTATH, v);
        }
        let ie = if ch < 16 { self.a16(ADIEL) >> ch & 1 } else { self.a16(ADIEH) >> (ch - 16) & 1 };
        if ie != 0 {
            self.raise_irq(ADCAN0_IRQ + ch);
        }
        let c = self.a16(ADCON3L) & !0x0100;
        self.a_set(ADCON3L, c);
    }
}
