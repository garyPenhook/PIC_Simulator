//! Data EEPROM of the PIC18F16Q41 (DS40002214G 10.1-10.5, p.87-108).

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn machine() -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F16Q41.toml")).unwrap();
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

fn adr(m: &mut Machine) -> u32 {
    let r = [a(m, "NVMADRL"), a(m, "NVMADRH"), a(m, "NVMADRU")];
    u32::from(m.cpu.read(r[0])) | u32::from(m.cpu.read(r[1])) << 8 | u32::from(m.cpu.read(r[2])) << 16
}

fn unlock_go(m: &mut Machine) {
    let (c0, lock) = (a(m, "NVMCON0"), a(m, "NVMLOCK"));
    m.cpu.write(lock, 0x55);
    m.cpu.write(lock, 0xAA);
    m.cpu.write(c0, 1);
}

fn nvmif(m: &mut Machine) -> bool {
    let r = a(m, "PIR0");
    m.cpu.read(r) & 0x10 != 0
}

fn run_to(m: &mut Machine, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

#[test]
fn write_then_read() {
    let mut m = machine();
    let (d, c0, c1) = (a(&m, "NVMDATL"), a(&m, "NVMCON0"), a(&m, "NVMCON1"));
    setadr(&mut m, 0x380005);
    m.cpu.write(d, 0x5A);
    m.cpu.write(c1, 3);
    let t0 = m.cpu.cycles;
    unlock_go(&mut m);
    assert_eq!(m.cpu.read(c0) & 1, 1, "GO stays set");
    run_to(&mut m, t0 + 9_000);
    assert_eq!(m.cpu.read(c0) & 1, 1);
    assert!(!nvmif(&mut m));
    run_to(&mut m, t0 + 10_100);
    assert_eq!(m.cpu.read(c0) & 1, 0);
    assert!(nvmif(&mut m));
    assert_eq!(m.cpu.read(c1) & 0x80, 0, "no WRERR");
    m.cpu.write(c1, 0);
    m.cpu.write(d, 0);
    m.cpu.write(c0, 1);
    assert_eq!(m.cpu.read(d), 0x5A);
    assert!(m.cpu.read(c0) & 1 == 0, "a read completes at once");
}

#[test]
fn write_without_unlock_sets_wrerr() {
    let mut m = machine();
    let (d, c0, c1, lock) = (a(&m, "NVMDATL"), a(&m, "NVMCON0"), a(&m, "NVMCON1"), a(&m, "NVMLOCK"));
    setadr(&mut m, 0x380000);
    m.cpu.write(d, 0x11);
    m.cpu.write(c1, 3);
    m.cpu.write(c0, 1);
    assert_eq!(m.cpu.read(c0) & 1, 0);
    assert_eq!(m.cpu.read(c1) & 0x80, 0x80, "WRERR");
    m.cpu.write(c1, 3); // software clears WRERR but cannot set it
    assert_eq!(m.cpu.read(c1), 3);
    m.cpu.write(lock, 0x55);
    m.cpu.write(lock, 0x55); // not AAh
    m.cpu.write(c0, 1);
    assert_eq!(m.cpu.read(c1) & 0x80, 0x80);
    assert_eq!(m.cpu.read(lock), 0, "NVMLOCK reads zero");
}

#[test]
fn invalid_address_and_page_op_on_dfm() {
    let mut m = machine();
    let (c0, c1) = (a(&m, "NVMCON0"), a(&m, "NVMCON1"));
    setadr(&mut m, 0x380200); // 512 bytes: one past the end
    m.cpu.write(c1, 3);
    unlock_go(&mut m);
    assert_eq!(m.cpu.read(c0) & 1, 0);
    assert_eq!(m.cpu.read(c1) & 0x80, 0x80);
    m.cpu.write(c1, 0);
    setadr(&mut m, 0x380000);
    m.cpu.write(c1, 6); // page erase on the DFM
    unlock_go(&mut m);
    assert_eq!(m.cpu.read(c1) & 0x80, 0x80);
}

#[test]
fn registers_are_blocked_while_go_is_set_and_post_increment() {
    let mut m = machine();
    let (d, c0, c1) = (a(&m, "NVMDATL"), a(&m, "NVMCON0"), a(&m, "NVMCON1"));
    setadr(&mut m, 0x380010);
    m.cpu.write(d, 0x77);
    m.cpu.write(c1, 4); // write and post increment
    let t0 = m.cpu.cycles;
    unlock_go(&mut m);
    m.cpu.write(c1, 0); // blocked
    assert_eq!(m.cpu.read(c1) & 7, 4);
    run_to(&mut m, t0 + 10_100);
    assert_eq!(adr(&mut m), 0x380011);
    m.cpu.write(c1, 1); // read and post increment from 0x380011, then 0x380012
    m.cpu.write(c0, 1);
    assert_eq!(adr(&mut m), 0x380012);
    setadr(&mut m, 0x380010);
    m.cpu.write(c1, 0);
    m.cpu.write(c0, 1);
    assert_eq!(m.cpu.read(d), 0x77);
}
