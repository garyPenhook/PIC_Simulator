//! Routines and typed variables from the debug unit `basil -g` appends to the ELF (see `src/dwarf.rs` in the compiler)
//! or that MPLAB XC8 writes for each C file (`xc8-cc -g`),
//! together with the line table of `lines.rs`. The simulator front ends use it to show variables by name and type,
//! the routine the PC is in, and a call stack with source lines.

use crate::lines::LineMap;
use gimli::{AttributeValue, EndianSlice, LittleEndian};
use object::{Object, ObjectSection};

type R<'a> = EndianSlice<'a, LittleEndian>;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Unsigned,
    Signed,
    Bool,
    Char,
    Float,
}

/// A variable at a fixed data-memory address, or (`frame`) at an offset in its routine's frame on XC8's software stack.
#[derive(Clone, Debug)]
pub struct Var {
    pub name: String,
    /// The data (or, for `rom`, program) address; 0 for a variable in a stack frame (`frame`).
    pub addr: u32,
    /// Size of the whole variable (an array: all elements).
    pub bytes: u32,
    pub kind: Kind,
    /// Element size; equals `bytes` for a scalar.
    pub elem: u32,
    pub array: bool,
    /// The type's name as the compiler wrote it (`Byte`, `Integer`, `Char`, ...).
    pub ty: String,
    /// Index into `DebugInfo::funcs` for a parameter or local; `None` for a global.
    pub scope: Option<usize>,
    pub param: bool,
    /// The variable is a constant in program memory (`addr` is a program address), not in data memory.
    pub rom: bool,
    /// The full type: members of a structure or union, enumerators, what a pointer points to.
    pub layout: Ty,
    /// For a parameter or local on XC8's software stack (`-mstack=reentrant`, or a routine `hybrid` puts there): the offset
    /// from the routine's frame base (`DW_OP_fbreg`). Its address is then known only while a frame of the routine is live;
    /// see `DebugInfo::frames` and `Var::place`.
    pub frame: Option<i32>,
    /// The compile unit (source file) that describes it: an index into `DebugInfo::units`.
    pub unit: usize,
    /// A global declared `static` (no `DW_AT_external`): it belongs to its unit, and another unit may have one of the same
    /// name (`DebugInfo::global`). False for parameters and locals.
    pub file_static: bool,
}

/// A routine's program-address range `[low, high)`: byte addresses on a PIC18 part, word addresses on a PIC16 (the units of the PC).
#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub low: u32,
    pub high: u32,
    /// The data address of the stack pointer the routine's frame base is read from (`DW_AT_frame_base`: XC8 names FSR1L).
    pub frame_reg: Option<u32>,
    /// The compile unit (source file) the routine is in: an index into `DebugInfo::units`. Two `static` routines of the
    /// same name in different files are told apart by it (`DebugInfo::func_label`).
    pub unit: usize,
}

/// One live call of a routine: innermost first in `DebugInfo::frames`. `base` is the frame base of that call (the value
/// `DW_OP_fbreg` offsets add to), when it can be worked out.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub func: Option<usize>,
    pub base: Option<u32>,
}

/// The PIC18 registers XC8's software-stack prologues and epilogues name (from the device description), besides FSR1L
/// (`DebugInfo::fsr1l`): FSR1H, POSTINC1 and POSTDEC1. A PIC16 needs none (its prologues use `ADDFSR 1,k`).
#[derive(Clone, Copy, Debug, Default)]
pub struct StackRegs {
    pub fsr1h: u32,
    pub postinc1: u32,
    pub postdec1: u32,
}

/// How a routine sets up its frame on XC8's software stack: the prologue at its start adds `auto` bytes to FSR1. The frame
/// base of a call is FSR1 at the call plus `auto` (`DebugInfo::frames`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub auto: i32,
}

#[derive(Clone, Debug, Default)]
pub struct DebugInfo {
    pub funcs: Vec<Func>,
    pub vars: Vec<Var>,
    /// The name (`DW_AT_name`, the source file's path) of each compile unit the routines and variables come from: XC8
    /// writes one per `.c` file of the program, basil one for the program.
    pub units: Vec<String>,
    pub lines: LineMap,
    /// Every structure or union type seen, by the id a `Form::BackPtr` carries.
    pub structs: std::collections::HashMap<u32, Ty>,
    /// Program-memory address ranges that hold constants (`const` tables and string literals): variables there are read
    /// from program memory, not data memory.
    pub rom: Vec<(u32, u32)>,
    /// Where a one-byte pointer into program memory (PIC18 `const` data) starts counting: the address of XC8's `smallconst` section.
    pub near_base: Option<u32>,
    /// True for a PIC16 (program memory is addressed in words, one byte of constant data per word).
    pub word_addressed: bool,
    /// Set by `Machine::load_debug` for a PIC18 part; needed to read a PIC18 routine's prologue (see `shape`).
    pub stack_regs: Option<StackRegs>,
    /// A PIC16: the end (exclusive) of the part's linear data region, set by `Machine16::load_debug` (`data_view`).
    pub linear_end: Option<u32>,
    /// The part's FSR1L (from the device description, set by the machines' `load_debug`): `frames` gives a frame base only
    /// to a routine whose `DW_AT_frame_base` names it.
    pub fsr1l: Option<u32>,
}

