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
    /// PIC18 NOSC/NDIV clock switch in progress (Q10/Q41): `[A157]`.
    delayed: bool,
    pending: Option<Pending>,
    /// PIC18(L)F2x/4xK22 SCS/IRCF/PLL change in progress: the cycle the new clock is ready and the OSCCON, OSCCON2 and OSCTUNE
    /// values of the clock that keeps running until then (`[A157]`).
    k22_pending: Option<(u64, [u8; 3])>,
}

/// The clock a K22 selects, for the start-up time of a switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum K22Src {
    /// LP, XT or HS on OSC1/OSC2: the OST counts 1024 oscillations.
    Crystal,
    /// EC or RC: no OST.
    Ext,
    Sosc,
    Lf,
    /// HFINTOSC or MFINTOSC.
    Int,
}

/// A requested clock switch: the cycle at which the NOSC-selected oscillator (and its PLL) is ready, and whether NOSCR/CSWIF
/// have been raised yet (CSWHOLD = 1 keeps the switch waiting after that).
#[derive(Clone, Copy, Debug)]
struct Pending {
    ready_at: u64,
    noscr: bool,
}

/// What a write or a poll did to a clock switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SwitchEvent {
    /// NOSCR rose: the new oscillator is ready, so CSWIF is set (DS40002043F 5.3.1 p.48, DS40002214G 12.2.3).
    pub ready: bool,
    /// The switch happened: OSCCON2 now equals OSCCON1 and the clock frequency changed.
    pub done: bool,
}

/// Timing facts the core hands to a write or a poll.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub now: u64,
    /// Frequency the part runs at now: instruction cycles pass at this rate while the new oscillator starts.
    pub cur_mhz: f64,
    /// Frequency assumed for an external oscillator (its real value is not known to the model).
    pub ext_mhz: f64,
}

