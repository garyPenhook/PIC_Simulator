//! Stack overflow and underflow (DS40002043F 11.1.2.2-11.1.2.3 p.111, Table 9-3 p.90; PIC18F27Q10).
//! STKOVF and STKUNF are PCON0 bits 7 and 6; STVREN is bit 4 of CONFIG2H (0x300003).

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn rec(addr: u16, ty: u8, data: &[u8]) -> String {
    let mut b = vec![data.len() as u8, (addr >> 8) as u8, addr as u8, ty];
    b.extend_from_slice(data);
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

/// `prog` at 0; CONFIG2H = `cfg2h` (STVREN is bit 4).
fn machine(prog: &[u8], cfg2h: u8) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    let hex = rec(0, 0, prog) + &rec(0, 4, &[0x00, 0x30]) + &rec(3, 0, &[cfg2h]) + &rec(0, 1, &[]);
    m.load_hex(&hex).unwrap();
    m
}

const CALL_SELF: [u8; 4] = [0x00, 0xEC, 0x00, 0xF0]; // CALL 0
const RETURN: [u8; 2] = [0x12, 0x00];
const ON: u8 = 0xFF;
const OFF: u8 = 0xEF;

fn r(m: &mut Machine, n: &str) -> u8 {
    let a = m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr;
    m.cpu.read(a)
}

#[test]
fn thirty_one_calls_fit_the_thirty_second_resets() {
    let mut m = machine(&CALL_SELF, ON);
    for _ in 0..31 {
        m.cpu.step();
    }
    assert_eq!(r(&mut m, "STKPTR"), 31);
    assert_eq!(r(&mut m, "PCON0") & 0x80, 0);
    m.cpu.step();
    assert_eq!(r(&mut m, "PCON0") & 0x80, 0x80, "STKOVF");
    assert_eq!(r(&mut m, "STKPTR"), 0);
    assert_eq!(m.cpu.pc, 0);
    // the flag stays set through the next reset-vector run
    m.cpu.step();
    assert_eq!(r(&mut m, "PCON0") & 0x80, 0x80);
}

#[test]
fn overflow_without_stvren_sets_the_flag_and_holds_the_pointer() {
    let mut m = machine(&CALL_SELF, OFF);
    for _ in 0..40 {
        m.cpu.step();
    }
    assert_eq!(r(&mut m, "PCON0") & 0x80, 0x80);
    assert_eq!(r(&mut m, "STKPTR"), 31);
}

#[test]
fn underflow_returns_zero_and_resets() {
    let mut m = machine(&RETURN, ON);
    m.cpu.step();
    assert_eq!(r(&mut m, "PCON0") & 0x40, 0x40, "STKUNF");
    assert_eq!(r(&mut m, "PCON0") & 0x80, 0);
    assert_eq!(r(&mut m, "STKPTR"), 0);
    assert_eq!(m.cpu.pc, 0);
}

#[test]
fn underflow_without_stvren_sets_the_flag_only() {
    // NOP, RETURN: PC goes to 0 by the pop, no reset: SFRs keep their values (WREG = 5)
    let mut m = machine(&[0x05, 0x0E, 0x12, 0x00], OFF); // MOVLW 5; RETURN
    m.cpu.step();
    m.cpu.step();
    assert_eq!(r(&mut m, "PCON0") & 0x40, 0x40);
    assert_eq!(m.cpu.pc, 0);
    assert_eq!(r(&mut m, "WREG"), 5, "not a reset");
}

#[test]
fn the_flag_stays_set_through_the_next_underflow() {
    let mut m = machine(&RETURN, ON);
    m.cpu.step();
    assert_eq!(r(&mut m, "PCON0") & 0x40, 0x40);
    m.cpu.step(); // RETURN again, the flag stays
    assert_eq!(r(&mut m, "PCON0") & 0x40, 0x40);
}

#[test]
fn a_push_onto_a_full_stack_overwrites_the_top() {
    // 31 CALLs from 0x0000 .. then the 32nd push (PC after the CALL) replaces the top entry
    let mut m = machine(&CALL_SELF, OFF);
    for _ in 0..32 {
        m.cpu.step();
    }
    assert_eq!(r(&mut m, "STKPTR"), 31);
    assert_eq!(r(&mut m, "TOSL"), 4, "return address of CALL 0 at 0 is 4");
}
