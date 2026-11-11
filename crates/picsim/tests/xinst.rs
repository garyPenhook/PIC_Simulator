//! ADDFSR/SUBFSR: extended instructions on the Q10, run only with XINST = 0 (DS40002043F 38.2 p.681; XINST is CONFIG2
//! bit 15, 0x300003 bit 7, p.31); standard instructions on the Q41 (DS40002214G 45.2 p.864). The rest of the extended
//! set is not modelled, and a note says so when XINST is set.

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn rec(addr: u16, ty: u8, data: &[u8]) -> String {
    let mut b = vec![data.len() as u8, (addr >> 8) as u8, addr as u8, ty];
    b.extend_from_slice(data);
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

/// ADDFSR 0,5; BRA self. `cfg3` is the byte at 0x300003 (XINST in bit 7).
fn machine(part: &str, cfg3: u8) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let mut m = Machine::new(dev).unwrap();
    let hex = rec(0, 0, &[0x05, 0xE8, 0xFF, 0xD7]) + &rec(0, 4, &[0x00, 0x30]) + &rec(3, 0, &[cfg3]) + &rec(0, 1, &[]);
    m.load_hex(&hex).unwrap();
    m
}

fn fsr0l(m: &mut Machine) -> u8 {
    let a = m.dev.find_sfr("FSR0L").unwrap().addr;
    m.cpu.read(a)
}

fn run(part: &str, cfg3: u8) -> (u8, u8, Vec<String>) {
    let mut m = machine(part, cfg3);
    let before = fsr0l(&mut m);
    for _ in 0..3 {
        m.cpu.step();
    }
    let after = fsr0l(&mut m);
    (before, after, m.cpu.notes.iter().cloned().collect())
}

#[test]
fn q10_addfsr_needs_xinst() {
    let (b, a, notes) = run("18F27Q10", 0xFF);
    assert_eq!(a, b, "XINST off: ADDFSR is not an instruction");
    assert!(notes.iter().any(|n| n.contains("undefined opcode E805")), "{notes:?}");
    assert!(!notes.iter().any(|n| n.contains("XINST")), "{notes:?}");

    let (b, a, notes) = run("18F27Q10", 0x7F);
    assert_eq!(a, b.wrapping_add(5));
    assert!(notes.iter().any(|n| n.contains("XINST set") && n.contains("ADDFSR and SUBFSR are modelled")), "{notes:?}");
}

#[test]
fn q41_addfsr_is_standard() {
    let (b, a, notes) = run("18F16Q41", 0xFF);
    assert_eq!(a, b.wrapping_add(5));
    assert!(!notes.iter().any(|n| n.contains("undefined") || n.contains("XINST")), "{notes:?}");

    let (_, _, notes) = run("18F16Q41", 0x7F);
    assert!(notes.iter().any(|n| n.contains("XINST set") && n.contains("nothing is modelled")), "{notes:?}");
}
