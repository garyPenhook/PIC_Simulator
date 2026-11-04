//! Windowed watchdog and the resets it causes (DS40002043F ch.10 p.96-106, Table 9-3 p.89-90; PIC18F27Q10).
//! Fosc is 64 MHz (the default), so one watchdog clock (31 kHz) is 516.13 instruction cycles; WDTPS = 0 times out after 32 clocks = 16517 cycles.

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn rec(addr: u16, ty: u8, data: &[u8]) -> String {
    let mut b = vec![data.len() as u8, (addr >> 8) as u8, addr as u8, ty];
    b.extend_from_slice(data);
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

/// A machine with `prog` at address 0 and CONFIG3L = `cfg3l` (WDTE in bits 6:5, WDTCPS in bits 4:0).
fn machine(prog: &[u8], cfg3l: u8) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    let hex = rec(0, 0, prog) + &rec(0, 4, &[0x00, 0x30]) + &rec(4, 0, &[cfg3l]) + &rec(0, 1, &[]);
    m.load_hex(&hex).unwrap();
    m
}

const ON: u8 = 0x7F; // WDTE = 11, WDTCPS = 11111
const SEN: u8 = 0x3F; // WDTE = 01
const OFF: u8 = 0x1F; // WDTE = 00
const BRA_SELF: [u8; 2] = [0xFF, 0xD7];
const CLRWDT: [u8; 2] = [0x04, 0x00];
const RESET: [u8; 2] = [0xFF, 0x00];

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

#[test]
fn reset_values_follow_the_configuration() {
    let mut m = machine(&BRA_SELF, ON);
    assert_eq!(r(&mut m, "WDTCON0"), 0x16, "WDTPS = 01011 (2 s), SEN = 0");
    assert_eq!(r(&mut m, "WDTCON1"), 0x07);
}

#[test]
fn time_out_resets_the_part_and_sets_rwdt_and_to() {
    let mut m = machine(&BRA_SELF, ON);
    w(&mut m, "WDTCON0", 0x00); // 1:32 = 1 ms
    w(&mut m, "PCON0", 0xFF);
    w(&mut m, "TRISC", 0x00); // reset value 0xFF comes back with the reset
    let a = 0x50;
    m.cpu.write(a, 0xAA);
    let t0 = m.cpu.cycles;
    run_to(&mut m, t0 + 16000);
    assert_eq!(r(&mut m, "PCON0") & 0x10, 0x10, "no time-out yet");
    assert_eq!(r(&mut m, "TRISC"), 0x00);
    run_to(&mut m, t0 + 17000);
    assert_eq!(r(&mut m, "PCON0"), 0xEF, "RWDT cleared, the others unchanged");
    assert_eq!(r(&mut m, "STATUS") & 0x40, 0, "nTO");
    assert_eq!(r(&mut m, "TRISC"), 0xFF, "registers take their reset values");
    assert_eq!(m.cpu.read(a), 0xAA, "RAM is kept");
    assert_eq!(r(&mut m, "WDTCON0"), 0x16, "WDTPS back to the configured value");
}

#[test]
fn clrwdt_keeps_it_alive() {
    let mut m = machine(&[0x04, 0x00, 0xFE, 0xD7], ON); // clrwdt; bra -2
    w(&mut m, "WDTCON0", 0x00);
    w(&mut m, "PCON0", 0xFF);
    let t0 = m.cpu.cycles;
    run_to(&mut m, t0 + 100_000);
    assert_eq!(r(&mut m, "PCON0"), 0xFF);
}

#[test]
fn off_and_software_enable() {
    let mut m = machine(&BRA_SELF, OFF);
    w(&mut m, "WDTCON0", 0x01);
    run_to(&mut m, 200_000);
    assert_eq!(r(&mut m, "PCON0") & 0x10, 0x10, "WDTE = 00: SEN is ignored");
    let mut m = machine(&BRA_SELF, SEN);
    w(&mut m, "WDTCON0", 0x00); // 1 ms, SEN = 0
    run_to(&mut m, 40_000);
    assert_eq!(r(&mut m, "PCON0") & 0x10, 0x10, "off");
    w(&mut m, "WDTCON0", 0x01);
    let t = m.cpu.cycles;
    run_to(&mut m, t + 17_000);
    assert_eq!(r(&mut m, "PCON0") & 0x10, 0, "SEN = 1 turns it on");
}

#[test]
fn read_only_fields_follow_the_configuration() {
    let mut m = machine(&BRA_SELF, 0x4A); // WDTE = 10, WDTCPS = 01010
    assert_eq!(r(&mut m, "WDTCON0"), 0x14);
    w(&mut m, "WDTCON0", 0x00);
    assert_eq!(r(&mut m, "WDTCON0"), 0x14, "WDTPS is read only");
    w(&mut m, "WDTCON1", 0x73);
    assert_eq!(r(&mut m, "WDTCON1"), 0x73, "CS and WINDOW are writable with the default WDTCCS and WDTCWS");
}

#[test]
fn unarmed_clrwdt_in_windowed_mode_is_a_window_violation() {
    let mut m = machine(&[CLRWDT[0], CLRWDT[1], BRA_SELF[0], BRA_SELF[1]], ON);
    w(&mut m, "WDTCON0", 0x00);
    w(&mut m, "WDTCON1", 0x03); // window 50 %
    w(&mut m, "PCON0", 0xFF);
    m.cpu.step(); // clrwdt without reading WDTCON0 first
    assert_eq!(r(&mut m, "PCON0"), 0xDF, "WDTWV cleared only");
    assert_eq!(r(&mut m, "STATUS") & 0x40, 0x40, "TO unchanged by a window violation (Table 9-3)");
}

