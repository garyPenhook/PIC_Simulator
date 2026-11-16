//! Q41 programmable 1..32-bit CRC, DS40002214G ch.13 pp.201-220.
//! SETUP uses bits 3:2, as the register diagram and device pack show (the prose says 4:3,
//! overlapping ACCM). The serial shifter advances one bit per instruction cycle (modelling
//! assumption, not measured hardware timing). Scanner/arbitration remain explicitly unmodelled.
use super::{Flag, addr};
use crate::mem::DataMem;
use devdesc::Device;

#[derive(Clone)]
struct Word {
    data: u32,
    bits: u32,
    pos: u32,
    plen: u32,
    poly: u32,
    reverse: bool,
    augmented: bool,
}
#[derive(Clone)]
pub struct CrcQ {
    pub regs: [usize; 11],
    crcif: Flag,
    md: Option<(usize, u8)>,
    acc: u32,
    poly: u32,
    shift: u32,
    cur: Option<Word>,
    held: Option<u32>,
    last: u64,
}
impl CrcQ {
    pub fn find(dev: &Device) -> Option<Self> {
        let names = [
            "CRCDATAL", "CRCDATAH", "CRCDATAU", "CRCDATAT", "CRCOUTL", "CRCOUTH", "CRCOUTU", "CRCOUTT", "CRCCON0", "CRCCON1", "CRCCON2",
        ];
        let mut regs = [0; 11];
        for (i, n) in names.into_iter().enumerate() {
            regs[i] = addr(dev, n)?;
        }
        let md = dev
            .sfr
            .iter()
            .find_map(|s| s.fields.iter().find(|f| f.name == "CRCMD").map(|f| (s.addr as usize, 1 << f.pos)));
        Some(Self {
            regs,
            crcif: Flag::find(dev, "CRCIF")?,
            md,
            acc: 0,
            poly: 1,
            shift: 0,
            cur: None,
            held: None,
            last: 0,
        })
    }
    fn enabled(&self, m: &DataMem) -> bool {
        m.mem[self.regs[8]] & 0x80 != 0 && !self.md.is_some_and(|(a, mask)| m.mem[a] & mask != 0)
    }
    fn go(&self, m: &DataMem) -> bool {
        self.enabled(m) && m.mem[self.regs[8]] & 0x40 != 0
    }
    pub fn thaw(&mut self, d: u64) {
        self.last += d;
    }
    pub fn busy(&self) -> bool {
        self.cur.is_some() || self.held.is_some()
    }
    fn start(&mut self, m: &DataMem, data: u32) {
        self.shift = data;
        self.cur = Some(Word {
            data,
            bits: u32::from(m.mem[self.regs[10]] & 31) + 1,
            pos: 0,
            plen: u32::from(m.mem[self.regs[9]] & 31) + 1,
            poly: self.poly | 1,
            reverse: m.mem[self.regs[8]] & 2 != 0,
            augmented: m.mem[self.regs[8]] & 0x10 != 0,
        });
    }
    fn status(&self, m: &mut DataMem) {
        let a = self.regs[8];
        m.mem[a] = m.mem[a] & !0x21 | if self.busy() { 0x20 } else { 0 } | u8::from(self.held.is_some());
    }
    pub fn read(&self, k: u8, m: &DataMem) -> u8 {
        if (4..8).contains(&k) {
            let value = match (m.mem[self.regs[8]] >> 2) & 3 {
                1 => self.shift,
                2 => self.poly | 1,
                _ => self.acc,
            };
            (value >> (8 * (k - 4))) as u8
        } else {
            m.mem[self.regs[k as usize]]
        }
    }
    pub fn write(&mut self, k: u8, v: u8, now: u64, m: &mut DataMem) {
        self.sync(m, now);
        let a = self.regs[k as usize];
        if (4..8).contains(&k) {
            let sh = 8 * (k - 4);
            let target = match (m.mem[self.regs[8]] >> 2) & 3 {
                1 => return,
                2 => &mut self.poly,
                _ => &mut self.acc,
            };
            *target = (*target & !(0xFFu32 << sh)) | u32::from(v) << sh;
            self.poly |= 1;
        } else if k == 8 {
            m.mem[a] = v & 0xDE | m.mem[a] & 0x21;
            if !self.enabled(m) {
                self.cur = None;
                self.held = None;
            } else if self.go(m)
                && self.cur.is_none()
                && let Some(data) = self.held.take()
            {
                self.start(m, data);
            }
        } else {
            m.mem[a] = v & m.implemented[a];
            if k == 0 && self.enabled(m) {
                let data = (0..4).fold(0, |w, i| w | u32::from(m.mem[self.regs[i]]) << (8 * i));
                if self.go(m) && self.cur.is_none() {
                    self.start(m, data);
                } else {
                    self.held = Some(data);
                }
            }
        }
        self.status(m);
    }
    pub fn sync(&mut self, m: &mut DataMem, now: u64) {
        let elapsed = now.saturating_sub(self.last);
        self.last = now;
        if !self.enabled(m) {
            self.cur = None;
            self.held = None;
            self.status(m);
            return;
        }
        if !self.go(m) {
            return;
        }
        let busy = self.busy();
        for _ in 0..elapsed {
            let Some(w) = &mut self.cur else { break };
            let mask = if w.plen == 32 { u32::MAX } else { (1u32 << w.plen) - 1 };
            let bit = (w.data >> if w.reverse { w.pos } else { w.bits - 1 - w.pos }) & 1;
            let msb = self.acc >> (w.plen - 1) & 1;
            self.acc = (self.acc << 1 | if w.augmented { 0 } else { bit }) & mask;
            if msb ^ if w.augmented { bit } else { 0 } != 0 {
                self.acc ^= w.poly & mask;
            }
            self.shift = if w.reverse { self.shift >> 1 } else { self.shift << 1 };
            w.pos += 1;
            if w.pos == w.bits {
                self.cur = None;
                if let Some(data) = self.held.take() {
                    self.start(m, data);
                }
            }
        }
        if busy && !self.busy() {
            self.crcif.set(m);
        }
        self.status(m);
    }
}
