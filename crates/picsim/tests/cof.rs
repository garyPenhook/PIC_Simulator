//! Debug information read from the COFF file of a PIC BASIC compiler (`Declare Create_Coff = On`) and its source.
//! The fixtures were built by that compiler: `data/cof/pic18/v.*` (PIC18F27Q10: every variable type, a routine with parameters,
//! a local and a result) and `data/cof/pic16/u.*` (PIC16F18313, program addresses in words).

use devdesc::Device;
use picsim::cofinfo::{cof_for, device_of_listing};
use picsim::debuginfo::{DebugInfo, Kind};
use picsim::machine16::Machine16;
use picsim::Machine;
use std::path::{Path, PathBuf};

fn dir(sub: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/cof").join(sub)
}

fn info(sub: &str, stem: &str, word: bool) -> DebugInfo {
    let d = dir(sub);
    DebugInfo::from_cof(&std::fs::read(d.join(format!("{stem}.cof"))).unwrap(), word, &d).unwrap()
}

#[test]
fn the_part_comes_from_the_build_log() {
    assert_eq!(device_of_listing(&dir("pic18").join("v.hex")).as_deref(), Some("18F27Q10"));
    assert_eq!(device_of_listing(&dir("pic16").join("u.hex")).as_deref(), Some("16F18313"));
    assert_eq!(cof_for(&dir("pic18").join("v.hex")), Some(dir("pic18").join("v.cof")));
}

#[test]
fn variables_have_the_types_the_source_declares() {
    let d = info("pic18", "v", false);
    let g = |n: &str| d.vars.iter().find(|v| v.name == n && v.scope.is_none()).unwrap_or_else(|| panic!("global {n}"));
    let sc = |n: &str, k: Kind, bytes: u32, addr: u32, ty: &str| {
        let v = g(n);
        assert_eq!((v.addr, v.bytes, &v.kind, v.ty.as_str(), v.array), (addr, bytes, &k, ty, false), "{n}");
    };
    sc("vb", Kind::Unsigned, 1, 0x01, "Byte");
    sc("vw", Kind::Unsigned, 2, 0x02, "Word");
    sc("vd", Kind::Unsigned, 4, 0x04, "Dword");
    // The COFF file writes the same type for Byte and SByte; the source says which.
    sc("vsb", Kind::Signed, 1, 0x08, "SByte");
    sc("vsw", Kind::Signed, 2, 0x09, "SWord");
    sc("vsd", Kind::Signed, 4, 0x0B, "SDword");
    sc("vf", Kind::FloatMchp, 4, 0x0F, "Float");
    let s = g("vs");
    assert_eq!((s.addr, s.bytes, s.array, &s.kind, s.ty.as_str()), (0x13, 11, true, &Kind::Char, "String"));
    let a = g("arr");
    assert_eq!((a.addr, a.bytes, a.elem, a.array, a.ty.as_str()), (0x1E, 5, 1, true, "Byte[5]"));
    let w = g("warr");
    assert_eq!((w.addr, w.bytes, w.elem, w.ty.as_str()), (0x23, 6, 2, "Word[3]"));
    // A bit variable is the bit of a byte.
    let f = g("flag");
    assert_eq!((f.addr, f.bytes, f.ty.as_str()), (0x00, 1, "Bit"));
    assert_eq!(f.show(&[1]), "1");
    assert_eq!(f.show(&[0xFE]), "0");
    // Names are found in any case.
    assert_eq!(d.global("VSW", None).map(|v| v.addr), Some(0x09));
}