impl DebugInfo {
    pub fn from_elf(bytes: &[u8]) -> Result<Self, String> {
        let lines = LineMap::from_elf(bytes).unwrap_or_default();
        let obj = object::File::parse(bytes).map_err(|e| e.to_string())?;
        let load = |id: gimli::SectionId| -> Result<R<'_>, gimli::Error> {
            let data = obj.section_by_name(id.name()).and_then(|s| s.data().ok()).unwrap_or(&[]);
            Ok(EndianSlice::new(data, LittleEndian))
        };
        let dwarf = gimli::Dwarf::load(load).map_err(|e| e.to_string())?;
        let mut info = DebugInfo { lines, ..Default::default() };
        // Constants live in program memory: loadable sections that are neither code nor the image of initialised data. A variable
        // in one of them is a constant unless it is also inside a data-memory (NOBITS) section, as the two address spaces overlap.
        let ram: Vec<(u32, u32)> = obj.sections().filter(|s| s.kind() == object::SectionKind::UninitializedData).map(|s| (s.address() as u32, (s.address() + s.size()) as u32)).collect();
        for s in obj.sections() {
            let name = s.name().unwrap_or("");
            if name.starts_with("smallconst") && info.near_base.is_none_or(|b| (s.address() as u32) < b) {
                info.near_base = Some(s.address() as u32);
            }
            let flags: u64 = match s.flags() {
                object::SectionFlags::Elf { sh_flags, .. } => sh_flags.0,
                _ => 0,
            };
            let is_const = s.kind() != object::SectionKind::UninitializedData && flags & 0x2 != 0 && flags & 0x4 == 0 && !name.starts_with("idata") && !name.starts_with("cinit") && s.size() > 0;
            if is_const {
                info.rom.push((s.address() as u32, (s.address() + s.size()) as u32));
            }
        }
        let mut units = dwarf.units();
        while let Some(h) = units.next().map_err(|e| e.to_string())? {
            let unit = dwarf.unit(h).map_err(|e| e.to_string())?;
            let mut entries = unit.entries();
            // Only the compiler's own units: the assembler writes entries for its symbols too, and XC8 writes one for the
            // device header, which holds every SFR as a variable.
            let mut ours = false;
            let mut xc8 = false;
            let mut cur_func: Option<usize> = None;
            let mut func_depth = 0isize;
            while let Some(e) = entries.next_dfs().map_err(|e| e.to_string())? {
                let depth = e.depth();
                if depth <= func_depth && cur_func.is_some() {
                    cur_func = None;
                }
                match e.tag() {
                    gimli::DW_TAG_compile_unit => {
                        let text = |at: gimli::DwAt| e.attr_value(at).and_then(|v| dwarf.attr_string(&unit, v).ok()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        let producer = text(gimli::DW_AT_producer);
                        // XC8 writes a unit per C file; only those (not the `.s` or `.h` units) hold the program's routines and variables.
                        xc8 = producer.starts_with("Microchip MPLAB XC8");
                        ours = producer == "basil" || (producer.starts_with("Microchip MPLAB XC8") && text(gimli::DW_AT_name).to_ascii_lowercase().ends_with(".c"));
                        if ours {
                            info.units.push(text(gimli::DW_AT_name));
                        }
                    }
                    gimli::DW_TAG_subprogram if ours => {
                        let name = attr_name(&dwarf, &unit, e).unwrap_or_default();
                        // high_pc is an address (basil, XC8 so far) or, in DWARF 4 style, an offset from low_pc. XC8 4.00 writes
                        // the routine's last address (the second byte of a PIC18's final RETURN, a PIC16's final RETURN word),
                        // not the address after it, so its end is one more.
                        let (low, high) = match (e.attr_value(gimli::DW_AT_low_pc), e.attr_value(gimli::DW_AT_high_pc)) {
                            (Some(AttributeValue::Addr(l)), Some(AttributeValue::Addr(h))) => (l, if xc8 && h >= l { h + 1 } else { h }),
                            (Some(AttributeValue::Addr(l)), Some(h)) if h.udata_value().is_some() => (l, l + h.udata_value().unwrap_or(0)),
                            _ => continue,
                        };
                        cur_func = Some(info.funcs.len());
                        func_depth = depth;
                        let frame_reg = match e.attr_value(gimli::DW_AT_frame_base) {
                            Some(AttributeValue::Exprloc(x)) => fixed_addr(x.0.slice()),
                            Some(AttributeValue::Block(b)) => fixed_addr(b.slice()),
                            _ => None,
                        };
                        info.funcs.push(Func { name, low: low as u32, high: high as u32, frame_reg, unit: info.units.len().saturating_sub(1) });
                    }
                    gimli::DW_TAG_variable | gimli::DW_TAG_formal_parameter if ours => {
                        let name = attr_name(&dwarf, &unit, e).unwrap_or_default();
                        let ex: Vec<u8> = match e.attr_value(gimli::DW_AT_location) {
                            Some(AttributeValue::Exprloc(x)) => x.0.slice().to_vec(),
                            Some(AttributeValue::Block(b)) => b.slice().to_vec(),
                            _ => continue,
                        };
                        // DW_OP_addr <address> (4 bytes on a PIC18, 2 may do on a PIC16), or, for a local on XC8's software
                        // stack, DW_OP_fbreg <sleb128>: an offset from the routine's frame base.
                        let (addr, frame) = match (fixed_addr(&ex), ex.first()) {
                            (Some(a), _) => (a, None),
                            (None, Some(&0x91)) if cur_func.is_some() => {
                                let mut r = EndianSlice::new(&ex[1..], LittleEndian);
                                match gimli::Reader::read_sleb128(&mut r) {
                                    Ok(off) if r.is_empty() => (0, Some(off as i32)),
                                    _ => continue,
                                }
                            }
                            _ => continue,
                        };
                        let Some(AttributeValue::UnitRef(t)) = e.attr_value(gimli::DW_AT_type) else { continue };
                        let Some(ty) = describe(&dwarf, &unit, t, &mut Vec::new()) else { continue };
                        register(&ty, &mut info.structs);
                        let rom = frame.is_none() && xc8 && info.rom.iter().any(|r| (r.0..r.1).contains(&addr)) && !ram.iter().any(|r| (r.0..r.1).contains(&addr));
                        let scope = if e.tag() == gimli::DW_TAG_formal_parameter || cur_func.is_some() { cur_func } else { None };
                        // A global without DW_AT_external is `static` to its file (XC8 writes the attribute for the others).
                        let file_static = xc8 && scope.is_none() && !matches!(e.attr_value(gimli::DW_AT_external), Some(AttributeValue::Flag(true)));
                        info.vars.push(Var { rom, name, addr, bytes: ty.bytes, kind: ty.kind.clone(), elem: ty.elem, array: ty.array, ty: ty.name.clone(), layout: ty.clone(), scope, param: e.tag() == gimli::DW_TAG_formal_parameter, frame, unit: info.units.len().saturating_sub(1), file_static });
                    }
                    _ => {}
                }
            }
        }
        Ok(info)
    }

    /// The routine containing the program byte address `pc`.
    pub fn func_at(&self, pc: u32) -> Option<usize> {
        self.funcs.iter().position(|f| f.low <= pc && pc < f.high)
    }

