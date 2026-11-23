//! Debug information from the MPLAB-style COFF file (`prog.cof`) that a PIC BASIC compiler writes when its
//! `Declare Create_Coff = On` is set, together with the BASIC source it names.
//!
//! The COFF file holds a line table (program address -> source line) and the global and routine-prefixed variables with their
//! data addresses and a C-like basic type. It does not say which routines exist, and a `Byte` and an `SByte` (or a `Word` and an
//! `SWord`) look the same in it. Both are recovered from the source: `Proc` .. `EndProc` ranges, parameters, `Dim` lines and their
//! declared types. Without the source the line table and the variables still load, with the types the COFF gives.
//!
//! Layout used (little endian): a 20-byte file header and 18 bytes of optional header, 40-byte section headers, 16-byte line
//! records `{ source symbol index u32, line u16, address u32, flags u16, function u32 }`, 20-byte symbols
//! `{ name[8], value u32, section i16, type u16, reserved u16, class u8, aux count u8 }` and a string table of NUL-terminated
//! names behind them. Program addresses are bytes on a PIC18 and words on a PIC16.

use crate::debuginfo::{DebugInfo, Form, Func, Kind, Ty, Var};
use crate::lines::LineMap;
use std::path::{Path, PathBuf};

const MAGIC: u16 = 0x1240;

fn u16at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn u32at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn cstr(b: &[u8], o: usize) -> String {
    let s = b.get(o..).unwrap_or(&[]);
    String::from_utf8_lossy(&s[..s.iter().position(|c| *c == 0).unwrap_or(s.len())]).into_owned()
}

/// A symbol of the COFF file that is a variable: its name, data address, basic type, and for an array its size in bytes and element count.
#[derive(Debug, Clone)]
struct CofVar {
    name: String,
    addr: u32,
    basic: u16,
    array: Option<(u32, u32)>,
}

#[derive(Debug, Default)]
struct Cof {
    /// Source file names by the index of their `.file` symbol.
    files: Vec<(u32, String)>,
    /// (address, symbol index of the file, line), in file order.
    rows: Vec<(u32, u32, u32)>,
    /// Ends of the sections that carry line records, in program addresses.
    ends: Vec<u32>,
    vars: Vec<CofVar>,
}

fn parse(bytes: &[u8], word_addressed: bool) -> Result<Cof, String> {
    let bad = || "truncated COFF file".to_string();
    if u16at(bytes, 0).ok_or_else(bad)? != MAGIC {
        return Err("not an MPLAB COFF file (magic 0x1240 expected)".into());
    }
    let nscns = u16at(bytes, 2).ok_or_else(bad)? as usize;
    let symptr = u32at(bytes, 8).ok_or_else(bad)? as usize;
    let nsyms = u32at(bytes, 12).ok_or_else(bad)? as usize;
    let opt = u16at(bytes, 16).ok_or_else(bad)? as usize;
    let mut cof = Cof::default();
    for i in 0..nscns {
        let h = 20 + opt + 40 * i;
        let (paddr, size, lnptr) = (u32at(bytes, h + 8).ok_or_else(bad)?, u32at(bytes, h + 16).ok_or_else(bad)?, u32at(bytes, h + 28).ok_or_else(bad)? as usize);
        let nln = u16at(bytes, h + 34).ok_or_else(bad)? as usize;
        for k in 0..nln {
            let r = lnptr.saturating_add(16 * k);
            let (file, line, addr) = (u32at(bytes, r).ok_or_else(bad)?, u16at(bytes, r + 4).ok_or_else(bad)?, u32at(bytes, r + 6).ok_or_else(bad)?);
            cof.rows.push((addr, file, u32::from(line)));
        }
        if nln > 0 {
            cof.ends.push(paddr.saturating_add(if word_addressed { size / 2 } else { size }));
        }
    }
    let strtab = bytes.get(symptr + 20 * nsyms..).ok_or_else(bad)?;
    let mut i = 0;
    while i < nsyms {
        let o = symptr + 20 * i;
        let rec = bytes.get(o..o + 20).ok_or_else(bad)?;
        let name = if rec[..4] == [0; 4] { cstr(strtab, u32at(rec, 4).unwrap_or(0) as usize) } else { cstr(&rec[..8], 0) };
        let (value, scn, ty, class, naux) = (u32at(rec, 8).unwrap_or(0), u16at(rec, 12).unwrap_or(0) as i16, u16at(rec, 14).unwrap_or(0), rec[18], rec[19] as usize);
        let aux = bytes.get(o + 20..o + 20 * (1 + naux)).unwrap_or(&[]);
        if name == ".file" {
            if let Some(off) = u32at(aux, 0) {
                cof.files.push((i as u32, cstr(strtab, off as usize)));
            }
        } else if scn == 1 && class == 2 && !name.starts_with('.') && !name.contains(" (bit") {
            // Basic type in the low 5 bits; bits 5-6 are the first derived type (3 = array).
            let array = (ty >> 5) & 3 == 3 && aux.len() >= 10;
            let array = if array { Some((u32::from(u16at(aux, 6).unwrap_or(0)), u32::from(u16at(aux, 8).unwrap_or(0)))) } else { None };
            cof.vars.push(CofVar { name, addr: value, basic: ty & 0x1f, array });
        }
        i += 1 + naux;
    }
    Ok(cof)
}

