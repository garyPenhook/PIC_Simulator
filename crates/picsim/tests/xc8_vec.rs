//! Vectored interrupts (`MVECEN = 1`) on the PIC18F16Q41 with a program built by MPLAB XC8
//! (`xc8-cc -mcpu=18F16Q41 -mdfp=PIC18F-Q_DFP/1.31.492/xc8 -g -O0 xc8_vec_q41.c -o xc8_vec_q41.elf`): the vector table at IVTBASE,
//! the service routine address (entry << 2), the vector number in WREG, IVTAD, INTCON1.STAT, priorities and a bad vector.

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

fn machine() -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F16Q41.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_hex(&std::fs::read_to_string(format!("{DATA}/xc8_vec_q41.hex")).unwrap()).unwrap();
    m.load_elf(Path::new(&format!("{DATA}/xc8_vec_q41.elf"))).unwrap();
    m
}

fn sfr(m: &Machine, n: &str) -> u8 {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F16Q41.toml")).unwrap();
    m.cpu.peek(dev.find_sfr(n).unwrap().addr as u32)
}

fn var(m: &Machine, n: &str) -> u8 {
    let v = m.debug.vars.iter().find(|v| v.name == n).unwrap_or_else(|| panic!("variable {n}"));
    m.cpu.peek(v.addr)
}

#[test]
fn both_routines_run_through_the_vector_table_without_a_note() {
    let mut m = machine();
    m.run(None, 200_000);
    assert!(var(&m, "t0n") > 2, "Timer0 ISR entered through its table entry: {}", var(&m, "t0n"));
    assert!(var(&m, "t2n") > 2, "Timer2 ISR entered: {}", var(&m, "t2n"));
    assert!(m.cpu.notes.iter().all(|n| !n.contains("MVECEN")), "{:?}", m.cpu.notes);
}

#[test]
fn w_holds_the_vector_number_ivtad_the_vector_address_and_stat_the_level() {
    let mut m = machine();
    let isr = m.debug.funcs.iter().find(|f| f.name == "t2isr").unwrap().low;
    // Step until the PC is the Timer2 routine.
    let mut guard = 0;
    while m.cpu.pc != isr && guard < 400_000 {
        m.cpu.step();
        guard += 1;
    }
    assert_eq!(m.cpu.pc, isr, "the Timer2 routine is reached");
    // Vector number of TMR2 = 0x1B; IVTAD = IVTBASE + 2 * 0x1B = 0x08 + 0x36.
    assert_eq!(sfr(&m, "WREG"), 0x1B, "the vector number is in WREG on entry");
    assert_eq!(sfr(&m, "IVTADL"), 0x3E);
    assert_eq!(sfr(&m, "INTCON1") >> 6, 0b10, "STAT: in a high-priority routine");
}

#[test]
fn a_vector_outside_the_program_memory_resets_with_nmemv_clear() {
    let mut m = machine();
    // Point the Timer2 vector entry at 0xFFFF (<< 2 is beyond 64 KB).
    let entry = 0x08 + 2 * 0x1B;
    m.cpu.prog[entry] = 0xFF;
    m.cpu.prog[entry + 1] = 0xFF;
    let pcon1 = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F16Q41.toml")).unwrap().find_sfr("PCON1").unwrap().addr as u32;
    m.cpu.m.mem[pcon1 as usize] = 0x02; // nMEMV set: no violation yet
    m.run(None, 100_000);
    assert!(m.cpu.notes.iter().any(|n| n.contains("no valid vector")), "{:?}", m.cpu.notes);
    assert_eq!(sfr(&m, "PCON1") & 0x02, 0, "nMEMV cleared by the memory violation reset");
    assert_eq!(var(&m, "t2n"), 0);
}