    /// True when `addr` is where a routine with code starts (not an empty-range symbol such as basil's `__halt`).
    pub fn is_routine_entry(&self, addr: u32) -> bool {
        self.funcs.iter().any(|f| f.low == addr && f.high > f.low)
    }

    /// Program memory as one byte per address, which is how `Var::show_with` reads constants and what pointers into it point at:
    /// the bytes themselves on a PIC18; on a PIC16 the low byte of each 14-bit word, which is where XC8 keeps one byte of constant data.
    pub fn rom_image<'a>(&self, prog: &'a [u8]) -> std::borrow::Cow<'a, [u8]> {
        if self.word_addressed {
            prog.chunks(2).map(|w| w[0]).collect::<Vec<u8>>().into()
        } else {
            prog.into()
        }
    }

    /// Marks the program as built for a PIC16: program addresses count words, and the section sizes in the ELF count bytes.
    pub fn set_word_addressed(&mut self) {
        self.word_addressed = true;
        for r in &mut self.rom {
            r.1 = r.0 + (r.1 - r.0) / 2;
        }
        let rom = self.rom.clone();
        for v in &mut self.vars {
            v.rom = v.rom && rom.iter().any(|r| (r.0..r.1).contains(&v.addr));
        }
    }

    /// The address and size of a global written as a path: `pt`, `pt.x`, `sh.tag[1]`, `pts[1].y`. `None` when any part is not found.
    /// A name several files define `static` is found as `global` finds it from outside any routine, or as `path@file.c`.
    pub fn path_addr(&self, path: &str) -> Option<(u32, u32)> {
        self.path_find(path, None, &|v| !v.rom).map(|r| (r.0, r.1))
    }

    /// As `path_addr`, but also finds a `const` kept in program memory; the flag says the address is a program address.
    pub fn path_ref(&self, path: &str) -> Option<(u32, u32, bool)> {
        self.path_ref_at(path, None)
    }

    /// As `path_ref`, with the global's name looked up as C does in the routine containing `pc` (`global`).
    pub fn path_ref_at(&self, path: &str, pc: Option<u32>) -> Option<(u32, u32, bool)> {
        self.path_find(path, pc, &|_| true)
    }

    fn path_find(&self, path: &str, pc: Option<u32>, keep: &dyn Fn(&Var) -> bool) -> Option<(u32, u32, bool)> {
        let (path, file) = split_file(path);
        let end = path.find(['.', '[']).unwrap_or(path.len());
        let v = self.global_where(path[..end].trim(), file, pc, keep)?;
        let (off, ty) = v.layout.resolve(&path[end..])?;
        Some((v.addr + off, ty.bytes, v.rom))
    }

    /// The global `name` as C sees it from the routine containing `pc`: the `static` of that routine's file, else the one
    /// with external linkage, else (from start-up code, or a routine of a third file) the first of that name.
    /// `name@file.c` is the one of that file (the file's name without its folder).
    pub fn global(&self, name: &str, pc: Option<u32>) -> Option<&Var> {
        let (name, file) = split_file(name);
        self.global_where(name, file, pc, &|_| true)
    }

    fn global_where(&self, name: &str, file: Option<&str>, pc: Option<u32>, keep: &dyn Fn(&Var) -> bool) -> Option<&Var> {
        let all = || self.vars.iter().filter(|v| v.scope.is_none() && v.name == name && keep(v));
        if let Some(file) = file {
            return all().find(|v| self.unit_file(v.unit) == file);
        }
        let unit = pc.and_then(|pc| self.func_at(pc)).map(|f| self.funcs[f].unit);
        all().find(|v| v.file_static && Some(v.unit) == unit).or_else(|| all().find(|v| !v.file_static)).or_else(|| all().next())
    }

    /// The routine `name`, or `name@file.c` for the one of that file: the first of that name.
    pub fn routine(&self, name: &str) -> Option<usize> {
        let (name, file) = split_file(name);
        self.funcs.iter().position(|f| f.name == name && file.is_none_or(|file| self.unit_file(f.unit) == file))
    }

    /// The source file of compile unit `u`, without its folder.
    pub fn unit_file(&self, u: usize) -> &str {
        self.units.get(u).map_or("", |n| n.rsplit(['/', '\\']).next().unwrap_or(n))
    }

    /// How to name variable `v` where its name alone is ambiguous: a global that another global shares its name with (a
    /// `static` in each of two files) gets its file, `count (util.c)`; any other variable is its name.
    pub fn var_label(&self, v: &Var) -> String {
        if v.scope.is_none() && self.vars.iter().filter(|w| w.scope.is_none() && w.name == v.name).nth(1).is_some() {
            format!("{} ({})", v.name, self.unit_file(v.unit))
        } else {
            v.name.clone()
        }
    }

    /// As `var_label`, for routine `f`: two `static` routines of the same name in different files get their files.
    pub fn func_label(&self, f: usize) -> String {
        let n = &self.funcs[f].name;
        if self.funcs.iter().filter(|g| g.name == *n).nth(1).is_some() {
            format!("{n} ({})", self.unit_file(self.funcs[f].unit))
        } else {
            n.clone()
        }
    }

    /// True when `name` (without `@file`) is a routine, or a global, of more than one file (a `static` in each).
    pub fn is_ambiguous(&self, name: &str) -> bool {
        let (n, file) = split_file(name);
        file.is_none()
            && (self.funcs.iter().filter(|f| f.name == n).nth(1).is_some() || self.vars.iter().filter(|v| v.scope.is_none() && v.name == n).nth(1).is_some())
    }

    /// When `name` is ambiguous (`is_ambiguous`), a message saying so and how to name one: for front ends that take a
    /// symbol by name outside any routine, which would otherwise pick one of them.
    pub fn ambiguous(&self, name: &str) -> Option<String> {
        if !self.is_ambiguous(name) {
            return None;
        }
        let funcs: Vec<usize> = self.funcs.iter().filter(|f| f.name == name).map(|f| f.unit).collect();
        let vars: Vec<usize> = self.vars.iter().filter(|v| v.scope.is_none() && v.name == name).map(|v| v.unit).collect();
        let (what, units) = if funcs.len() > 1 { ("a routine", funcs) } else { ("a variable", vars) };
        let files: Vec<&str> = units.iter().map(|&u| self.unit_file(u)).collect();
        Some(format!("{name} is {what} in more than one file ({}): name one as {name}@{}", files.join(", "), files[0]))
    }

    /// `len` constant bytes starting at program address `addr` (see `rom_image` for what a byte is on a PIC16); bytes past the end read 0.
    pub fn rom_bytes(&self, prog: &[u8], addr: u32, len: usize) -> Vec<u8> {
        (0..len as u32)
            .map(|i| {
                let a = (addr + i) as usize;
                prog.get(if self.word_addressed { a * 2 } else { a }).copied().unwrap_or(0)
            })
            .collect()
    }

    /// Globals, then the parameters and locals of routine `f` when given.
    pub fn visible(&self, f: Option<usize>) -> impl Iterator<Item = &Var> {
        self.vars.iter().filter(move |v| v.scope.is_none() || v.scope == f)
    }

    /// The first `max` instructions of routine `f` as (address in PC units, first word); `prog` is program memory as the
    /// core holds it (little-endian words; on a PIC16 two bytes per 14-bit word).
    fn code(&self, f: &Func, prog: &[u8], max: usize) -> Vec<(u32, u16)> {
        let word = |a: u32| {
            let i = if self.word_addressed { 2 * a as usize } else { a as usize };
            let w = u16::from(prog.get(i).copied().unwrap_or(0xFF)) | u16::from(prog.get(i + 1).copied().unwrap_or(0xFF)) << 8;
            if self.word_addressed { w & 0x3FFF } else { w }
        };
        let mut out = Vec::new();
        let mut a = f.low;
        while a < f.high && out.len() < max {
            let w = word(a);
            out.push((a, w));
            // Instruction lengths from the PIC18 disassembler (2- and 3-word instructions); every PIC16 instruction is one word.
            a += if self.word_addressed { 1 } else { 2 * crate::disasm::disasm(a, w, word(a + 2), word(a + 4), false).1 };
        }
        out
    }

    /// A stack-pointer adjustment XC8 writes in a prologue or epilogue, starting at instruction `i`: the bytes it adds to FSR1
    /// and how many instructions it takes. PIC18 (encodings as in `disasm.rs`, DS40002214G Table 45-2): `MOVF POSTINC1,F` (+1)
    /// or `MOVF POSTDEC1,F` (-1) in the access bank, or `MOVLW lo; ADDWF FSR1L,F; MOVLW hi; ADDWFC FSR1H,F` (a 16-bit
    /// amount). PIC16 (`pic16.rs` `disasm_for`): `ADDFSR 1,k`, k from -32 to 31 (the assembler writes several for more).
    fn sp_step(&self, code: &[(u32, u16)], i: usize) -> Option<(i32, usize)> {
        let w = code.get(i)?.1;
        if self.word_addressed {
            return (w & 0x3F80 == 0x3100 && w & 0x40 != 0).then_some((((w & 0x3F) as i32) << 26 >> 26, 1));
        }
        let (r, fsr1l) = (self.stack_regs?, self.fsr1l?);
        let f = |a: u32| (a & 0xFF) as u16;
        if w == 0x5200 | f(r.postinc1) {
            return Some((1, 1));
        }
        if w == 0x5200 | f(r.postdec1) {
            return Some((-1, 1));
        }
        match code.get(i..i + 4)? {
            [(_, lo), (_, add), (_, hi), (_, addc)] if lo >> 8 == 0x0E && *add == 0x2600 | f(fsr1l) && hi >> 8 == 0x0E && *addc == 0x2200 | f(r.fsr1h) => {
                Some((i32::from(((hi & 0xFF) << 8 | lo & 0xFF) as i16), 4))
            }
            _ => None,
        }
    }

    /// How routine `f` sets up its frame: the run of stack-pointer increments it starts with (XC8 writes the prologue first).
    /// `None` when the PIC18 stack registers are not known.
    pub fn shape(&self, f: usize, prog: &[u8]) -> Option<Shape> {
        if !self.word_addressed && (self.stack_regs.is_none() || self.fsr1l.is_none()) {
            return None;
        }
        let func = self.funcs.get(f)?;
        // The longest prologue: a PIC18 frame of 128 bytes (XC8's limit) as one MOVF POSTINC1,F per byte, and the 4-instruction
        // group the scan looks ahead to.
        let code = self.code(func, prog, 132);
        let (mut auto, mut i) = (0, 0);
        while let Some((d, n)) = self.sp_step(&code, i).filter(|s| s.0 > 0) {
            auto += d;
            i += n;
        }
        Some(Shape { auto })
    }

    /// The first instructions at which the cores note FSR1 (`Pic18::entry_watch`), sorted: `main`, which start-up code
    /// enters with GOTO, and every routine whose frame base is FSR1L. A routine on the software stack can be entered by a
    /// jump rather than a CALL: XC8 4.00 at -O3 and -Os turns CALL; RETURN into GOTO (PIC16: LJMP), after pushing the
    /// callee's arguments, so FSR1 at the jumping routine's call is not where the callee's frame starts.
    pub fn entry_points(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.funcs.iter().filter(|f| f.name == "main" || (f.frame_reg.is_some() && f.frame_reg == self.fsr1l)).map(|f| f.low).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// The live calls, innermost first: the routine the PC is in, then the routine of each pending CALL in `callers` (the
    /// addresses of the CALL instructions, innermost first), one entry per call, so a recursive routine appears once per level.
    ///
    /// `entries` holds, for each of those calls in the same order, FSR1 when the call was made (`Pic18::entry_sp`,
    /// `Pic16::entry_sp`), or when the routine's first instruction was reached (the bottom routine, and one entered by a
    /// jump; see `entry_points`). A call's frame base is that plus what the routine's
    /// prologue adds, at every instruction of the call: also inside the prologue and epilogue, and while a statement pushes
    /// the arguments of another call. `None` without an entry, when the routine is not known, or when its frame base is not
    /// FSR1L (`fsr1l`).
    pub fn frames(&self, pc: u32, callers: &[u32], entries: &[Option<u32>], prog: &[u8]) -> Vec<Frame> {
        // Each routine's shape once, however deep a recursion goes.
        let mut memo: std::collections::HashMap<usize, Option<Shape>> = std::collections::HashMap::new();
        std::iter::once(pc)
            .chain(callers.iter().copied())
            .enumerate()
            .map(|(k, at)| {
                let func = self.func_at(at);
                let base = func.zip(entries.get(k).copied().flatten()).and_then(|(i, e)| {
                    if self.funcs[i].frame_reg.is_none() || self.funcs[i].frame_reg != self.fsr1l {
                        return None;
                    }
                    let s = (*memo.entry(i).or_insert_with(|| self.shape(i, prog)))?;
                    u32::try_from(i64::from(e) + i64::from(s.auto)).ok()
                });
                Frame { func, base }
            })
            .collect()
    }

    /// Data memory as variables are read from it. A PIC18: `mem` itself. A PIC16: `mem` (banked addresses) followed, from
    /// 2000h, by the linear view of the general-purpose RAM that XC8's software stack (FSR1) and pointers to it use: the
    /// 80-byte blocks of the banks laid end to end, up to the part's `linear_end` (2FF0h, banks 0-50: DS40002343F 9.6.2 p.57,
    /// Figure 9-20; 29B0h on the PIC16F183xx: DS40001839E 4.5.2 p.62).
    pub fn data_view<'a>(&self, mem: &'a [u8]) -> std::borrow::Cow<'a, [u8]> {
        if !self.word_addressed {
            return mem.into();
        }
        // FSR values below 2000h are banked addresses (DS40002343F 9.6.1 p.56), so the linear view starts there. A data memory
        // larger than that banked space would collide with it: then no linear view.
        if mem.len() > 0x2000 {
            return mem.into();
        }
        let mut v = mem.to_vec();
        v.resize(0x2000, 0);
        let end = self.linear_end.unwrap_or(0x2FF0).saturating_sub(0x2000).min((mem.len() / 128) as u32 * 80);
        v.extend((0..end).map(|n| mem.get(crate::pic16::linear_to_data(n)).copied().unwrap_or(0)));
        v.into()
    }
}

