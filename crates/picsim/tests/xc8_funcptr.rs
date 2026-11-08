//! Function pointers in C programs built with `xc8-cc -g -O0`: the value is a program address and the Variables tab names the
//! routine it enters. Fixtures `data/xc8_fp_q10.*` (PIC18F27Q10) and `data/xc8_fp_p16.*` (PIC16F17146) are built from the same
//! program, `data/xc8_fp_TAG.c`: `xc8-cc -mcpu=PART -mdfp=DFP -g -O0 xc8_fp_TAG.c -o xc8_fp_TAG.elf`. Both run to the final loop.

use devdesc::Device;
use picsim::debuginfo::DebugInfo;
use picsim::machine16::Machine16;
use picsim::Machine;
use std::path::Path;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

fn device(part: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap()
}

fn run(tag: &str) -> (DebugInfo, Vec<u8>) {
    let base = format!("{DATA}/xc8_fp_{tag}");
    let hex = std::fs::read_to_string(format!("{base}.hex")).unwrap();
    if tag == "q10" {
        let mut m = Machine::new(device("18F27Q10")).unwrap();
        m.load_hex(&hex).unwrap();
        m.load_elf(Path::new(&format!("{base}.elf"))).unwrap();
        m.run(None, 20_000);
        (m.debug.clone(), (0..0x1000).map(|a| m.cpu.peek(a)).collect())
    } else {
        let mut m = Machine16::new(device("16F17146")).unwrap();
        m.load_hex(&hex).unwrap();
        m.load_elf(Path::new(&format!("{base}.elf"))).unwrap();
        m.run(None, 20_000);
        (m.debug.clone(), (0..0x1000).map(|a| m.cpu.peek(a)).collect())
    }
}

#[test]
fn function_pointers_name_the_routine_they_enter() {
    for (tag, a, b, w) in [("q10", "0x00109A", "0x0010A0", 6), ("p16", "0x07D3", "0x07D0", 4)] {
        let (d, mem) = run(tag);
        let var = |n: &str| d.vars.iter().find(|v| v.name == n && v.scope.is_none()).unwrap_or_else(|| panic!("{tag}: {n}"));
        assert_eq!(var("fn").show_with(&mem, &d.rom_image(&[]), &d), format!("{a} -> a"), "{tag}");
        assert_eq!(var("fn2").show_with(&mem, &d.rom_image(&[]), &d), format!("{b} -> b"), "{tag}");
        assert_eq!(var("tbl").show_with(&mem, &d.rom_image(&[]), &d), format!("{{{b} -> b, {a} -> a}}"), "{tag}");
        assert_eq!(var("fn").show(&mem), a, "{tag}: without the routine table only the address shows");
        assert_eq!(a.len(), w + 2);
        // The value is the routine's entry address in the units of the PC: bytes on a PIC18, words on a PIC16.
        let entry = d.funcs.iter().find(|f| f.name == "a").unwrap().low;
        assert_eq!(format!("0x{entry:0w$X}"), a, "{tag}");
    }
}

#[test]
fn a_null_function_pointer_shows_null() {
    let (d, _) = run("q10");
    let v = d.vars.iter().find(|v| v.name == "fn").unwrap();
    assert_eq!(v.show_with(&vec![0u8; 0x1000], &[], &d), "NULL");
}
