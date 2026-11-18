//! DS40001412H 9.8, 10.3.2 and 24.3: fixed interrupts, PORTB mismatch and WDT.
use devdesc::Device;
use picsim::{Machine, Stop};
use std::path::Path;
fn rec(addr: u16, kind: u8, data: &[u8]) -> String {
    let mut b = vec![data.len() as u8, (addr >> 8) as u8, addr as u8, kind];
    b.extend_from_slice(data);
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(
        ":{}{:02X}\n",
        b.iter().map(|x| format!("{x:02X}")).collect::<String>(),
        sum.wrapping_neg()
    )
}
fn machine(part: &str, mode: u8, ps: u8, sleep: bool) -> Machine {
    let dev = Device::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml")),
    )
    .unwrap();
    let mut m = Machine::new(dev).unwrap();
    let prog = if sleep {
        vec![3, 0, 0xFF, 0xD7]
    } else {
        vec![0xFF, 0xD7]
    };
    m.load_hex(
        &(rec(0, 0, &prog)
            + &rec(0, 4, &[0, 0x30])
            + &rec(3, 0, &[mode | ps << 2])
            + &rec(0, 1, &[])),
    )
    .unwrap();
    m.cpu.p.board.fosc_mhz = 4.0;
    m
}
fn w(m: &mut Machine, n: &str, v: u8) {
    m.cpu.write(m.dev.find_sfr(n).unwrap().addr, v);
}
fn r(m: &mut Machine, n: &str) -> u8 {
    m.cpu.read(m.dev.find_sfr(n).unwrap().addr)
}
fn to(m: &mut Machine, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}
#[test]
fn watchdog_modes_reset_sleep_and_software_enable() {
    for part in ["18F25K22", "18F45K22", "18LF26K22"] {
        let mut m = machine(part, 3, 0, false);
        w(&mut m, "TRISC", 0);
        m.cpu.write(0x50, 0xAA);
        to(&mut m, 3900);
        assert_eq!(r(&mut m, "TRISC"), 0);
        to(&mut m, 4100);
        assert_eq!(r(&mut m, "TRISC"), 255);
        assert_eq!(r(&mut m, "RCON") & 8, 0);
        assert_eq!(m.cpu.read(0x50), 0xAA);
        let mut m = machine(part, 2, 0, false);
        to(&mut m, 5000);
        assert_ne!(r(&mut m, "RCON") & 8, 0);
        w(&mut m, "WDTCON", 1);
        to(&mut m, 9100);
        assert_eq!(r(&mut m, "RCON") & 8, 0);
        let mut m = machine(part, 3, 0, true);
        m.cpu.step();
        m.cpu.step();
        assert!(!m.cpu.sleep_dead());
        assert_eq!(r(&mut m, "RCON") & 12, 0);
        assert!(m.cpu.cycles >= 4000 && m.cpu.cycles < 4010);
        let mut m = machine(part, 1, 0, true);
        m.cpu.step();
        assert!(m.cpu.sleep_dead());
    }
}
#[test]
fn clrwdt_postscale_and_ircf_changes_clear_the_counter() {
    let mut m = machine("18F25K22", 3, 1, false);
    w(&mut m, "TRISC", 0);
    to(&mut m, 7000);
    // CLRWDT at the current PC, then return to BRA loop.
    m.cpu.prog[m.cpu.pc as usize..m.cpu.pc as usize + 4].copy_from_slice(&[4, 0, 0xFF, 0xD7]);
    m.cpu.step();
    to(&mut m, 14000);
    assert_eq!(r(&mut m, "TRISC"), 0);
    to(&mut m, 15100);
    assert_eq!(r(&mut m, "TRISC"), 255);
    let mut m = machine("18F25K22", 3, 0, false);
    w(&mut m, "TRISC", 0);
    to(&mut m, 3000);
    w(&mut m, "OSCCON", 0x70);
    to(&mut m, 6500);
    assert_eq!(r(&mut m, "TRISC"), 0);
    to(&mut m, 7100);
    assert_eq!(r(&mut m, "TRISC"), 255);
}
#[test]
fn fixed_int_pins_select_edges_and_wake_sleep() {
    for k in 0..3 {
        let mut m = machine("18F45K22", 0, 0, true);
        m.load_stimulus(&format!("[[pin]]\nport='B'\nbit={k}\nlevel=1\nat=100\n[[pin]]\nport='B'\nbit={k}\nlevel=0\nat=200\n")).unwrap();
        w(&mut m, "ANSELB", 0);
        w(&mut m, "INTCON2", 0x70);
        if k == 0 {
            w(&mut m, "INTCON", 0x10);
        } else {
            w(&mut m, "INTCON3", 1 << (k + 2));
        }
        assert_ne!(m.run(None, 110), Stop::Sleep);
        let reg = if k == 0 { "INTCON" } else { "INTCON3" };
        let mask = if k == 0 { 2 } else { 1 << (k - 1) };
        assert_ne!(r(&mut m, reg) & mask, 0, "INT{k}");
        w(&mut m, reg, 0);
        w(&mut m, "INTCON2", 0);
        to(&mut m, 210);
        assert_ne!(r(&mut m, reg) & mask, 0, "falling INT{k}");
    }
}
#[test]
fn portb_mismatch_persists_until_port_access_and_peek_has_no_effect() {
    let mut m = machine("18F25K22", 0, 0, true);
    m.load_stimulus("[[pin]]\nport='B'\nbit=4\nlevel=1\nat=100\n")
        .unwrap();
    w(&mut m, "ANSELB", 0);
    w(&mut m, "IOCB", 0x10);
    w(&mut m, "INTCON", 8);
    assert_ne!(m.run(None, 110), Stop::Sleep);
    assert_eq!(r(&mut m, "INTCON") & 1, 1);
    let port = m.dev.find_sfr("PORTB").unwrap().addr;
    assert_eq!(m.cpu.peek(port) & 0x10, 0x10);
    w(&mut m, "INTCON", 8);
    m.cpu.step();
    assert_eq!(r(&mut m, "INTCON") & 1, 1);
    assert_eq!(r(&mut m, "PORTB") & 0x10, 0x10);
    m.cpu.step();
    w(&mut m, "INTCON", 8);
    m.cpu.step();
    assert_eq!(r(&mut m, "INTCON") & 1, 0);
}
#[test]
fn movff_portb_does_not_end_mismatch() {
    let mut m = machine("18F25K22", 0, 0, false);
    m.load_stimulus("[[pin]]\nport='B'\nbit=4\nlevel=1\nat=10\n")
        .unwrap();
    w(&mut m, "ANSELB", 0);
    w(&mut m, "IOCB", 0x10);
    to(&mut m, 12);
    // MOVFF PORTB, 0x50.
    m.cpu.prog[..4].copy_from_slice(&[0x81, 0xCF, 0x50, 0xF0]);
    m.cpu.pc = 0;
    m.cpu.step();
    assert_eq!(m.cpu.read(0x50) & 0x10, 0x10);
    w(&mut m, "INTCON", 0);
    m.cpu.step();
    assert_eq!(r(&mut m, "INTCON") & 1, 1);
}

#[test]
fn mclr_preserves_the_portb_mismatch_baseline() {
    let mut m = machine("18F25K22", 0, 0, false);
    m.load_stimulus("[[pin]]\nport='B'\nbit=4\nlevel=1\nat=10\n").unwrap();
    w(&mut m, "ANSELB", 0);
    w(&mut m, "IOCB", 0x10);
    to(&mut m, 12);
    r(&mut m, "PORTB");
    m.cpu.reset_cond(picsim::periph::ResetKind::Mclr);
    w(&mut m, "ANSELB", 0);
    w(&mut m, "IOCB", 0x10);
    w(&mut m, "INTCON", 0);
    m.cpu.step();
    assert_eq!(r(&mut m, "INTCON") & 1, 0);
}
