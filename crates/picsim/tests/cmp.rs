//! Comparators C1/C2 (DS40002043F ch.34 p.589-600; PIC18F27Q10) with `[[analog]]` stimulus.

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

fn run_for(m: &mut Machine, n: u64) {
    let c = m.cpu.cycles + n;
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

const STIM: &str = "[[analog]]\npin = \"RA2\"\nvolts = 2.0\n[[analog]]\npin = \"RA0\"\nvolts = 1.0\n[[analog]]\npin = \"RA0\"\nvolts = 3.0\nat = 100\n";

#[test]
fn output_follows_the_inputs_and_polarity() {
    let mut m = machine(STIM);
    w(&mut m, "CM1CON0", 0x80); // C1IN0+ (RA2) vs C1IN0- (RA0)
    assert_eq!(r(&mut m, "CM1CON0") & 0x40, 0x40, "2.0 V > 1.0 V");
    assert_eq!(r(&mut m, "CMOUT"), 1);
    run_for(&mut m, 120);
    assert_eq!(r(&mut m, "CM1CON0") & 0x40, 0, "1 V input rose to 3.0 V");
    assert_eq!(r(&mut m, "CMOUT"), 0);
    w(&mut m, "CM1CON0", 0x90); // POL
    assert_eq!(r(&mut m, "CM1CON0") & 0x40, 0x40);
    w(&mut m, "CM1CON0", 0x00); // disabled: the comparator makes 0
    assert_eq!(r(&mut m, "CM1CON0") & 0x40, 0);
}

#[test]
fn out_and_cmout_are_read_only() {
    let mut m = machine(STIM);
    w(&mut m, "CM1CON0", 0xC0);
    assert_eq!(r(&mut m, "CM1CON0") & 0x40, 0x40, "EN set, OUT written 1 but follows the comparator");
    w(&mut m, "CMOUT", 3);
    assert_eq!(r(&mut m, "CMOUT"), 1);
}

#[test]
fn flag_follows_intp_and_intn_edges() {
    let mut m = machine(STIM);
    w(&mut m, "CM1CON1", 0x01); // INTN only
    w(&mut m, "CM1CON0", 0x80); // output rises: INTP clear, no flag
    assert_eq!(r(&mut m, "PIR2") & 0x01, 0);
    run_for(&mut m, 120); // falls at cycle 100
    assert_eq!(r(&mut m, "PIR2") & 0x01, 0x01, "C1IF on the falling edge");
    let mut m = machine(STIM);
    w(&mut m, "CM1CON1", 0x02); // INTP only
    w(&mut m, "CM1CON0", 0x80);
    assert_eq!(r(&mut m, "PIR2") & 0x01, 0x01, "enabling raises the output");
}

#[test]
fn changing_pol_while_disabled_can_raise_the_flag() {
    let mut m = machine(STIM);
    w(&mut m, "CM1CON1", 0x02);
    w(&mut m, "CM1CON0", 0x10); // EN = 0, POL = 1: the disabled comparator's 0 is inverted
    assert_eq!(r(&mut m, "PIR2") & 0x01, 0x01);
}

#[test]
fn fvr_buffer_and_avss_selects() {
    let mut m = machine("[[analog]]\npin = \"RA2\"\nvolts = 1.5\n[[analog]]\npin = \"RA2\"\nvolts = 0.5\nat = 100\n");
    w(&mut m, "FVRCON", 0x84); // FVR on, comparator buffer 1x = 1.024 V
    w(&mut m, "CM1NCH", 6);
    w(&mut m, "CM1CON0", 0x80);
    assert_eq!(r(&mut m, "CM1CON0") & 0x40, 0x40, "1.5 V > 1.024 V");
    run_for(&mut m, 120);
    assert_eq!(r(&mut m, "CM1CON0") & 0x40, 0, "0.5 V < 1.024 V");
    w(&mut m, "CM1NCH", 7); // AVSS
    assert_eq!(r(&mut m, "CM1CON0") & 0x40, 0x40, "0.5 V > 0 V");
    w(&mut m, "FVRCON", 0x88); // 2x = 2.048 V
    w(&mut m, "CM1NCH", 6);
    assert_eq!(r(&mut m, "CM1CON0") & 0x40, 0);
}

#[test]
fn c2_uses_rb0_as_its_second_positive_input() {
    let mut m = machine("[[analog]]\npin = \"RB0\"\nvolts = 2.0\n");
    w(&mut m, "CM2PCH", 1);
    w(&mut m, "CM2NCH", 7);
    w(&mut m, "CM2CON0", 0x80);
    assert_eq!(r(&mut m, "CM2CON0") & 0x40, 0x40);
    assert_eq!(r(&mut m, "CMOUT"), 2);
}

#[test]
fn output_drives_a_pin_through_pps_and_logs_the_edges() {
    let mut m = machine(STIM);
    w(&mut m, "RC0PPS", 0x0D); // CMP1
    w(&mut m, "TRISC", 0xFE);
    w(&mut m, "CM1CON0", 0x80);
    run_for(&mut m, 150);
    let log: Vec<_> = m.cpu.p.pin_log.iter().filter(|e| e.1 == b'C' && e.2 == 0).map(|e| (e.0, e.3)).collect();
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(log[0].1 && !log[1].1);
    assert_eq!(log[1].0, 400, "falls in cycle 100");
}

#[test]
fn comparator_is_a_cwg_input() {
    let mut m = machine(STIM);
    w(&mut m, "RC1PPS", 0x01); // CWG1A
    w(&mut m, "TRISC", 0xFD);
    w(&mut m, "CWG1ISM", 5); // CMP1
    w(&mut m, "CWG1STR", 0x01); // steering, A follows
    w(&mut m, "CM1CON0", 0x80);
    w(&mut m, "CWG1CON0", 0x80);
    run_for(&mut m, 150);
    let a: Vec<_> = m.cpu.p.pin_log.iter().filter(|e| e.1 == b'C' && e.2 == 1).map(|e| e.3).collect();
    assert_eq!(a.last(), Some(&false), "{a:?}");
    assert!(a.contains(&true), "{a:?}");
}

#[test]
fn comparator_low_shuts_the_cwg_down() {
    let mut m = machine(STIM);
    w(&mut m, "CWG1AS0", 0x14); // LSBD, LSAC = tri-state default
    w(&mut m, "CWG1AS0", 0x0C); // LSAC = 11: outputs high on shutdown
    w(&mut m, "CWG1AS1", 0x10); // AS4E: C1 low shuts down
    w(&mut m, "CM1CON0", 0x80); // C1 high: no shutdown yet
    w(&mut m, "CWG1CON0", 0x80);
    assert_eq!(r(&mut m, "CWG1AS0") & 0x80, 0);
    run_for(&mut m, 120); // C1 falls at cycle 100
    assert_eq!(r(&mut m, "CWG1AS0") & 0x80, 0x80);
    assert_eq!(r(&mut m, "PIR7") & 0x01, 0x01, "CWG1IF");
}

#[test]
fn bad_analog_stimulus_is_refused() {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    assert!(m.load_stimulus("[[analog]]\npin = \"RA0\"\nvolts = 9\n").is_err());
    assert!(m.load_stimulus("[[analog]]\npin = \"RA0\"\n").is_err());
    assert!(m.load_stimulus("[[analog]]\npin = \"RZ0\"\nvolts = 1\n").is_err());
}