#[test]
fn armed_clrwdt_needs_an_open_window() {
    let mut m = machine(&[CLRWDT[0], CLRWDT[1], BRA_SELF[0], BRA_SELF[1]], ON);
    w(&mut m, "WDTCON0", 0x00);
    w(&mut m, "WDTCON1", 0x03);
    w(&mut m, "PCON0", 0xFF);
    r(&mut m, "WDTCON0"); // arm
    assert_eq!(r(&mut m, "WDTTMR") & 0x04, 0x04, "STATE");
    m.cpu.step();
    assert_eq!(r(&mut m, "PCON0"), 0xDF, "the window is closed in the first half");
    // Again, with the window open: the clear restarts the period.
    let mut m = machine(&[CLRWDT[0], CLRWDT[1], BRA_SELF[0], BRA_SELF[1]], ON);
    w(&mut m, "WDTCON0", 0x00);
    w(&mut m, "WDTCON1", 0x03);
    w(&mut m, "PCON0", 0xFF);
    m.cpu.pc = 2;
    let t0 = m.cpu.cycles;
    run_to(&mut m, t0 + 9000);
    assert!(r(&mut m, "WDTTMR") >> 3 >= 16, "WDTTMR[4:0] reached the open half");
    r(&mut m, "WDTCON0");
    m.cpu.pc = 0;
    m.cpu.step();
    assert_eq!(r(&mut m, "PCON0"), 0xFF, "valid CLRWDT");
    assert_eq!(r(&mut m, "WDTTMR") & 0x04, 0, "disarmed again");
    run_to(&mut m, t0 + 9000 + 15_000);
    assert_eq!(r(&mut m, "PCON0"), 0xFF, "the old deadline passed without a reset");
    run_to(&mut m, t0 + 9000 + 17_000);
    assert_eq!(r(&mut m, "PCON0") & 0x10, 0, "time-out after the restarted period");
}

#[test]
fn reset_instruction_clears_ri_and_restarts() {
    let mut m = machine(&[0x00, 0x00, RESET[0], RESET[1]], OFF);
    w(&mut m, "PCON0", 0xFF);
    w(&mut m, "TRISC", 0x00);
    m.cpu.step();
    assert_eq!(m.cpu.pc, 2);
    m.cpu.step(); // reset
    assert_eq!(r(&mut m, "PCON0"), 0xFB, "RI cleared");
    assert_eq!(m.cpu.pc, 0);
    assert_eq!(r(&mut m, "TRISC"), 0xFF);
}

const SLEEP: [u8; 2] = [0x03, 0x00];

/// WDTE = 3 keeps counting in Sleep; the time-out wakes the part with TO = PD = 0 and no reset, continuing at PC + 2 (DS40002043F 10.6 p.99, Table 9-3 p.90).
#[test]
fn watchdog_wakes_from_sleep() {
    let mut m = machine(&[SLEEP[0], SLEEP[1], 0x00, 0x00, BRA_SELF[0], BRA_SELF[1]], ON);
    w(&mut m, "WDTCON0", 0x00);
    w(&mut m, "PCON0", 0xFF);
    m.cpu.step(); // SLEEP
    assert_eq!(r(&mut m, "STATUS") & 0x60, 0x40, "TO = 1, PD = 0");
    assert!(!m.cpu.sleep_dead());
    let t0 = m.cpu.cycles;
    while m.cpu.state != picsim::pic18::State::Running {
        m.cpu.step();
    }
    assert!(m.cpu.cycles >= t0 + 16000 && m.cpu.cycles <= t0 + 17000, "woke at the time-out, {}", m.cpu.cycles - t0);
    assert_eq!(m.cpu.pc, 2, "PC + 2, no reset");
    assert_eq!(r(&mut m, "STATUS") & 0x60, 0, "TO = PD = 0");
    assert_eq!(r(&mut m, "PCON0"), 0xFF, "unchanged");
}

/// WDTE = 2 is off while asleep: nothing ends the Sleep, so the run stops there.
#[test]
fn watchdog_off_in_sleep_in_mode_2() {
    let mut m = machine(&[SLEEP[0], SLEEP[1]], 0x5F); // WDTE = 10
    m.cpu.step();
    assert!(m.cpu.sleep_dead());
}

/// An interrupt flag with its enable ends the Sleep (TO = 1, PD = 0); one pending already makes SLEEP a NOP.
#[test]
fn interrupt_wakes_from_sleep() {
    let mut m = machine(&[SLEEP[0], SLEEP[1], 0x00, 0x00], OFF);
    let (pie, pir) = (addr(&m, "PIE0"), addr(&m, "PIR0"));
    m.cpu.write(pie, 0x01);
    m.cpu.step();
    assert_eq!(m.cpu.state, picsim::pic18::State::Sleeping);
    assert!(!m.cpu.sleep_dead());
    m.cpu.write(pir, 0x01);
    m.cpu.step();
    assert_eq!(m.cpu.state, picsim::pic18::State::Running);
    assert_eq!(m.cpu.pc, 2);
    assert_eq!(r(&mut m, "STATUS") & 0x60, 0x40);

    let mut m = machine(&[SLEEP[0], SLEEP[1], 0x00, 0x00], OFF);
    m.cpu.write(pie, 0x01);
    m.cpu.write(pir, 0x01);
    m.cpu.step();
    assert_eq!(m.cpu.state, picsim::pic18::State::Running, "SLEEP executes as a NOP");
}