impl Oscillator {
    pub fn new(d: &Device) -> Self {
        let k22 = d.find_sfr("OSCCON").is_some_and(|s| s.fields.iter().any(|f| f.name == "IRCF"));
        Self {
            regs: ["OSCCON", "OSCCON1", "OSCCON2", "OSCCON3", "OSCFRQ", "OSCEN", "OSCSTAT", "OSCSTAT1", "OSCTUNE"]
                .into_iter()
                .filter_map(|n| d.find_sfr(n).map(|s| (n.into(), s.addr as usize, s.implemented.unwrap_or(255) as u8)))
                .collect(),
            pic16: d.device.family == "pic16e",
            legacy: d.find_sfr("OSCFRQ").is_some_and(|s| s.fields.iter().any(|f| f.name == "HFFRQ")) && d.device.family == "pic16e",
            k22,
            q41: d.find_sfr("SHADCON").is_some(),
            sosc: d.find_sfr("OSCEN").is_some_and(|s| s.fields.iter().any(|f| f.name == "SOSCEN")),
            // RSTOSC=001 is reserved on the 171xx; 131xx/132xx add HFINTOSC at 16 MHz.
            hf16_reset: d.device.name.contains("F131") || d.device.name.contains("F132"),
            delayed: !k22 && d.find_sfr("OSCCON1").is_some() && d.find_sfr("OSCCON3").is_some(),
            pending: None,
            k22_pending: None,
        }
    }
    /// A NOSC/NDIV write starts a timed switch (`[A157]`).
    pub fn delayed(&self) -> bool {
        self.delayed
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

    /// Write model-owned registers. With a `Timing` a switch takes the oscillator start-up time (`[A157]`); without one it is instantaneous, optionally held by CSWHOLD.
    pub fn write_at(&mut self, m: &mut [u8], a: usize, v: u8, cfg: &Config, t: Option<Timing>) -> SwitchEvent {
        let mut ev = SwitchEvent::default();
        let before = if self.k22 { self.k22_view(m) } else { [0; 3] };
        if ["OSCCON2", "OSCSTAT", "OSCSTAT1"].iter().any(|n| self.addr(n) == Some(a)) && !self.k22 {
            return ev;
        }
        if self.addr("OSCCON") == Some(a) && !self.k22 {
            return ev;
        }
        let con1 = self.addr("OSCCON1") == Some(a);
        if con1 {
            if cfg.get("CSWEN") == Some(0) || v & 15 > 9 {
                return ev;
            }
            let src = v >> 4 & 7;
            if src == (if self.legacy { 3 } else { 4 }) && !self.sosc {
                return ev;
            }
            if !(if self.legacy {
                matches!(src, 0 | 1 | 3 | 4 | 6 | 7)
            } else {
                matches!(src, 2 | 4 | 5 | 6 | 7)
            }) {
                return ev;
            }
        }
        let con3 = self.addr("OSCCON3") == Some(a);
        let v = if con3 && self.delayed { v & !0x18 | m[a] & 0x18 } else { v }; // ORDY and NOSCR are read only
        m[a] = v;
        if self.k22 && let Some(t) = t && ["OSCCON", "OSCCON2", "OSCTUNE"].iter().any(|n| self.addr(n) == Some(a)) {
            self.k22_request(m, cfg, before, t);
        }
        if !self.k22 && self.addr("OSCCON1").is_some() {
            match t.filter(|_| self.delayed) {
                None => {
                    if self.get(m, "OSCCON3") & 0x80 == 0 {
                        self.set(m, "OSCCON2", self.get(m, "OSCCON1"));
                        self.set(m, "OSCCON3", self.get(m, "OSCCON3") | 0x10);
                    }
                }
                Some(t) => {
                    if con1 {
                        ev = self.request(m, cfg, t);
                    }
                    let p = self.poll(m, t.now);
                    ev = SwitchEvent { ready: ev.ready || p.ready, done: ev.done || p.done };
                }
            }
        }
        self.status(m);
        ev
    }

    /// NOSC/NDIV written (`[A157]`; DS40002043F 5.3.1 p.48, DS40002214G 12.2.2-12.2.4 p.185): the request equal to the current
    /// selection (COSC/CDIV) abandons any switch; a divider-only change is ready at once; otherwise the new source needs its
    /// start-up time, during which the old clock keeps running and ORDY is 0.
    fn request(&mut self, m: &mut [u8], cfg: &Config, t: Timing) -> SwitchEvent {
        let (new, cur) = (self.get(m, "OSCCON1"), self.get(m, "OSCCON2"));
        let c3 = self.get(m, "OSCCON3");
        if new == cur {
            self.pending = None;
            self.set(m, "OSCCON3", c3 & !0x08 | 0x10);
            return SwitchEvent::default();
        }
        let us = if new >> 4 & 7 == cur >> 4 & 7 { 0.0 } else { self.startup_us(new >> 4 & 7, cfg, t.ext_mhz) };
        let ready_at = t.now + (us * t.cur_mhz / 4.0).ceil() as u64;
        self.pending = Some(Pending { ready_at, noscr: false });
        self.set(m, "OSCCON3", c3 & !0x18);
        SwitchEvent::default()
    }

    /// Start-up time of the oscillator a NOSC value selects, in microseconds. Sources: the OST counts 1024 oscillations for
    /// LP, XT and HS and is not used with the EC modes (DS40002043F 5.2.1.3 p.44, 5.4.4 p.51, RST05 p.704; DS40001839E 7.2.1.3
    /// p.82, 7.4.4 p.88; DS40002343F 11.1.1.3 p.87); the PLL locks in 200 us typical (PLL03: DS40002043F p.701, DS40002214G
    /// p.900, DS40002343F p.631, DS40002686B p.606, DS40001839E Table 35-9 p.425) once its input is ready. The data sheets give
    /// no time for HFINTOSC, LFINTOSC or the secondary oscillator: those are ready at once. NOSC codes: the 4x PLL on EXTOSC is
    /// 010 (001 on the PIC16F183xx) and EXTOSC is 111; the PIC16F183xx also has HFINTOSC with the 2x PLL (000).
    fn startup_us(&self, src: u8, cfg: &Config, ext_mhz: f64) -> f64 {
        let crystal = cfg.get("FEXTOSC").is_some_and(|f| f <= 2);
        let (ext, pll) = if self.legacy { (matches!(src, 1 | 7), matches!(src, 0 | 1)) } else { (matches!(src, 2 | 7), src == 2) };
        (if ext && crystal { 1024.0 / ext_mhz } else { 0.0 }) + if pll { 200.0 } else { 0.0 }
    }

    /// The K22 oscillator registers `[OSCCON, OSCCON2, OSCTUNE]` as the running clock sees them: the values from before a
    /// pending switch until the new clock is ready (`[A157]`).
    pub fn k22_view(&self, m: &[u8]) -> [u8; 3] {
        self.k22_pending.map_or_else(|| ["OSCCON", "OSCCON2", "OSCTUNE"].map(|n| self.get(m, n)), |(_, old)| old)
    }

    fn k22_src(&self, v: [u8; 3], cfg: &Config) -> (K22Src, bool) {
        let [c, c2, tune] = v;
        let fosc = cfg.get("FOSC").unwrap_or(0);
        let internal = if c >> 4 & 7 == 0 && tune & 0x80 == 0 && c2 & 0x10 == 0 { K22Src::Lf } else { K22Src::Int };
        match c & 3 {
            1 => (K22Src::Sosc, false),
            2 | 3 => (internal, false),
            _ if fosc & 14 == 8 => (internal, tune & 0x40 != 0),
            _ if fosc >= 8 => (K22Src::Ext, tune & 0x40 != 0),
            _ => (if fosc <= 3 { K22Src::Crystal } else { K22Src::Ext }, cfg.get("PLLCFG").is_some_and(|p| p != 0)),
        }
    }

    /// OSCCON, OSCCON2 or OSCTUNE written (`[A157]`; DS40001412H 2.11.3 p.40, Figure 2-9 p.41, Table 2-2 p.32, 2.10 p.38, Table
    /// 27-11 p.440): the old clock runs until the new one is ready. Ready after TOST = 1024 oscillations for LP/XT/HS, TPLL =
    /// 2 ms more for the PLL on an external oscillator, TIOBST = 0.25 ms (typical) for the HFINTOSC/MFINTOSC when it was not
    /// running; a change within the running INTOSC (IRCF) is immediate (3.2.3 p.45).
    fn k22_request(&mut self, m: &[u8], cfg: &Config, before: [u8; 3], t: Timing) {
        let after = ["OSCCON", "OSCCON2", "OSCTUNE"].map(|n| self.get(m, n));
        let (from, to) = (self.k22_src(before, cfg), self.k22_src(after, cfg));
        let mut us = 0.0;
        if from != to {
            if to.0 == K22Src::Crystal {
                us += 1024.0 / t.ext_mhz;
            }
            if to.0 == K22Src::Int && from.0 != K22Src::Int {
                us += 250.0;
            }
            if to.1 && !from.1 && matches!(to.0, K22Src::Crystal | K22Src::Ext) {
                us += 2000.0;
            }
        }
        self.k22_pending = (us > 0.0).then(|| (t.now + (us * t.cur_mhz / 4.0).ceil() as u64, before));
    }

    /// Advance a pending switch to cycle `now`: when the new oscillator is ready NOSCR rises (and CSWIF is set); the switch
    /// completes then, unless CSWHOLD holds it (DS40002043F 5.3.1 p.48, Figures 5-6, 5-7 p.49).
    pub fn poll(&mut self, m: &mut [u8], now: u64) -> SwitchEvent {
        let mut ev = SwitchEvent::default();
        if let Some((ready, _)) = self.k22_pending {
            if now >= ready {
                self.k22_pending = None;
                ev.done = true;
            }
            return ev;
        }
        let Some(mut p) = self.pending else { return ev };
        if now < p.ready_at {
            return ev;
        }
        if !p.noscr {
            p.noscr = true;
            ev.ready = true;
        }
        let c3 = self.get(m, "OSCCON3");
        if c3 & 0x80 == 0 {
            self.set(m, "OSCCON2", self.get(m, "OSCCON1"));
            self.set(m, "OSCCON3", c3 & !0x08 | 0x10);
            self.pending = None;
            ev.done = true;
            self.status(m);
        } else {
            self.set(m, "OSCCON3", c3 | 0x08);
            self.pending = Some(p);
        }
        ev
    }

    /// A reset abandons a switch.
    pub fn cancel(&mut self) {
        self.pending = None;
        self.k22_pending = None;
    }

    /// None: an external source or a reserved selection has no derivable frequency.
    pub fn mhz(&self, m: &[u8], cfg: &Config) -> Option<f64> {
        if self.k22 {
            let [c, _, tune] = self.k22_view(m);
            let scs = c & 3;
            if scs == 1 {
                return Some(0.032768);
            }
            if !(scs >= 2 || cfg.get("FOSC").is_some_and(|v| v & 14 == 8)) {
                return None;
            }
            let ircf = (c >> 4 & 7) as usize;
            let f = [0.03125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0][ircf];
            return Some(f * if scs == 0 && tune & 0x40 != 0 { 4.0 } else { 1.0 });
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
