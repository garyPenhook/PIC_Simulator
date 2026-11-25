//! The front-end object for the dsPIC33 core, the counterpart of `Machine` (PIC18) and `Machine16` (PIC16).
//! Symbols and breakpoints are program addresses as the PC shows them (two per instruction) and data addresses.

use crate::debuginfo::DebugInfo;
use crate::dspic::Dspic;
use crate::hex::Image;
use crate::machine::{LineStep, Stop};
use devdesc::Device;
use std::collections::BTreeMap;

pub struct MachineDs {
    pub cpu: Dspic,
    pub dev: Device,
    pub syms: BTreeMap<String, u32>,
    pub breakpoints: Vec<u32>,
    pub debugger: crate::debugger::Debugger,
    /// Routines, typed variables and source lines from the ELF (`basil -g`); empty without one.
    pub debug: DebugInfo,
    image: Image,
    /// Stimulus file text, applied again after every reset (the peripherals are rebuilt then).
    stimulus: Option<String>,
    /// `run` stops at its `until` address even when the PC gets there by a CALL (`basil-sim --until ROUTINE`).
    pub until_routine: bool,
    /// Stop after an instruction touches an SFR without a model.
    pub strict: bool,
    /// Notes about the files loaded, given to the core again after every reset.
    load_notes: Vec<String>,
}

impl MachineDs {
    pub fn new(dev: Device) -> Result<MachineDs, String> {
        let cpu = Dspic::new(&dev)?;
        Ok(MachineDs { cpu, dev, syms: BTreeMap::new(), breakpoints: Vec::new(), debugger: crate::debugger::Debugger::default(), debug: DebugInfo::default(), image: Image::default(), stimulus: None, until_routine: false, strict: false, load_notes: Vec::new() })
    }

    pub fn load_hex(&mut self, text: &str) -> Result<(), String> {
        let img = Image::parse(text).map_err(|e| e.to_string())?;
        self.cpu.load(&img);
        self.note_load(crate::hex::stray_note(&self.dev, &img));
        self.image.bytes.extend(img.bytes);
        let empty = crate::hex::empty_note(&self.dev, &self.image);
        self.load_notes.retain(|n| Some(n) != empty.as_ref());
        match empty {
            Some(n) => self.note_load(Some(n)),
            None => {
                self.cpu.notes.retain(|n| !n.starts_with("the hex file has no program bytes"));
            }
        }
        Ok(())
    }

    /// Drive the part as a stimulus file says (see `stimulus.rs`).
    pub fn load_stimulus(&mut self, text: &str) -> Result<(), String> {
        let st = crate::stimulus::Stimulus::parse(text)?;
        st.apply_dspic(&self.dev, &mut self.cpu)?;
        self.stimulus = Some(text.to_string());
        Ok(())
    }

    /// Power-on reset: the core and memory return to their reset state and the program is loaded again.
    pub fn reset(&mut self) -> Result<(), String> {
        let mut c = Dspic::new(&self.dev)?;
        c.load(&self.image);
        if let Some(t) = &self.stimulus {
            crate::stimulus::Stimulus::parse(t)?.apply_dspic(&self.dev, &mut c)?;
        }
        self.cpu = c;
        self.cpu.notes.extend(self.load_notes.iter().cloned());
        self.debugger.reset();
        Ok(())
    }

    fn note_load(&mut self, n: Option<String>) {
        if let Some(n) = n {
            self.cpu.notes.insert(n.clone());
            self.load_notes.push(n);
        }
    }

    /// The symbols of the assembly listing basil writes for a dsPIC33 (`.equ NAME, 0xADDR` for variables) and of the link
    /// map (`0xADDR  name` for routines and labels).
    pub fn load_symbols_from_asm(&mut self, asm: &str) {
        for l in asm.lines() {
            let l = l.trim();
            if let Some(r) = l.strip_prefix(".equ ") {
                let mut it = r.splitn(2, ',');
                if let (Some(n), Some(rest)) = (it.next(), it.next()) {
                    let a = rest.split(';').next().unwrap_or("").trim();
                    if let Some(v) = a.strip_prefix("0x").and_then(|h| u32::from_str_radix(h, 16).ok()) {
                        self.syms.entry(n.trim().to_string()).or_insert(v);
                    }
                }
            }
        }
    }

    pub fn load_symbols_from_map(&mut self, map: &str) {
        for l in map.lines() {
            let mut it = l.split_whitespace();
            if let (Some(a), Some(n), None) = (it.next(), it.next(), it.next())
                && let Some(v) = a.strip_prefix("0x").and_then(|h| u32::from_str_radix(h, 16).ok())
                && !n.starts_with('.')
                && !n.contains('=')
            {
                self.syms.entry(n.to_string()).or_insert(v);
            }
        }
    }

    pub fn load_elf(&mut self, path: &std::path::Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.load_debug(DebugInfo::from_elf(&bytes).map_err(|e| format!("{}: {e}", path.display()))?);
        Ok(())
    }

    /// A COFF file is not written for a dsPIC33 program.
    pub fn load_cof(&mut self, _path: &std::path::Path) -> Result<(), String> {
        Err("COF files are not supported for dsPIC33 parts".into())
    }

