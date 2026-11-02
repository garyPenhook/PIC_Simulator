//! Peripheral models that live on the instruction clock: Timer0, Timer1, Timer2 and the interrupt flag
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
use std::collections::BTreeSet;

/// A flag bit in a PIRx register.
#[derive(Clone, Copy, Debug)]
struct Flag {
    addr: usize,
    mask: u8,
}

impl Flag {
    fn find(dev: &Device, bit: &str) -> Option<Flag> {
        for s in dev.sfr.iter().filter(|s| s.name.starts_with("PIR")) {
            if let Some(f) = s.fields.iter().find(|f| f.name == bit) {
                return Some(Flag { addr: s.addr as usize, mask: 1 << f.pos });
            }
        }
        None
    }
    fn set(self, m: &mut DataMem) {
        m.mem[self.addr] |= self.mask;
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
const T1H: u8 = 6;
const T1C: u8 = 7;
const T1CLK: u8 = 8;
const T2T: u8 = 9;
const T2P: u8 = 10;
const T2C: u8 = 11;
const T2HLT: u8 = 12;
const T2CLK: u8 = 13;

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

pub struct Periph {
    t0: Option<Timer0>,
    t1: Option<Timer1>,
    t2: Option<Timer2>,
    pub notes: BTreeSet<String>,
    /// Cycle of the latest flag set by `sync` since the core last looked (`take_event`).
    event: Option<u64>,
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
        let mut p = Periph { t0: None, t1: None, t2: None, notes: BTreeSet::new(), event: None };
        let mut hook = |a: usize, k: u8| m.special[a] = Special::Hook(k);
        if let (Some(l), Some(h), Some(c0), Some(c1), Some(flag)) =
            (addr(dev, "TMR0L"), addr(dev, "TMR0H"), addr(dev, "T0CON0"), addr(dev, "T0CON1"), Flag::find(dev, "TMR0IF"))
        {
            for (a, k) in [(l, T0L), (h, T0H), (c0, T0C0), (c1, T0C1)] {
                hook(a, k);
            }
            p.t0 = Some(Timer0 { l, h, c0, c1, flag, cnt: 0, cmp: 0xFF, pre: 0, post: 0, last: 0 });
        }
        if let (Some(l), Some(h), Some(con), Some(clk), Some(gcon), Some(flag)) = (
            addr(dev, "TMR1L"),
            addr(dev, "TMR1H"),
            addr(dev, "T1CON"),
            addr(dev, "T1CLK"),
            addr(dev, "T1GCON"),
            Flag::find(dev, "TMR1IF"),
        ) {
            for (a, k) in [(l, T1L), (h, T1H), (con, T1C), (clk, T1CLK)] {
                hook(a, k);
            }
            p.t1 = Some(Timer1 { l, h, con, clk, gcon, flag, cnt: 0, pre: 0, last: 0 });
        }
        if let (Some(tmr), Some(pr), Some(con), Some(hlt), Some(clk), Some(flag)) = (
            addr(dev, "T2TMR"),
            addr(dev, "T2PR"),
            addr(dev, "T2CON"),
            addr(dev, "T2HLT"),
            addr(dev, "T2CLKCON"),
            Flag::find(dev, "TMR2IF"),
        ) {
            for (a, k) in [(tmr, T2T), (pr, T2P), (con, T2C), (hlt, T2HLT), (clk, T2CLK)] {
                hook(a, k);
            }
            p.t2 = Some(Timer2 { tmr, pr, con, hlt, clk, flag, cnt: 0, pre: 0, post: 0, last: 0 });
        }
        p
    }

    /// Bring every running timer to cycle `now`.
    pub fn sync(&mut self, m: &mut DataMem, now: u64) {
        let mut ev: Vec<u64> = Vec::new();
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
        if let Some(t) = &mut self.t1 {
            let (base, pre0) = (t.last, t.pre);
            let dt = now - t.last;
            t.last = now;
            let con = m.mem[t.con];
            if con & 1 != 0 && dt > 0 {
                let cs = m.mem[t.clk] & 0x1F;
                if cs != 1 && cs != 2 {
                    self.notes.insert(format!("Timer1 clock source CS={cs} is not modelled (only FOSC/4 and FOSC); it does not count"));
                } else {
                    let ticks = if cs == 2 { dt * 4 } else { dt };
                    let clocks = prescale(&mut t.pre, u32::from((con >> 4) & 3), ticks);
                    let sh = u32::from((con >> 4) & 3);
                    let ratio = if cs == 2 { 4 } else { 1 };
                    let total = u64::from(t.cnt) + clocks;
                    if total >= 65536 {
                        t.flag.set(m);
                        let last_wrap = (total / 65536) * 65536 - u64::from(t.cnt);
                        ev.push(at(base, pre0, sh, last_wrap, ratio));
                    }
                    t.cnt = total as u16;
                }
            }
            if m.mem[t.con] & 2 == 0 {
                m.mem[t.h] = (t.cnt >> 8) as u8;
            }
            m.mem[t.l] = t.cnt as u8;
        }
        if let Some(t) = &mut self.t2 {
            let (base, pre0) = (t.last, t.pre);
            let dt = now - t.last;
            t.last = now;
            let con = m.mem[t.con];
            if con & 0x80 != 0 && dt > 0 {
                let cs = m.mem[t.clk] & 0x1F;
                let mode = m.mem[t.hlt] & 0x1F;
                if cs != 1 && cs != 2 {
                    self.notes.insert(format!("Timer2 clock source CS={cs} is not modelled (only FOSC/4 and FOSC); it does not count"));
                } else {
                    if mode != 0 && mode != 8 {
                        self.notes.insert(format!("Timer2 MODE={mode:#04x} needs an external reset/gate source (S4); run as the software mode"));
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
        for e in ev {
            self.raised(e);
        }
    }

    pub fn read(&mut self, k: u8, m: &mut DataMem) -> u8 {
        match k {
            T0L => {
                let t = self.t0.as_ref().unwrap();
                if m.mem[t.c0] & 0x10 != 0 {
                    m.mem[t.h] = (t.cnt >> 8) as u8;
                }
                t.cnt as u8
            }
            T1L => {
                let t = self.t1.as_ref().unwrap();
                if m.mem[t.con] & 2 != 0 {
                    m.mem[t.h] = (t.cnt >> 8) as u8;
                }
                t.cnt as u8
            }
            T1H => {
                let t = self.t1.as_ref().unwrap();
                if m.mem[t.con] & 2 != 0 { m.mem[t.h] } else { (t.cnt >> 8) as u8 }
            }
            T2T => self.t2.as_ref().unwrap().cnt,
            _ => unreachable!("read of hook {k} has no model"),
        }
    }

    /// Plain registers (TMR0H, T0CON0/1, T1CON, ...) read the stored byte.
    pub fn is_stored(k: u8) -> bool {
        !matches!(k, T0L | T1L | T1H | T2T)
    }

    /// A write: `v` is already masked to the implemented bits. The caller has synced to the writing cycle.
    pub fn write(&mut self, k: u8, v: u8, m: &mut DataMem) {
        match k {
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
            T1L => {
                let t = self.t1.as_mut().unwrap();
                t.pre = 0;
                if m.mem[t.con] & 2 != 0 {
                    t.cnt = u16::from(m.mem[t.h]) << 8 | u16::from(v);
                } else {
                    t.cnt = t.cnt & 0xFF00 | u16::from(v);
                }
                m.mem[t.l] = v;
            }
            T1H => {
                let t = self.t1.as_mut().unwrap();
                t.pre = 0;
                if m.mem[t.con] & 2 == 0 {
                    t.cnt = t.cnt & 0xFF | u16::from(v) << 8;
                }
                m.mem[t.h] = v;
            }
            T1C => {
                let t = self.t1.as_mut().unwrap();
                m.mem[t.con] = v;
                if v & 1 != 0 && m.mem[t.gcon] & 0x80 != 0 {
                    self.notes.insert("Timer1 gate (GE) is not modelled; the timer counts regardless of the gate".into());
                }
            }
            T1CLK | T2P | T2HLT | T2CLK => {
                let a = self.addr_of(k);
                m.mem[a] = v;
            }
            T2T => {
                let t = self.t2.as_mut().unwrap();
                t.cnt = v;
                t.pre = 0;
                t.post = 0;
                m.mem[t.tmr] = v;
            }
            T2C => {
                let t = self.t2.as_mut().unwrap();
                m.mem[t.con] = v;
                t.pre = 0;
                t.post = 0;
            }
            _ => unreachable!("write of hook {k} has no model"),
        }
    }

    fn addr_of(&self, k: u8) -> usize {
        match k {
            T1CLK => self.t1.as_ref().unwrap().clk,
            T2P => self.t2.as_ref().unwrap().pr,
            T2HLT => self.t2.as_ref().unwrap().hlt,
            _ => self.t2.as_ref().unwrap().clk,
        }
    }
}
