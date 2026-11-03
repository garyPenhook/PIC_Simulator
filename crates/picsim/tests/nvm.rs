//! Data EEPROM of the PIC18F27Q10 (DS40002043F 12.3, p.157-159, 12.5, MEM24 p.697).

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn machine() -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    Machine::new(dev).unwrap()
}

fn a(m: &Machine, n: &str) -> u32 {
    m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr
}

fn setadr(m: &mut Machine, adr: u32) {
    for (n, sh) in [("NVMADRL", 0), ("NVMADRH", 8), ("NVMADRU", 16)] {
        let r = a(m, n);
        m.cpu.write(r, (adr >> sh) as u8);
    }
}

fn unlock_wr(m: &mut Machine) {
    let (c1, c2) = (a(m, "NVMCON1"), a(m, "NVMCON2"));
    m.cpu.write(c2, 0x55);
    m.cpu.write(c2, 0xAA);
    m.cpu.write(c1, 0x10);
}

fn nvmif(m: &mut Machine) -> bool {
    let r = a(m, "PIR7");
    m.cpu.read(r) & 0x20 != 0
}

#[test]
fn write_then_read() {
    let mut m = machine();
    let (d, c0, c1) = (a(&m, "NVMDATL"), a(&m, "NVMCON0"), a(&m, "NVMCON1"));
    setadr(&mut m, 0x310005);
    m.cpu.write(d, 0x5A);
    m.cpu.write(c0, 0x80);
    let t0 = m.cpu.cycles;
    unlock_wr(&mut m);
    assert_eq!(m.cpu.read(c1) & 0x10, 0x10, "WR stays set");
    while m.cpu.cycles < t0 + 9_000 {
        m.cpu.step();
    }
    assert_eq!(m.cpu.read(c1) & 0x10, 0x10);
    assert!(!nvmif(&mut m));
    while m.cpu.cycles < t0 + 10_100 {
        m.cpu.step();
    }
    assert_eq!(m.cpu.read(c1) & 0x10, 0);
    assert!(nvmif(&mut m));
    m.cpu.write(c0, 0); // reads need no NVMEN
    m.cpu.write(d, 0);
    m.cpu.write(c1, 1);
    assert_eq!(m.cpu.read(d), 0x5A);
}

#[test]
fn blocked_without_nvmen_or_unlock() {
    let mut m = machine();
    let (d, c0, c1, c2) = (a(&m, "NVMDATL"), a(&m, "NVMCON0"), a(&m, "NVMCON1"), a(&m, "NVMCON2"));
    setadr(&mut m, 0x310000);
    m.cpu.write(d, 0x11);
    unlock_wr(&mut m); // NVMEN clear
    assert_eq!(m.cpu.read(c1) & 0x10, 0);
    m.cpu.write(c0, 0x80);
    m.cpu.write(c2, 0x55);
    m.cpu.write(c2, 0x55); // not AAh
    m.cpu.write(c1, 0x10);
    assert_eq!(m.cpu.read(c1) & 0x10, 0);
    m.cpu.write(c1, 0x10); // no unlock at all
    assert_eq!(m.cpu.read(c1) & 0x10, 0);
    assert_eq!(m.cpu.read(c2), 0, "NVMCON2 reads zero");
}

#[test]
fn invalid_address_sets_nvmerr() {
    let mut m = machine();
    let (d, c0, c1) = (a(&m, "NVMDATL"), a(&m, "NVMCON0"), a(&m, "NVMCON1"));
    setadr(&mut m, 0x310400); // 1024 bytes: one past the end
    m.cpu.write(d, 1);
    m.cpu.write(c0, 0x80);
    unlock_wr(&mut m);
    assert_eq!(m.cpu.read(c1) & 0x10, 0);
    assert_eq!(m.cpu.read(c0) & 0x10, 0x10, "NVMERR");
}
