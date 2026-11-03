//! I2C host of the PIC18F06Q41 (DS40002214G ch.36): transfers by count, flags, client bytes, Stop and errors.

use devdesc::Device;
use picsim::Machine;
use picsim::periph::I2cEv;
use std::path::Path;

/// BAUD = 1: one SCL period is 10 cycles.
fn machine(stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F06Q41.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_stimulus(stim).unwrap();
    w(&mut m, "I2C1BAUD", 1);
    w(&mut m, "I2C1CON0", 0x84); // EN, MODE = 100
    m
}

fn w(m: &mut Machine, n: &str, v: u8) {
    let a = m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr;
    m.cpu.write(a, v);
}

fn rd(m: &mut Machine, n: &str) -> u8 {
    let a = m.dev.find_sfr(n).unwrap().addr;
    m.cpu.read(a)
}

fn log(m: &Machine) -> Vec<I2cEv> {
    m.cpu.p.i2c_log.iter().map(|t| t.2).collect()
}

/// Run until `f` holds (at most 3000 cycles).
fn until(m: &mut Machine, what: &str, f: impl Fn(&mut Machine) -> bool) {
    let end = m.cpu.cycles + 3000;
    while !f(m) {
        assert!(m.cpu.cycles < end, "timeout waiting for {what}");
        m.cpu.step();
    }
}

