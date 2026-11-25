//! Peripherals of the dsPIC33CK: the interrupt controller registers, I/O ports, Timer1, the watchdog and the program
//! memory controller. A register of a module without a model is plain storage and its name is reported (see
//! `Dspic::unmodelled`).

use crate::dspic::*;
use devdesc::Device;
use std::collections::HashMap;

/// Register addresses found by name in the device description.
#[derive(Default, Clone)]
pub struct Addrs {
    pub t1con: usize,
    pub tmr1: usize,
    pub pr1: usize,
    pub rcon: usize,
    pub wdtconl: usize,
    pub ports: Vec<Port>,
    pub nvmcon: usize,
    pub nvmkey: usize,
    pub nvmadrl: usize,
    pub nvmadrh: usize,
    pub nvmsrcadrl: usize,
    pub nvmsrcadrh: usize,
}

#[derive(Default, Clone)]
pub struct Port {
    pub letter: u8,
    pub ansel: usize,
    pub tris: usize,
    pub port: usize,
    pub lat: usize,
    pub odc: usize,
    /// bits of the port the part has
    pub mask: u16,
}

pub struct Periph {
    pub a: Addrs,
    /// the cycle count the peripherals have run to
    pub last: u64,
    /// levels the outside world drives on the pins of ports A..D (one bit per pin)
    pub pins_in: Vec<u16>,
    /// Timer1
    pub t1_frac: u64,
    pub t1_on: bool,
    /// NVM write latches (double word) and the unlock sequence
    pub nvm_latch: [u32; 4],
    pub nvm_key: u8,
    pub nvm_unlocked: bool,
    pub wdt_count: u64,
    pub wdt_woke: bool,
    pub modelled: Vec<(usize, usize)>,
    pub fosc_override: Option<f64>,
    pub uart: Vec<crate::dspic_uart::UartState>,
    pub adc: crate::dspic_adc::AdcState,
    pub has_uart: [bool; 3],
    /// characters the UARTs sent: (UART number, cycle, byte)
    pub tx: crate::log::Log<(u8, u64, u8)>,
    /// pin level changes still to come: (port index, cycle, mask, levels)
    pub pin_events: Vec<(usize, u64, u16, u16)>,
    /// the `[[adc]]` values: (channel, from cycle, code)
    pub adc_in: Vec<(u8, u64, u16)>,
    pub names: HashMap<usize, String>,
}

fn addr(dev: &Device, n: &str) -> usize {
    dev.find_sfr(n).map_or(0, |s| s.addr as usize)
}

impl Periph {
    pub fn new(dev: &Device) -> Result<Periph, String> {
        let mut a = Addrs {
            t1con: addr(dev, "T1CON"),
            tmr1: addr(dev, "TMR1"),
            pr1: addr(dev, "PR1"),
            rcon: addr(dev, "RCON"),
            wdtconl: addr(dev, "WDTCONL"),
            nvmcon: addr(dev, "NVMCON"),
            nvmkey: addr(dev, "NVMKEY"),
            nvmadrl: addr(dev, "NVMADRL"),
            nvmadrh: addr(dev, "NVMADRH"),
            nvmsrcadrl: addr(dev, "NVMSRCADRL"),
            nvmsrcadrh: addr(dev, "NVMSRCADRH"),
            ports: Vec::new(),
        };
        for (i, l) in ['A', 'B', 'C', 'D', 'E'].iter().enumerate() {
            let tris = addr(dev, &format!("TRIS{l}"));
            if tris == 0 {
                continue;
            }
            let mask = dev.find_sfr(&format!("PORT{l}")).and_then(|s| s.implemented).map_or(0xFFFF, |m| m as u16);
            let _ = i;
            a.ports.push(Port { letter: *l as u8, ansel: addr(dev, &format!("ANSEL{l}")), tris, port: addr(dev, &format!("PORT{l}")), lat: addr(dev, &format!("LAT{l}")), odc: addr(dev, &format!("ODC{l}")), mask });
        }
        let mut names = HashMap::new();
        for s in &dev.sfr {
            names.insert(s.addr as usize, s.name.clone());
        }
        let n = a.ports.len();
        Ok(Periph { a, last: 0, pins_in: vec![0; n], t1_frac: 0, t1_on: false, nvm_latch: [0xFF_FFFF; 4], nvm_key: 0, nvm_unlocked: false, wdt_count: 0, wdt_woke: false, modelled: Vec::new(), fosc_override: None, uart: vec![Default::default(); 3], adc: Default::default(), has_uart: [dev.find_sfr("U1MODE").is_some(), dev.find_sfr("U2MODE").is_some(), dev.find_sfr("U3MODE").is_some()], tx: crate::log::Log::new(), pin_events: Vec::new(), adc_in: Vec::new(), names })
    }

