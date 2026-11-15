//! Nominal system oscillator frequencies. Sources: docs/FACTS-SIM.md, Firmware clock timing.
use crate::config::Config;
use devdesc::Device;

#[derive(Clone, Debug)]
pub(crate) struct Oscillator {
    regs: Vec<(String, usize, u8)>,
    pic16: bool,
    legacy: bool,
    k22: bool,
    q41: bool,
    sosc: bool,
    hf16_reset: bool,
}

impl Oscillator {
    pub fn new(d: &Device) -> Self {
        Self {
            regs: ["OSCCON", "OSCCON1", "OSCCON2", "OSCCON3", "OSCFRQ", "OSCEN", "OSCSTAT", "OSCSTAT1", "OSCTUNE"]
                .into_iter()
                .filter_map(|n| d.find_sfr(n).map(|s| (n.into(), s.addr as usize, s.implemented.unwrap_or(255) as u8)))
                .collect(),
            pic16: d.device.family == "pic16e",
            legacy: d.find_sfr("OSCFRQ").is_some_and(|s| s.fields.iter().any(|f| f.name == "HFFRQ")) && d.device.family == "pic16e",
            k22: d.find_sfr("OSCCON").is_some_and(|s| s.fields.iter().any(|f| f.name == "IRCF")),
            q41: d.find_sfr("SHADCON").is_some(),
            sosc: d.find_sfr("OSCEN").is_some_and(|s| s.fields.iter().any(|f| f.name == "SOSCEN")),
            // RSTOSC=001 is reserved on the 171xx; 131xx/132xx add HFINTOSC at 16 MHz.
            hf16_reset: d.device.name.contains("F131") || d.device.name.contains("F132"),
        }
    }
    fn addr(&self, n: &str) -> Option<usize> {
        self.regs.iter().find(|(s, _, _)| s == n).map(|(_, a, _)| *a)
    }
    pub fn addresses(&self) -> impl Iterator<Item = usize> + '_ {
        self.regs.iter().map(|(_, a, _)| *a)
    }
    pub fn contains(&self, a: usize) -> bool {
        self.addresses().any(|r| a == r)
    }
    fn get(&self, m: &[u8], n: &str) -> u8 {
        self.addr(n).map_or(0, |a| m[a])
    }
    fn set(&self, m: &mut [u8], n: &str, v: u8) {
        if let Some((_, a, mask)) = self.regs.iter().find(|(s, _, _)| s == n) {
            m[*a] = v & mask;
        }
    }

    pub fn reset(&self, m: &mut [u8], cfg: &Config) {
        let Some(r) = cfg.get("RSTOSC") else { return };
        let (src, div, frq) = if self.legacy {
            (if r == 6 { 6 } else { r as u8 }, if r == 6 { 2 } else { 0 }, if r == 6 { 3 } else { 6 })
        } else if self.pic16 {
            if self.addr("OSCCON").is_some() {
                // PIC16F152xx: COSC only, no run-time source switching
                (r as u8, 0, if r == 2 { 0 } else { 5 })
            } else {
                (
                    if (r == 0 || (r == 1 && self.hf16_reset)) && self.addr("OSCCON1").is_some() {
                        6
                    } else {
                        r as u8
                    },
                    if r == 6 && self.addr("OSCCON1").is_some() { 2 } else { 0 },
                    if r == 0 {
                        5
                    } else if r == 1 {
                        4
                    } else if r == 6 && self.addr("OSCCON1").is_none() {
                        0
                    } else {
                        2
                    },
                )
            }
        } else {
            return;
        }; // PIC18 reset values already supplied by config::reset_sfrs
        self.set(m, "OSCCON1", src << 4 | div);
        self.set(m, "OSCCON2", src << 4 | div);
        self.set(m, "OSCCON", src << 4);
        self.set(m, "OSCFRQ", frq);
        self.status(m);
    }

    pub fn status(&self, m: &mut [u8]) {
        if self.k22 {
            return;
        }
        let c = if self.addr("OSCCON2").is_some() {
            self.get(m, "OSCCON2")
        } else {
            self.get(m, "OSCCON")
        };
        let src = c >> 4 & 7;
        let active = if self.legacy {
            match src {
                0 => 0x41,
                6 => 0x40,
                4 => 0x10,
                3 => 8,
                _ => 0x80,
            }
        } else if self.addr("OSCCON").is_some() {
            if src == 1 {
                0x10
            } else if src == 3 {
                0x80
            } else {
                0x40
            }
        } else {
            match src {
                0 | 6 => 0x40,
                5 => 0x10,
                4 => 8,
                2 => 0x81,
                _ => 0x80,
            }
        };
        let ready = self.get(m, "OSCEN") | active;
        self.set(m, "OSCSTAT", ready);
        self.set(m, "OSCSTAT1", ready & 0xDD);
    }

    /// Write model-owned registers. Switching is instantaneous, optionally held by CSWHOLD.
    pub fn write(&self, m: &mut [u8], a: usize, v: u8, cfg: &Config) {
        if ["OSCCON2", "OSCSTAT", "OSCSTAT1"].iter().any(|n| self.addr(n) == Some(a)) && !self.k22 {
            return;
        }
        if self.addr("OSCCON") == Some(a) && !self.k22 {
            return;
        }
        if self.addr("OSCCON1") == Some(a) {
            if cfg.get("CSWEN") == Some(0) || v & 15 > 9 {
                return;
            }
            let src = v >> 4 & 7;
            if src == (if self.legacy { 3 } else { 4 }) && !self.sosc {
                return;
            }
            if !(if self.legacy {
                matches!(src, 0 | 1 | 3 | 4 | 6 | 7)
            } else {
                matches!(src, 2 | 4 | 5 | 6 | 7)
            }) {
                return;
            }
        }
        m[a] = v;
        if !self.k22 && self.addr("OSCCON1").is_some() && self.get(m, "OSCCON3") & 0x80 == 0 {
            self.set(m, "OSCCON2", self.get(m, "OSCCON1"));
            self.set(m, "OSCCON3", self.get(m, "OSCCON3") | 0x10);
        }
        self.status(m);
    }

    /// None: an external source or a reserved selection has no derivable frequency.
    pub fn mhz(&self, m: &[u8], cfg: &Config) -> Option<f64> {
        if self.k22 {
            let c = self.get(m, "OSCCON");
            let scs = c & 3;
            if scs == 1 {
                return Some(0.032768);
            }
            if !(scs >= 2 || cfg.get("FOSC").is_some_and(|v| v & 14 == 8)) {
                return None;
            }
            let ircf = (c >> 4 & 7) as usize;
            let f = [0.03125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0][ircf];
            return Some(f * if scs == 0 && self.get(m, "OSCTUNE") & 0x40 != 0 { 4.0 } else { 1.0 });
        }
        let c = if self.addr("OSCCON2").is_some() {
            self.get(m, "OSCCON2")
        } else {
            self.get(m, "OSCCON")
        };
        let src = c >> 4 & 7;
        let frq = self.get(m, "OSCFRQ") as usize;
        let hf = if self.legacy {
            [Some(1.0), Some(2.0), None, Some(4.0), Some(8.0), Some(12.0), Some(16.0), Some(32.0)]
                .get(frq.min(7))
                .copied()
                .flatten()
        } else if self.pic16 {
            [1.0, 2.0, 4.0, 8.0, 16.0, 32.0].get(frq).copied()
        } else {
            [1.0, 2.0, 4.0, 8.0, 12.0, 16.0, 32.0, 48.0, 64.0].get(frq).copied()
        };
        let f = if self.legacy {
            match src {
                0 => hf.filter(|f| [8.0, 12.0, 16.0].contains(f)).map(|f| f * 2.0),
                6 => hf,
                4 => Some(0.031),
                3 if self.sosc => Some(0.032768),
                _ => None,
            }
        } else if self.addr("OSCCON").is_some() {
            match src {
                0 | 2 => hf,
                1 => Some(0.031),
                _ => None,
            }
        } else {
            match src {
                6 => hf,
                0 if self.q41 || (self.pic16 && self.addr("OSCCON1").is_none()) => hf,
                5 => Some(0.031),
                4 if self.sosc => Some(0.032768),
                _ => None,
            }
        };
        f.map(|f| f / f64::from(1u16 << (c & 15).min(9)))
    }
}