fn run_for(m: &mut Machine, n: u64) {
    let c = m.cpu.cycles + n;
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

/// Past the bus free time, with the flags the enable may set cleared.
fn ready(m: &mut Machine) {
    run_for(m, 20);
    w(m, "I2C1PIR", 0);
}

fn flag(m: &Machine, name: &str) -> bool {
    for s in m.dev.sfr.iter().filter(|s| s.name.starts_with("PIR")) {
        if let Some(f) = s.fields.iter().chain(s.legacy.iter()).find(|f| f.name == name) {
            return m.cpu.m.mem[s.addr as usize] & (1 << f.pos) != 0;
        }
    }
    panic!("no {name}")
}

#[test]
fn bus_free_time_and_enable_flags() {
    let mut m = machine("");
    assert_eq!(rd(&mut m, "I2C1STAT0") & 0x80, 0, "BFRE needs 8 prescaled clocks");
    run_for(&mut m, 20);
    assert_eq!(rd(&mut m, "I2C1STAT0") & 0x80, 0x80);
    assert_eq!(rd(&mut m, "I2C1PIR") & 5, 5, "start / stop flags after the enable");
}

#[test]
fn write_two_bytes_to_a_client_then_stop() {
    let mut m = machine("[[i2c]]\naddress = 0x50\n");
    ready(&mut m);
    w(&mut m, "I2C1CNTL", 2);
    w(&mut m, "I2C1TXB", 0xA0);
    assert_eq!(rd(&mut m, "I2C1STAT1") & 0x20, 0, "a write to TXB clears TXBE at once");
    until(&mut m, "address in the shift register", |m| rd(m, "I2C1STAT1") & 0x20 != 0);
    assert_eq!(rd(&mut m, "I2C1STAT0") & 0x20, 0x20, "MMA");
    assert!(rd(&mut m, "I2C1PIR") & 1 != 0, "SCIF");
    assert!(flag(&m, "I2C1TXIF"), "count != 0 and TXBE");
    w(&mut m, "I2C1TXB", 0x10);
    until(&mut m, "TXBE", |m| rd(m, "I2C1STAT1") & 0x20 != 0);
    w(&mut m, "I2C1TXB", 0x55);
    until(&mut m, "Stop", |m| rd(m, "I2C1PIR") & 4 != 0);
    assert!(rd(&mut m, "I2C1PIR") & 0x80 != 0, "CNTIF");
    assert_eq!(rd(&mut m, "I2C1STAT0") & 0x20, 0, "MMA clear after the Stop");
    assert_eq!(rd(&mut m, "I2C1CNTL"), 0);
    assert_eq!(
        log(&m),
        [I2cEv::Start, I2cEv::Addr(0xA0, true), I2cEv::Write(0x10, true), I2cEv::Write(0x55, true), I2cEv::Stop]
    );
}

#[test]
fn read_two_bytes_with_nack_on_the_last() {
    let mut m = machine("[[i2c]]\naddress = 0x50\ndata = [0xCA, 0xFE]\n");
    ready(&mut m);
    w(&mut m, "I2C1CNTL", 2);
    w(&mut m, "I2C1CON1", 0x80); // ACKCNT = 1 (NACK the last byte), ACKDT = 0
    w(&mut m, "I2C1TXB", 0xA1);
    until(&mut m, "RXBF", |m| rd(m, "I2C1STAT1") & 1 != 0);
    assert!(flag(&m, "I2C1RXIF"));
    assert_eq!(rd(&mut m, "I2C1RXB"), 0xCA);
    assert!(!flag(&m, "I2C1RXIF"), "a read of RXB clears it");
    until(&mut m, "RXBF", |m| rd(m, "I2C1STAT1") & 1 != 0);
    assert_eq!(rd(&mut m, "I2C1RXB"), 0xFE);
    until(&mut m, "Stop", |m| rd(m, "I2C1PIR") & 4 != 0);
    assert_eq!(
        log(&m),
        [I2cEv::Start, I2cEv::Addr(0xA1, true), I2cEv::Read(0xCA, true), I2cEv::Read(0xFE, false), I2cEv::Stop]
    );
}

#[test]
fn unowned_address_sets_nack_and_stops() {
    let mut m = machine("");
    ready(&mut m);
    w(&mut m, "I2C1TXB", 0xB4); // count 0: nothing to send after the address
    until(&mut m, "Stop", |m| rd(m, "I2C1PIR") & 4 != 0);
    assert_eq!(rd(&mut m, "I2C1CON1") & 0x20, 0x20, "ACKSTAT");
    assert!(rd(&mut m, "I2C1ERR") & 0x10 != 0, "NACKIF");
    assert_eq!(log(&m), [I2cEv::Start, I2cEv::Addr(0xB4, false), I2cEv::Stop]);
}

#[test]
fn restart_after_write_then_read_and_buffer_errors() {
    let mut m = machine("[[i2c]]\naddress = 0x50\ndata = [0x77]\n");
    ready(&mut m);
    w(&mut m, "I2C1CON0", 0xC4); // RSEN
    w(&mut m, "I2C1CON1", 0x80); // NACK the last byte
    w(&mut m, "I2C1CNTL", 1);
    w(&mut m, "I2C1TXB", 0xA0);
    until(&mut m, "TXBE", |m| rd(m, "I2C1STAT1") & 0x20 != 0);
    w(&mut m, "I2C1TXB", 0x10);
    until(&mut m, "MDR", |m| rd(m, "I2C1CON0") & 8 != 0);
    w(&mut m, "I2C1CNTL", 1);
    w(&mut m, "I2C1TXB", 0xA1); // Restart with the read address
    until(&mut m, "RXBF", |m| rd(m, "I2C1STAT1") & 1 != 0);
    assert_eq!(rd(&mut m, "I2C1RXB"), 0x77);
    w(&mut m, "I2C1CON0", 0x84); // RSEN off: Stop after the byte
    until(&mut m, "MDR or Stop", |m| rd(m, "I2C1PIR") & 4 != 0 || rd(m, "I2C1CON0") & 8 != 0);
    if rd(&mut m, "I2C1CON0") & 8 != 0 {
        w(&mut m, "I2C1CON1", 8); // P
        run_for(&mut m, 12);
    }
    assert!(rd(&mut m, "I2C1PIR") & 2 != 0, "RSCIF");
    assert_eq!(
        log(&m),
        [I2cEv::Start, I2cEv::Addr(0xA0, true), I2cEv::Write(0x10, true), I2cEv::Restart, I2cEv::Addr(0xA1, true), I2cEv::Read(0x77, false), I2cEv::Stop]
    );
    // an empty RXB read sets RXRE and NACKIF
    w(&mut m, "I2C1ERR", 0);
    rd(&mut m, "I2C1RXB");
    assert_eq!(rd(&mut m, "I2C1STAT1") & 8, 8, "RXRE");
    assert!(rd(&mut m, "I2C1ERR") & 0x10 != 0);
}
