//! Source line table read from the `.debug_line` section of an ELF built with `basil -g`:
//! program-memory byte address -> (source file, line). The simulator uses it to show the BASIC line that is executing.

use gimli::{AttributeValue, EndianSlice, LittleEndian};
use object::{Object, ObjectSection};
use std::path::PathBuf;

#[derive(Debug, Default, Clone)]
pub struct LineMap {
    /// Source files (absolute when the compiler recorded them so).
    pub files: Vec<PathBuf>,
    /// (address, file index, line), sorted by address; each row holds until the next one.
    rows: Vec<(u32, u32, u32)>,
    /// End of the last sequence: addresses at or past it have no line.
    ends: Vec<u32>,
}

impl LineMap {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Parses the line programs of an ELF image. Errors are returned as text for the status line.
    pub fn from_elf(bytes: &[u8]) -> Result<Self, String> {
        let obj = object::File::parse(bytes).map_err(|e| e.to_string())?;
        let sec = obj.section_by_name(".debug_line").ok_or("no .debug_line (build with basil -g)")?;
        let data = sec.data().map_err(|e| e.to_string())?;
        let dl = gimli::DebugLine::new(data, LittleEndian);
        let mut map = LineMap::default();
        let mut off = 0usize;
        while off + 4 <= data.len() {
            // Each compilation unit has its own program: step by its initial length field.
            let len = u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as usize;
            let next = off + 4 + len;
            let prog = dl.program(gimli::DebugLineOffset(off), 4, None, None).map_err(|e| e.to_string())?;
            let mut rows = prog.rows();
            let mut local: Vec<PathBuf> = Vec::new();
            while let Some((hdr, row)) = rows.next_row().map_err(|e| e.to_string())? {
                if local.is_empty() {
                    local = file_list(hdr);
                }
                let path = local.get(row.file_index() as usize).cloned().unwrap_or_default();
                let fi = match map.files.iter().position(|f| *f == path) {
                    Some(i) => i,
                    None => {
                        map.files.push(path);
                        map.files.len() - 1
                    }
                } as u32;
                let line = row.line().map_or(0, |l| l.get() as u32);
                let addr = row.address() as u32;
                if row.end_sequence() {
                    map.ends.push(addr);
                } else if line != 0 {
                    map.rows.push((addr, fi, line));
                }
            }
            off = next;
        }
        map.rows.sort_by_key(|r| r.0);
        map.rows.dedup_by_key(|r| r.0); // keep one row per address
        Ok(map)
    }

    /// The BASIC source position of the instruction at `addr`; None for generated code.
    pub fn lookup(&self, addr: u32) -> Option<(&PathBuf, u32)> {
        let i = self.rows.partition_point(|r| r.0 <= addr).checked_sub(1)?;
        let (a, f, l) = self.rows[i];
        // A row ends at the next row's address; the instruction at `addr` belongs to it when no sequence ended between.
        if self.ends.iter().any(|e| *e > a && *e <= addr) {
            return None;
        }
        let file = &self.files[f as usize];
        // Generated assembly (main.S, .inc files) is not source the user wrote.
        file.extension().is_some_and(|e| e.eq_ignore_ascii_case("bas")).then_some((file, l))
    }

    /// The lowest address generated for `line` of `file`: where a breakpoint on that source line goes.
    pub fn first_addr(&self, file: &std::path::Path, line: u32) -> Option<u32> {
        let fi = self.files.iter().position(|f| f == file)? as u32;
        self.rows.iter().filter(|r| r.1 == fi && r.2 == line).map(|r| r.0).min()
    }

    /// Source files that contain code, in the order met; the user's `.bas` files come first.
    pub fn source_files(&self) -> Vec<&PathBuf> {
        let mut v: Vec<&PathBuf> = self.files.iter().filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("bas"))).collect();
        v.sort();
        v
    }
}

fn file_list(hdr: &gimli::LineProgramHeader<EndianSlice<'_, LittleEndian>>) -> Vec<PathBuf> {
    let text = |v: AttributeValue<EndianSlice<'_, LittleEndian>>| match v {
        AttributeValue::String(s) => Some(String::from_utf8_lossy(s.slice()).into_owned()),
        _ => None,
    };
    let mut out = Vec::new();
    // DWARF 5 numbers files from 0, earlier versions from 1; index 0 is then unused.
    if hdr.version() < 5 {
        out.push(PathBuf::new());
    }
    for f in hdr.file_names() {
        let name = text(f.path_name()).unwrap_or_default();
        let dir = f.directory(hdr).and_then(text).unwrap_or_default();
        let p = PathBuf::from(&name);
        out.push(if p.is_absolute() || dir.is_empty() { p } else { PathBuf::from(dir).join(p) });
    }
    out
}

/// The `.hex` to load for a program given as a `.hex` or a `.elf` (the hex written with it: beside the ELF, or in `dist/`).
pub fn program_hex(path: &std::path::Path) -> Result<PathBuf, String> {
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("elf")) {
        return Ok(path.to_path_buf());
    }
    let stem = path.file_stem().unwrap_or_default();
    [path.with_extension("hex"), path.parent().map(|d| d.join("dist").join(stem).with_extension("hex")).unwrap_or_default()]
        .into_iter()
        .find(|h| h.is_file())
        .ok_or_else(|| format!("no .hex beside {}", path.display()))
}
