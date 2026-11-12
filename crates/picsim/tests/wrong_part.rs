//! A hex or ELF built for another part gives a note (REVIEW-2026-11-11 C2).

use devdesc::Device;
use picsim::machine::Machine;
use picsim::machine16::Machine16;
use std::path::{Path, PathBuf};

fn dev(part: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap()
}

fn sim(f: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/sim").join(f)
}

fn hex() -> String {
    std::fs::read_to_string(sim("blink_q10.hex")).unwrap()
}

#[test]
fn a_pic18_hex_on_a_pic16_part_gives_a_note_that_survives_a_reset() {
    let mut m = Machine16::new(dev("16F15244")).unwrap();
    m.load_hex(&hex()).unwrap();
    let stray = |m: &Machine16| m.cpu.notes.iter().any(|n| n.contains("outside every memory region of the PIC16F15244"));
    assert!(stray(&m), "{:?}", m.cpu.notes);
    m.load_elf(&sim("blink_q10.elf")).unwrap();
    assert!(m.cpu.notes.iter().any(|n| n.contains("the ELF was built for 18F27Q10")), "{:?}", m.cpu.notes);
    m.reset().unwrap();
    assert!(stray(&m) && m.cpu.notes.iter().any(|n| n.contains("ELF")), "kept after a reset: {:?}", m.cpu.notes);
}

#[test]
fn the_right_part_gives_no_note_and_another_pic18_part_names_the_elf() {
    let mut m = Machine::new(dev("18F27Q10")).unwrap();
    m.load_hex(&hex()).unwrap();
    m.load_elf(&sim("blink_q10.elf")).unwrap();
    assert!(m.cpu.notes.is_empty(), "{:?}", m.cpu.notes);
    let mut q = Machine::new(dev("18F16Q41")).unwrap();
    q.load_hex(&hex()).unwrap();
    q.load_elf(&sim("blink_q10.elf")).unwrap();
    assert!(q.cpu.notes.iter().any(|n| n.contains("the ELF was built for 18F27Q10 but the part is the PIC18F16Q41")), "{:?}", q.cpu.notes);
    q.reset();
    assert!(q.cpu.notes.iter().any(|n| n.contains("ELF")), "{:?}", q.cpu.notes);
}

#[test]
fn elf_parts_reads_basil_and_xc8_includes() {
    let b = b"x\0/opt/pack/xc8/pic/include/proc/pic18f27q10.inc\0y/proc/PIC16F18056.h\0proc/pic\0proc/picz.c";
    let p: Vec<String> = picsim::hex::elf_parts(b).into_iter().collect();
    assert_eq!(p, ["16F18056", "18F27Q10"]);
}
