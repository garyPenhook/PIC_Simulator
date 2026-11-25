//! Programs the Basil compiler built for the dsPIC33CK64MC105 (`tests/dspic/*.hex`) run to their `__halt` with the results
//! the compiler's own MDB-checked tests expect (`Basic_Compiler/tests/exec_dspic`), and the UART shows its output.
use picsim::machine_ds::MachineDs;
use picsim::Stop;
use std::path::Path;

fn machine(name: &str) -> MachineDs {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dev = devdesc::Device::load(&root.join("devices/DSPIC33CK64MC105.toml")).unwrap();
    let mut m = MachineDs::new(dev).unwrap();
    m.load_hex(&std::fs::read_to_string(root.join(format!("tests/dspic/{name}.hex"))).unwrap()).unwrap();
    m
}

fn word(m: &MachineDs, a: u32) -> u32 {
    let b = m.peek(a, 4);
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

#[test]
fn arithmetic_matches_the_compilers_expectations() {
    let mut m = machine("arith");
    assert_eq!(m.run(Some(0x376), 100_000), Stop::Halt);
    assert_eq!(m.cpu.cycles, 762, "cycles to __halt: MDB's stopwatch gives 762 for the same hex");
    assert_eq!(m.peek(0x1000, 1), [44]);
    assert_eq!(word(&m, 0x1002) & 0xFFFF, 60000);
    assert_eq!(word(&m, 0x1004) & 0xFFFF, (-35i16) as u16 as u32);
    assert_eq!(word(&m, 0x1006), 300_000);
    assert_eq!(word(&m, 0x100A) as i32, -14285);
}

#[test]
fn timer1_interrupts_the_main_line() {
    let mut m = machine("isr_timer1");
    assert_eq!(m.run(Some(0x31C), 5_000_000), Stop::Halt);
    assert_eq!(word(&m, 0x1000) & 0xFFFF, 3, "ticks");
    assert_eq!(m.peek(0x1002, 1), [1], "done");
}

#[test]
fn uart_sends_what_the_program_writes() {
    let mut m = machine("lib_serial");
    assert_eq!(m.run(Some(0x318), 1_000_000), Stop::Halt);
    assert_eq!(m.cpu.p.tx.iter().map(|t| t.2).collect::<Vec<_>>(), b"U");
    // 9600 baud at 4 MIPS: 10 bits of 417 cycles
    assert!(m.cpu.cycles > 4000 && m.cpu.cycles < 4600, "{}", m.cpu.cycles);
}

#[test]
fn the_watchdog_resets_a_program_that_never_clears_it() {
    use picsim::dspic::Dspic;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dev = devdesc::Device::load(&root.join("devices/DSPIC33CK64MC105.toml")).unwrap();
    let mut c = Dspic::new(&dev).unwrap();
    // FWDT: FWDTEN = ON (bit 15) with RWDTPS = PS1 (a 1 ms period), the rest erased; `bra .` at the reset vector's target
    let fwdt = 0xAF20 / 2;
    c.set_prog(fwdt as usize, 0xFF_FFFF & !0x1F);
    c.set_prog(0, 0x37_FFFF); // bra -1: a loop on itself
    c.clock_reset();
    let mut n = 0;
    while c.cycles < 100_000 && c.p.tx.is_empty() {
        c.step();
        n += 1;
        if c.mem[0xF80] & 0x10 != 0 {
            break;
        }
    }
    assert!(n > 0 && c.mem[0xF80] & 0x10 != 0, "WDTO set after the time-out");
}
