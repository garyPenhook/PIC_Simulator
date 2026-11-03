//! MSSP of the PIC18F27Q10 (DS40002043F ch.28): SPI host and I2C host, timing, flags, client bytes.

use devdesc::Device;
use picsim::Machine;
use picsim::periph::I2cEv;
use std::path::Path;

fn machine(stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_stimulus(stim).unwrap();
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

fn sspif(m: &Machine) -> bool {
    for s in m.dev.sfr.iter().filter(|s| s.name.starts_with("PIR")) {
        if let Some(f) = s.fields.iter().chain(s.legacy.iter()).find(|f| f.name == "SSP1IF") {
            return m.cpu.m.mem[s.addr as usize] & (1 << f.pos) != 0;
        }
    }
    panic!("no SSP1IF")
}

fn clear_sspif(m: &mut Machine) {
    for s in m.dev.sfr.iter().filter(|s| s.name.starts_with("PIR")) {
        if let Some(f) = s.fields.iter().chain(s.legacy.iter()).find(|f| f.name == "SSP1IF") {
            m.cpu.m.mem[s.addr as usize] &= !(1 << f.pos);
        }
    }
}

fn run_to(m: &mut Machine, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

fn run_for(m: &mut Machine, n: u64) {
    let c = m.cpu.cycles + n;
    run_to(m, c);
}

#[test]
fn spi_fosc4_byte_timing_flags_and_client() {
    let mut m = machine("[[spi]]\ndata = [0xA5, 0x3C]\n");
    w(&mut m, "SSP1CON1", 0x20); // SSPEN, SSPM = 0000: Fosc/4
    let t0 = m.cpu.cycles;
    w(&mut m, "SSP1BUF", 0x9F);
    run_to(&mut m, t0 + 7);
    assert_eq!(rd(&mut m, "SSP1STAT") & 1, 0, "BF before the eighth bit");
    assert!(!sspif(&m));
    run_to(&mut m, t0 + 8);
    assert_eq!(rd(&mut m, "SSP1STAT") & 1, 1);
    assert!(sspif(&m));
    assert_eq!(rd(&mut m, "SSP1BUF"), 0xA5);
    assert_eq!(rd(&mut m, "SSP1STAT") & 1, 0, "a read clears BF");
    clear_sspif(&mut m);
    w(&mut m, "SSP1BUF", 0x00);
    run_for(&mut m, 8);
    assert_eq!(rd(&mut m, "SSP1BUF"), 0x3C);
    w(&mut m, "SSP1BUF", 0x11);
    run_for(&mut m, 8);
    assert_eq!(rd(&mut m, "SSP1BUF"), 0x00, "fill after the list");
    let sent: Vec<u8> = m.cpu.p.spi_tx.iter().map(|t| t.2).collect();
    assert_eq!(sent, [0x9F, 0x00, 0x11]);
    assert_eq!(m.cpu.p.spi_tx[0].1, t0);
}

#[test]
fn spi_write_collision_and_baud_generator() {
    let mut m = machine("[[spi]]\ndata = [0x55]\n");
    w(&mut m, "SSP1ADD", 3);
    w(&mut m, "SSP1CON1", 0x2A); // SSPEN, SSPM = 1010: Fosc/(4 (3 + 1)): 4 cycles a bit
    let t0 = m.cpu.cycles;
    w(&mut m, "SSP1BUF", 0x81);
    run_to(&mut m, t0 + 31);
    w(&mut m, "SSP1BUF", 0xFF); // ignored: WCOL
    assert_eq!(rd(&mut m, "SSP1CON1") & 0x80, 0x80);
    assert!(!sspif(&m));
    run_to(&mut m, t0 + 32);
    assert!(sspif(&m));
    assert_eq!(rd(&mut m, "SSP1BUF"), 0x55);
    assert_eq!(m.cpu.p.spi_tx.len(), 1);
    assert_eq!(m.cpu.p.spi_tx[0].2, 0x81);
}

#[test]
fn spi_disabled_module_does_not_transfer() {
    let mut m = machine("");
    w(&mut m, "SSP1BUF", 0x42);
    run_for(&mut m, 50);
    assert!(m.cpu.p.spi_tx.is_empty());
    assert!(!sspif(&m));
}

/// I2C host with SSPxADD = 9: a bit is 10 cycles, TBRG is 5.
fn i2c(stim: &str) -> Machine {
    let mut m = machine(stim);
    w(&mut m, "SSP1ADD", 9);
    w(&mut m, "SSP1CON1", 0x28); // SSPEN, SSPM = 1000
    m
}

fn log(m: &Machine) -> Vec<I2cEv> {
    m.cpu.p.i2c_log.iter().map(|t| t.2).collect()
}

#[test]
fn i2c_start_timing_and_no_queuing() {
    let mut m = i2c("");
    let t0 = m.cpu.cycles;
    w(&mut m, "SSP1CON2", 1); // SEN
    run_to(&mut m, t0 + 4);
    assert_eq!(rd(&mut m, "SSP1STAT") & 8, 0);
    run_to(&mut m, t0 + 5);
    assert_eq!(rd(&mut m, "SSP1STAT") & 8, 8, "S after TBRG");
    assert_eq!(rd(&mut m, "SSP1CON2") & 1, 1);
    w(&mut m, "SSP1CON2", 0x04); // PEN while the Start runs: dropped
    assert_eq!(rd(&mut m, "SSP1CON2") & 0x1F, 1);
    w(&mut m, "SSP1BUF", 0x55); // no queuing: WCOL
    assert_eq!(rd(&mut m, "SSP1CON1") & 0x80, 0x80);
    run_to(&mut m, t0 + 10);
    assert_eq!(rd(&mut m, "SSP1CON2") & 0x1F, 0);
    assert!(sspif(&m));
    assert_eq!(log(&m), [I2cEv::Start]);
}

#[test]
fn i2c_write_to_a_client_and_stop() {
    let mut m = i2c("[[i2c]]\naddress = 0x50\n");
    w(&mut m, "SSP1CON2", 1);
    run_for(&mut m, 10);
    clear_sspif(&mut m);
    w(&mut m, "SSP1CON2", 0x40); // ACKSTAT preset so the update shows
    let t0 = m.cpu.cycles;
    w(&mut m, "SSP1BUF", 0xA0);
    assert_eq!(rd(&mut m, "SSP1STAT") & 5, 5, "BF and R/W while transmitting");
    run_to(&mut m, t0 + 79);
    assert_eq!(rd(&mut m, "SSP1STAT") & 1, 1);
    run_to(&mut m, t0 + 80);
    assert_eq!(rd(&mut m, "SSP1STAT") & 1, 0, "BF clears after the eighth bit (16 TBRG)");
    run_to(&mut m, t0 + 84);
    assert_eq!(rd(&mut m, "SSP1CON2") & 0x40, 0x40);
    run_to(&mut m, t0 + 85);
    assert_eq!(rd(&mut m, "SSP1CON2") & 0x40, 0, "acknowledged, written on the ninth clock (17 TBRG)");
    assert!(!sspif(&m));
    run_to(&mut m, t0 + 90);
    assert!(sspif(&m), "SSPxIF after the ninth clock (18 TBRG)");
    assert_eq!(rd(&mut m, "SSP1STAT") & 4, 0);
    clear_sspif(&mut m);
    w(&mut m, "SSP1BUF", 0x07);
    run_for(&mut m, 180);
    assert_eq!(rd(&mut m, "SSP1CON2") & 0x40, 0);
    w(&mut m, "SSP1CON2", 4); // Stop
    run_for(&mut m, 10);
    assert_eq!(rd(&mut m, "SSP1STAT") & 0x18, 0x10, "P set, S clear");
    run_for(&mut m, 5);
    assert_eq!(rd(&mut m, "SSP1CON2") & 4, 0);
    assert_eq!(log(&m), [I2cEv::Start, I2cEv::Addr(0xA0, true), I2cEv::Write(0x07, true), I2cEv::Stop]);
}

#[test]
fn i2c_address_nobody_owns_is_not_acknowledged() {
    let mut m = i2c("[[i2c]]\naddress = 0x50\n");
    w(&mut m, "SSP1CON2", 1);
    run_for(&mut m, 10);
    w(&mut m, "SSP1BUF", 0xA2); // 0x51
    run_for(&mut m, 180);
    assert_eq!(rd(&mut m, "SSP1CON2") & 0x40, 0x40);
    assert_eq!(log(&m)[1], I2cEv::Addr(0xA2, false));
}

#[test]
fn i2c_read_bytes_and_nack() {
    let mut m = i2c("[[i2c]]\naddress = 0x50\ndata = [0x12, 0x34]\n");
    w(&mut m, "SSP1CON2", 1);
    run_for(&mut m, 10);
    w(&mut m, "SSP1BUF", 0xA1);
    run_for(&mut m, 180);
    assert_eq!(rd(&mut m, "SSP1CON2") & 0x40, 0);
    clear_sspif(&mut m);
    let t0 = m.cpu.cycles;
    w(&mut m, "SSP1CON2", 8); // RCEN
    run_to(&mut m, t0 + 15);
    assert!(!sspif(&m));
    w(&mut m, "SSP1CON2", 1); // active: dropped
    assert_eq!(rd(&mut m, "SSP1CON2") & 0x1F, 8);
    run_to(&mut m, t0 + 80);
    assert!(sspif(&m));
    assert_eq!(rd(&mut m, "SSP1CON2") & 8, 0, "RCEN clears");
    assert_eq!(rd(&mut m, "SSP1STAT") & 1, 1);
    assert_eq!(rd(&mut m, "SSP1BUF"), 0x12);
    clear_sspif(&mut m);
    w(&mut m, "SSP1CON2", 0x10); // ACK (ACKDT = 0)
    run_for(&mut m, 10);
    assert!(sspif(&m));
    assert_eq!(rd(&mut m, "SSP1CON2") & 0x10, 0);
    clear_sspif(&mut m);
    w(&mut m, "SSP1CON2", 8);
    run_for(&mut m, 80);
    assert_eq!(rd(&mut m, "SSP1BUF"), 0x34);
    w(&mut m, "SSP1CON2", 0x30); // ACKDT = 1 and ACKEN: NACK
    run_for(&mut m, 10);
    w(&mut m, "SSP1CON2", 0x20 | 4);
    run_for(&mut m, 20);
    assert_eq!(log(&m), [I2cEv::Start, I2cEv::Addr(0xA1, true), I2cEv::Read(0x12, true), I2cEv::Read(0x34, false), I2cEv::Stop]);
}

#[test]
fn i2c_receive_overflow_and_unowned_read() {
    let mut m = i2c("");
    w(&mut m, "SSP1CON2", 8);
    run_for(&mut m, 80);
    assert_eq!(rd(&mut m, "SSP1CON1") & 0x40, 0);
    w(&mut m, "SSP1CON2", 8); // BF still set: the second byte overflows
    run_for(&mut m, 80);
    assert_eq!(rd(&mut m, "SSP1CON1") & 0x40, 0x40);
    assert_eq!(rd(&mut m, "SSP1BUF"), 0xFF);
}

#[test]
fn restart_and_disable() {
    let mut m = i2c("[[i2c]]\naddress = 0x50\n");
    w(&mut m, "SSP1CON2", 1);
    run_for(&mut m, 10);
    w(&mut m, "SSP1CON2", 2); // RSEN
    run_for(&mut m, 9);
    assert_eq!(rd(&mut m, "SSP1CON2") & 2, 2);
    run_for(&mut m, 6);
    assert_eq!(rd(&mut m, "SSP1CON2") & 2, 0);
    assert_eq!(log(&m), [I2cEv::Start, I2cEv::Restart]);
    w(&mut m, "SSP1CON1", 0x08); // SSPEN cleared
    assert_eq!(rd(&mut m, "SSP1STAT") & 0x1F, 0);
}
