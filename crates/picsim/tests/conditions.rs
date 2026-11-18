//! Conditional breakpoints on both cores: a data condition (edge-triggered) and a conditional code breakpoint with a hit count.
use devdesc::Device;
use picsim::debugger::{Cmp, Condition};
use picsim::machine16::Machine16;
use picsim::{Machine, Stop};
use std::path::Path;

fn dev(part: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap()
}

/// Intel hex for `words` at byte address 0 (little endian).
fn hex(words: &[u16]) -> String {
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut rec = vec![bytes.len() as u8, 0, 0, 0];
    rec.extend(&bytes);
    let sum = rec.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    format!(":{}{:02X}\n:00000001FF\n", rec.iter().map(|b| format!("{b:02X}")).collect::<String>(), sum.wrapping_neg())
}

fn cond(name: &str, addr: u32, cmp: Cmp, value: u32) -> Condition {
    Condition { name: name.into(), addr, wide: false, cmp, value, at: None, after: 1, hits: 0 }
}

/// PIC18: `0: INCF 0x20,F,A   2: BRA 0` counts forever in 0x20.
fn counter18() -> Machine {
    let mut m = Machine::new(dev("18F27Q10")).unwrap();
    m.load_hex(&hex(&[0x2A20, 0xD7FE])).unwrap();
    m
}

/// PIC16: `0: INCF 0x20,F   1: BRA 0` counts forever in 0x20.
fn counter16() -> Machine16 {
    let mut m = Machine16::new(dev("16F17146")).unwrap();
    m.load_hex(&hex(&[0x0AA0, 0x33FE])).unwrap();
    m
}

#[test]
fn pic18_data_condition_stops_when_the_counter_reaches_the_value() {
    let mut m = counter18();
    m.debugger.conditions.push(cond("COUNT", 0x20, Cmp::Eq, 5));
    let stop = m.run(None, 10_000);
    assert_eq!(stop, Stop::Condition { index: 0, value: 5 });
    assert_eq!(m.cpu.peek(0x20), 5, "stopped right after the instruction that made it 5");
    // Resuming does not stop again while it stays at 5 (it moves on at once), and the next 5 is 256 counts away.
    let stop = m.run(None, 10_000);
    assert_eq!(stop, Stop::Condition { index: 0, value: 5 });
    assert!(m.cpu.cycles > 256, "the second stop is one wrap later");
}

#[test]
fn pic18_bit_condition_and_greater_than() {
    let mut m = counter18();
    m.debugger.conditions.push(cond("COUNT", 0x20, Cmp::BitSet(3), 0));
    assert_eq!(m.run(None, 10_000), Stop::Condition { index: 0, value: 8 });
    let mut m = counter18();
    m.debugger.conditions.push(cond("COUNT", 0x20, Cmp::Gt, 9));
    assert_eq!(m.run(None, 10_000), Stop::Condition { index: 0, value: 10 });
}

#[test]
fn pic18_code_condition_stops_before_the_instruction_and_counts_hits() {
    let mut m = counter18();
    // At the BRA (address 2), once the counter is at least 3, on the 2nd hit: the counter is 4 when the BRA is about to run.
    m.debugger.conditions.push(Condition { at: Some(2), after: 2, ..cond("COUNT", 0x20, Cmp::Ge, 3) });
    let stop = m.run(None, 10_000);
    assert_eq!(stop, Stop::Condition { index: 0, value: 4 });
    assert_eq!(m.cpu.pc, 2, "before the instruction at the address");
    // A power-on style reset of the debugger starts the hit count again.
    m.debugger.reset();
    assert_eq!(m.debugger.conditions[0].hits, 0);
}

#[test]
fn a_condition_that_never_holds_does_not_stop_the_run() {
    let mut m = counter18();
    m.debugger.conditions.push(cond("COUNT", 0x20, Cmp::Eq, 300));
    assert_eq!(m.run(None, 5_000), Stop::Cycles);
}

#[test]
fn conditions_also_stop_a_source_line_step() {
    let mut m = counter18();
    m.debugger.conditions.push(cond("COUNT", 0x20, Cmp::Eq, 2));
    let s = m.begin_line_step(false, false);
    // No line information, so the step runs until something stops it: the condition.
    let r = (0..100).find_map(|_| m.line_step(&s, 50).flatten());
    assert_eq!(r, Some(Stop::Condition { index: 0, value: 2 }));
}

#[test]
fn pic16_data_and_code_conditions() {
    let mut m = counter16();
    m.debugger.conditions.push(cond("COUNT", 0x20, Cmp::Eq, 7));
    assert_eq!(m.run(None, 10_000), Stop::Condition { index: 0, value: 7 });
    let mut m = counter16();
    m.debugger.conditions.push(Condition { at: Some(1), after: 1, ..cond("COUNT", 0x20, Cmp::Ge, 4) });
    assert_eq!(m.run(None, 10_000), Stop::Condition { index: 0, value: 4 });
    assert_eq!(m.cpu.pc, 1);
}