#[test]
fn routines_have_their_parameters_locals_and_result() {
    let d = info("pic18", "v", false);
    let calc = d.routine("calc").expect("Calc");
    let f = &d.funcs[calc];
    assert_eq!(f.name, "Calc");
    assert!(f.low < f.high);
    assert_eq!(d.func_at(f.low), Some(calc));
    assert_ne!(d.func_at(f.high), Some(calc), "ends where the next routine starts");
    let twice = d.routine("Twice").expect("Twice");
    assert_eq!(d.funcs[twice].low, f.high);
    let seen: Vec<_> = d.visible(Some(calc)).filter(|v| v.scope.is_some()).map(|v| (v.name.as_str(), v.param, v.ty.as_str(), v.addr)).collect();
    assert_eq!(seen, [("a", true, "Word", 0x29), ("b", true, "SByte", 0x2B), ("Result", false, "Word", 0x2C), ("loc", false, "Byte", 0x2E)]);
    // another routine's parameters are not visible here
    assert!(!d.visible(Some(calc)).any(|v| v.name == "x"));
    assert!(d.visible(None).all(|v| v.scope.is_none()));
}

#[test]
fn lines_map_to_the_source_file() {
    let d = info("pic18", "v", false);
    let file = d.lines.files.first().expect("source file").clone();
    assert!(file.ends_with("v.bas") && file.is_file(), "found beside the COFF file: {}", file.display());
    let calc = d.routine("Calc").unwrap();
    let body = d.lines.first_addr(&file, 25).expect("line 25 has code"); // Result = loc + b
    assert!(d.funcs[calc].low <= body && body < d.funcs[calc].high);
    assert_eq!(d.lines.lookup(body).map(|(f, l)| (f.clone(), l)), Some((file.clone(), 25)));
    // start-up code the compiler adds has no line
    assert_eq!(d.lines.lookup(0x10), None);
}

fn values(mem: &[u8], d: &DebugInfo, scope: Option<usize>) -> Vec<(String, String)> {
    d.visible(scope).map(|v| (v.name.clone(), v.show_with(mem, &[], d))).collect()
}

#[test]
fn a_running_pic18_program_is_read_by_name() {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_hex(&std::fs::read_to_string(dir("pic18").join("v.hex")).unwrap()).unwrap();
    m.load_cof(&dir("pic18").join("v.cof")).unwrap();
    let file = m.debug.lines.files.first().unwrap().clone();
    let stop = m.debug.lines.first_addr(&file, 44).expect("the Stop line"); // the program's last statement
    for _ in 0..200_000 {
        if m.cpu.pc == stop {
            break;
        }
        m.step();
    }
    assert_eq!(m.cpu.pc, stop, "reached the Stop line");
    let v: std::collections::HashMap<_, _> = values(&m.cpu.m.mem, &m.debug, None).into_iter().collect();
    assert_eq!(v["vb"], "2 (0x02)");
    assert_eq!(v["flag"], "1");
    // The main program is a routine of its own, found after the routines inside its section.
    let main = m.debug.func_at(stop).expect("the main program");
    assert_eq!(m.debug.funcs[main].name, "Main");
    assert_ne!(m.debug.func_at(m.debug.lines.first_addr(&file, 25).unwrap()), Some(main));
    assert_eq!(v["vw"], "7 (0x0007)");
    assert_eq!(v["vsb"], "-3 (0xFD)");
    assert_eq!(v["vsw"], "-300 (0xFED4)");
    assert_eq!(v["vsd"], "-70000 (0xFFFEEE90)");
    assert!(v["vf"].starts_with("1.5 "), "{}", v["vf"]);
    assert_eq!(v["vs"], "\"hi\"");
    assert_eq!(v["warr"], "{0 (0x0000), 0 (0x0000), 513 (0x0201)}");
}