    pub fn load_debug(&mut self, d: DebugInfo) {
        for f in &d.funcs {
            self.syms.entry(f.name.clone()).or_insert(f.low);
        }
        for v in d.vars.iter().filter(|v| v.scope.is_none() && !v.rom) {
            self.syms.entry(v.name.clone()).or_insert(v.addr);
        }
        self.debug = d;
    }

    /// Where each live call is, innermost first: the address of the CALL/RCALL instruction that made it.
    pub fn call_sites(&self) -> Vec<u32> {
        self.cpu.return_addresses().into_iter().map(|ra| self.cpu.call_before(ra)).collect()
    }

    /// The live calls with their frame bases (none are known for a dsPIC), innermost first.
    pub fn frames(&self) -> Vec<crate::debuginfo::Frame> {
        let sites = self.call_sites();
        let entries: Vec<Option<u32>> = vec![None; sites.len() + 1];
        self.debug.frames(self.cpu.pc, &sites, &entries, &self.cpu.prog_b)
    }

    /// Starts a source-line step (see `line_step`).
    pub fn begin_line_step(&self, over: bool, out: bool) -> LineStep {
        LineStep { over, out, here: self.debug.lines.lookup(self.cpu.pc).map(|(f, l)| (f.clone(), l)), d0: self.cpu.call_depth.clamp(0, 255) as u8 }
    }

    /// Runs a source-line step for at most `budget` instructions: `None` not finished, `Some(None)` on a new line (or, for
    /// `out`, after the routine returned), `Some(Some(stop))` a breakpoint or `__halt` came first.
    pub fn line_step(&mut self, s: &LineStep, budget: u32) -> Option<Option<Stop>> {
        if self.strict && !self.cpu.unmodelled.is_empty() {
            return Some(Some(Stop::Unmodelled));
        }
        let halt = self.syms.get("__halt").copied();
        for _ in 0..budget {
            let before = self.cpu.call_depth;
            if let Some(stop) = self.step() {
                return Some(Some(stop));
            }
            if self.strict && !self.cpu.unmodelled.is_empty() {
                return Some(Some(Stop::Unmodelled));
            }
            if self.cpu.sleep_dead() {
                return Some(Some(Stop::Sleep));
            }
            let pc = self.cpu.pc;
            if Some(pc) == halt && self.cpu.call_depth <= before {
                return Some(Some(Stop::Halt));
            }
            if self.breakpoints.contains(&pc) {
                return Some(Some(Stop::Breakpoint(pc)));
            }
            if let Some((index, value)) = self.debugger.cond_hit_at(pc, |a| self.cpu.peek(a)) {
                return Some(Some(Stop::Condition { index, value }));
            }
            let d = self.cpu.call_depth;
            if s.out {
                if d < i32::from(s.d0) {
                    return Some(None);
                }
                continue;
            }
            if s.over && d > i32::from(s.d0) {
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

    /// Run until `until` (a program address), a breakpoint, SLEEP with nothing to wake it, or `max_cycles` more cycles.
    pub fn run(&mut self, until: Option<u32>, max_cycles: u64) -> Stop {
        if self.strict && !self.cpu.unmodelled.is_empty() {
            return Stop::Unmodelled;
        }
        let limit = self.cpu.cycles.saturating_add(max_cycles);
        let mut first = true;
        let mut depth = self.cpu.call_depth;
        while self.cpu.cycles < limit {
            let pc = self.cpu.pc;
            let d = self.cpu.call_depth;
            let called = d > depth;
            depth = d;
            if Some(pc) == until && (!called || self.until_routine) {
                return Stop::Halt;
            }
            if !first && self.breakpoints.contains(&pc) {
                return Stop::Breakpoint(pc);
            }
            if !first && let Some((index, value)) = self.debugger.cond_hit_at(pc, |a| self.cpu.peek(a)) {
                return Stop::Condition { index, value };
            }
            first = false;
            if let Some(stop) = self.step() {
                return stop;
            }
            if self.strict && !self.cpu.unmodelled.is_empty() {
                return Stop::Unmodelled;
            }
            if self.cpu.sleep_dead() {
                return Stop::Sleep;
            }
        }
        Stop::Cycles
    }

    /// Execute one instruction, observing data changes when enabled.
    pub fn step(&mut self) -> Option<Stop> {
        if !self.debugger.active() {
            self.cpu.step();
            return None;
        }
        let before = self.debugger.snapshot(|a| self.cpu.peek(a));
        let cond_before = self.debugger.cond_state(|a| self.cpu.peek(a));
        self.debugger.sample(self.cpu.time_ns() as u64, |a| self.cpu.peek(a));
        self.cpu.step();
        self.debugger.sample(self.cpu.time_ns() as u64, |a| self.cpu.peek(a));
        if let Some(c) = self.debugger.changed(&before, |a| self.cpu.peek(a)) {
            return Some(Stop::Watchpoint(c));
        }
        self.debugger.cond_hit_data(&cond_before, |a| self.cpu.peek(a)).map(|(index, value)| Stop::Condition { index, value })
    }

    /// Bytes of a variable (little endian) as the CPU would read them, without side effects.
    pub fn peek(&self, addr: u32, len: usize) -> Vec<u8> {
        (0..len).map(|i| self.cpu.peek(addr + i as u32)).collect()
    }
}
