//! Constants in program memory built with MPLAB XC8 (`xc8-cc -g -O0`): a `const` table is read from program memory, and a
//! pointer into program memory (a string literal, a table) shows what it points at. Fixtures `data/xc8_const_q10.*`
//! (PIC18F27Q10) and `data/xc8_const_p16.*` (PIC16F17146) are built from the same program, `xc8_const_TAG.c`
//! (the two sources are identical): `xc8-cc -mcpu=PART -mdfp=DFP -g -O0 xc8_const_TAG.c -o xc8_const_TAG.elf`.

use devdesc::Device;
use picsim::debuginfo::DebugInfo;
use picsim::machine16::Machine16;
use picsim::Machine;
use std::path::Path;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

fn device(part: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap()
}

/// The debug info, program memory and data memory after the program has run into its final loop.
fn run(tag: &str) -> (DebugInfo, Vec<u8>, Vec<u8>) {
    let base = format!("{DATA}/xc8_const_{tag}");
    let hex = std::fs::read_to_string(format!("{base}.hex")).unwrap();
    if tag == "q10" {
        let mut m = Machine::new(device("18F27Q10")).unwrap();
        m.load_hex(&hex).unwrap();
        m.load_elf(Path::new(&format!("{base}.elf"))).unwrap();
        m.run(None, 20_000);
        (m.debug.clone(), m.cpu.prog.clone(), (0..0x1000).map(|a| m.cpu.peek(a)).collect())
    } else {
        let mut m = Machine16::new(device("16F17146")).unwrap();
        m.load_hex(&hex).unwrap();
        m.load_elf(Path::new(&format!("{base}.elf"))).unwrap();
        m.run(None, 20_000);
        (m.debug.clone(), m.cpu.prog.clone(), (0..0x1000).map(|a| m.cpu.peek(a)).collect())
    }
}

#[test]
fn constants_are_read_from_program_memory_and_pointers_into_it_follow() {
    for tag in ["q10", "p16"] {
        let (d, prog, mem) = run(tag);
        let rom = d.rom_image(&prog);
        let var = |n: &str| d.vars.iter().find(|v| v.name == n && v.scope.is_none()).unwrap_or_else(|| panic!("{tag}: {n}"));
        let show = |n: &str| var(n).show_with(&mem, &rom, &d);

        assert!(var("tab").rom && var("names").rom, "{tag}");
        assert!(!var("buf").rom && !var("msg").rom && !var("rp").rom, "{tag}: variables in data memory are not constants");
        assert_eq!(show("tab"), "{7 (0x07), 8 (0x08), 9 (0x09)}", "{tag}");
        // A pointer into program memory shows the byte it points at; one into data memory, as before.
        assert!(show("msg").ends_with(" -> 119 (0x77)"), "{tag}: {}", show("msg")); // 'w' of "world"
        assert!(show("tp").ends_with(" -> 7 (0x07)"), "{tag}: {}", show("tp"));
        assert!(show("rp").ends_with(" -> 97 (0x61)"), "{tag}: {}", show("rp"));
        // A table of string pointers in program memory: each element points into program memory too.
        let names = show("names");
        assert!(names.starts_with('{') && names.contains(" -> 97 (0x61)") && names.contains(" -> 99 (0x63)"), "{tag}: {names}");
        // A constant is not in data memory, so Watch and the symbol table must not offer a data address for it.
        assert_eq!(d.path_addr("tab[1]"), None, "{tag}");
    }
}

#[test]
fn constants_sit_above_the_vectors_and_the_ram_of_this_part() {
    let (d, ..) = run("q10");
    assert!(d.rom.iter().all(|r| r.0 >= 0x1000), "constants are above the RAM and the vectors");
}
