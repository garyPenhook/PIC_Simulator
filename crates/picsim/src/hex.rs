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
        for (n, raw) in text.lines().enumerate() {
            let line = n + 1;
            let l = raw.trim();
            if l.is_empty() {
                continue;
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
            if sum.wrapping_add(ck) != 0 {
                return Err(HexError(format!("line {line}: checksum")));
            }
            match kind {
                0 => {
                    for (i, b) in data.iter().enumerate() {
                        img.bytes.insert(base.wrapping_add(addr).wrapping_add(i as u32), *b);
                    }
                }
                1 => break,
                2 if len == 2 => base = (u32::from(data[0]) << 8 | u32::from(data[1])) << 4,
                4 if len == 2 => base = (u32::from(data[0]) << 8 | u32::from(data[1])) << 16,
                _ => {}
            }
        }
        Ok(img)
    }
}
