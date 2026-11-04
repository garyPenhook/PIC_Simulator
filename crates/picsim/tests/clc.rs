//! Configurable logic cells (DS40002043F ch.26 p.406-426; PIC18F27Q10). Inputs are RB0/RB1 through CLCIN0PPS/CLCIN1PPS, driven by the
//! stimulus; the data selection is D1S = 0 (CLCIN0), D2S = 1 (CLCIN1), D3S = 33 (CLC1_out).

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn machine(stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_stimulus(stim).unwrap();
    m
}

fn addr(m: &Machine, n: &str) -> u32 {
    m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr
}

fn w(m: &mut Machine, n: &str, v: u8) {
    let a = addr(m, n);
    m.cpu.write(a, v);
}

fn r(m: &mut Machine, n: &str) -> u8 {
    let a = addr(m, n);
    m.cpu.read(a)
}

fn run_to(m: &mut Machine, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

/// Value of the PIR bit `bit`.
fn flag(m: &mut Machine, bit: &str) -> bool {
    let (a, mask) = m
        .dev
        .sfr
        .iter()
        .find_map(|s| s.fields.iter().chain(s.legacy.iter()).find(|f| f.name == bit).map(|f| (s.addr, 1u8 << f.pos)))
        .unwrap_or_else(|| panic!("no {bit}"));
    m.cpu.read(a) & mask != 0
}

/// RB0 high from cycle 100 to 200 and from 300 on; RB1 high from cycle 150 to 250.
const STIM: &str = "[[pin]]\nport = \"B\"\nbit = 0\nlevel = 1\nat = 100\n[[pin]]\nport = \"B\"\nbit = 0\nlevel = 0\nat = 200\n[[pin]]\nport = \"B\"\nbit = 0\nlevel = 1\nat = 300\n[[pin]]\nport = \"B\"\nbit = 1\nlevel = 1\nat = 150\n[[pin]]\nport = \"B\"\nbit = 1\nlevel = 0\nat = 250\n";

/// CLC1 with D1 = RB0, D2 = RB1; gate 1 takes D1 true, gate 2 takes D2 true, gates 3 and 4 are constants set by `pol` (bits 3:2).
fn setup(m: &mut Machine, mode: u8, pol: u8) {
    w(m, "ANSELB", 0);
    w(m, "CLCIN0PPS", 0x08); // RB0
    w(m, "CLCIN1PPS", 0x09); // RB1
    w(m, "CLC1SEL0", 0);
    w(m, "CLC1SEL1", 1);
    w(m, "CLC1GLS0", 0x02); // G1D1T
    w(m, "CLC1GLS1", 0x08); // G2D2T
    w(m, "CLC1GLS2", 0);
    w(m, "CLC1GLS3", 0);
    w(m, "CLC1POL", pol);
    w(m, "CLC1CON", 0x80 | mode);
}

fn out(m: &mut Machine) -> bool {
    r(m, "CLC1CON") & 0x20 != 0
}

#[test]
fn four_input_and_follows_the_pins() {
    let mut m = machine(STIM);
    setup(&mut m, 2, 0x0C); // g3 = g4 = 1 (empty gates with polarity 1, Table 26-1)
    run_to(&mut m, 50);
    assert!(!out(&mut m));
    run_to(&mut m, 120);
    assert!(!out(&mut m), "RB0 only");
    run_to(&mut m, 170);
    assert!(out(&mut m), "both high");
    assert_eq!(r(&mut m, "CLCDATA") & 1, 1);
    run_to(&mut m, 210);
    assert!(!out(&mut m), "RB0 fell");
    assert_eq!(r(&mut m, "CLCDATA") & 1, 0);
}

#[test]
fn polarity_and_disable() {
    let mut m = machine(STIM);
    setup(&mut m, 2, 0x8C); // POL
    run_to(&mut m, 50);
    assert!(out(&mut m), "AND is 0, inverted");
    w(&mut m, "CLC1CON", 0x02); // EN = 0
    assert!(!out(&mut m), "a disabled cell has a zero output");
}

#[test]
fn and_or_and_or_xor() {
    let mut m = machine(STIM);
    setup(&mut m, 0, 0x00); // (g1 & g2) | (g3 & g4) with g3 = g4 = 0
    run_to(&mut m, 170);
    assert!(out(&mut m));
    run_to(&mut m, 210);
    assert!(!out(&mut m));
    let mut m = machine(STIM);
    setup(&mut m, 1, 0x00); // (g1 | g2) ^ (g3 | g4)
    run_to(&mut m, 120);
    assert!(out(&mut m), "RB0 only");
    run_to(&mut m, 170);
    assert!(out(&mut m), "both: OR is 1");
    run_to(&mut m, 270);
    assert!(!out(&mut m), "neither");
}

#[test]
fn d_flip_flop_takes_d_on_the_rising_clock() {
    // Mode 100: D = g2 (RB1), clock = g1 (RB0), S = g4, R = g3, both constant 0.
    let mut m = machine(STIM);
    setup(&mut m, 4, 0x00);
    run_to(&mut m, 120);
    assert!(!out(&mut m), "clock rose while RB1 was low");
    run_to(&mut m, 270);
    assert!(!out(&mut m), "no rising clock since");
    run_to(&mut m, 320);
    assert!(!out(&mut m), "RB1 low again at the 300 rising edge");
    // A clock edge while RB1 is high: RB0 rises at 100 with RB1 high from 90.
    let stim = "[[pin]]\nport = \"B\"\nbit = 1\nlevel = 1\nat = 90\n[[pin]]\nport = \"B\"\nbit = 0\nlevel = 1\nat = 100\n[[pin]]\nport = \"B\"\nbit = 1\nlevel = 0\nat = 150\n";
    let mut m = machine(stim);
    setup(&mut m, 4, 0x00);
    run_to(&mut m, 120);
    assert!(out(&mut m), "D = 1 captured");
    run_to(&mut m, 170);
    assert!(out(&mut m), "D fell but no clock edge");
    w(&mut m, "CLC1POL", 0x04); // g3 = 1 resets
    run_to(&mut m, 180);
    assert!(!out(&mut m), "R");
}

#[test]
fn interrupt_flag_follows_intp_and_intn() {
    let mut m = machine(STIM);
    setup(&mut m, 2, 0x0C);
    w(&mut m, "CLC1CON", 0x80 | 0x10 | 2); // INTP
    run_to(&mut m, 140);
    assert!(!flag(&mut m, "CLC1IF"));
    run_to(&mut m, 170);
    assert!(flag(&mut m, "CLC1IF"), "rising edge at 150");
    let mut m = machine(STIM);
    setup(&mut m, 2, 0x0C);
    w(&mut m, "CLC1CON", 0x80 | 0x08 | 2); // INTN
    run_to(&mut m, 170);
    assert!(!flag(&mut m, "CLC1IF"));
    run_to(&mut m, 210);
    assert!(flag(&mut m, "CLC1IF"), "falling edge at 200");
}

#[test]
fn output_drives_a_pin_through_pps() {
    let mut m = machine(STIM);
    w(&mut m, "RC0PPS", 0x18); // CLC1OUT
    w(&mut m, "TRISC", 0xFE);
    setup(&mut m, 2, 0x0C);
    run_to(&mut m, 300);
    let log: Vec<_> = m.cpu.p.pin_log.iter().filter(|e| e.1 == b'C' && e.2 == 0).map(|e| (e.0, e.3)).collect();
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(log[0].1 && !log[1].1);
    assert_eq!(log[0].0, 4 * 150);
    assert_eq!(log[1].0, 4 * 200);
}

#[test]
fn one_cell_feeds_another() {
    let mut m = machine(STIM);
    setup(&mut m, 2, 0x0C);
    // CLC2: D1 = CLC1_out, gate 1 takes it inverted (D1N), others constant 0 -> g1 = !CLC1_out, mode AND needs all 1: use AND-OR (g1 & g2) | (g3 & g4).
    w(&mut m, "CLC2SEL0", 33);
    w(&mut m, "CLC2GLS0", 0x01);
    w(&mut m, "CLC2GLS1", 0);
    w(&mut m, "CLC2GLS2", 0);
    w(&mut m, "CLC2GLS3", 0);
    w(&mut m, "CLC2POL", 0x02); // g2 = 1
    w(&mut m, "CLC2CON", 0x80); // AND-OR: g1 & g2
    run_to(&mut m, 120);
    assert_eq!(r(&mut m, "CLCDATA") & 3, 2, "CLC1 low, CLC2 high");
    run_to(&mut m, 170);
    assert_eq!(r(&mut m, "CLCDATA") & 3, 1, "CLC1 high, CLC2 low");
}