    pub fn clrwdt(&mut self) {
        self.wdt_count = 0;
    }

    pub fn wdt_woke(&mut self) -> bool {
        std::mem::take(&mut self.wdt_woke)
    }

    pub fn can_wake(&self) -> bool {
        false
    }

    /// The oscillator frequency in MHz (the FRC at 8 MHz until the clock models are in).

    pub fn set_fosc_override(&mut self, f: f64) {
        self.fosc_override = Some(f);
    }
}

impl Dspic {
    fn m16(&self, a: usize) -> u16 {
        u16::from(self.mem[a]) | u16::from(self.mem[a + 1]) << 8
    }

    fn set16(&mut self, a: usize, v: u16) {
        self.mem[a] = v as u8;
        self.mem[a + 1] = (v >> 8) as u8;
    }

    /// Sets the flag bit of interrupt request `irq` (IFSx).
    pub fn raise_irq(&mut self, irq: usize) {
        let a = crate::dspic_exc::IFS_BASE + 2 * (irq / 16);
        let v = self.m16(a) | 1 << (irq % 16);
        self.set16(a, v);
    }

    fn port_of(&self, a: usize) -> Option<(usize, u8)> {
        for (i, p) in self.p.a.ports.iter().enumerate() {
            for (k, r) in [p.ansel, p.tris, p.port, p.lat, p.odc].iter().enumerate() {
                if *r != 0 && *r == a {
                    return Some((i, k as u8));
                }
            }
        }
        None
    }

    /// The pin levels of port `i` as the CPU reads PORTx: an input shows what is driven, an output its latch.
    fn port_read(&self, i: usize) -> u16 {
        let p = &self.p.a.ports[i];
        let tris = self.m16(p.tris);
        let lat = self.m16(p.lat);
        ((lat & !tris) | (self.p.pins_in[i] & tris)) & p.mask
    }

    /// Reads an SFR word (not a CPU register).
    pub fn sfr_read(&mut self, a: usize) -> u16 {
        if Self::adc_owns(a) {
            self.advance_periph();
            if let Some(v) = self.adc_read(a) {
                return v;
            }
        }
        if let Some((n, off)) = self.uart_at(a) {
            self.advance_periph();
            if let Some(v) = self.uart_read(n, off) {
                return v;
            }
        }
        if let Some((i, 2)) = self.port_of(a) {
            return self.port_read(i);
        }
        if a == self.p.a.tmr1 && a != 0 {
            self.advance_periph();
        }
        self.note_unmodelled(a);
        self.m16(a)
    }

    fn note_unmodelled(&mut self, a: usize) {
        if self.p.is_modelled(a) {
            return;
        }
        if let Some(n) = self.p.names.get(&a) {
            self.unmodelled.insert(n.clone());
        }
    }

