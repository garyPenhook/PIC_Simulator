//! Q41 16-bit PWM, DS40002214G ch.30 pp.441-465. One two-output slice per module.
//! FOSC clock, prescale/postscale, buffered PR/P1/P2, LD, left/right alignment and
//! pulsed/toggled compare. Other clock/mode/reset/load sources are diagnosed, not guessed.
use super::{Flag, addr};
use crate::mem::DataMem;
use devdesc::Device;
use std::collections::BTreeSet;

#[derive(Clone)]
pub struct PwmQ {
    pub n: u8,
    pub regs: [usize; 15],
    period_flag: Flag,
    group_flag: Flag,
    md: Option<(usize, u8)>,
    last: u64,
    pre: u16,
    post: u16,
    count: u16,
    values: [u16; 3],
    on: bool,
    seq: bool,
    out: [bool; 2],
    toggle: [bool; 2],
}
const NAMES: [&str; 15] = [
    "ERS", "CLK", "LDS", "PRL", "PRH", "CPRE", "PIPOS", "GIR", "GIE", "CON", "S1CFG", "S1P1L", "S1P1H", "S1P2L", "S1P2H",
];
impl PwmQ {
    pub fn find(dev: &Device, n: u8) -> Option<Self> {
        let mut regs = [0; 15];
        for (i, name) in NAMES.iter().enumerate() {
            regs[i] = addr(dev, &format!("PWM{n}{name}"))?;
        }
        let md = dev.sfr.iter().find_map(|s| {
            s.fields
                .iter()
                .find(|f| f.name == format!("PWM{n}MD"))
                .map(|f| (s.addr as usize, 1 << f.pos))
        });
        Some(Self {
            n,
            regs,
            period_flag: Flag::find(dev, &format!("PWM{n}PIF"))?,
            group_flag: Flag::find(dev, &format!("PWM{n}IF"))?,
            md,
            last: 0,
            pre: 0,
            post: 0,
            count: 0,
            values: [0; 3],
            on: false,
            seq: false,
            out: [false; 2],
            toggle: [false; 2],
        })
    }
    fn word(&self, m: &DataMem, i: usize) -> u16 {
        u16::from(m.mem[self.regs[i]]) | u16::from(m.mem[self.regs[i + 1]]) << 8
    }
    fn load(&mut self, m: &mut DataMem) {
        self.values = [self.word(m, 3), self.word(m, 11), self.word(m, 13)];
        m.mem[self.regs[9]] &= !4;
    }
    pub fn enabled(&self, m: &DataMem) -> bool {
        m.mem[self.regs[9]] & 0x80 != 0 && !self.md.is_some_and(|(a, mask)| m.mem[a] & mask != 0)
    }
    fn emit(&mut self, m: &DataMem, tick: u64, raw: [bool; 2], events: &mut Vec<(u64, u8, bool)>) {
        let cfg = m.mem[self.regs[10]];
        for (i, &v) in raw.iter().enumerate() {
            let v = v != (cfg & (0x40 << i) != 0);
            if self.out[i] != v {
                self.out[i] = v;
                events.push((tick, 6 + (self.n - 1) * 2 + i as u8, v));
            }
        }
    }
    fn levels(&self, m: &DataMem) -> [bool; 2] {
        let cfg = m.mem[self.regs[10]];
        std::array::from_fn(|i| {
            let p = self.values[1 + i];
            let active = match cfg & 7 {
                0 => self.count < p,
                1 => u32::from(self.count) + u32::from(p) > u32::from(self.values[0]),
                4 => self.count == p,
                5 => self.toggle[i],
                _ => false,
            };
            active && (cfg & 8 == 0 || i == usize::from(self.seq))
        })
    }
    fn group(&self, m: &mut DataMem) {
        if m.mem[self.regs[7]] & m.mem[self.regs[8]] & 3 != 0 {
            self.group_flag.set_in(&mut m.mem);
        } else {
            self.group_flag.clear(m);
        }
    }
    pub fn write(&mut self, sub: u8, v: u8, now: u64, m: &mut DataMem, events: &mut Vec<(u64, u8, bool)>) {
        let a = self.regs[sub as usize];
        m.mem[a] = v & m.implemented[a];
        self.sync(m, now, false, events, &mut BTreeSet::new());
        self.group(m);
    }
    pub fn sync(&mut self, m: &mut DataMem, now: u64, asleep: bool, events: &mut Vec<(u64, u8, bool)>, notes: &mut BTreeSet<String>) {
        let start = self.last;
        self.last = now;
        let enabled = self.enabled(m);
        if !enabled {
            self.on = false;
            self.pre = 0;
            self.post = 0;
            self.count = 0;
            self.seq = false;
            self.toggle = [false; 2];
            self.emit(m, 4 * now, [false; 2], events);
            self.group(m);
            return;
        }
        if !self.on {
            self.load(m);
            self.on = true;
            self.emit(m, 4 * now, self.levels(m), events);
        }
        let cfg = m.mem[self.regs[10]];
        if !matches!(cfg & 7, 0 | 1 | 4 | 5) {
            notes.insert(format!(
                "PWM{} mode {} is not modelled (left/right aligned and compare are)",
                self.n,
                cfg & 7
            ));
        }
        if m.mem[self.regs[0]] != 0 || m.mem[self.regs[2]] != 0 {
            notes.insert(format!("PWM{} external reset and automatic load sources are not modelled", self.n));
        }
        if m.mem[self.regs[1]] & 15 != 2 {
            notes.insert(format!(
                "PWM{} clock {} is not modelled (FOSC = 2 is)",
                self.n,
                m.mem[self.regs[1]] & 15
            ));
            return;
        }
        if asleep {
            return;
        }
        for tick in (4 * start + 1)..=4 * now {
            self.pre += 1;
            if self.pre <= u16::from(m.mem[self.regs[5]]) {
                continue;
            }
            self.pre = 0;
            if self.count == self.values[0] {
                self.count = 0;
                self.seq = !self.seq;
                self.post += 1;
                if self.post > u16::from(m.mem[self.regs[6]]) {
                    self.post = 0;
                    self.period_flag.set(m);
                }
                if m.mem[self.regs[9]] & 4 != 0 {
                    self.load(m);
                }
            } else {
                self.count = self.count.wrapping_add(1);
            }
            for i in 0..2 {
                let p = self.values[1 + i];
                let hit = match cfg & 7 {
                    0 | 4 | 5 => self.count == p,
                    1 => p != 0 && u32::from(self.count) + u32::from(p) == u32::from(self.values[0]) + 1,
                    _ => false,
                };
                if hit {
                    m.mem[self.regs[7]] |= 1 << i;
                    if m.mem[self.regs[8]] & (1 << i) != 0 {
                        self.group_flag.set(m);
                    }
                    if cfg & 7 == 5 {
                        self.toggle[i] = !self.toggle[i];
                    }
                    if let Some(q) = &mut m.dma_requests {
                        q.push((usize::MAX, 0x58 + (self.n - 1) * 2 + i as u8));
                    }
                }
            }
            self.emit(m, tick, self.levels(m), events);
        }
        self.group(m);
    }
}
