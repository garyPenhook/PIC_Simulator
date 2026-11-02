//! PIC18 disassembler (standard instruction set, DS40002214G Table 45-2) for traces and the GUI.

/// Disassemble the instruction whose first word is `w`; `w2`/`w3` are the following words and `at` its byte
/// address. Returns the text and the length in words. `wide_fsr` selects the 14-bit LFSR encoding.
pub fn disasm(at: u32, w: u16, w2: u16, w3: u16, wide_fsr: bool) -> (String, u32) {
    let f = w & 0xFF;
    let a = if w & 0x100 != 0 { "BANKED" } else { "ACCESS" };
    let d = if w & 0x200 != 0 { "f" } else { "w" };
    let fda = |m: &str| format!("{m:<7}0x{f:02X}, {d}, {a}");
    let fa = |m: &str| format!("{m:<7}0x{f:02X}, {a}");
    let bit = |m: &str| format!("{m:<7}0x{f:02X}, {}, {a}", (w >> 9) & 7);
    let rel = |off: i32| format!("0x{:06X}", (at as i32 + 2 + off * 2) as u32);
    let one = |s: String| (s, 1);
    match w >> 12 {
        0x0 => match w >> 8 {
            0x00 => match w & 0xFF {
                0x00 => one("nop".into()),
                0x03 => one("sleep".into()),
                0x04 => one("clrwdt".into()),
                0x05 => one("push".into()),
                0x06 => one("pop".into()),
                0x07 => one("daw".into()),
                0x08 => one("tblrd*".into()),
                0x09 => one("tblrd*+".into()),
                0x0A => one("tblrd*-".into()),
                0x0B => one("tblrd+*".into()),
                0x0C => one("tblwt*".into()),
                0x0D => one("tblwt*+".into()),
                0x0E => one("tblwt*-".into()),
                0x0F => one("tblwt+*".into()),
                0x10 | 0x11 => one(format!("retfie {}", w & 1)),
                0x12 | 0x13 => one(format!("return {}", w & 1)),
                0x14 => one("callw".into()),
                0x60..=0x6F => {
                    let s = u32::from(w & 0xF) << 10 | u32::from(w2 & 0xFFF) >> 2;
                    let dd = u32::from(w2 & 3) << 12 | u32::from(w3 & 0xFFF);
                    (format!("movffl 0x{s:04X}, 0x{dd:04X}"), 3)
                }
                0xFF => one("reset".into()),
                _ => one(format!("dw 0x{w:04X}")),
            },
            0x01 => one(format!("movlb  {}", w & 0x3F)),
            0x02 | 0x03 => one(fa("mulwf")),
            0x04..=0x07 => one(fda("decf")),
            0x08 => one(format!("sublw  0x{:02X}", w & 0xFF)),
            0x09 => one(format!("iorlw  0x{:02X}", w & 0xFF)),
            0x0A => one(format!("xorlw  0x{:02X}", w & 0xFF)),
            0x0B => one(format!("andlw  0x{:02X}", w & 0xFF)),
            0x0C => one(format!("retlw  0x{:02X}", w & 0xFF)),
            0x0D => one(format!("mullw  0x{:02X}", w & 0xFF)),
            0x0E => one(format!("movlw  0x{:02X}", w & 0xFF)),
            _ => one(format!("addlw  0x{:02X}", w & 0xFF)),
        },
        0x1 => one(fda(["iorwf", "andwf", "xorwf", "comf"][((w >> 10) & 3) as usize])),
        0x2 => one(fda(["addwfc", "addwf", "incf", "decfsz"][((w >> 10) & 3) as usize])),
        0x3 => one(fda(["rrcf", "rlcf", "swapf", "incfsz"][((w >> 10) & 3) as usize])),
        0x4 => one(fda(["rrncf", "rlncf", "infsnz", "dcfsnz"][((w >> 10) & 3) as usize])),
        0x5 => one(fda(["movf", "subfwb", "subwfb", "subwf"][((w >> 10) & 3) as usize])),
        0x6 => one(fa(["cpfslt", "cpfseq", "cpfsgt", "tstfsz", "setf", "clrf", "negf", "movwf"][((w >> 9) & 7) as usize])),
        0x7 => one(bit("btg")),
        0x8 => one(bit("bsf")),
        0x9 => one(bit("bcf")),
        0xA => one(bit("btfss")),
        0xB => one(bit("btfsc")),
        0xC => (format!("movff  0x{:03X}, 0x{:03X}", w & 0xFFF, w2 & 0xFFF), 2),
        0xD => {
            let off = i32::from(w & 0x7FF) - if w & 0x400 != 0 { 0x800 } else { 0 };
            one(format!("{:<7}{}", if w & 0x800 != 0 { "rcall" } else { "bra" }, rel(off)))
        }
        0xE => match (w >> 8) & 0xF {
            c @ 0..=7 => {
                let m = ["bz", "bnz", "bc", "bnc", "bov", "bnov", "bn", "bnn"][c as usize];
                one(format!("{m:<7}{}", rel(i32::from(w as u8 as i8))))
            }
            8 => one(format!("addfsr {}, {}", (w >> 6) & 3, w & 0x3F)),
            9 => one(format!("subfsr {}, {}", (w >> 6) & 3, w & 0x3F)),
            0xC | 0xD => {
                let k = (u32::from(w2 & 0xFFF) << 8 | u32::from(w & 0xFF)) << 1;
                (format!("call   0x{k:06X}, {}", (w >> 8) & 1), 2)
            }
            0xE => {
                let k = if wide_fsr {
                    u32::from(w & 0xF) << 10 | u32::from(w2 & 0x3FF)
                } else {
                    u32::from(w & 0xF) << 8 | u32::from(w2 & 0xFF)
                };
                (format!("lfsr   {}, 0x{k:04X}", (w >> 4) & 3), 2)
            }
            0xF => {
                let k = (u32::from(w2 & 0xFFF) << 8 | u32::from(w & 0xFF)) << 1;
                (format!("goto   0x{k:06X}"), 2)
            }
            _ => one(format!("dw 0x{w:04X}")),
        },
        _ => one("nop".into()),
    }
}