    /// Writes an SFR word; `mask` selects the bytes written.
    pub fn sfr_write(&mut self, a: usize, v: u16, mask: u16) {
        let old = self.m16(a);
        let mut new = (old & !mask) | (v & mask);
        // interrupt controller: the flags are plain storage here (software clears them), INTTREG is read only
        if a == crate::dspic_exc::INTTREG {
            return;
        }
        if let Some((i, k)) = self.port_of(a) {
            match k {
                2 => {
                    // writing PORTx writes the latch
                    let lat = self.p.a.ports[i].lat;
                    let o = self.m16(lat);
                    let n = (o & !mask) | (v & mask);
                    self.set16(lat, n & self.p.a.ports[i].mask);
                }
                _ => {
                    new &= self.p.a.ports[i].mask;
                    self.set16(a, new);
                }
            }
            return;
        }
        if a == self.p.a.t1con && a != 0 {
            self.advance_periph();
            self.set16(a, new & 0xB736);
            self.p.t1_on = new & 0x8000 != 0;
            return;
        }
        if a == self.p.a.tmr1 && a != 0 {
            self.advance_periph();
            self.set16(a, new);
            self.p.t1_frac = 0;
            return;
        }
        if let Some((n, off)) = self.uart_at(a) {
            self.advance_periph();
            self.uart_write(n, off, v, mask);
            return;
        }
        if Self::adc_owns(a) {
            self.advance_periph();
            self.adc_write(a, v, mask);
            return;
        }
        if a == 0xF84 {
            self.osccon_write(v, mask);
            return;
        }
        if a == self.p.a.nvmkey && a != 0 {
            self.nvm_key_write(new as u8);
            return;
        }
        if a == self.p.a.nvmcon && a != 0 {
            self.nvmcon_write(new);
            return;
        }
        if a == self.p.a.wdtconl && a != 0 {
            self.set16(a, new);
            return;
        }
        self.note_unmodelled(a);
        self.set16(a, new);
    }

    /// Runs the peripherals up to the current cycle.
    pub fn advance_periph(&mut self) {
        let now = self.cycles;
        if now <= self.p.last {
            return;
        }
        let dc = now - self.p.last;
        self.p.last = now;
        self.timer1(dc);
        self.clock_advance();
        self.uart_advance();
        self.adc_advance();
        self.pins_advance();
        self.wdt_advance(dc);
    }

    fn timer1(&mut self, dc: u64) {
        let a = self.p.a.clone();
        if a.t1con == 0 || !self.p.t1_on {
            return;
        }
        let con = self.m16(a.t1con);
        let presc: u64 = [1, 8, 64, 256][((con >> 4) & 3) as usize];
        let tcs = con & 2 != 0;
        // internal clock: Fp = one tick per instruction cycle; TECS selects an external source otherwise (not modelled)
        if tcs && (con >> 8) & 3 == 0 {
            return; // T1CK pin
        }
        let total = self.p.t1_frac + dc;
        let ticks = total / presc;
        self.p.t1_frac = total % presc;
        if ticks == 0 {
            return;
        }
        let pr = u64::from(self.m16(a.pr1));
        let mut tmr = u64::from(self.m16(a.tmr1));
        let mut left = ticks;
        let mut fired = false;
        while left > 0 {
            if tmr + left <= pr {
                tmr += left;
                break;
            }
            // reach PR1: the next tick after the match clears the counter and sets the flag
            let to_match = pr.saturating_sub(tmr);
            left -= to_match + 1;
            tmr = 0;
            fired = true;
            if pr == 0 && left == 0 {
                break;
            }
        }
        self.set16(a.tmr1, tmr as u16);
        if fired {
            self.raise_irq(1); // T1IF (IFS0 bit 1)
        }
    }

    // ---- NVM ----

    fn nvm_key_write(&mut self, k: u8) {
        // the unlock sequence 0x55 then 0xAA
        match (self.p.nvm_key, k) {
            (0, 0x55) => self.p.nvm_key = 1,
            (1, 0xAA) => {
                self.p.nvm_key = 0;
                self.p.nvm_unlocked = true;
            }
            _ => {
                self.p.nvm_key = 0;
                self.p.nvm_unlocked = false;
            }
        }
    }

    fn nvmcon_write(&mut self, new: u16) {
        let a = self.p.a.nvmcon;
        let old = self.m16(a);
        // WR (bit 15) is set by software only with the key sequence done; the operation runs at once
        let mut v = new & !0x8000;
        if new & 0x8000 != 0 && self.p.nvm_unlocked && old & 0x4000 != 0 {
            self.p.nvm_unlocked = false;
            self.nvm_operation(new & 0x000F);
        }
        v &= !0x1000 | 0;
        self.set16(a, v | (old & 0x0000));
    }

