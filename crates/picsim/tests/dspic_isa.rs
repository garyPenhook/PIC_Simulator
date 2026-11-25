//! The decoder against instruction words the MPLAB XC-DSC assembler produced (`tests/data/dspic_encodings.txt`; written by
//! hand-picked instruction forms, one per line: `word [word2] | assembler source | picsim disassembly`).
use picsim::dspic_isa::{decode, format};

#[test]
fn every_assembled_word_decodes_to_the_recorded_text() {
    let text = include_str!("data/dspic_encodings.txt");
    let mut n = 0;
    for l in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let mut parts = l.split(" | ");
        let words: Vec<u32> = parts.next().unwrap().split_whitespace().map(|w| u32::from_str_radix(w, 16).unwrap()).collect();
        let src = parts.next().unwrap();
        let want = parts.next().unwrap();
        let d = decode(words[0], words.get(1).copied().unwrap_or(0)).unwrap_or_else(|| panic!("{src}: {:06X} does not decode", words[0]));
        assert_eq!(format(&d.inst, 0, true), want, "{src}");
        assert_eq!(usize::from(d.words), words.len().max(1).min(2), "{src}: length");
        n += 1;
    }
    assert!(n > 250, "{n} lines");
}

#[test]
fn unused_opcodes_do_not_decode() {
    for w in [0x03_0000u32, 0x0B_0000, 0x0D_8000 | 0x10_0000, 0x4F_0000 | 0x3F_0000, 0xF4_0000, 0xF5_0000, 0xF6_0000, 0xF7_0000] {
        // 0x0B, 0xF4..0xF7 are unassigned; the others just have to not panic
        let _ = decode(w, 0);
    }
    assert!(decode(0x0B_0000, 0).is_none());
    assert!(decode(0xF4_0000, 0).is_none());
    assert!(decode(0xFE_1000, 0).is_none());
}
