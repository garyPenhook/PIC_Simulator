//! SPI host of the PIC18-Q41 (DS40002214G ch.35, PIC18F16Q41): FIFOs, transfer timing, counter, flags, client bytes.

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn machine(stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F16Q41.toml")).unwrap();
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

fn rd(m: &mut Machine, n: &str) -> u8 {
    let a = addr(m, n);
    m.cpu.read(a)
}

fn is_set(m: &Machine, bit: &str) -> bool {
    for s in m.dev.sfr.iter().filter(|s| s.name.starts_with("PIR")) {
        if let Some(f) = s.fields.iter().chain(s.legacy.iter()).find(|f| f.name == bit) {
            return m.cpu.m.mem[s.addr as usize] & (1 << f.pos) != 0;
        }
    }
    panic!("no {bit}")
}

fn run_for(m: &mut Machine, n: u64) {
    let c = m.cpu.cycles + n;
    run_to(m, c);
}

fn run_to(m: &mut Machine, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

/// What lib/spi.bas sets up: BAUD, mode 0 (CKE = 1), TWIDTH 0, TXR = RXR = 1, EN | MST | BMODE.
fn setup(m: &mut Machine, baud: u8) {
    w(m, "SPI1BAUD", baud);
    w(m, "SPI1CON1", 0x44);
    w(m, "SPI1TWIDTH", 0);
    w(m, "SPI1CON2", 3);
    w(m, "SPI1CON0", 0x83);
}

#[test]
fn full_duplex_byte_timing_and_flags() {
    let mut m = machine("[[spi]]\ndata = [0xA5, 0x3C]\n");
    setup(&mut m, 3); // SCK = Fosc/8: a bit is 2 cycles, a byte 16, start delay ceil(6/4) = 2
    assert!(!is_set(&m, "SPI1RXIF"));
    assert!(is_set(&m, "SPI1TXIF"));
    assert_eq!(rd(&mut m, "SPI1STATUS") & 0x20, 0x20); // TXBE
    let t0 = m.cpu.cycles;
    w(&mut m, "SPI1TXB", 0x5A);
    run_for(&mut m, 1);
    let start = m.cpu.p.spi_tx[0].1;
    assert!(start >= t0 + 2 && start <= t0 + 4, "start {start} t0 {t0}: start delay ceil((BAUD + 3) / 4) = 2 cycles after the write");
    run_to(&mut m, start + 15);
    assert!(!is_set(&m, "SPI1RXIF"), "a byte is 16 cycles");
    run_to(&mut m, start + 17);
    assert!(is_set(&m, "SPI1RXIF"));
    assert_eq!(rd(&mut m, "SPI1RXB"), 0xA5);
    assert!(!is_set(&m, "SPI1RXIF"));
    assert_eq!(m.cpu.p.spi_tx, vec![(1, start, 0x5A)]);
    assert_eq!(rd(&mut m, "SPI1INTF") & 0x80, 0x80, "SRMTIF");
    // Reading an empty FIFO returns 0 and sets RXRE.
    assert_eq!(rd(&mut m, "SPI1RXB"), 0);
    assert_eq!(rd(&mut m, "SPI1STATUS") & 8, 8);
    w(&mut m, "SPI1STATUS", 0);
    assert_eq!(rd(&mut m, "SPI1STATUS") & 8, 0);
    // Second byte gets the second client byte.
    w(&mut m, "SPI1TXB", 0x01);
    run_for(&mut m, 30);
    assert_eq!(rd(&mut m, "SPI1RXB"), 0x3C);
}

#[test]
fn fifo_limits_and_counter() {
    let mut m = machine("[[spi]]\ndata = [1, 2, 3, 4]\n");
    setup(&mut m, 1);
    for v in [0x10, 0x20, 0x30] {
        w(&mut m, "SPI1TXB", v);
    }
    assert_eq!(rd(&mut m, "SPI1STATUS") & 0x80, 0x80, "third write into a full TxFIFO sets TXWE");
    run_for(&mut m, 100);
    // Two bytes sent; the RxFIFO is full, so nothing more goes out until one is read.
    assert_eq!(m.cpu.p.spi_tx.len(), 2);
    assert_eq!(rd(&mut m, "SPI1STATUS") & 1, 1, "RXBF");
    assert_eq!(rd(&mut m, "SPI1RXB"), 1);
    assert_eq!(rd(&mut m, "SPI1RXB"), 2);
    // BMODE = 1 counter wraps below zero and sets TCZIF when it reaches zero: a write of 2 then two bytes.
    w(&mut m, "SPI1STATUS", 4); // CLB
    w(&mut m, "SPI1CON0", 0);
    w(&mut m, "SPI1CON0", 0x83);
    w(&mut m, "SPI1TCNTH", 0);
    w(&mut m, "SPI1TCNTL", 2);
    w(&mut m, "SPI1TXB", 0xAA);
    w(&mut m, "SPI1TXB", 0xBB);
    run_for(&mut m, 100);
    assert_eq!(rd(&mut m, "SPI1TCNTL"), 0);
    assert_eq!(rd(&mut m, "SPI1INTF") & 0x40, 0x40, "TCZIF");
}

#[test]
fn receive_only_resends_last_and_width() {
    let mut m = machine("[[spi]]\ndata = [0xFF, 0xFF]\n");
    w(&mut m, "SPI1BAUD", 1);
    w(&mut m, "SPI1CON1", 0x44);
    w(&mut m, "SPI1TWIDTH", 4);
    w(&mut m, "SPI1CON2", 1); // receive only
    w(&mut m, "SPI1CON0", 0x83);
    w(&mut m, "SPI1TCNTL", 2);
    run_for(&mut m, 100);
    assert_eq!(rd(&mut m, "SPI1RXB"), 0x0F, "4-bit transfers use the low 4 bits");
    assert_eq!(rd(&mut m, "SPI1RXB"), 0x0F);
    assert_eq!(m.cpu.p.spi_tx.len(), 2);
}

#[test]
fn interrupt_flag_follows_enable() {
    let mut m = machine("[[spi]]\ndata = [1]\n");
    setup(&mut m, 1);
    w(&mut m, "SPI1INTE", 0x80); // SRMTIE
    assert!(!is_set(&m, "SPI1IF"));
    w(&mut m, "SPI1TXB", 7);
    run_for(&mut m, 40);
    assert!(is_set(&m, "SPI1IF"));
    w(&mut m, "SPI1INTF", 0);
    assert!(!is_set(&m, "SPI1IF"));
}