/// A type as the debug info describes it.
#[derive(Clone, Debug)]
pub struct Ty {
    pub bytes: u32,
    /// Element size of an array; the whole size otherwise.
    pub elem: u32,
    pub kind: Kind,
    pub array: bool,
    pub name: String,
    pub form: Form,
}

#[derive(Clone, Debug)]
pub enum Form {
    /// Integer, character, boolean or float: `kind` says which.
    Scalar,
    Array { of: Box<Ty>, count: u32 },
    /// A structure, or a union (every member at offset 0).
    Struct { union: bool, members: Vec<Member>, id: u32 },
    Enum(Vec<(i64, String)>),
    /// `to` is `None` for `void *`, a function pointer, or a type that points back into itself (`struct node *next`).
    Pointer { to: Option<Box<Ty>>, space: PtrSpace },
    /// A pointer to a function: the value is a program address, shown with the routine it enters.
    FuncPtr,
    /// A pointer to a structure that was still being described (`struct node *next` inside `struct node`): `id` is the key
    /// into `DebugInfo::structs`, where the structure is found once the whole type has been read.
    BackPtr { id: u32 },
}

/// Where a pointer's value points, from the `DW_AT_address_class` XC8 writes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PtrSpace {
    /// Data memory.
    Data,
    /// A one-byte offset into the `smallconst` section of program memory (PIC18 `const` data, class 31).
    Near,
    /// Two bytes whose bit 15 says program memory (the word address below it) when set and data memory when clear (PIC16 `const` pointers, class 25).
    Either,
}

