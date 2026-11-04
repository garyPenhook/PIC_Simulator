//! The MCLR pin as a reset input (DS40002043F 9.4 p.87-88, 9.10 p.88, Table 9-3 p.89, CONFIG2 p.32; PIC18F27Q10).
//! MCLRE is bit 0 of CONFIG2L (0x300002), LVP is bit 5 of CONFIG4H (0x300007).

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn rec(addr: u16, ty: u8, data: &[u8]) -> String {
    let mut b = vec![data.len() as u8, (addr >> 8) as u8, addr as u8, ty];
    b.extend_from_slice(data);
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

/// MOVLW 5; then BRA self at 2. `cfg2l`, `cfg4h` are the configuration bytes.
fn machine(cfg2l: u8, cfg4h: u8, stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    let hex = rec(0, 0, &[0x05, 0x0E, 0xFF, 0xD7]) + &rec(0, 4, &[0x00, 0x30]) + &rec(2, 0, &[cfg2l]) + &rec(7, 0, &[cfg4h]) + &rec(0, 1, &[]);
    m.load_hex(&hex).unwrap();
    m.load_stimulus(stim).unwrap();
    m
}

fn r(m: &mut Machine, n: &str) -> u8 {
    let a = m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr;
    m.cpu.read(a)
}

fn run_to(m: &mut Machine, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

const PULSE: &str = "[[mclr]]\nlevel = 0\nat = 100\n[[mclr]]\nlevel = 1\nat = 200\n";

#[test]
fn a_low_pulse_resets_and_clears_rmclr() {
    let mut m = machine(0xFF, 0xFF, PULSE);
    run_to(&mut m, 50);
    assert_eq!(r(&mut m, "WREG"), 5);
    assert_eq!(r(&mut m, "PCON0") & 0x08, 0x08);
    assert_eq!(m.cpu.pc, 2);
    run_to(&mut m, 100);
    assert_eq!(r(&mut m, "PCON0") & 0x08, 0, "RMCLR");
    assert_eq!(m.cpu.pc, 0);
}

#[test]
fn the_part_is_held_while_the_pin_is_low_then_runs_again() {
    let mut m = machine(0xFF, 0xFF, PULSE);
    run_to(&mut m, 100);
    assert_eq!(m.cpu.pc, 0);
    run_to(&mut m, 199);
    assert_eq!(m.cpu.pc, 0, "held in reset");
    run_to(&mut m, 210);
    assert_eq!(r(&mut m, "WREG"), 5, "MOVLW ran after the release");
}

#[test]
fn with_mclr_disabled_the_pin_does_nothing() {
    // LVP = 0 (bit 5 clear) and MCLRE = 0
    let mut m = machine(0xFE, 0xDF, PULSE);
    run_to(&mut m, 300);
    assert_eq!(r(&mut m, "PCON0") & 0x08, 0x08);
    assert_eq!(m.cpu.pc, 2);
}

#[test]
fn lvp_makes_the_pin_mclr_whatever_mclre_says() {
    let mut m = machine(0xFE, 0xFF, PULSE);
    run_to(&mut m, 120);
    assert_eq!(r(&mut m, "PCON0") & 0x08, 0);
}

#[test]
fn mclr_wakes_the_part_from_sleep_with_to_pd_as_in_sleep() {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    let hex = rec(0, 0, &[0x03, 0x00, 0xFF, 0xD7]) + &rec(0, 4, &[0x00, 0x30]) + &rec(7, 0, &[0xFF]) + &rec(0, 1, &[]);
    m.load_hex(&hex).unwrap();
    m.load_stimulus("[[mclr]]\nlevel = 0\nat = 1000\n[[mclr]]\nlevel = 1\nat = 1100\n").unwrap();
    m.cpu.step(); // SLEEP
    m.cpu.step();
    run_to(&mut m, 1050);
    assert_eq!(m.cpu.pc, 0);
    assert_eq!(r(&mut m, "PCON0") & 0x08, 0);
    assert_eq!(r(&mut m, "STATUS") & 0x60, 0x40, "nTO = 1, nPD = 0 stay as SLEEP left them (Table 9-3: -10u)");
}

#[test]
fn bad_stimulus_is_an_error() {
    let mut m = machine(0xFF, 0xFF, "");
    assert!(m.load_stimulus("[[mclr]]\nlevel = 2\n").is_err());
    assert!(m.load_stimulus("[[mclr]]\nlevel = 0\nfoo = 1\n").is_err());
}