/// `Z:\tmp\prog\main.bas` as a path here: the drive letter goes and the backslashes become slashes.
fn unix_path(s: &str) -> PathBuf {
    let s = s.replace('\\', "/");
    let s = if s.as_bytes().get(1) == Some(&b':') { s[2..].to_string() } else { s };
    PathBuf::from(s)
}

/// The file in `dir` whose name is `name` in any case.
fn find_nocase(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)))
}

/// Where the source named in the COFF file is on this machine: the recorded path, else its file name beside the COFF file
/// (the project may have moved, or the compiler may have run under Wine with a drive of its own).
fn locate(recorded: &str, dir: &Path) -> PathBuf {
    let p = unix_path(recorded);
    if p.is_file() {
        return std::fs::canonicalize(&p).unwrap_or(p);
    }
    let base = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match find_nocase(dir, &base) {
        Some(f) => std::fs::canonicalize(&f).unwrap_or(f),
        None => p,
    }
}

// ---- the source ------------------------------------------------------------------------------------------------------------

/// A name as written in a `Dim` or a parameter list, with its type name in lower case: `byte`, `sword`, `string`, ...
#[derive(Debug, Clone)]
struct Decl {
    name: String,
    ty: String,
}

#[derive(Debug, Default, Clone)]
struct Proc {
    name: String,
    /// Line of `Proc` and of `EndProc` (1-based).
    line: u32,
    end: u32,
    params: Vec<Decl>,
    locals: Vec<Decl>,
    ret: Option<String>,
}

#[derive(Debug, Default)]
struct Source {
    globals: Vec<Decl>,
    procs: Vec<Proc>,
}

/// The line without its `'` comment (a `'` inside a string stays).
fn code(line: &str) -> &str {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '\'' if !quoted => return &line[..i],
            _ => {}
        }
    }
    line
}

/// `name`, `name[5]`, `name As Type`, `name As String * 10`: the name (without any index) and the lower-case type.
fn declaration(text: &str) -> Option<Decl> {
    let lower = text.to_ascii_lowercase();
    let at = lower.find(" as ")?;
    // Lower casing ASCII keeps every byte offset, so the name can be cut from the original text.
    let name = text[..at].trim().split(['[', ' ']).next()?.to_string();
    let ty = lower[at + 4..].trim().split(|c: char| c.is_whitespace() || c == '*' || c == '=').next()?.to_string();
    (!name.is_empty() && !ty.is_empty()).then_some(Decl { name, ty })
}

fn scan_source(text: &str) -> Source {
    let mut src = Source::default();
    let mut open: Option<Proc> = None;
    for (n, raw) in text.lines().enumerate() {
        let line = code(raw).trim();
        let lower = line.to_ascii_lowercase();
        let mut words = lower.split_whitespace();
        match words.next() {
            Some("proc") => {
                let head = line[4..].trim();
                let name = head.split(['(', ' ', ',']).next().unwrap_or("").to_string();
                let mut p = Proc { name, line: n as u32 + 1, ..Proc::default() };
                if let (Some(a), Some(b)) = (head.find('('), head.rfind(')')) {
                    for part in head[a + 1..b].split(',') {
                        p.params.extend(declaration(part));
                    }
                    p.ret = head[b + 1..].trim().trim_start_matches(',').split_whitespace().next().map(str::to_ascii_lowercase);
                }
                open = Some(p);
            }
            Some("endproc") => {
                if let Some(mut p) = open.take() {
                    p.end = n as u32 + 1;
                    src.procs.push(p);
                }
            }
            Some("dim" | "global" | "static") => {
                if let Some(at) = lower.find("dim ") {
                    if let Some(d) = declaration(&line[at + 4..]) {
                        match &mut open {
                            Some(p) => p.locals.push(d),
                            None => src.globals.push(d),
                        }
                    }
                }
            }
            _ => {}
        }
    }
    src
}

