//! Interrupt-on-change and the INT0-INT2 pins, awake and in Sleep (DS40002043F ch.17 p.270-280, 15.8 p.195, 15.13.1 p.198; PIC18F27Q10).
//! The pin RB0 is driven by `[[pin]]` entries of the stimulus.

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn rec(addr: u16, ty: u8, data: &[u8]) -> String {
    let mut b = vec![data.len() as u8, (addr >> 8) as u8, addr as u8, ty];
    b.extend_from_slice(data);
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

fn machine(prog: &[u8], stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_hex(&(rec(0, 0, prog) + &rec(0, 4, &[0x00, 0x30]) + &rec(0, 1, &[]))).unwrap();
    m.load_stimulus(stim).unwrap();
    m.cpu.write(m.dev.find_sfr("ANSELB").unwrap().addr, 0); // digital inputs
    m
}

fn r(m: &mut Machine, n: &str) -> u8 {
    let a = m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr;
    m.cpu.read(a)
}

fn w(m: &mut Machine, n: &str, v: u8) {
    let a = m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr;
    m.cpu.write(a, v);
}

fn run_to(m: &mut Machine, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

const BRA_SELF: [u8; 2] = [0xFF, 0xD7];
const SLEEP_BRA: [u8; 4] = [0x03, 0x00, 0xFF, 0xD7];
const UP_DOWN: &str = "[[pin]]\nport = \"B\"\nbit = 0\nlevel = 1\nat = 100\n[[pin]]\nport = \"B\"\nbit = 0\nlevel = 0\nat = 200\n";

#[test]
fn positive_edge_sets_the_flag_and_iocif_until_cleared() {
    let mut m = machine(&BRA_SELF, UP_DOWN);
    w(&mut m, "IOCBP", 0x01);
    run_to(&mut m, 50);
    assert_eq!(r(&mut m, "IOCBF"), 0);
    run_to(&mut m, 150);
    assert_eq!(r(&mut m, "IOCBF"), 0x01);
    assert_eq!(r(&mut m, "PIR0") & 0x10, 0x10, "IOCIF");
    run_to(&mut m, 300); // the falling edge is not enabled
    assert_eq!(r(&mut m, "IOCBF"), 0x01);
    w(&mut m, "IOCBF", 0);
    m.cpu.step();
    assert_eq!(r(&mut m, "PIR0") & 0x10, 0, "IOCIF follows IOCBF");
}

#[test]
fn negative_edge_only() {
    let mut m = machine(&BRA_SELF, UP_DOWN);
    w(&mut m, "IOCBN", 0x01);
    run_to(&mut m, 150);
    assert_eq!(r(&mut m, "IOCBF"), 0, "rising edge not enabled");
    run_to(&mut m, 250);
    assert_eq!(r(&mut m, "IOCBF"), 0x01);
}

#[test]
fn an_edge_with_no_enable_is_ignored() {
    let mut m = machine(&BRA_SELF, UP_DOWN);
    run_to(&mut m, 300);
    assert_eq!(r(&mut m, "IOCBF"), 0);
    assert_eq!(r(&mut m, "PIR0") & 0x10, 0);
}

#[test]
fn int0_follows_the_edge_select() {
    let mut m = machine(&BRA_SELF, UP_DOWN);
    let ic = r(&mut m, "INTCON");
    w(&mut m, "INTCON", ic | 0x01); // INT0EDG = 1: rising
    run_to(&mut m, 150);
    assert_eq!(r(&mut m, "PIR0") & 0x01, 0x01, "INT0IF");
    let mut m = machine(&BRA_SELF, UP_DOWN);
    let ic = r(&mut m, "INTCON");
    w(&mut m, "INTCON", ic & !0x01); // falling
    run_to(&mut m, 150);
    assert_eq!(r(&mut m, "PIR0") & 0x01, 0);
    run_to(&mut m, 250);
    assert_eq!(r(&mut m, "PIR0") & 0x01, 0x01);
}

#[test]
fn ioc_wakes_the_part_from_sleep_when_iocie_is_set() {
    let mut m = machine(&SLEEP_BRA, UP_DOWN);
    w(&mut m, "IOCBP", 0x01);
    w(&mut m, "PIE0", 0x10);
    m.cpu.step(); // SLEEP
    run_to(&mut m, 100);
    assert_eq!(r(&mut m, "IOCBF"), 0x01, "updated before the first instruction after the wake-up");
    m.cpu.step();
    assert!(m.cpu.cycles < 110, "woke at the edge, cycle {}", m.cpu.cycles);
}

#[test]
fn ioc_without_iocie_does_not_wake() {
    let mut m = machine(&SLEEP_BRA, UP_DOWN);
    w(&mut m, "IOCBP", 0x01);
    m.cpu.step();
    for _ in 0..50 {
        m.cpu.step();
    }
    assert_eq!(r(&mut m, "IOCBF"), 0x01, "the flag is set in Sleep");
    assert_eq!(m.cpu.pc, 2, "still asleep (SLEEP executed, PC at the next instruction)");
    assert!(r(&mut m, "STATUS") & 0x20 == 0, "PD = 0: asleep");
}

#[test]
fn int0_wakes_the_part_from_sleep() {
    let mut m = machine(&SLEEP_BRA, UP_DOWN);
    let ic = r(&mut m, "INTCON");
    w(&mut m, "INTCON", ic | 0x01);
    w(&mut m, "PIE0", 0x01);
    m.cpu.step();
    run_to(&mut m, 100);
    assert_eq!(r(&mut m, "PIR0") & 0x01, 0x01);
    m.cpu.step();
    assert!(m.cpu.cycles < 110, "woke at the edge, cycle {}", m.cpu.cycles);
}
