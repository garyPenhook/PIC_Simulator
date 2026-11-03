//! A part loaded with a program: core, memory, symbols, and the run loop.

use crate::hex::Image;
use crate::pic18::Pic18;
use devdesc::Device;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// The PC reached the halt address.
    Halt,
    /// The cycle budget ran out.
    Cycles,
    /// SLEEP with nothing that can wake the part.
    Sleep,
    /// A breakpoint address was reached.
    Breakpoint(u32),
}

pub struct Machine {
    pub cpu: Pic18,
    pub dev: Device,
    pub syms: BTreeMap<String, u32>,
    pub breakpoints: Vec<u32>,
    image: Image,
    /// Stimulus file text, applied again after every reset (the peripherals are rebuilt then).
    stimulus: Option<String>,
}

impl Machine {
    pub fn new(dev: Device) -> Result<Machine, String> {
        if dev.device.family != "pic18" {
            return Err(format!("{}: family {} is not simulated yet", dev.device.name, dev.device.family));
        }
        let cpu = Pic18::new(&dev)?;
        Ok(Machine { cpu, dev, syms: BTreeMap::new(), breakpoints: Vec::new(), image: Image::default(), stimulus: None })
    }

    pub fn load_hex(&mut self, text: &str) -> Result<(), String> {
        let img = Image::parse(text).map_err(|e| e.to_string())?;
        self.cpu.load(&img);
        self.image = img;
        Ok(())
    }

    /// Drive the part as a stimulus file says (see `stimulus.rs`).
    pub fn load_stimulus(&mut self, text: &str) -> Result<(), String> {
        crate::stimulus::Stimulus::parse(text)?.apply(&self.dev, &mut self.cpu.p)?;
        self.stimulus = Some(text.to_string());
        Ok(())
    }

    /// Power-on reset: the core and memory return to their reset state and the program is loaded again.
    pub fn reset(&mut self) {
        if let Ok(mut c) = Pic18::new(&self.dev) {
            c.load(&self.image);
            if let Some(t) = &self.stimulus
                && let Ok(st) = crate::stimulus::Stimulus::parse(t)
            {
                let _ = st.apply(&self.dev, &mut c.p); // it applied once on this device, so it applies again
            }
            self.cpu = c;
        }
    }

    /// Symbols from the assembly listing basil writes next to the hex: `;SYM name 0xADDR` for code labels and
    /// `name equ 0xADDR` for variables.
    pub fn load_symbols_from_asm(&mut self, asm: &str) {
        for l in asm.lines() {
            if let Some(r) = l.strip_prefix(";SYM ") {
                let mut it = r.split_whitespace();
                if let (Some(n), Some(a)) = (it.next(), it.next())
                    && let Some(v) = a.strip_prefix("0x").and_then(|h| u32::from_str_radix(h, 16).ok())
                {
                    self.syms.insert(n.to_string(), v);
                }
            } else {
                let mut it = l.split_whitespace();
                if let (Some(n), Some("equ"), Some(a)) = (it.next(), it.next(), it.next())
                    && let Some(v) = a.strip_prefix("0x").and_then(|h| u32::from_str_radix(h, 16).ok())
                {
                    self.syms.insert(n.to_string(), v);
                }
            }
        }
    }

    /// Run until `until` (a program address), a breakpoint, SLEEP, or `max_cycles` more cycles.
    pub fn run(&mut self, until: Option<u32>, max_cycles: u64) -> Stop {
        let limit = self.cpu.cycles.saturating_add(max_cycles);
        let mut first = true;
        while self.cpu.cycles < limit {
            let pc = self.cpu.pc;
            if Some(pc) == until {
                return Stop::Halt;
            }
            if !first && self.breakpoints.contains(&pc) {
                return Stop::Breakpoint(pc);
            }
            first = false;
            self.cpu.step();
            if self.cpu.state != crate::pic18::State::Running {
                return Stop::Sleep;
            }
        }
        Stop::Cycles
    }

    /// Bytes of a variable (little endian) read without side effects.
    pub fn peek(&self, addr: u32, len: usize) -> Vec<u8> {
        (0..len).map(|i| self.cpu.m.mem.get(addr as usize + i).copied().unwrap_or(0)).collect()
    }
}