    fn nvm_operation(&mut self, op: u16) {
        let adr = (u32::from(self.m16(self.p.a.nvmadrh)) << 16) | u32::from(self.m16(self.p.a.nvmadrl));
        match op {
            // 0b0001 double-word program
            1 => {
                let base = adr & !3 & 0x7F_FFFF;
                for i in 0..2u32 {
                    let idx = ((base + 2 * i) / 2) as usize;
                    if idx < self.prog.len() {
                        let v = self.prog[idx] & self.p.nvm_latch[i as usize] & 0xFF_FFFF;
                        self.set_prog(idx, v);
                    }
                }
            }
            // 0b0011 page erase (1024 instructions)
            3 => {
                let page = (adr / 2048) * 2048;
                let first = (page / 2) as usize;
                for i in 0..1024usize {
                    if first + i < self.prog.len() {
                        self.set_prog(first + i, 0xFF_FFFF);
                    }
                }
            }
            _ => {}
        }
        self.p.nvm_latch = [0xFF_FFFF; 4];
        self.raise_irq(14); // NVMIF
    }

    /// `TBLWTL`/`TBLWTH`: writes a word of the program memory latches (TBLPAG 0xFA is the write latch space).
    pub(crate) fn table_write(&mut self, addr: u32, high: bool, byte: bool, v: u16) {
        if addr >> 16 != 0xFA {
            return;
        }
        let i = ((addr & 0x7) >> 1) as usize;
        let l = &mut self.p.nvm_latch[i];
        if high {
            *l = (*l & 0x00_FFFF) | (u32::from(v & 0xFF) << 16);
        } else if byte {
            if addr & 1 == 0 {
                *l = (*l & 0xFF_FF00) | u32::from(v & 0xFF);
            } else {
                *l = (*l & 0xFF_00FF) | (u32::from(v & 0xFF) << 8);
            }
        } else {
            *l = (*l & 0xFF_0000) | u32::from(v);
        }
    }
}

impl Periph {
    fn is_modelled(&self, a: usize) -> bool {
        let x = &self.a;
        a == x.t1con || a == x.tmr1 || a == x.pr1 || a == x.nvmcon || a == x.nvmkey || a == x.nvmadrl || a == x.nvmadrh || a == x.rcon || a == x.wdtconl || (0xF84..0xF8C).contains(&a)
            || x.ports.iter().any(|p| [p.ansel, p.tris, p.port, p.lat, p.odc].contains(&a))
            || (0x800..0x8E0).contains(&a) || Dspic::adc_owns(a)
            || (0x238..0x24C).contains(&a) || (0x248..0x24A).contains(&a) || (0x260..0x274).contains(&a) || (0xF00..0xF14).contains(&a)
    }
}

impl Dspic {
    /// What the peripherals saw, printed after a run (serial output and so on).
    pub fn report(&self, out: &mut dyn std::io::Write) {
        for n in self.p.tx.iter().map(|t| t.0).collect::<std::collections::BTreeSet<u8>>() {
            let bytes: Vec<u8> = self.p.tx.iter().filter(|t| t.0 == n).map(|t| t.2).collect();
            let _ = writeln!(out, "tx{n}: {}", String::from_utf8_lossy(&bytes).escape_debug());
        }
    }
}

impl Dspic {
    /// Drives the pins of port `letter` from cycle `at` on (`mask` selects the pins, `val` their levels).
    pub fn pin_feed(&mut self, letter: u8, at: u64, mask: u16, val: u16) -> bool {
        let Some(i) = self.p.a.ports.iter().position(|p| p.letter == letter) else { return false };
        let k = self.p.pin_events.partition_point(|e| e.1 <= at);
        self.p.pin_events.insert(k, (i, at, mask, val));
        true
    }

    pub(crate) fn pins_advance(&mut self) {
        while self.p.pin_events.first().is_some_and(|e| e.1 <= self.cycles) {
            let (i, _, mask, val) = self.p.pin_events.remove(0);
            let o = self.p.pins_in[i];
            self.p.pins_in[i] = (o & !mask) | (val & mask);
        }
    }

    /// Sets the code `[[adc]]` gives for an input from cycle `at` on.
    pub fn adc_feed(&mut self, ch: u8, at: u64, code: u16) {
        self.p.adc_in.push((ch, at, code));
    }
}
