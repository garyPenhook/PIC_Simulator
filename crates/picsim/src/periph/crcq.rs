//! Q41 programmable 1..32-bit CRC, DS40002214G ch.13 pp.201-220.
//! SETUP uses bits 3:2, as the register diagram and device pack show (the prose says 4:3,
//! overlapping ACCM). The serial shifter advances one bit per instruction cycle (modelling
//! assumption, not measured hardware timing). The scanner shares the CPU/DMA arbiter; timing assumptions are in FACTS-SIM A156.
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
    pub scan_regs: [usize; 8],
    scanif: Flag,
    scanmd: Option<(usize, u8)>,
    scan_data: u32,
    scan_bytes: u8,
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
            scan_regs: ["SCANLADRL", "SCANLADRH", "SCANLADRU", "SCANHADRL", "SCANHADRH", "SCANHADRU", "SCANCON0", "SCANTRIG"].map(|n| addr(dev, n).unwrap()),
            scanif: Flag::find(dev, "SCANIF")?,
            scanmd: dev.sfr.iter().find_map(|s| s.fields.iter().find(|f| f.name == "SCANMD").map(|f| (s.addr as usize, 1 << f.pos))),
            scan_data: 0,
            scan_bytes: 0,
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
        if k >= 11 { return m.mem[self.scan_regs[(k - 11) as usize]]; }
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
        if k >= 11 {
            let a = self.scan_regs[(k - 11) as usize];
            let old = m.mem[a];
            m.mem[a] = v & m.implemented[a];
            if k == 17 {
                m.mem[a] &= !1; // BUSY is hardware status.
                if !self.go(m) { m.mem[a] &= !0x20; }
                if old & 0x20 != 0 && m.mem[a] & 0x20 == 0 { self.scanif.set(m); }
                if m.mem[a] & 0x20 == 0 { self.scan_data = 0; self.scan_bytes = 0; }
            }
            return;
        }
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
    fn stop_scan(&mut self, m: &mut DataMem) {
        let a = self.scan_regs[6];
        if m.mem[a] & 0x20 != 0 { self.scanif.set(m); }
        m.mem[a] &= !0x21;
        self.scan_data = 0; self.scan_bytes = 0;
    }
    fn scan_addr(&self, m: &DataMem, start: usize) -> u32 {
        (0..3).fold(0, |v, i| v | u32::from(m.mem[self.scan_regs[start + i]]) << (8 * i))
    }
    pub fn scan_request(&mut self, m: &mut DataMem, trigger: Option<bool>, notes: &mut std::collections::BTreeSet<String>) -> Option<bool> {
        if !self.go(m) { self.stop_scan(m); return None; }
        let c = m.mem[self.scan_regs[6]];
        if c & 0xA0 != 0xA0 || self.scanmd.is_some_and(|(a, mask)| m.mem[a] & mask != 0) { return None; }
        if self.scan_addr(m, 0) > self.scan_addr(m, 3) { self.stop_scan(m); return None; }
        if c & 0x40 != 0 {
            if trigger.is_none() { notes.insert(format!("Q41 scanner trigger {} is not modelled; scan waits", m.mem[self.scan_regs[7]])); }
            if trigger != Some(true) && c & 2 == 0 { return None; }
        }
        if c & 2 == 0 && self.held.is_some() { return None; }
        Some(c & 4 != 0)
    }
    pub fn scan_fetch(&self, m: &DataMem, trigger: Option<bool>) -> Option<(u32, bool)> {
        let c = m.mem[self.scan_regs[6]];
        if self.held.is_some() || c & 0x40 != 0 && trigger != Some(true) { return None; }
        Some((self.scan_addr(m, 0), c & 4 != 0))
    }
    pub fn scan_byte(&mut self, value: u8, m: &mut DataMem) {
        let address = self.scan_addr(m, 0);
        self.scan_data |= u32::from(value) << (8 * self.scan_bytes);
        self.scan_bytes += 1;
        let next = address.wrapping_add(1) & 0x3FFFFF;
        for i in 0..3 { m.mem[self.scan_regs[i]] = (next >> (8 * i)) as u8; }
        let last = address >= self.scan_addr(m, 3);
        let width = ((m.mem[self.regs[10]] & 31) + 1).div_ceil(8);
        if self.scan_bytes >= width || last {
            let data = self.scan_data;
            for i in 0..4 { m.mem[self.regs[i]] = (data >> (8 * i)) as u8; }
            if self.cur.is_none() { self.start(m, data); } else { self.held = Some(data); }
            self.scan_data = 0; self.scan_bytes = 0;
            if last { self.stop_scan(m); }
            self.status(m);
        }
    }
    pub fn scan_active(&self, m: &DataMem) -> bool {
        m.mem[self.scan_regs[6]] & 0xA0 == 0xA0 && !self.scanmd.is_some_and(|(a, mask)| m.mem[a] & mask != 0)
    }
    pub fn trigger_select(&self, m: &DataMem) -> u8 { m.mem[self.scan_regs[7]] & 15 }

    pub fn sync(&mut self, m: &mut DataMem, now: u64) {
        if !self.go(m) { self.stop_scan(m); }
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
