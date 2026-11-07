//! The front-end object for the PIC16 enhanced mid-range core, the counterpart of `Machine` (which is PIC18 only).
//! Symbols are word addresses for code and data addresses for variables, as `;SYM` and `equ` give them.

use crate::hex::Image;
use crate::machine::{parse_asm_symbols, Stop};
use crate::pic16::Pic16;
use devdesc::Device;
use std::collections::BTreeMap;

pub struct Machine16 {
    pub cpu: Pic16,
    pub dev: Device,
    pub syms: BTreeMap<String, u32>,
    pub breakpoints: Vec<u32>,
    image: Image,
    /// Stimulus file text, applied again after every reset (the peripherals are rebuilt then).
    stimulus: Option<String>,
}

impl Machine16 {
    pub fn new(dev: Device) -> Result<Machine16, String> {
        let cpu = Pic16::new(&dev)?;
        Ok(Machine16 { cpu, dev, syms: BTreeMap::new(), breakpoints: Vec::new(), image: Image::default(), stimulus: None })
    }

    pub fn load_hex(&mut self, text: &str) -> Result<(), String> {
        let img = Image::parse(text).map_err(|e| e.to_string())?;
        self.cpu.load(&img);
        self.image.bytes.extend(img.bytes); // reset() reloads everything loaded so far, as the running machine has it
        Ok(())
    }

    /// Drive the part as a stimulus file says (see `stimulus.rs`); on a PIC16 part only `[[uart]]` is modelled.
    pub fn load_stimulus(&mut self, text: &str) -> Result<(), String> {
        let st = crate::stimulus::Stimulus::parse(text)?;
        st.apply16(&self.dev, &mut self.cpu)?;
        self.stimulus = Some(text.to_string());
        Ok(())
    }

    /// Power-on reset: the core and memory return to their reset state and the program is loaded again.
    pub fn reset(&mut self) -> Result<(), String> {
        let mut c = Pic16::new(&self.dev)?;
        c.load(&self.image);
        if let Some(t) = &self.stimulus {
            crate::stimulus::Stimulus::parse(t)?.apply16(&self.dev, &mut c)?; // it applied once on this device, so it applies again
        }
        self.cpu = c;
        Ok(())
    }

    pub fn load_symbols_from_asm(&mut self, asm: &str) {
        self.syms.extend(parse_asm_symbols(asm));
    }

    /// Run until `until` (a word address), a breakpoint, SLEEP with nothing to wake it, or `max_cycles` more cycles.
    /// Reaching `until` by a CALL (the stack grew) is a call into the routine that starts there, not the end.
    pub fn run(&mut self, until: Option<u32>, max_cycles: u64) -> Stop {
        let limit = self.cpu.cycles.saturating_add(max_cycles);
        let mut first = true;
        let mut depth = self.cpu.depth();
        while self.cpu.cycles < limit {
            let pc = self.cpu.pc;
            let d = self.cpu.depth();
            let called = d > depth;
            depth = d;
            if Some(pc) == until && !called {
                return Stop::Halt;
            }
            if !first && self.breakpoints.contains(&pc) {
                return Stop::Breakpoint(pc);
            }
            first = false;
            self.cpu.step();
            if self.cpu.sleep_dead() {
                return Stop::Sleep;
            }
        }
        Stop::Cycles
    }

    /// Bytes of a variable (little endian) as the CPU would read them, without side effects.
    pub fn peek(&self, addr: u32, len: usize) -> Vec<u8> {
        (0..len).map(|i| self.cpu.peek(addr + i as u32)).collect()
    }
}