// ---- types -----------------------------------------------------------------------------------------------------------------

/// Size, kind and name of a scalar type written in the source.
fn source_type(ty: &str) -> Option<(u32, Kind, &'static str)> {
    Some(match ty {
        "byte" => (1, Kind::Unsigned, "Byte"),
        "sbyte" => (1, Kind::Signed, "SByte"),
        "word" => (2, Kind::Unsigned, "Word"),
        "sword" => (2, Kind::Signed, "SWord"),
        "dword" => (4, Kind::Unsigned, "Dword"),
        "sdword" | "long" => (4, Kind::Signed, if ty == "long" { "Long" } else { "SDword" }),
        "float" | "double" => (4, Kind::FloatMchp, "Float"),
        _ => return None,
    })
}

/// What the COFF file alone says: its basic types are C's, and the compiler writes `char` for a Byte, `int` for a Word, `long` for a Dword.
fn coff_type(basic: u16) -> (u32, Kind, &'static str) {
    match basic {
        3 | 4 | 13 | 14 => (2, Kind::Unsigned, "Word"),
        5 | 15 => (4, Kind::Unsigned, "Dword"),
        6 | 7 => (4, Kind::FloatMchp, "Float"),
        _ => (1, Kind::Unsigned, "Byte"),
    }
}

fn scalar(bytes: u32, kind: Kind, name: &str) -> Ty {
    Ty { bytes, elem: bytes, kind, array: false, name: name.into(), form: Form::Scalar }
}

fn make_var(v: &CofVar, name: String, declared: Option<&str>, scope: Option<usize>, param: bool) -> Var {
    let (elem, kind, tyname) = match declared.and_then(source_type) {
        Some(t) => t,
        None => coff_type(v.basic),
    };
    let text = declared == Some("string");
    let layout = match v.array {
        Some((size, count)) if count > 0 => {
            // A string is an array of characters; the COFF element is a `char` either way.
            let (elem, kind, tyname) = if text { (1, Kind::Char, "Char") } else { (elem.min(size / count.max(1)).max(1), kind, tyname) };
            let one = scalar(elem, kind.clone(), tyname);
            let name = if text { "String".to_string() } else { format!("{tyname}[{count}]") };
            Ty { bytes: size, elem, kind, array: true, name, form: Form::Array { of: Box::new(one), count: size / elem } }
        }
        _ => scalar(elem, kind, tyname),
    };
    Var { name, addr: v.addr, bytes: layout.bytes, kind: layout.kind.clone(), elem: layout.elem, array: layout.array, ty: layout.name.clone(), scope, param, rom: false, layout, frame: None, unit: 0, file_static: false }
}

