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

/// A variable at a fixed data-memory address.
#[derive(Clone, Debug)]
pub struct Var {
    pub name: String,
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
}

/// A routine's program-address range `[low, high)`: byte addresses on a PIC18 part, word addresses on a PIC16 (the units of the PC).
#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub low: u32,
    pub high: u32,
}

#[derive(Clone, Debug, Default)]
pub struct DebugInfo {
    pub funcs: Vec<Func>,
    pub vars: Vec<Var>,
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
                    }
                    gimli::DW_TAG_subprogram if ours => {
                        let name = attr_name(&dwarf, &unit, e).unwrap_or_default();
                        // high_pc is an address (basil, XC8 so far) or, in DWARF 4 style, an offset from low_pc.
                        let (low, high) = match (e.attr_value(gimli::DW_AT_low_pc), e.attr_value(gimli::DW_AT_high_pc)) {
                            (Some(AttributeValue::Addr(l)), Some(AttributeValue::Addr(h))) => (l, h),
                            (Some(AttributeValue::Addr(l)), Some(h)) if h.udata_value().is_some() => (l, l + h.udata_value().unwrap_or(0)),
                            _ => continue,
                        };
                        cur_func = Some(info.funcs.len());
                        func_depth = depth;
                        info.funcs.push(Func { name, low: low as u32, high: high as u32 });
                    }
                    gimli::DW_TAG_variable | gimli::DW_TAG_formal_parameter if ours => {
                        let name = attr_name(&dwarf, &unit, e).unwrap_or_default();
                        let ex: Vec<u8> = match e.attr_value(gimli::DW_AT_location) {
                            Some(AttributeValue::Exprloc(x)) => x.0.slice().to_vec(),
                            Some(AttributeValue::Block(b)) => b.slice().to_vec(),
                            _ => continue,
                        };
                        if !matches!(ex.len(), 3 | 5) || ex[0] != 0x03 {
                            continue; // DW_OP_addr <address> is all the compiler writes: 4 bytes on a PIC18, 2 may do on a PIC16
                        }
                        let addr = ex[1..].iter().rev().fold(0u32, |a, b| a << 8 | u32::from(*b));
                        let Some(AttributeValue::UnitRef(t)) = e.attr_value(gimli::DW_AT_type) else { continue };
                        let Some(ty) = describe(&dwarf, &unit, t, &mut Vec::new()) else { continue };
                        register(&ty, &mut info.structs);
                        let rom = xc8 && info.rom.iter().any(|r| (r.0..r.1).contains(&addr)) && !ram.iter().any(|r| (r.0..r.1).contains(&addr));
                        info.vars.push(Var { rom, name, addr, bytes: ty.bytes, kind: ty.kind.clone(), elem: ty.elem, array: ty.array, ty: ty.name.clone(), layout: ty.clone(), scope: if e.tag() == gimli::DW_TAG_formal_parameter || cur_func.is_some() { cur_func } else { None }, param: e.tag() == gimli::DW_TAG_formal_parameter });
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
    pub fn path_addr(&self, path: &str) -> Option<(u32, u32)> {
        let end = path.find(['.', '[']).unwrap_or(path.len());
        let v = self.vars.iter().find(|v| v.scope.is_none() && v.name == path[..end].trim() && !v.rom)?;
        let (off, ty) = v.layout.resolve(&path[end..])?;
        Some((v.addr + off, ty.bytes))
    }

    /// Globals, then the parameters and locals of routine `f` when given.
    pub fn visible(&self, f: Option<usize>) -> impl Iterator<Item = &Var> {
        self.vars.iter().filter(move |v| v.scope.is_none() || v.scope == f)
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
        self.layout.show_in(mem, self.addr, 1, &Ctx::bare(mem))
    }

    /// As `show`, with the program's routines and structure types: a function pointer names the routine it enters and a
    /// pointer to its own structure type (`struct node *next`) shows what it points at.
    ///
    /// `rom` is the program memory as `DebugInfo::rom_image` gives it: a pointer into program memory (`const char *` to a
    /// string literal) shows what it points at, and a constant held in program memory (a `const` table) is read from there.
    pub fn show_with(&self, mem: &[u8], rom: &[u8], info: &DebugInfo) -> String {
        let ctx = Ctx { data: mem, funcs: &info.funcs, structs: Some(&info.structs), rom, near_base: info.near_base };
        self.layout.show_in(if self.rom { rom } else { mem }, self.addr, 1, &ctx)
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
        Kind::Signed if bits < 64 => format!("{} (0x{u:0w$X})", ((u << (64 - bits)) as i64) >> (64 - bits), w = b.len() * 2),
        _ => format!("{u} (0x{u:0w$X})", w = b.len() * 2),
    }
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
