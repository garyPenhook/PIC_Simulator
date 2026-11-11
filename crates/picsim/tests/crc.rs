//! CRC and memory scanner of the PIC18F27Q10 (DS40002043F ch.14 p.174-192).

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn rec(addr: u16, ty: u8, data: &[u8]) -> String {
    let mut b = vec![data.len() as u8, (addr >> 8) as u8, addr as u8, ty];
    b.extend_from_slice(data);
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

const BRA_SELF: [u8; 2] = [0xFF, 0xD7];
/// Program bytes 0x40.. that the scanner reads.
const DATA: [u8; 8] = [0x55, 0x66, 0x77, 0x88, 0x01, 0x02, 0x03, 0x04];

fn machine() -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    let hex = rec(0, 0, &BRA_SELF) + &rec(0x40, 0, &DATA) + &rec(0, 4, &[0x00, 0x30]) + &rec(0, 1, &[]);
    m.load_hex(&hex).unwrap();
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
fn run(m: &mut Machine, cycles: u64) {
    let to = m.cpu.cycles + cycles;
    while m.cpu.cycles < to {
        m.cpu.step();
    }
}
fn acc(m: &mut Machine) -> u16 {
    u16::from(r(m, "CRCACCH")) << 8 | u16::from(r(m, "CRCACCL"))
}

/// CRC-16-ANSI as in Figure 14-1: DLEN 7, PLEN 15, polynomial 0x8004 (+1).
fn setup(m: &mut Machine, accm: bool, shiftm: bool) {
    w(m, "CRCXORH", 0x80);
    w(m, "CRCXORL", 0x04);
    w(m, "CRCCON1", 0x7F);
    w(m, "CRCCON0", 0xC0 | if accm { 0x10 } else { 0 } | if shiftm { 2 } else { 0 });
}

fn feed(m: &mut Machine, bytes: &[u8]) {
    for &b in bytes {
        while r(m, "CRCCON0") & 1 != 0 {
            m.cpu.step();
        }
        w(m, "CRCDATH", 0);
        w(m, "CRCDATL", b);
    }
    while r(m, "CRCCON0") & 0x20 != 0 {
        m.cpu.step();
    }
}

#[test]
fn reset_values() {
    let mut m = machine();
    assert_eq!(r(&mut m, "CRCXORL") & 1, 1);
    assert_eq!(r(&mut m, "SCANCON0"), 0x10, "INVALID resets to 1");
    assert_eq!(r(&mut m, "SCANHADRL"), 0xFF);
    assert_eq!(r(&mut m, "SCANHADRU"), 0x3F);
}

#[test]
fn manual_entry_figure_14_1() {
    for (shiftm, want) in [(false, 0x32D6), (true, 0x6BA2)] {
        let mut m = machine();
        setup(&mut m, true, shiftm);
        feed(&mut m, &[0x55, 0x66, 0x77, 0x88]);
        assert_eq!(acc(&mut m), want);
        assert_eq!(r(&mut m, "CRCCON0") & 0x21, 0, "BUSY and FULL clear");
    }
}

#[test]
fn busy_full_and_the_interrupt_flag() {
    let mut m = machine();
    setup(&mut m, true, false);
    w(&mut m, "CRCDATL", 0x55);
    assert_eq!(r(&mut m, "CRCCON0") & 0x21, 0x20, "shifting: BUSY, shifter took the word");
    w(&mut m, "CRCDATL", 0x66);
    assert_eq!(r(&mut m, "CRCCON0") & 0x21, 0x21, "second word waits: FULL");
    let pir = m.dev.sfr.iter().find(|s| s.name.starts_with("PIR") && s.fields.iter().any(|f| f.name == "CRCIF")).unwrap().clone();
    let bit = pir.fields.iter().find(|f| f.name == "CRCIF").unwrap().pos;
    assert_eq!(m.cpu.read(pir.addr) & (1 << bit), 0, "CRCIF clear while BUSY");
    run(&mut m, 40);
    assert_eq!(r(&mut m, "CRCCON0") & 0x21, 0);
    assert_ne!(m.cpu.read(pir.addr) & (1 << bit), 0, "CRCIF set when BUSY falls");
}

#[test]
fn no_go_no_shifting() {
    let mut m = machine();
    setup(&mut m, true, false);
    w(&mut m, "CRCCON0", 0x80);
    w(&mut m, "CRCDATL", 0x55);
    run(&mut m, 40);
    assert_eq!(acc(&mut m), 0);
    assert_eq!(r(&mut m, "CRCCON0") & 0x21, 0x21);
    w(&mut m, "CRCCON0", 0xC0);
    run(&mut m, 20);
    assert_ne!(acc(&mut m), 0);
}

fn scan_regs(m: &mut Machine, lo: u8, hi: u8) {
    for (n, v) in [("SCANLADRL", lo), ("SCANLADRH", 0), ("SCANLADRU", 0), ("SCANHADRL", hi), ("SCANHADRH", 0), ("SCANHADRU", 0)] {
        w(m, n, v);
    }
}

fn expect(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0u16, |a, &b| {
        let mut a = a;
        for i in (0..8).rev() {
            let fb = (a >> 15 & 1) ^ u16::from(b >> i & 1);
            a <<= 1;
            if fb != 0 {
                a ^= 0x8005;
            }
        }
        a
    })
}

