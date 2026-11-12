//! Intel HEX loader. Handles record types 00 (data), 01 (end), 04 (extended linear address) and 02
//! (extended segment address); the PIC18 toolchain writes byte addresses, so no word doubling.

use std::collections::BTreeMap;

/// The bytes a hex file defines, by absolute byte address.
#[derive(Debug, Default, Clone)]
pub struct Image {
    pub bytes: BTreeMap<u32, u8>,
}

#[derive(Debug)]
pub struct HexError(pub String);

impl std::fmt::Display for HexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for HexError {}

fn hexbyte(s: &str, at: usize, line: usize) -> Result<u8, HexError> {
    s.get(at..at + 2)
        .and_then(|h| u8::from_str_radix(h, 16).ok())
        .ok_or_else(|| HexError(format!("line {line}: bad hex digits")))
}

impl Image {
    pub fn parse(text: &str) -> Result<Image, HexError> {
        let mut img = Image::default();
        let mut base: u32 = 0;
        let mut ended = None::<usize>;
        for (n, raw) in text.lines().enumerate() {
            let line = n + 1;
            let l = raw.trim();
            if l.is_empty() {
                continue;
            }
            if let Some(e) = ended {
                return Err(HexError(format!("line {line}: a record after the end record on line {e}")));
            }
            let body = l.strip_prefix(':').ok_or_else(|| HexError(format!("line {line}: no ':'")))?;
            let len = hexbyte(body, 0, line)? as usize;
            if body.len() < (len + 5) * 2 {
                return Err(HexError(format!("line {line}: record too short")));
            }
            let addr = u32::from(hexbyte(body, 2, line)?) << 8 | u32::from(hexbyte(body, 4, line)?);
            let kind = hexbyte(body, 6, line)?;
            let mut data = Vec::with_capacity(len);
            let mut sum = (len as u8).wrapping_add((addr >> 8) as u8).wrapping_add(addr as u8).wrapping_add(kind);
            for i in 0..len {
                let b = hexbyte(body, 8 + 2 * i, line)?;
                sum = sum.wrapping_add(b);
                data.push(b);
            }
            let ck = hexbyte(body, 8 + 2 * len, line)?;
            if body.len() > (len + 5) * 2 {
                return Err(HexError(format!("line {line}: characters after the checksum")));
            }
            if sum.wrapping_add(ck) != 0 {
                return Err(HexError(format!("line {line}: checksum")));
            }
            match kind {
                0 => {
                    for (i, b) in data.iter().enumerate() {
                        img.bytes.insert(base.wrapping_add(addr).wrapping_add(i as u32), *b);
                    }
                }
                1 => ended = Some(line),
                2 | 4 if len != 2 => return Err(HexError(format!("line {line}: a type {kind:02X} record must hold 2 bytes, not {len}"))),
                2 => base = (u32::from(data[0]) << 8 | u32::from(data[1])) << 4,
                4 => base = (u32::from(data[0]) << 8 | u32::from(data[1])) << 16,
                3 | 5 => {} // start address: the part starts at its reset vector
                _ => return Err(HexError(format!("line {line}: unknown record type {kind:02X}"))),
            }
        }
        Ok(img)
    }
}

/// A note when bytes of `img` fall in no memory region of `dev` (program, data EEPROM, user ID, configuration, device
/// ID), which is how a hex file built for another part or family shows itself (REVIEW-2026-11-11 C2). PIC16 regions are
/// in words, two hex bytes each.
pub fn stray_note(dev: &devdesc::Device, img: &Image) -> Option<String> {
    let k = if dev.device.family == "pic18" { 1 } else { 2 };
    let m = &dev.memory;
    let regions = [(m.program.start, m.program.size), (m.eeprom.start, m.eeprom.size), (m.user_id.start, m.user_id.size), (m.config.start, m.config.size), (m.device_id.start, m.device_id.size)];
    let inside = |a: u32| regions.iter().any(|&(s, n)| n > 0 && (k * s..k * (s + n)).contains(&a));
    let stray: Vec<u32> = img.bytes.keys().copied().filter(|&a| !inside(a)).collect();
    let (first, last) = (stray.first()?, stray.last()?);
    Some(format!(
        "{} hex byte(s) at {first:06X}-{last:06X} are outside every memory region of the {}: is the hex file built for another part?",
        stray.len(),
        dev.device.name
    ))
}

/// The parts an ELF's debug information names through the processor include it was built with (`proc/pic18f27q10.inc`
/// from basil, `proc/pic18f27q10.h` from XC8), upper case without the `PIC` prefix (`18F27Q10`).
pub fn elf_parts(bytes: &[u8]) -> std::collections::BTreeSet<String> {
    let key = b"proc/pic";
    let mut out = std::collections::BTreeSet::new();
    let mut i = 0;
    while let Some(p) = bytes[i..].windows(key.len()).position(|w| w.eq_ignore_ascii_case(key)) {
        let s = i + p + key.len();
        let n = bytes[s..].iter().take_while(|b| b.is_ascii_alphanumeric()).count();
        let rest = &bytes[s + n..];
        if n > 0 && (rest.starts_with(b".inc") || rest.starts_with(b".h")) {
            out.insert(String::from_utf8_lossy(&bytes[s..s + n]).to_ascii_uppercase());
        }
        i = s;
    }
    out
}

/// A note when the ELF names parts and `dev` is not one of them.
pub fn elf_part_note(dev: &devdesc::Device, bytes: &[u8]) -> Option<String> {
    let parts = elf_parts(bytes);
    let want = dev.device.name.to_ascii_uppercase();
    let want = want.strip_prefix("PIC").unwrap_or(&want);
    (!parts.is_empty() && !parts.contains(want)).then(|| format!("the ELF was built for {} but the part is the {}", parts.into_iter().collect::<Vec<_>>().join(", "), dev.device.name))
}

/// A note when `img` holds no byte of program memory (an empty hex file, or one with only configuration words) (C3).
pub fn empty_note(dev: &devdesc::Device, img: &Image) -> Option<String> {
    let k = if dev.device.family == "pic18" { 1 } else { 2 };
    let (s, n) = (dev.memory.program.start, dev.memory.program.size);
    (!img.bytes.keys().any(|&a| (k * s..k * (s + n)).contains(&a))).then(|| "the hex file has no program bytes: the part runs erased flash".to_string())
}

#[cfg(test)]
mod tests {
    use super::Image;

    const EOF: &str = ":00000001FF\n";

    #[test]
    fn malformed_records_are_errors() {
        let ok = format!(":020000040000FA\n:0100000012ED\n:0400000500000000F7\n{EOF}\n");
        assert_eq!(Image::parse(&ok).unwrap().bytes.get(&0), Some(&0x12));
        for (bad, why) in [
            (format!(":0100000012EDFF\n{EOF}"), "characters after the checksum"),
            (format!(":0100000400FB\n{EOF}"), "must hold 2 bytes"),
            (format!(":03000002000000FB\n{EOF}"), "must hold 2 bytes"),
            (format!("{EOF}:0100000012ED\n"), "after the end record"),
            (format!(":0000000AF6\n{EOF}"), "unknown record type"),
        ] {
            let e = Image::parse(&bad).err().unwrap_or_else(|| panic!("{bad} parsed")).0;
            assert!(e.contains(why), "{bad}: {e}");
        }
    }
}
