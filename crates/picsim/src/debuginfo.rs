//! Routines and typed variables from the debug unit `basil -g` appends to the ELF (see `src/dwarf.rs` in the compiler),
//! together with the line table of `lines.rs`. The simulator front ends use it to show variables by name and type,
//! the routine the PC is in, and a call stack with source lines.

use crate::lines::LineMap;
use gimli::{AttributeValue, EndianSlice, LittleEndian, Reader};
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
}

/// A routine's byte-address range `[low, high)`.
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
        let mut units = dwarf.units();
        while let Some(h) = units.next().map_err(|e| e.to_string())? {
            let unit = dwarf.unit(h).map_err(|e| e.to_string())?;
            let mut entries = unit.entries();
            // Only the compiler's own unit: the assembler writes variable entries for its symbols too.
            let mut ours = false;
            let mut cur_func: Option<usize> = None;
            let mut func_depth = 0isize;
            while let Some(e) = entries.next_dfs().map_err(|e| e.to_string())? {
                let depth = e.depth();
                if depth <= func_depth && cur_func.is_some() {
                    cur_func = None;
                }
                match e.tag() {
                    gimli::DW_TAG_compile_unit => {
                        ours = match e.attr_value(gimli::DW_AT_producer) {
                            Some(v) => dwarf.attr_string(&unit, v).map(|s| s.to_slice().map(|b| b == b"basil".as_slice()).unwrap_or(false)).unwrap_or(false),
                            None => false,
                        };
                    }
                    gimli::DW_TAG_subprogram if ours => {
                        let name = attr_name(&dwarf, &unit, e).unwrap_or_default();
                        let (Some(AttributeValue::Addr(low)), Some(AttributeValue::Addr(high))) = (e.attr_value(gimli::DW_AT_low_pc), e.attr_value(gimli::DW_AT_high_pc)) else {
                            continue;
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
                        if ex.len() != 5 || ex[0] != 0x03 {
                            continue; // DW_OP_addr <u32> is all the compiler writes
                        }
                        let addr = u32::from_le_bytes([ex[1], ex[2], ex[3], ex[4]]);
                        let Some(AttributeValue::UnitRef(t)) = e.attr_value(gimli::DW_AT_type) else { continue };
                        let Some(ty) = describe(&dwarf, &unit, t) else { continue };
                        info.vars.push(Var { name, addr, bytes: ty.bytes, kind: ty.kind, elem: ty.elem, array: ty.array, ty: ty.name, scope: if e.tag() == gimli::DW_TAG_formal_parameter || cur_func.is_some() { cur_func } else { None }, param: e.tag() == gimli::DW_TAG_formal_parameter });
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

    /// Globals, then the parameters and locals of routine `f` when given.
    pub fn visible(&self, f: Option<usize>) -> impl Iterator<Item = &Var> {
        self.vars.iter().filter(move |v| v.scope.is_none() || v.scope == f)
    }
}

struct Ty {
    bytes: u32,
    elem: u32,
    kind: Kind,
    array: bool,
    name: String,
}

fn attr_name(dwarf: &gimli::Dwarf<R<'_>>, unit: &gimli::Unit<R<'_>>, e: &gimli::DebuggingInformationEntry<R<'_>>) -> Option<String> {
    let v = e.attr_value(gimli::DW_AT_name)?;
    Some(dwarf.attr_string(unit, v).ok()?.to_string_lossy().into_owned())
}

fn describe(dwarf: &gimli::Dwarf<R<'_>>, unit: &gimli::Unit<R<'_>>, at: gimli::UnitOffset) -> Option<Ty> {
    let e = unit.entry(at).ok()?;
    match e.tag() {
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
                8 => Kind::Char,
                _ => Kind::Unsigned,
            };
            Some(Ty { bytes: size, elem: size, kind, array: false, name: attr_name(dwarf, unit, &e).unwrap_or_default() })
        }
        gimli::DW_TAG_array_type => {
            let Some(AttributeValue::UnitRef(t)) = e.attr_value(gimli::DW_AT_type) else { return None };
            let el = describe(dwarf, unit, t)?;
            let mut tree = unit.entries_at_offset(at).ok()?;
            tree.next_dfs().ok()?; // the array itself
            // The first child is the subrange holding the last index.
            let count = match tree.next_dfs().ok()? {
                Some(c) if c.tag() == gimli::DW_TAG_subrange_type => c.attr_value(gimli::DW_AT_upper_bound)?.udata_value()? as u32 + 1,
                _ => return None,
            };
            Some(Ty { bytes: el.bytes * count, elem: el.bytes, kind: el.kind, array: true, name: format!("{}[{count}]", el.name) })
        }
        _ => None,
    }
}

impl Var {
    /// The value in `mem` (data memory) as text. Multi-byte values are little-endian.
    pub fn show(&self, mem: &[u8]) -> String {
        let at = |a: u32, n: u32| -> Option<Vec<u8>> { mem.get(a as usize..(a + n) as usize).map(<[u8]>::to_vec) };
        let Some(raw) = at(self.addr, self.bytes) else { return "(outside RAM)".into() };
        if self.array {
            if self.kind == Kind::Char {
                // A string: up to the first NUL.
                let s: String = raw.iter().take_while(|b| **b != 0).map(|b| if (32..127).contains(b) { *b as char } else { '.' }).collect();
                return format!("\"{s}\"");
            }
            let n = (self.bytes / self.elem.max(1)) as usize;
            let items: Vec<String> = raw.chunks(self.elem.max(1) as usize).take(16).map(|c| scalar(&self.kind, c)).collect();
            return format!("{{{}{}}}", items.join(", "), if n > 16 { ", ..." } else { "" });
        }
        scalar(&self.kind, &raw)
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

/// The part an ELF was built for (`18F27Q10`): from the `.note.microchip` section the linker writes (vendor "Microchip", then the
/// part), else from the `pic18f27q10.inc` file name the assembler records.
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
    let key = b"pic18f";
    let mut from = 0;
    while let Some(i) = elf[from..].windows(key.len()).position(|w| w == key) {
        let start = from + i + 3; // after "pic"
        let name: Vec<u8> = elf[start..].iter().copied().take_while(u8::is_ascii_alphanumeric).collect();
        if elf[start + name.len()..].starts_with(b".inc") && name.len() < 16 {
            return String::from_utf8(name).ok().map(|n| n.to_ascii_uppercase());
        }
        from += i + key.len();
    }
    None
}