#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    pub offset: u32,
    pub ty: Ty,
    /// `(bit offset from the member's byte, width)` of a bit-field.
    pub bits: Option<(u32, u32)>,
}

/// Records every structure and union inside `ty` (and what its pointers point to) under its id.
fn register(ty: &Ty, into: &mut std::collections::HashMap<u32, Ty>) {
    match &ty.form {
        Form::Struct { members, id, .. } => {
            if into.insert(*id, ty.clone()).is_none() {
                for m in members {
                    register(&m.ty, into);
                }
            }
        }
        Form::Array { of, .. } => register(of, into),
        Form::Pointer { to: Some(t), .. } => register(t, into),
        _ => {}
    }
}

/// An id for the type entry at `at` that is unique across the program's units.
fn uid(unit: &gimli::Unit<R<'_>>, at: gimli::UnitOffset) -> u32 {
    at.to_debug_info_offset(&unit.header).map_or(at.0 as u32, |o| o.0 as u32)
}

fn attr_name(dwarf: &gimli::Dwarf<R<'_>>, unit: &gimli::Unit<R<'_>>, e: &gimli::DebuggingInformationEntry<R<'_>>) -> Option<String> {
    let v = e.attr_value(gimli::DW_AT_name)?;
    Some(dwarf.attr_string(unit, v).ok()?.to_string_lossy().into_owned())
}

fn offset_of(e: &gimli::DebuggingInformationEntry<R<'_>>) -> u32 {
    match e.attr_value(gimli::DW_AT_data_member_location) {
        Some(AttributeValue::Exprloc(x)) => {
            // DW_OP_plus_uconst <uleb128>; a union member has no location at all.
            let b = x.0.slice();
            if b.first() == Some(&0x23) {
                let (mut v, mut sh) = (0u32, 0);
                for c in &b[1..] {
                    v |= u32::from(c & 0x7f) << sh;
                    sh += 7;
                    if c & 0x80 == 0 {
                        break;
                    }
                }
                v
            } else {
                0
            }
        }
        Some(v) => v.udata_value().unwrap_or(0) as u32,
        None => 0,
    }
}

