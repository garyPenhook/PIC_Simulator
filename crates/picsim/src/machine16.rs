//! The front-end object for the PIC16 enhanced mid-range core, the counterpart of `Machine` (which is PIC18 only).
//! Symbols are word addresses for code and data addresses for variables, as `;SYM` and `equ` give them.

use crate::debuginfo::DebugInfo;
use crate::hex::Image;
use crate::machine::{parse_asm_symbols, LineStep, Stop};
use crate::pic16::Pic16;
use devdesc::Device;
use std::collections::BTreeMap;

pub struct Machine16 {
    pub cpu: Pic16,
    pub dev: Device,
    pub syms: BTreeMap<String, u32>,
    pub breakpoints: Vec<u32>,
    /// Routines, typed variables and source lines from the ELF (`basil -g`); empty without one. Program addresses in it are
    /// word addresses, as the PIC16 PC and the `;SYM` labels are.
    pub debug: DebugInfo,
    image: Image,
    /// Stimulus file text, applied again after every reset (the peripherals are rebuilt then).
    stimulus: Option<String>,
    /// `run` stops at its `until` address even when the PC gets there by a CALL: the address is the entry of a routine
    /// (`basil-sim --until ROUTINE`), not basil's `__halt`, which may be the first address of a routine that follows the program.
    pub until_routine: bool,
}

impl Machine16 {
    pub fn new(dev: Device) -> Result<Machine16, String> {
        let cpu = Pic16::new(&dev)?;
        Ok(Machine16 { cpu, dev, syms: BTreeMap::new(), breakpoints: Vec::new(), debug: DebugInfo::default(), image: Image::default(), stimulus: None, until_routine: false })
    }

    pub fn load_hex(&mut self, text: &str) -> Result<(), String> {
        let img = Image::parse(text).map_err(|e| e.to_string())?;
        self.cpu.load(&img);
        self.image.bytes.extend(img.bytes); // reset() reloads everything loaded so far, as the running machine has it
        Ok(())
    }

    /// Drive the part as a stimulus file says (see `stimulus.rs`); on a PIC16 part `[[uart]]`, `[[adc]]` and `[[pin]]`/`[[port]]` are modelled.
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
        c.set_board(self.cpu.board.fresh()); // the parts stay wired, back at power-on
        self.cpu = c;
        self.watch_entries();
        Ok(())
    }

    pub fn load_symbols_from_asm(&mut self, asm: &str) {
        self.syms.extend(parse_asm_symbols(asm));
    }

    /// Reads the debug information from an ELF file.
    pub fn load_elf(&mut self, path: &std::path::Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.load_debug(DebugInfo::from_elf(&bytes).map_err(|e| format!("{}: {e}", path.display()))?);
        Ok(())
    }

    /// Takes the debug information of an ELF: routines (and `__halt`) and global variables become symbols too,
    /// without replacing any that the assembly listing already gave.
    pub fn load_debug(&mut self, mut d: DebugInfo) {
        d.set_word_addressed();
        d.linear_end = Some(self.cpu.linear_end());
        d.fsr1l = self.dev.find_sfr("FSR1L").map(|s| s.addr);
        for f in &d.funcs {
            self.syms.entry(f.name.clone()).or_insert(f.low);
        }
        for v in d.vars.iter().filter(|v| v.scope.is_none() && !v.rom) {
            self.syms.entry(v.name.clone()).or_insert(v.addr);
        }
        self.debug = d;
        self.watch_entries();
    }

    /// Where each routine with a pending return is, innermost first: the CALL (the word before a return address), or for an
    /// interrupt (`Pic16::entry_irq`) the instruction it will resume at.
    pub fn call_sites(&self) -> Vec<u32> {
        (0..self.levels()).rev().map(|i| if self.cpu.entry_irq[i + 1] { self.cpu.stack_entry(i) } else { self.cpu.stack_entry(i).saturating_sub(1) }).collect()
    }

    /// Return-stack levels in use, within the 16 the part has.
    fn levels(&self) -> usize {
        (self.cpu.depth() as usize).min(16)
    }

    /// The live calls with their frame bases (`DebugInfo::frames`), innermost first: FSR1 at each call or entry from `Pic16::entry_sp`.
    pub fn frames(&self) -> Vec<crate::debuginfo::Frame> {
        // Past 16 levels (an overflow with STVREN = 0) the 17th push overwrote level 1, so no level noted by a push can be
        // trusted; level 0 (main, noted on entry) still can.
        let full = self.cpu.depth() > 16;
        let entries: Vec<Option<u32>> = (0..=self.levels()).rev().map(|l| self.cpu.entry_sp[l].filter(|_| l == 0 || !full)).collect();
        self.debug.frames(self.cpu.pc, &self.call_sites(), &entries, &self.cpu.prog)
    }

    /// Has the core note FSR1 when `main` or a routine with a stack frame is entered (`Pic16::entry_watch`); after loading
    /// debug information and a Reset.
    fn watch_entries(&mut self) {
        self.cpu.entry_watch = self.debug.entry_points();
        self.cpu.note_entry(); // the PC may already be at one
    }

    /// Starts a source-line step (see `line_step`).
    pub fn begin_line_step(&self, over: bool, out: bool) -> LineStep {
        LineStep { over, out, here: self.debug.lines.lookup(self.cpu.pc).map(|(f, l)| (f.clone(), l)), d0: self.cpu.depth() as u8 }
    }

    /// Runs a source-line step for at most `budget` instructions, as `Machine::line_step` does: `None` not finished, `Some(None)`
    /// on a new line (or, for `out`, after the routine returned), `Some(Some(stop))` a breakpoint or `__halt` came first. An
    /// interrupt entry pushes the return stack like a call, so `over` runs the handler to completion too.
    pub fn line_step(&mut self, s: &LineStep, budget: u32) -> Option<Option<Stop>> {
        let halt = self.syms.get("__halt").copied();
        for _ in 0..budget {
            let before = self.cpu.depth();
            self.cpu.step();
            if self.cpu.sleep_dead() {
                return Some(Some(Stop::Sleep)); // nothing can wake the part: the step would never finish
            }
            let pc = self.cpu.pc;
            // Arriving at `__halt` by a CALL is a call into the routine that starts there, not the end of the program.
            if Some(pc) == halt && self.cpu.depth() <= before {
                return Some(Some(Stop::Halt));
            }
            if self.breakpoints.contains(&pc) {
                return Some(Some(Stop::Breakpoint(pc)));
            }
            let d = self.cpu.depth();
            if s.out {
                if d < u32::from(s.d0) {
                    return Some(None);
                }
                continue;
            }
            if s.over && d > u32::from(s.d0) {
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

    /// Run until `until` (a word address), a breakpoint, SLEEP with nothing to wake it, or `max_cycles` more cycles.
    /// Reaching `until` by a CALL (the stack grew) is a call into the routine that starts there, not the end.
    pub fn run(&mut self, until: Option<u32>, max_cycles: u64) -> Stop {
        self.cpu.lazy = true;
        let stop = self.run_lazy(until, max_cycles);
        self.cpu.settle();
        stop
    }

    fn run_lazy(&mut self, until: Option<u32>, max_cycles: u64) -> Stop {
        let limit = self.cpu.cycles.saturating_add(max_cycles);
        let mut first = true;
        let mut depth = self.cpu.depth();
        while self.cpu.cycles < limit {
            let pc = self.cpu.pc;
            let d = self.cpu.depth();
            let called = d > depth;
            depth = d;
            if Some(pc) == until && (!called || self.until_routine) {
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
