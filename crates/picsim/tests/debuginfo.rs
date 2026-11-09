use picsim::debuginfo::{DebugInfo, Form, Kind, Ty, Var};

fn info() -> DebugInfo {
    DebugInfo::from_elf(include_bytes!("data/i2c_q10_g.elf")).unwrap()
}

#[test]
fn routines_have_ranges_and_halt_is_recorded() {
    let d = info();
    let send = d.funcs.iter().position(|f| f.name == "I2cSend").unwrap();
    let f = &d.funcs[send];
    assert!(f.low < f.high);
    assert_eq!(d.func_at(f.low), Some(send));
    assert_eq!(d.func_at(f.high - 2), Some(send));
    assert_ne!(d.func_at(f.high), Some(send));
    // __halt is an empty range: a symbol, never "the routine the PC is in".
    let h = d.funcs.iter().position(|f| f.name == "__halt").unwrap();
    assert_eq!(d.funcs[h].low, d.funcs[h].high);
    assert_ne!(d.func_at(d.funcs[h].low), Some(h));
}

#[test]
fn globals_params_and_locals_are_scoped() {
    let d = info();
    let g = |n: &str| d.vars.iter().find(|v| v.name == n && v.scope.is_none()).unwrap_or_else(|| panic!("global {n}"));
    assert_eq!((g("iq_s0").addr, g("iq_s0").bytes, &g("iq_s0").kind), (0x0f, 1, &Kind::Unsigned));
    assert_eq!(g("ready").kind, Kind::Bool);
    let send = d.funcs.iter().position(|f| f.name == "I2cSend").unwrap();
    let names: Vec<_> = d.visible(Some(send)).map(|v| (v.name.as_str(), v.param)).collect();
    assert!(names.contains(&("b", true)), "{names:?}");
    assert!(names.contains(&("Result", false)));
    assert!(names.contains(&("ready", false)), "globals stay visible");
    // another routine's locals do not
    let wait = d.funcs.iter().position(|f| f.name == "SetPps").unwrap();
    assert!(!d.visible(Some(wait)).any(|v| v.name == "ack"));
    // outside any routine only globals
    assert!(d.visible(None).all(|v| v.scope.is_none()));
}

fn var(kind: Kind, bytes: u32, elem: u32, array: bool) -> Var {
    let one = Ty { bytes: elem, elem, kind: kind.clone(), array: false, name: String::new(), form: Form::Scalar };
    let layout = if array { Ty { bytes, elem, kind: kind.clone(), array, name: String::new(), form: Form::Array { of: Box::new(one), count: bytes / elem } } } else { one };
    Var { name: "x".into(), addr: 2, bytes, kind, elem, array, ty: String::new(), scope: None, param: false, rom: false, layout, frame: None, unit: 0, file_static: false }
}

#[test]
fn values_are_shown_by_type() {
    let mem = [0, 0, 0xFE, 0xFF, 0x00, 0x00, b'H', b'i', 0, 0x01, 0x02];
    assert_eq!(var(Kind::Signed, 2, 2, false).show(&mem), "-2 (0xFFFE)");
    assert_eq!(var(Kind::Unsigned, 2, 2, false).show(&mem), "65534 (0xFFFE)");
    assert_eq!(var(Kind::Bool, 1, 1, false).show(&mem), "true");
    assert_eq!(var(Kind::Char, 4, 1, true).show(&[0, 0, b'H', b'i', 0, 0]), "\"Hi\"");
    assert_eq!(var(Kind::Unsigned, 3, 1, true).show(&[0, 0, 1, 2, 3]), "{1 (0x01), 2 (0x02), 3 (0x03)}");
    let f = 1.5f32.to_bits().to_le_bytes();
    assert!(var(Kind::Float, 4, 4, false).show(&[0, 0, f[0], f[1], f[2], f[3]]).starts_with("1.5 "));
    assert_eq!(var(Kind::Unsigned, 1, 1, false).show(&[0]), "(outside RAM)");
}

#[test]
fn device_is_read_from_the_elf() {
    assert_eq!(picsim::debuginfo::device_of(b"\0/xc8/pic/include/pic18.inc\0PIC18F-Q_DFP/1.31\0pic18f27q10.inc\0X").as_deref(), Some("18F27Q10"));
    assert_eq!(picsim::debuginfo::device_of(b"nothing here"), None);
}

#[test]
fn device_is_read_from_the_microchip_note() {
    // as written by the linker into .note.microchip of a -g build (no .inc names in that ELF)
    let note = b"\n\0\0\0\t\0\0\0\0\0\0\0Microchip\0\0\018F27Q10\0\0\0\0_stray_";
    assert_eq!(picsim::debuginfo::device_of(note).as_deref(), Some("18F27Q10"));
    // the real ELF of a -g build (tests/data/zed_g.elf)
    assert_eq!(picsim::debuginfo::device_of(include_bytes!("data/zed_g.elf")).as_deref(), Some("18F27Q10"));
    assert_eq!(picsim::debuginfo::device_of(include_bytes!("data/i2c_q10_g.elf")).as_deref(), Some("18F27Q10"));
}
