//! A part loaded with a program: core, memory, symbols, and the run loop.

use crate::debuginfo::DebugInfo;
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

/// State of a source-line step that runs over several calls of `Machine::line_step`.
#[derive(Clone, Debug)]
pub struct LineStep {
    over: bool,
    out: bool,
    here: Option<(std::path::PathBuf, u32)>,
    d0: u8,
}

pub struct Machine {
    pub cpu: Pic18,
    pub dev: Device,
    pub syms: BTreeMap<String, u32>,
    pub breakpoints: Vec<u32>,
    /// Routines, typed variables and source lines from the ELF (`basil -g`); empty without one.
    pub debug: DebugInfo,
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
        Ok(Machine { cpu, dev, syms: BTreeMap::new(), breakpoints: Vec::new(), debug: DebugInfo::default(), image: Image::default(), stimulus: None })
    }

    pub fn load_hex(&mut self, text: &str) -> Result<(), String> {
        let img = Image::parse(text).map_err(|e| e.to_string())?;
        self.cpu.load(&img);
        self.image = img;
        Ok(())
    }

    /// Drive the part as a stimulus file says (see `stimulus.rs`).
    pub fn load_stimulus(&mut self, text: &str) -> Result<(), String> {
        let st = crate::stimulus::Stimulus::parse(text)?;
        let mclr = st.mclr.clone();
        st.apply(&self.dev, &mut self.cpu.p)?;
        self.cpu.set_mclr_events(mclr);
        self.stimulus = Some(text.to_string());
        self.cpu.stim = self.stimulus.clone();
        Ok(())
    }

    /// Power-on reset: the core and memory return to their reset state and the program is loaded again.
    pub fn reset(&mut self) {
        if let Ok(mut c) = Pic18::new(&self.dev) {
            c.load(&self.image);
            if let Some(t) = &self.stimulus
                && let Ok(st) = crate::stimulus::Stimulus::parse(t)
            {
                c.set_mclr_events(st.mclr.clone());
                let _ = st.apply(&self.dev, &mut c.p); // it applied once on this device, so it applies again
            }
            c.p.set_board(self.cpu.p.board.fresh());
            c.stim = self.stimulus.clone();
            self.cpu = c;
        }
    }

    /// Reads the debug information from an ELF file.
    pub fn load_elf(&mut self, path: &std::path::Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.load_debug(DebugInfo::from_elf(&bytes).map_err(|e| format!("{}: {e}", path.display()))?);
        Ok(())
    }

    /// Takes the debug information of an ELF: routines (and `__halt`) and global variables become symbols too,
    /// without replacing any that the assembly listing already gave.
    pub fn load_debug(&mut self, d: DebugInfo) {
        for f in &d.funcs {
            self.syms.entry(f.name.clone()).or_insert(f.low);
        }
        for v in d.vars.iter().filter(|v| v.scope.is_none()) {
            self.syms.entry(v.name.clone()).or_insert(v.addr);
        }
        self.debug = d;
    }

    /// Symbols from the assembly listing basil writes next to the hex: `;SYM name 0xADDR` for code labels and
    /// `name equ 0xADDR` for variables.
    pub fn load_symbols_from_asm(&mut self, asm: &str) {
        self.syms.extend(parse_asm_symbols(asm));
    }

    /// Hardware stack depth (STKPTR).
    pub fn stack_depth(&self) -> u8 {
        self.cpu.m.mem[self.cpu.m.regs.stkptr as usize] & 0x7F
    }

    /// Starts a source-line step (see `line_step`).
    pub fn begin_line_step(&self, over: bool, out: bool) -> LineStep {
        LineStep { over, out, here: self.debug.lines.lookup(self.cpu.pc).map(|(f, l)| (f.clone(), l)), d0: self.stack_depth() }
    }

    /// Runs a source-line step for at most `budget` instructions. `None`: not finished, call again. `Some(None)`: stopped on
    /// a new line (or, for `out`, after the routine returned). `Some(Some(stop))`: a breakpoint or `__halt` came first.
    /// `over` runs calls and interrupts to completion; `out` runs until the current routine returns.
    pub fn line_step(&mut self, s: &LineStep, budget: u32) -> Option<Option<Stop>> {
        let halt = self.syms.get("__halt").copied();
        for _ in 0..budget {
            let before = self.stack_depth();
            self.cpu.step();
            let pc = self.cpu.pc;
            // Arriving at `__halt` by a CALL is a call into the routine that starts there, not the end of the program.
            if Some(pc) == halt && self.stack_depth() <= before {
                return Some(Some(Stop::Halt));
            }
            if self.breakpoints.contains(&pc) {
                return Some(Some(Stop::Breakpoint(pc)));
            }
            let d = self.stack_depth();
            if s.out {
                if d < s.d0 {
                    return Some(None);
                }
                continue;
            }
            if s.over && d > s.d0 {
                continue;
            }
            if let Some((f, l)) = self.debug.lines.lookup(pc)
                && s.here.as_ref().is_none_or(|(hf, hl)| hf != f || *hl != l)
            {
                return Some(None);
            }
        }
        None
    }

    /// A source-line step in one go, giving up after `limit` instructions (`Stop::Cycles`).
    pub fn step_line(&mut self, over: bool, out: bool, limit: u32) -> Option<Stop> {
        let s = self.begin_line_step(over, out);
        self.line_step(&s, limit).unwrap_or(Some(Stop::Cycles))
    }

    /// Run until `until` (a program address), a breakpoint, SLEEP, or `max_cycles` more cycles.
    pub fn run(&mut self, until: Option<u32>, max_cycles: u64) -> Stop {
        let limit = self.cpu.cycles.saturating_add(max_cycles);
        let mut first = true;
        let mut depth = self.stack_depth();
        while self.cpu.cycles < limit {
            let pc = self.cpu.pc;
            // `__halt` is the address after the program body, where a routine may start: reaching it by a CALL
            // (the stack grew) is a call, not the end of the program.
            let d = self.stack_depth();
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

/// The symbols of an assembly listing basil writes: `;SYM name 0xADDR` for code labels and `name equ 0xADDR` for variables.
pub fn parse_asm_symbols(asm: &str) -> Vec<(String, u32)> {
    let hex = |a: &str| a.strip_prefix("0x").and_then(|h| u32::from_str_radix(h, 16).ok());
    let mut out = Vec::new();
    for l in asm.lines() {
        if let Some(r) = l.strip_prefix(";SYM ") {
            let mut it = r.split_whitespace();
            if let (Some(n), Some(a)) = (it.next(), it.next())
                && let Some(v) = hex(a)
            {
                out.push((n.to_string(), v));
            }
        } else {
            let mut it = l.split_whitespace();
            if let (Some(n), Some("equ"), Some(a)) = (it.next(), it.next(), it.next())
                && let Some(v) = hex(a)
            {
                out.push((n.to_string(), v));
            }
        }
    }
    out
}