fn describe(dwarf: &gimli::Dwarf<R<'_>>, unit: &gimli::Unit<R<'_>>, at: gimli::UnitOffset, open: &mut Vec<gimli::UnitOffset>) -> Option<Ty> {
    let e = unit.entry(at).ok()?;
    match e.tag() {
        // `volatile`, `const` and typedef names (`uint8_t`) are the type they wrap.
        gimli::DW_TAG_volatile_type | gimli::DW_TAG_const_type | gimli::DW_TAG_typedef => match e.attr_value(gimli::DW_AT_type) {
            Some(AttributeValue::UnitRef(t)) => {
                let mut ty = describe(dwarf, unit, t, open)?;
                if e.tag() == gimli::DW_TAG_typedef && !ty.array {
                    ty.name = attr_name(dwarf, unit, &e).unwrap_or(ty.name);
                }
                Some(ty)
            }
            _ => None,
        },
        gimli::DW_TAG_base_type => {
            let size = e.attr_value(gimli::DW_AT_byte_size)?.udata_value()? as u32;
            let enc = match e.attr_value(gimli::DW_AT_encoding)? {
                AttributeValue::Encoding(a) => u64::from(a.0),
                v => v.udata_value()?,
            };
            let kind = match enc {
                2 => Kind::Bool,
                4 => Kind::Float,
                5 => Kind::Signed,
                6 | 8 => Kind::Char, // signed_char (C `char`), unsigned_char
                _ => Kind::Unsigned,
            };
            let name = attr_name(dwarf, unit, &e).unwrap_or_default();
            // C `unsigned char` (`uint8_t`) is a small number or a byte array far more often than text; plain `char` stays text.
            let kind = if name == "unsigned char" { Kind::Unsigned } else { kind };
            Some(Ty { bytes: size, elem: size, kind, array: false, name, form: Form::Scalar })
        }
        gimli::DW_TAG_array_type => {
            let Some(AttributeValue::UnitRef(t)) = e.attr_value(gimli::DW_AT_type) else { return None };
            let el = describe(dwarf, unit, t, open)?;
            let mut tree = unit.entries_at_offset(at).ok()?;
            tree.next_dfs().ok()?; // the array itself
            // The first child is the subrange holding the last index.
            let count = match tree.next_dfs().ok()? {
                Some(c) if c.tag() == gimli::DW_TAG_subrange_type => c.attr_value(gimli::DW_AT_upper_bound)?.udata_value()? as u32 + 1,
                _ => return None,
            };
            let name = format!("{}[{count}]", el.name);
            Some(Ty { bytes: el.bytes.saturating_mul(count), elem: el.bytes, kind: el.kind.clone(), array: true, name, form: Form::Array { of: Box::new(el), count } })
        }
        gimli::DW_TAG_structure_type | gimli::DW_TAG_union_type => {
            let union = e.tag() == gimli::DW_TAG_union_type;
            let bytes = e.attr_value(gimli::DW_AT_byte_size)?.udata_value()? as u32;
            let name = format!("{} {}", if union { "union" } else { "struct" }, attr_name(dwarf, unit, &e).unwrap_or_default()).trim_end().to_string();
            open.push(at);
            let mut members = Vec::new();
            let mut tree = unit.entries_at_offset(at).ok()?;
            tree.next_dfs().ok()?;
            while let Ok(Some(c)) = tree.next_dfs() {
                if c.depth() <= 0 {
                    break; // left the structure's children
                }
                if c.tag() != gimli::DW_TAG_member || c.depth() != 1 {
                    continue;
                }
                let Some(AttributeValue::UnitRef(t)) = c.attr_value(gimli::DW_AT_type) else { continue };
                let Some(ty) = describe(dwarf, unit, t, open) else { continue };
                let bits = c.attr_value(gimli::DW_AT_bit_size).and_then(|b| b.udata_value()).map(|w| {
                    // DWARF 4: data_bit_offset from the start of the structure; older: bit_offset from the MSB of a storage unit.
                    let off = c.attr_value(gimli::DW_AT_data_bit_offset).and_then(|v| v.udata_value());
                    let old = c.attr_value(gimli::DW_AT_bit_offset).and_then(|v| v.udata_value());
                    let lsb = match (off, old) {
                        (Some(o), _) => (o as u32).saturating_sub(8 * offset_of(c)),
                        (None, Some(o)) => (8 * ty.bytes).saturating_sub(o as u32 + w as u32),
                        _ => 0,
                    };
                    (lsb, w as u32)
                });
                members.push(Member { name: attr_name(dwarf, unit, c).unwrap_or_default(), offset: offset_of(c), ty, bits });
            }
            open.pop();
            Some(Ty { bytes, elem: bytes, kind: Kind::Unsigned, array: false, name, form: Form::Struct { union, members, id: uid(unit, at) } })
        }
        gimli::DW_TAG_enumeration_type => {
            let bytes = e.attr_value(gimli::DW_AT_byte_size)?.udata_value()? as u32;
            let name = format!("enum {}", attr_name(dwarf, unit, &e).unwrap_or_default()).trim_end().to_string();
            let mut names = Vec::new();
            let mut tree = unit.entries_at_offset(at).ok()?;
            tree.next_dfs().ok()?;
            while let Ok(Some(c)) = tree.next_dfs() {
                if c.depth() <= 0 {
                    break;
                }
                if c.tag() == gimli::DW_TAG_enumerator {
                    let v = match c.attr_value(gimli::DW_AT_const_value) {
                        Some(AttributeValue::Sdata(i)) => i,
                        Some(v) => v.udata_value().unwrap_or(0) as i64,
                        None => continue,
                    };
                    names.push((v, attr_name(dwarf, unit, c).unwrap_or_default()));
                }
            }
            Some(Ty { bytes, elem: bytes, kind: Kind::Unsigned, array: false, name, form: Form::Enum(names) })
        }
        gimli::DW_TAG_pointer_type => {
            let bytes = e.attr_value(gimli::DW_AT_byte_size).and_then(|v| v.udata_value()).unwrap_or(2) as u32;
            let to_function = matches!(e.attr_value(gimli::DW_AT_type), Some(AttributeValue::UnitRef(t)) if unit.entry(t).is_ok_and(|x| x.tag() == gimli::DW_TAG_subroutine_type));
            if to_function {
                return Some(Ty { bytes, elem: bytes, kind: Kind::Unsigned, array: false, name: "function *".into(), form: Form::FuncPtr });
            }
            let target = match e.attr_value(gimli::DW_AT_type) {
                Some(AttributeValue::UnitRef(t)) if !open.contains(&t) => describe(dwarf, unit, t, open),
                Some(AttributeValue::UnitRef(t)) => {
                    // A pointer back to a structure being described: name only, so a list node does not recurse forever.
                    let n = unit.entry(t).ok().and_then(|x| attr_name(dwarf, unit, &x)).unwrap_or_default();
                    return Some(Ty { bytes, elem: bytes, kind: Kind::Unsigned, array: false, name: format!("struct {n} *"), form: Form::BackPtr { id: uid(unit, t) } });
                }
                _ => None,
            };
            let space = match e.attr_value(gimli::DW_AT_address_class) {
                Some(AttributeValue::AddressClass(gimli::DwAddr(31))) if bytes == 1 => PtrSpace::Near,
                Some(AttributeValue::AddressClass(gimli::DwAddr(25))) if bytes == 2 => PtrSpace::Either,
                _ => PtrSpace::Data,
            };
            let name = format!("{} *", target.as_ref().map_or("void", |t| t.name.as_str()));
            Some(Ty { bytes, elem: bytes, kind: Kind::Unsigned, array: false, name, form: Form::Pointer { to: target.map(Box::new), space } })
        }
        _ => None,
    }
}