impl DebugInfo {
    /// Builds the debug information from `prog.cof`. `word_addressed` is true for a PIC16 (program addresses count words). `dir` is
    /// the folder of the COFF file, where the source is looked for when the path it records does not exist here.
    pub fn from_cof(bytes: &[u8], word_addressed: bool, dir: &Path) -> Result<Self, String> {
        let cof = parse(bytes, word_addressed)?;
        let mut files: Vec<PathBuf> = Vec::new();
        let mut index = |sym: u32| -> Option<usize> {
            let name = &cof.files.iter().find(|f| f.0 == sym)?.1;
            let path = locate(name, dir);
            Some(files.iter().position(|f| *f == path).unwrap_or_else(|| {
                files.push(path);
                files.len() - 1
            }))
        };
        let mut rows: Vec<(u32, u32, u32)> = Vec::new();
        for &(addr, sym, line) in &cof.rows {
            if let Some(f) = index(sym) {
                rows.push((addr, f as u32, line));
            }
        }
        let mut info = DebugInfo { ignore_case: true, word_addressed, ..Default::default() };
        // The source of each file that has lines: routines, types.
        let sources: Vec<Source> = files.iter().map(|f| std::fs::read_to_string(f).map(|t| scan_source(&t)).unwrap_or_default()).collect();
        info.units = files.iter().map(|f| f.to_string_lossy().into_owned()).collect();
        let mut all: Vec<u32> = rows.iter().filter(|r| r.2 != 0).map(|r| r.0).collect();
        all.sort_unstable();
        all.dedup();
        let end_of = |after: u32| all.iter().copied().find(|a| *a > after).or_else(|| cof.ends.iter().copied().filter(|e| *e > after).min());
        // Routines: the addresses of the lines between `Proc` and `EndProc`.
        for (unit, src) in sources.iter().enumerate() {
            for p in &src.procs {
                let addrs: Vec<u32> = rows.iter().filter(|r| r.1 as usize == unit && (p.line..=p.end).contains(&r.2)).map(|r| r.0).collect();
                let (Some(&low), Some(&last)) = (addrs.iter().min(), addrs.iter().max()) else { continue };
                let high = end_of(last).unwrap_or(last + 1);
                info.funcs.push(Func { name: p.name.clone(), low, high, frame_reg: None, unit });
            }
        }
        // Variables: a name is a global of the source, or a routine's name followed by a parameter, local or `Result`.
        for v in &cof.vars {
            let lower = v.name.to_ascii_lowercase();
            if let Some(d) = sources.iter().flat_map(|s| s.globals.iter()).find(|d| d.name.eq_ignore_ascii_case(&lower)) {
                info.vars.push(make_var(v, d.name.clone(), Some(&d.ty), None, false));
                continue;
            }
            let mut placed = false;
            'procs: for (unit, src) in sources.iter().enumerate() {
                for p in &src.procs {
                    let Some(rest) = lower.strip_prefix(&p.name.to_ascii_lowercase()) else { continue };
                    let fi = info.funcs.iter().position(|f| f.name == p.name && f.unit == unit);
                    let hit = p.params.iter().map(|d| (d, true)).chain(p.locals.iter().map(|d| (d, false))).find(|(d, _)| d.name.eq_ignore_ascii_case(rest)).map(|(d, param)| (d.ty.clone(), param, d.name.clone()));
                    let hit = hit.or_else(|| (rest == "result").then(|| (p.ret.clone().unwrap_or_default(), false, "Result".to_string())));
                    if let (Some((ty, param, name)), Some(fi)) = (hit, fi) {
                        info.vars.push(make_var(v, name, Some(&ty), Some(fi), param));
                        placed = true;
                        break 'procs;
                    }
                }
            }
            if !placed {
                info.vars.push(make_var(v, v.name.clone(), None, None, false));
            }
        }
        let ends: Vec<u32> = cof.ends.clone();
        info.lines = LineMap::from_rows(files, rows, ends);
        Ok(info)
    }
}

/// The COFF file that goes with a program given as a `.hex`: the same stem beside it, or one folder up.
pub fn cof_for(program: &Path) -> Option<PathBuf> {
    let stem = program.file_stem()?;
    [Some(program.with_extension("cof")), program.parent().and_then(|d| d.parent()).map(|d| d.join(stem).with_extension("cof"))].into_iter().flatten().find(|e| e.is_file())
}

/// The part a program was built for, when the compiler that wrote its COFF file left it in the listing (`LIST P = 18F27Q10` in
/// `prog.asm` or `prog.lst`) or the build log (`prog.pbe`, first line `18F27Q10,82,4,64`).
pub fn device_of_listing(program: &Path) -> Option<String> {
    // Only for a program that has such a COFF file: another build's `.lst` is not this compiler's listing.
    cof_for(program)?;
    if let Ok(t) = std::fs::read_to_string(program.with_extension("pbe"))
        && let Some(first) = t.lines().next()
        && let Some(part) = first.split(',').next()
        && part.len() >= 6
        && part.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return Some(part.to_ascii_uppercase());
    }
    for ext in ["asm", "lst"] {
        let Ok(t) = std::fs::read_to_string(program.with_extension(ext)) else { continue };
        for l in t.lines().take(80) {
            let l = l.to_ascii_uppercase();
            if let Some(at) = l.find("LIST") && let Some(p) = l[at..].find("P =") {
                let part: String = l[at + p + 3..].trim_start().chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                if part.len() >= 6 {
                    return Some(part);
                }
            }
        }
    }
    None
}
