//! PIC16 standalone 10-bit PWM: DS40002519D ch.26-27, DS40002195E ch.23,
//! DS40001839E ch.23/30, DS40002325C PWM chapter. Duty latches at Timer2/4/6
//! rollover, without the CCP model's MDB-specific pulse extension. Output is sampled
//! at instruction boundaries, rounding a fractional-cycle falling edge upward.
use devdesc::Device;

#[derive(Clone)]
pub struct Pwm16 {
    pub con: usize,
    pub dcl: usize,
    pub dch: usize,
    pub code: u8,
    pub md: Option<(usize, u8)>,
    pub selector: Option<(usize, u8)>,
    raw: bool,
    pub clear_at: Option<u64>,
}
impl Pwm16 {
    pub fn find(dev: &Device, n: u8) -> Option<Self> {
        let addr = |s: String| dev.find_sfr(&s).map(|r| r.addr as usize);
        let con = addr(format!("PWM{n}CON"))?;
        let dcl = addr(format!("PWM{n}DCL"))?;
        let dch = addr(format!("PWM{n}DCH"))?;
        let name = dev.device.name.replacen("PIC16LF", "PIC16F", 1);
        let code = if name.starts_with("PIC16F131") || name.starts_with("PIC16F132") {
            0x2B + n
        } else if name.starts_with("PIC16F152") {
            n
        } else if name.starts_with("PIC16F180") {
            8 + n
        } else if name.starts_with("PIC16F183") {
            n - 3
        } else {
            return None;
        };
        let field = |name: &str| {
            dev.sfr.iter().find_map(|r| {
                r.fields
                    .iter()
                    .find(|f| f.name == name)
                    .map(|f| (r.addr as usize, f.pos))
            })
        };
        Some(Self {
            con,
            dcl,
            dch,
            code,
            md: field(&format!("PWM{n}MD")).map(|(a, pos)| (a, 1 << pos)),
            selector: field(&format!("P{n}TSEL")),
            raw: false,
            clear_at: None,
        })
    }
    pub fn enabled(&self, m: &[u8]) -> bool {
        m[self.con] & 0x80 != 0 && !self.md.is_some_and(|(a, b)| m[a] & b != 0)
    }
    pub fn timer(&self, m: &[u8]) -> u8 {
        match self.selector.map_or(1, |(a, pos)| m[a] >> pos & 3) {
            1 => 2,
            2 => 4,
            3 => 6,
            _ => 0,
        }
    }
    pub fn output(&self, m: &[u8]) -> bool {
        self.enabled(m) && (self.raw != (m[self.con] & 0x10 != 0))
    }
    pub fn update(&mut self, m: &mut [u8]) {
        if !self.enabled(m) {
            self.raw = false;
            self.clear_at = None;
        }
        if self.output(m) {
            m[self.con] |= 0x20;
        } else {
            m[self.con] &= !0x20;
        }
    }
    pub fn apply(&mut self, m: &mut [u8], ends: &[u64], pr: u8, sh: u32, now: u64) {
        let duty = (u64::from(m[self.dch]) << 2) | u64::from(m[self.dcl] >> 6);
        let pulse = (duty << sh).div_ceil(4);
        for &at in ends {
            self.raw = duty != 0;
            self.clear_at = (duty != 0 && duty < 4 * (u64::from(pr) + 1)).then_some(at + pulse);
        }
        if self.clear_at.is_some_and(|at| at <= now) {
            self.raw = false;
            self.clear_at = None;
        }
        self.update(m);
    }
}