/// What displaying a value may look up: the routines (function pointers) and the structure types (back pointers).
struct Ctx<'a> {
    /// Data memory: where a pointer into data points, whichever memory the pointer itself is in.
    data: &'a [u8],
    funcs: &'a [Func],
    structs: Option<&'a std::collections::HashMap<u32, Ty>>,
    /// Program memory as one byte per address (`DebugInfo::rom_image`), for pointers into it.
    rom: &'a [u8],
    near_base: Option<u32>,
}

impl<'a> Ctx<'a> {
    fn bare(data: &'a [u8]) -> Self {
        Ctx { data, funcs: &[], structs: None, rom: &[], near_base: None }
    }
}

impl Var {
    /// The value in `mem` (data memory) as text. Multi-byte values are little-endian.
    pub fn show(&self, mem: &[u8]) -> String {
        match self.place(None) {
            Some(a) => self.layout.show_in(mem, a, 1, &Ctx::bare(mem)),
            None => "(no frame here)".into(),
        }
    }

    /// As `show`, with the program's routines and structure types: a function pointer names the routine it enters and a
    /// pointer to its own structure type (`struct node *next`) shows what it points at.
    ///
    /// `rom` is the program memory as `DebugInfo::rom_image` gives it: a pointer into program memory (`const char *` to a
    /// string literal) shows what it points at, and a constant held in program memory (a `const` table) is read from there.
    pub fn show_with(&self, mem: &[u8], rom: &[u8], info: &DebugInfo) -> String {
        self.show_framed(mem, rom, info, None)
    }

    /// Where the variable is in the call whose frame base is `base` (`DebugInfo::frames`): its fixed address, or for a
    /// variable on the software stack the base plus its offset; `None` when that base is not known.
    pub fn place(&self, base: Option<u32>) -> Option<u32> {
        match self.frame {
            None => Some(self.addr),
            Some(off) => base.and_then(|b| u32::try_from(i64::from(b) + i64::from(off)).ok()),
        }
    }

    /// As `show_with`, for the call whose frame base is `base`; `mem` should be `DebugInfo::data_view` (on a PIC16 a frame is
    /// at a linear address).
    pub fn show_framed(&self, mem: &[u8], rom: &[u8], info: &DebugInfo, base: Option<u32>) -> String {
        let Some(a) = self.place(base) else { return "(no frame here)".into() };
        let ctx = Ctx { data: mem, funcs: &info.funcs, structs: Some(&info.structs), rom, near_base: info.near_base };
        self.layout.show_in(if self.rom { rom } else { mem }, a, 1, &ctx)
    }
}

impl Ty {
    /// Follow `.member` and `[index]` steps; returns the byte offset from the start of `self` and the type reached.
    pub fn resolve(&self, mut path: &str) -> Option<(u32, &Ty)> {
        let (mut off, mut ty) = (0, self);
        path = path.trim();
        while !path.is_empty() {
            if let Some(rest) = path.strip_prefix('.') {
                let end = rest.find(['.', '[']).unwrap_or(rest.len());
                let Form::Struct { members, .. } = &ty.form else { return None };
                let m = members.iter().find(|m| m.name == rest[..end].trim() && m.bits.is_none())?;
                off += m.offset;
                ty = &m.ty;
                path = &rest[end..];
            } else if let Some(rest) = path.strip_prefix('[') {
                let (idx, rest) = rest.split_once(']')?;
                let Form::Array { of, count } = &ty.form else { return None };
                let i: u32 = idx.trim().parse().ok().filter(|i| i < count)?;
                off += i * of.bytes;
                ty = of;
                path = rest;
            } else {
                return None;
            }
        }
        Some((off, ty))
    }

    /// The value of this type at data address `addr`. `deref` is how many levels of pointer to follow.
    pub fn show_at(&self, mem: &[u8], addr: u32, deref: u32) -> String {
        self.show_in(mem, addr, deref, &Ctx::bare(mem))
    }

    fn show_in(&self, mem: &[u8], addr: u32, deref: u32, ctx: &Ctx<'_>) -> String {
        let Some(raw) = mem.get(addr as usize..(addr as usize).saturating_add(self.bytes as usize)) else { return "(outside RAM)".into() };
        match &self.form {
            Form::Scalar => scalar(&self.kind, raw),
            Form::Array { of, count } => {
                if of.kind == Kind::Char && matches!(of.form, Form::Scalar) {
                    // A string: up to the first NUL.
                    let s: String = raw.iter().take_while(|b| **b != 0).map(|b| if (32..127).contains(b) { *b as char } else { '.' }).collect();
                    return format!("\"{s}\"");
                }
                let items: Vec<String> = (0..(*count).min(16)).map(|i| of.show_in(mem, addr + i * of.bytes, deref, ctx)).collect();
                format!("{{{}{}}}", items.join(", "), if *count > 16 { ", ..." } else { "" })
            }
            Form::Struct { members, .. } => {
                let items: Vec<String> = members.iter().map(|m| format!("{} = {}", m.name, m.show(mem, addr, deref, ctx))).collect();
                format!("{{{}}}", items.join(", "))
            }
            Form::Enum(names) => {
                let u: u64 = raw.iter().rev().fold(0, |x, y| x << 8 | u64::from(*y));
                match names.iter().find(|(v, _)| *v as u64 == u) {
                    Some((_, n)) => format!("{n} ({u})"),
                    None => format!("{u}"),
                }
            }
            Form::FuncPtr => {
                let u: u64 = raw.iter().rev().fold(0, |x, y| x << 8 | u64::from(*y));
                if u == 0 {
                    return "NULL".into();
                }
                let at = format!("0x{u:0w$X}", w = raw.len() * 2);
                match ctx.funcs.iter().find(|f| f.high > f.low && (f.low..f.high).contains(&(u as u32))) {
                    Some(f) if f.low == u as u32 => format!("{at} -> {}", f.name),
                    Some(f) => format!("{at} -> {}+{}", f.name, u as u32 - f.low),
                    None => at,
                }
            }
            Form::BackPtr { id } => {
                let u: u64 = raw.iter().rev().fold(0, |x, y| x << 8 | u64::from(*y));
                if u == 0 {
                    return "NULL".into();
                }
                let at = format!("0x{u:0w$X}", w = raw.len() * 2);
                match ctx.structs.and_then(|m| m.get(id)) {
                    Some(t) if deref > 0 && u as usize + t.bytes as usize <= ctx.data.len() => format!("{at} -> {}", t.show_in(ctx.data, u as u32, deref - 1, ctx)),
                    _ => at,
                }
            }
            Form::Pointer { to, space } => {
                let u: u64 = raw.iter().rev().fold(0, |x, y| x << 8 | u64::from(*y));
                if u == 0 {
                    return "NULL".into();
                }
                let at = format!("0x{u:0w$X}", w = raw.len() * 2);
                // The memory the pointer points into and the address in it.
                let (buf, a) = match space {
                    PtrSpace::Data => (Some(ctx.data), Some(u)),
                    PtrSpace::Near => (Some(ctx.rom), ctx.near_base.map(|b| u64::from(b) + u)),
                    PtrSpace::Either if u & 0x8000 != 0 => (Some(ctx.rom), Some(u & 0x7FFF)),
                    PtrSpace::Either => (Some(ctx.data), Some(u)),
                };
                match (to, buf, a) {
                    (Some(t), Some(b), Some(a)) if deref > 0 && a as usize + t.bytes as usize <= b.len() => format!("{at} -> {}", t.show_in(b, a as u32, deref - 1, ctx)),
                    _ => at,
                }
            }
        }
    }
}