#[test]
fn burst_scan_computes_the_crc_and_stalls_the_cpu() {
    let mut m = machine();
    setup(&mut m, true, false);
    scan_regs(&mut m, 0x40, 0x47);
    w(&mut m, "SCANCON0", 0x81); // SCANEN, Burst
    let t0 = m.cpu.cycles;
    w(&mut m, "SCANCON0", 0xC1);
    m.cpu.step();
    assert!(m.cpu.cycles - t0 >= 48, "the CPU waited for the scan: {}", m.cpu.cycles - t0);
    assert_eq!(r(&mut m, "SCANCON0") & 0x40, 0, "SCANGO cleared at the end");
    run(&mut m, 20);
    assert_eq!(acc(&mut m), expect(&DATA));
    let hit = |m: &mut Machine, f: &str| {
        let pir = m.dev.sfr.iter().find(|s| s.name.starts_with("PIR") && s.fields.iter().any(|x| x.name == f)).unwrap();
        let bit = pir.fields.iter().find(|x| x.name == f).unwrap().pos;
        m.cpu.read(pir.addr) & (1 << bit) != 0
    };
    assert!(hit(&mut m, "SCANIF"));
    assert!(hit(&mut m, "CRCIF"));
}

#[test]
fn concurrent_scan_runs_alongside_the_cpu() {
    let mut m = machine();
    setup(&mut m, true, false);
    scan_regs(&mut m, 0x40, 0x47);
    w(&mut m, "SCANCON0", 0x80);
    w(&mut m, "SCANCON0", 0xC0);
    assert_ne!(r(&mut m, "SCANCON0") & 0x40, 0, "running");
    run(&mut m, 200);
    assert_eq!(r(&mut m, "SCANCON0") & 0x40, 0);
    assert_eq!(acc(&mut m), expect(&DATA));
}

#[test]
fn scan_needs_the_crc_running() {
    let mut m = machine();
    w(&mut m, "CRCCON0", 0x80); // EN without GO
    scan_regs(&mut m, 0x40, 0x47);
    w(&mut m, "SCANCON0", 0x80);
    w(&mut m, "SCANCON0", 0xC0);
    assert_eq!(r(&mut m, "SCANCON0") & 0x50, 0x10, "not started, INVALID");
}

#[test]
fn writes_to_the_address_registers_are_ignored_while_scanning() {
    let mut m = machine();
    setup(&mut m, true, false);
    scan_regs(&mut m, 0x40, 0x47);
    w(&mut m, "SCANCON0", 0x80);
    w(&mut m, "SCANCON0", 0xC0);
    w(&mut m, "SCANHADRL", 0x41);
    assert_eq!(r(&mut m, "SCANHADRL"), 0x47);
}

#[test]
fn low_above_high_clears_scango() {
    let mut m = machine();
    setup(&mut m, true, false);
    scan_regs(&mut m, 0x50, 0x47);
    w(&mut m, "SCANCON0", 0x80);
    w(&mut m, "SCANCON0", 0xC0);
    assert_eq!(r(&mut m, "SCANCON0") & 0x40, 0);
}
