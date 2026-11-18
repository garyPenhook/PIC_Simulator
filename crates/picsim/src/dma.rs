//! Q41 DMA (DS40002214G ch.16, pp.246-282).
//! Four independent banks, two bus phases per byte, message reload, software start/stop,
//! peripheral start/abort requests and completion/overrun flags. The system arbiter honors
//! CPU/channel priorities and PRLOCK; DMA-only mirrors are not modelled. DMA is paused in Sleep. Zero sizes wrap as 12-bit counters.
use crate::mem::{DataMem, Special};
use devdesc::Device;

const NAMES: [&str; 23] = [
    "BUF", "DCNTL", "DCNTH", "DPTRL", "DPTRH", "DSZL", "DSZH", "DSAL", "DSAH", "SCNTL", "SCNTH", "SPTRL", "SPTRH", "SPTRU", "SSZL", "SSZH",
    "SSAL", "SSAH", "SSAU", "CON0", "CON1", "AIRQ", "SIRQ",
];
const SELECT: u8 = 23;
const CON: usize = 19;
const MODE: usize = 20;

pub(crate) struct Dma {
    bank: [[u8; 23]; 4],
    selected: u8,
    masks: [u8; 23],
    pir: Vec<usize>,
    md: [Option<(usize, u8)>; 4],
    pub lock: usize,
    scanpr: usize,
    priorities: [usize; 6], // MAIN, ISR, DMA1..4
    key: u8,
    locked_once: bool,
}
fn get(b: &[u8; 23], a: usize, len: usize) -> u32 {
    (0..len).fold(0, |v, i| v | u32::from(b[a + i]) << (8 * i))
}
fn put(b: &mut [u8; 23], a: usize, len: usize, v: u32) {
    for i in 0..len {
        b[a + i] = (v >> (8 * i)) as u8;
    }
}
impl Dma {
    pub fn bind(dev: &Device, m: &mut DataMem) -> Option<Self> {
        let selector = dev.find_sfr("DMASELECT")?.addr as usize;
        let mut masks = [0; 23];
        for (i, n) in NAMES.iter().enumerate() {
            let a = dev.find_sfr(&format!("DMAn{n}"))?.addr as usize;
            m.special[a] = Special::Dma(i as u8);
            masks[i] = m.implemented[a];
        }
        m.special[selector] = Special::Dma(SELECT);
        m.dma_requests = Some(Vec::new());
        let lock = dev.find_sfr("PRLOCK")?.addr as usize;
        let priorities = ["MAINPR", "ISRPR", "DMA1PR", "DMA2PR", "DMA3PR", "DMA4PR"]
            .map(|n| dev.find_sfr(n).unwrap().addr as usize);
        for a in [lock].into_iter().chain(priorities).chain(dev.find_sfr("SCANPR").map(|s| s.addr as usize)) {
            m.special[a] = Special::Arbiter;
        }
        let md = std::array::from_fn(|i| {
            dev.sfr.iter().find_map(|s| {
                s.fields
                    .iter()
                    .find(|f| f.name == format!("DMA{}MD", i + 1))
                    .map(|f| (s.addr as usize, 1 << f.pos))
            })
        });
        Some(Self {
            bank: [[0; 23]; 4],
            selected: 0,
            masks,
            pir: (0..11).map(|i| dev.find_sfr(&format!("PIR{i}")).unwrap().addr as usize).collect(),
            md,
            scanpr: dev.find_sfr("SCANPR")?.addr as usize,
            lock, priorities, key: 0, locked_once: false,
        })
    }
    pub fn priority_write(&mut self, a: usize, v: u8, one_way: bool, m: &mut DataMem) {
        if a == self.lock {
            let armed = self.key == 2;
            self.key = match (self.key, v) { (_, 0x55) => 1, (1, 0xAA) => 2, _ => 0 };
            if armed && v <= 1 && !(one_way && self.locked_once && v == 0) {
                m.mem[a] = v;
                self.locked_once |= v == 1;
            }
        } else {
            self.key = 0;
            if m.mem[self.lock] & 1 == 0 { m.mem[a] = v & 7; }
        }
    }
    pub fn scanner_wins(&self, m: &DataMem, isr: bool, free: Option<(bool, bool)>, eeprom: bool, dma: Option<usize>) -> bool {
        if m.mem[self.lock] & 1 == 0 { return false; }
        let priority = m.mem[self.scanpr] & 7;
        if free.is_none() && priority >= m.mem[self.priorities[usize::from(isr)]] & 7 { return false; }
        if free.is_some_and(|(_, flash)| !eeprom && !flash) { return false; }
        dma.is_none_or(|i| priority < m.mem[self.priorities[i + 2]] & 7)
    }
    pub fn enabled(&self) -> bool {
        self.bank.iter().any(|b| b[CON] & 0x80 != 0)
    }
    pub fn read(&self, k: u8) -> u8 {
        if k == SELECT {
            self.selected
        } else {
            self.bank.get(self.selected as usize).map_or(0, |b| b[k as usize])
        }
    }
    pub fn write(&mut self, k: u8, v: u8, _m: &mut DataMem) {
        if k == SELECT {
            self.selected = v & 7;
            return;
        }
        let Some(b) = self.bank.get_mut(self.selected as usize) else {
            return;
        };
        let k = k as usize;
        // BUF and live pointers are read only (DS pp.270,274,278).
        if k == 0 || (3..=4).contains(&k) || (11..=13).contains(&k) {
            return;
        }
        let v = v & self.masks[k];
        if k == CON {
            let old = b[CON];
            b[CON] = (v & 0xE4) | (old & 1);
            if v & 0x80 == 0 {
                b[CON] = v & 0x44;
            }
            if old & 0x80 == 0 && v & 0x80 != 0 {
                let (s, d, sc, dc) = (get(b, 16, 3), get(b, 7, 2), get(b, 14, 2), get(b, 5, 2));
                put(b, 11, 3, s);
                put(b, 3, 2, d);
                put(b, 9, 2, sc);
                put(b, 1, 2, dc);
                b[CON] &= !1;
            }
        } else {
            b[k] = v;
        }
    }
    fn flag(&self, channel: usize, sub: usize, m: &mut DataMem) {
        let vector = [0x14, 0x34, 0x4C, 0x54][channel] + sub;
        let a = self.pir[vector / 8];
        let bit = 1 << (vector % 8);
        m.mem[a] |= bit;
        if let Some(q) = &mut m.dma_requests {
            q.push((a, bit));
        }
    }
    /// Returns (channel, write phase, source memory region, address, buffer).
    pub fn next(&mut self, m: &mut DataMem, isr: bool, free: Option<(bool, bool)>) -> Option<(usize, bool, u8, u32, u8)> {
        let requests = m.dma_requests.as_mut().map(std::mem::take).unwrap_or_default();
        let request = |source: u8| {
            if (0x58..=0x5D).contains(&source) {
                return requests.contains(&(usize::MAX, source));
            }
            let v = source as usize;
            let level = matches!(
                source,
                0x18 | 0x19 | 0x20 | 0x21 | 0x28 | 0x29 | 0x38 | 0x39 | 0x40 | 0x41 | 0x48 | 0x49
            );
            self.pir.get(v / 8).is_some_and(|&a| {
                source != 0
                    && if level {
                        m.mem[a] & (1 << (v % 8)) != 0
                    } else {
                        requests.iter().any(|&(r, mask)| r == a && mask & (1 << (v % 8)) != 0)
                    }
            })
        };
        let triggers: [(bool, bool, bool); 4] = std::array::from_fn(|i| {
            let b = &self.bank[i];
            (
                request(b[22]),
                request(b[21]),
                matches!(
                    b[22],
                    0x18 | 0x19 | 0x20 | 0x21 | 0x28 | 0x29 | 0x38 | 0x39 | 0x40 | 0x41 | 0x48 | 0x49
                ),
            )
        });
        for (i, &(start, abort, level)) in triggers.iter().enumerate() {
            if self.md[i].is_some_and(|(a, mask)| m.mem[a] & mask != 0) {
                continue;
            }
            let b = &mut self.bank[i];
            if b[CON] & 0x80 == 0 {
                continue;
            }
            if abort && b[CON] & 4 != 0 {
                b[CON] &= !0x64;
                self.flag(i, 3, m);
            } else if start && b[CON] & 0x40 != 0 {
                if b[CON] & 0x20 != 0 {
                    if !level {
                        self.flag(i, 2, m);
                    }
                } else {
                    b[CON] |= 0x20;
                }
            }
        }
        if m.mem[self.lock] & 1 == 0 { return None; }
        let cpu_priority = m.mem[self.priorities[usize::from(isr)]] & 7;
        let mut order = [0, 1, 2, 3];
        order.sort_by_key(|&i| (m.mem[self.priorities[i + 2]] & 7, i));
        for i in order {
            if self.md[i].is_some_and(|(a, mask)| m.mem[a] & mask != 0) {
                continue;
            }
            let b = &self.bank[i];
            if b[CON] & 0xA0 == 0xA0 {
                let write = b[CON] & 1 != 0;
                let region = (b[MODE] >> 3) & 3;
                let higher = m.mem[self.priorities[i + 2]] & 7 < cpu_priority;
                // Equal priorities favor ISR, MAIN, then channel number (Table 7-1).
                let available = free.is_some_and(|(data, flash)| if write || region == 0 { data } else if region == 1 { flash } else { true });
                if free.is_none() && !higher || free.is_some() && !available { continue; }
                return Some((i, write, (b[MODE] >> 3) & 3, if write { get(b, 3, 2) } else { get(b, 11, 3) }, b[0]));
            }
        }
        None
    }
    pub fn read_complete(&mut self, i: usize, value: u8) {
        self.bank[i][0] = value;
        self.bank[i][CON] |= 1;
    }
    pub fn written(&mut self, i: usize, m: &mut DataMem) {
        let b = &mut self.bank[i];
        b[CON] &= !1;
        let mut reload = [false; 2];
        for (n, (count, size, ptr, addr, len, mode, stop)) in
            [(9, 14, 11, 16, 3, (b[MODE] >> 1) & 3, 1), (1, 5, 3, 7, 2, b[MODE] >> 6, 0x20)]
                .into_iter()
                .enumerate()
        {
            let left = get(b, count, 2).wrapping_sub(1) & 0xFFF;
            reload[n] = left == 0;
            if left == 0 {
                let (sz, a) = (get(b, size, 2), get(b, addr, len));
                put(b, count, 2, sz);
                put(b, ptr, len, a);
                b[CON] &= !0x20;
                if b[MODE] & stop != 0 {
                    b[CON] &= !0x40;
                }
            } else {
                put(b, count, 2, left);
                let p = get(b, ptr, len);
                let p = match mode {
                    1 => p.wrapping_add(1),
                    2 => p.wrapping_sub(1),
                    _ => p,
                };
                put(b, ptr, len, p & if len == 3 { 0x3FFFFF } else { 0xFFFF });
            }
        }
        for (n, yes) in reload.into_iter().enumerate() {
            if yes {
                self.flag(i, n, m);
            }
        }
    }
}