impl Member {
    fn show(&self, mem: &[u8], base: u32, deref: u32, ctx: &Ctx<'_>) -> String {
        let at = base + self.offset;
        match self.bits {
            Some((lsb, width)) if (1..64).contains(&width) && lsb < 64 => {
                let Some(raw) = mem.get(at as usize..(at as usize).saturating_add(self.ty.bytes as usize)) else { return "(outside RAM)".into() };
                let u: u64 = raw.iter().rev().fold(0, |x, y| x << 8 | u64::from(*y));
                let v = (u >> lsb) & ((1u64 << width) - 1);
                if self.ty.kind == Kind::Signed && v >> (width - 1) & 1 == 1 { format!("{}", v as i64 - (1i64 << width)) } else { format!("{v}") }
            }
            _ => self.ty.show_in(mem, at, deref, ctx),
        }
    }
}

fn scalar(kind: &Kind, b: &[u8]) -> String {
    let u: u64 = b.iter().rev().fold(0, |x, y| x << 8 | u64::from(*y));
    let bits = 8 * b.len() as u32;
    match kind {
        Kind::Bool => (u != 0).to_string(),
        Kind::Char => format!("'{}' ({u})", if (32..127).contains(&(u as u8)) { u as u8 as char } else { '.' }),
        Kind::Float if b.len() == 4 => format!("{} (0x{u:08X})", f32::from_bits(u as u32)),
        Kind::Signed if bits == 64 => format!("{} (0x{u:016X})", u as i64),
        Kind::Signed if bits < 64 => format!("{} (0x{u:0w$X})", ((u << (64 - bits)) as i64) >> (64 - bits), w = b.len() * 2),
        _ => format!("{u} (0x{u:0w$X})", w = b.len() * 2),
    }
}

/// A symbol as front ends take it: `name`, or `name@file.c` (also `path@file.c`, `pts[1].y@main.c`) for the one of that file.
pub fn split_file(sym: &str) -> (&str, Option<&str>) {
    match sym.rsplit_once('@') {
        Some((n, f)) if !n.trim().is_empty() && !f.trim().is_empty() => (n.trim(), Some(f.trim())),
        _ => (sym.trim(), None),
    }
}

/// The address of a `DW_OP_addr <address>` expression (4 address bytes on a PIC18, 2 may do on a PIC16).
fn fixed_addr(ex: &[u8]) -> Option<u32> {
    (matches!(ex.len(), 3 | 5) && ex[0] == 0x03).then(|| ex[1..].iter().rev().fold(0u32, |a, b| a << 8 | u32::from(*b)))
}

/// The ELF that goes with a program given as a `.hex` or `.elf`: the same stem beside it, or one folder up (basil writes
/// the hex into `dist/`).
pub fn elf_for(program: &std::path::Path) -> Option<std::path::PathBuf> {
    let stem = program.file_stem()?;
    [Some(program.with_extension("elf")), program.parent().and_then(|d| d.parent()).map(|d| d.join(stem).with_extension("elf"))].into_iter().flatten().find(|e| e.is_file())
}

/// The part an ELF was built for (`18F27Q10`, `16F17146`): from the `.note.microchip` section the linker writes (vendor "Microchip", then
/// the part), else from the `pic18f27q10.inc` / `pic16f17146.inc` file name the assembler records.
pub fn device_of(elf: &[u8]) -> Option<String> {
    let vendor = b"Microchip\0";
    let mut from = 0;
    while let Some(i) = elf[from..].windows(vendor.len()).position(|w| w == vendor) {
        let rest = &elf[from + i + vendor.len()..];
        let skip = rest.iter().take_while(|b| **b == 0).count();
        let name: Vec<u8> = rest[skip..].iter().copied().take_while(u8::is_ascii_alphanumeric).collect();
        if name.len() >= 6 && name.len() < 16 && name.starts_with(b"1") && name.iter().any(u8::is_ascii_uppercase) {
            return String::from_utf8(name).ok();
        }
        from += i + vendor.len();
    }
    for key in [b"pic18f", b"pic16f"] {
        let mut from = 0;
        while let Some(i) = elf[from..].windows(key.len()).position(|w| w == key) {
            let start = from + i + 3; // after "pic"
            let name: Vec<u8> = elf[start..].iter().copied().take_while(u8::is_ascii_alphanumeric).collect();
            if elf[start + name.len()..].starts_with(b".inc") && name.len() < 16 {
                return String::from_utf8(name).ok().map(|n| n.to_ascii_uppercase());
            }
            from += i + key.len();
        }
    }
    None
}