#[test]
fn a_running_pic16_program_stops_in_its_routine() {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/16F18313.toml")).unwrap();
    let mut m = Machine16::new(dev).unwrap();
    m.load_hex(&std::fs::read_to_string(dir("pic16").join("u.hex")).unwrap()).unwrap();
    m.load_cof(&dir("pic16").join("u.cof")).unwrap();
    let file = m.debug.lines.files.first().unwrap().clone();
    let at = m.debug.lines.first_addr(&file, 8).expect("the Proc body"); // total = total + n
    let routine = m.debug.routine("AddIt").unwrap();
    assert_eq!(m.debug.func_at(at), Some(routine));
    let mut hits = Vec::new();
    for _ in 0..10_000 {
        if m.cpu.pc == at {
            let v: std::collections::HashMap<_, _> = values(&m.cpu.mem, &m.debug, Some(routine)).into_iter().collect();
            hits.push((v["total"].clone(), v["n"].clone()));
            if hits.len() == 3 {
                break;
            }
        }
        m.step();
    }
    assert_eq!(hits, [("0 (0x0000)", "1 (0x01)"), ("1 (0x0001)", "2 (0x02)"), ("3 (0x0003)", "3 (0x03)")].map(|(a, b)| (a.to_string(), b.to_string())));
}

#[test]
fn values_are_encoded_for_writing_a_variable() {
    let d = info("pic18", "v", false);
    let g = |n: &str| d.global(n, None).unwrap_or_else(|| panic!("global {n}"));
    let enc = |n: &str, text: &str, cur: &[u8]| g(n).layout.encode(text, cur);
    assert_eq!(enc("vw", "1000", &[0, 0]), Ok(vec![0xE8, 0x03]));
    assert_eq!(enc("vw", "0xFFFF", &[0, 0]), Ok(vec![0xFF, 0xFF]));
    assert!(enc("vw", "65536", &[0, 0]).is_err());
    assert!(enc("vw", "-1", &[0, 0]).is_err());
    assert_eq!(enc("vsb", "-5", &[0]), Ok(vec![0xFB]));
    assert!(enc("vsb", "200", &[0]).is_err(), "decimal must be in the signed range");
    assert_eq!(enc("vsb", "0xC8", &[0]), Ok(vec![0xC8]), "hex may use the whole width");
    assert_eq!(enc("vsd", "-70000", &[0; 4]), Ok((-70000i32).to_le_bytes().to_vec()));
    assert!(enc("vb", "abc", &[0]).is_err());
    for bad in ["--1", "0x-5", "0x", "- 1", "1 2", ""] {
        assert!(enc("vsb", bad, &[0]).is_err(), "{bad:?}");
    }
    assert!(enc("vb", "1", &[0, 0]).is_err(), "wrong size: outside data memory");
    // A Bit changes only its bit.
    assert_eq!(enc("flag", "1", &[0b1010_0000]), Ok(vec![0b1010_0001]));
    assert_eq!(enc("flag", "0", &[0xFF]), Ok(vec![0xFE]));
    assert!(enc("flag", "2", &[0]).is_err());
    // Microchip float: what is written reads back the same.
    let f = enc("vf", "1.5", &[0; 4]).unwrap();
    assert_eq!(g("vf").layout.show_at(&f, 0, 1), "1.5 (0x0000407F)");
    // Strings and arrays are not scalars.
    assert!(enc("vs", "x", &[0; 11]).is_err());
    assert!(enc("arr", "1", &[0; 5]).is_err());
}

#[test]
fn a_bit_watch_ignores_the_other_bits_of_its_byte() {
    let run = |mask: u8| {
        let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
        let mut m = Machine::new(dev).unwrap();
        m.load_hex(&std::fs::read_to_string(dir("pic18").join("v.hex")).unwrap()).unwrap();
        m.load_cof(&dir("pic18").join("v.cof")).unwrap();
        m.debugger.bit_watch.push((0x00, mask)); // `flag` is bit 0 of the byte at 0
        (0..200_000).find_map(|_| m.step()).filter(|s| matches!(s, picsim::Stop::Watchpoint(_)))
    };
    match run(0x01) {
        Some(picsim::Stop::Watchpoint(c)) => assert_eq!((c.addr, c.before, c.after), (0, 0, 1)),
        other => panic!("expected the write to flag to stop: {other:?}"),
    }
    assert!(run(0x08).is_none(), "bit 3 never changes");
}
