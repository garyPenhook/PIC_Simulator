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

#[test]
fn memory_client_writes_wraps_and_retains_pointer_across_restart() {
    let mut m = i2c("[[i2c]]\naddress = 0x50\nmode = \"memory\"\ndata = [0x12, 0x34]\n");
    w(&mut m, "SSP1CON2", 1);
    run_for(&mut m, 10);
    // Write across FF -> 00, then select FF again.
    for byte in [0xA0, 0xFF, 0xAB, 0xCD] {
        w(&mut m, "SSP1BUF", byte);
        run_for(&mut m, 90);
    }
    w(&mut m, "SSP1CON2", 2);
    run_for(&mut m, 15);
    for byte in [0xA0, 0xFF] {
        w(&mut m, "SSP1BUF", byte);
        run_for(&mut m, 90);
    }
    w(&mut m, "SSP1CON2", 2);
    run_for(&mut m, 15);
    w(&mut m, "SSP1BUF", 0xA1);
    run_for(&mut m, 90);
    for expected in [0xAB, 0xCD, 0x34] {
        w(&mut m, "SSP1CON2", 8);
        run_for(&mut m, 80);
        assert_eq!(rd(&mut m, "SSP1BUF"), expected);
        w(&mut m, "SSP1CON2", 0x10);
        run_for(&mut m, 10);
    }
}

#[test]
fn readonly_memory_client_acknowledges_writes_without_changing_registers() {
    let mut m = i2c("[[i2c]]\naddress = 0x50\nmode = \"memory\"\ndata = [0x12, 0x34]\nreadonly = true\n");
    w(&mut m, "SSP1CON2", 1);
    run_for(&mut m, 10);
    for byte in [0xA0, 0, 0x99, 0x88] {
        w(&mut m, "SSP1BUF", byte);
        run_for(&mut m, 90);
        assert_eq!(rd(&mut m, "SSP1CON2") & 0x40, 0);
    }
    // A new transaction selects register zero again.
    w(&mut m, "SSP1CON2", 4);
    run_for(&mut m, 15);
    w(&mut m, "SSP1CON2", 1);
    run_for(&mut m, 10);
    for byte in [0xA0, 0] {
        w(&mut m, "SSP1BUF", byte);
        run_for(&mut m, 90);
    }
    w(&mut m, "SSP1CON2", 2);
    run_for(&mut m, 15);
    w(&mut m, "SSP1BUF", 0xA1);
    run_for(&mut m, 90);
    w(&mut m, "SSP1CON2", 8);
    run_for(&mut m, 80);
    assert_eq!(rd(&mut m, "SSP1BUF"), 0x12);
}

// ---- I2C client mode, driven by an external host (`[[i2c_host]]`) ----

/// Host with a 20-cycle SCL period: Start at once, a byte takes 180 cycles (eight bits at 160, then the ninth clock).
fn client(ops: &str, add: u8, con1: u8) -> (Machine, u64) {
    let mut m = machine(&format!("[[i2c_host]]\nperiod = 20\nops = {ops}\n"));
    w(&mut m, "SSP1ADD", add);
    let e = m.cpu.cycles;
    w(&mut m, "SSP1CON1", con1);
    (m, e)
}

fn ack_log(m: &Machine) -> Vec<I2cEv> { log(m) }

#[test]
fn client_receives_address_and_data() {
    let (mut m, e) = client(r#"["S", "w 0xA0", "w 0x12", "P"]"#, 0xA0, 0x36);
    run_to(&mut m, e + 5);
    assert_eq!(rd(&mut m, "SSP1STAT") & 0x18, 0x08, "S set by the Start");
    assert!(!sspif(&m), "SSPM = 0110: no interrupt on Start");
    run_to(&mut m, e + 10 + 160 - 1);
    assert_eq!(rd(&mut m, "SSP1STAT") & 1, 0, "BF not before the eighth falling edge");
    run_to(&mut m, e + 10 + 160 + 1);
    assert_eq!(rd(&mut m, "SSP1STAT") & 0x27, 0x01, "BF, R/W = 0 (write), D/A = 0 (address)");
    assert!(!sspif(&m));
    run_to(&mut m, e + 10 + 180 + 1);
    assert!(sspif(&m), "SSPxIF on the ninth falling edge");
    assert_eq!(rd(&mut m, "SSP1BUF"), 0xA0);
    assert_eq!(rd(&mut m, "SSP1STAT") & 1, 0);
    clear_sspif(&mut m);
    run_for(&mut m, 180);
    assert_eq!(rd(&mut m, "SSP1STAT") & 0x21, 0x21, "data byte: D/A and BF");
    assert!(sspif(&m));
    assert_eq!(rd(&mut m, "SSP1BUF"), 0x12);
    run_for(&mut m, 20);
    assert_eq!(rd(&mut m, "SSP1STAT") & 0x18, 0x10, "P set, S clear");
    assert_eq!(ack_log(&m), [I2cEv::Start, I2cEv::Addr(0xA0, true), I2cEv::Write(0x12, true), I2cEv::Stop]);
}

#[test]
fn client_ignores_another_address_and_honours_the_mask() {
    let (mut m, _) = client(r#"["S", "w 0xA2", "w 0x12", "P"]"#, 0xA0, 0x36);
    run_for(&mut m, 700);
    assert_eq!(rd(&mut m, "SSP1STAT") & 0x21, 0, "no BF, no D/A");
    assert!(!sspif(&m));
    assert_eq!(ack_log(&m), [I2cEv::Start, I2cEv::Addr(0xA2, false), I2cEv::Write(0x12, false), I2cEv::Stop]);
    // SSPxMSK: a zero bit is a don't care on A[7:1]
    let mut m = machine("[[i2c_host]]\nperiod = 20\nops = [\"S\", \"w 0xA6\", \"P\"]\n");
    w(&mut m, "SSP1ADD", 0xA0);
    w(&mut m, "SSP1MSK", 0xF9);
    w(&mut m, "SSP1CON1", 0x36);
    run_for(&mut m, 400);
    assert_eq!(ack_log(&m)[1], I2cEv::Addr(0xA6, true));
}

#[test]
fn client_overflow_is_nacked_and_sets_sspov() {
    let (mut m, _) = client(r#"["S", "w 0xA0", "w 0x12", "w 0x13", "P"]"#, 0xA0, 0x36);
    run_for(&mut m, 1000);
    assert_eq!(rd(&mut m, "SSP1CON1") & 0x40, 0x40, "SSPOV: the buffer was never read");
    assert_eq!(rd(&mut m, "SSP1STAT") & 1, 1);
    let l = ack_log(&m);
    assert_eq!(l[1], I2cEv::Addr(0xA0, true));
    assert_eq!(l[2], I2cEv::Write(0x12, false), "BF still set from the address: NACK");
}

#[test]
fn client_sen_stretches_until_ckp_is_set() {
    let (mut m, e) = client(r#"["S", "w 0xA0", "w 0x12", "P"]"#, 0xA0, 0x36);
    w(&mut m, "SSP1CON2", 1); // SEN
    run_to(&mut m, e + 10 + 181);
    assert_eq!(rd(&mut m, "SSP1CON1") & 0x10, 0, "CKP cleared after the acknowledged byte");
    run_for(&mut m, 1000);
    assert_eq!(ack_log(&m), [I2cEv::Start, I2cEv::Addr(0xA0, true)], "the host waits for the client");
    assert_eq!(rd(&mut m, "SSP1BUF"), 0xA0);
    let c = m.cpu.cycles;
    w(&mut m, "SSP1CON1", 0x36); // CKP
    run_to(&mut m, c + 1 + 160 + 1);
    assert_eq!(rd(&mut m, "SSP1BUF"), 0x12, "the next byte starts after the release");
}

#[test]
fn client_transmit_cycle() {
    let (mut m, e) = client(r#"["S", "w 0xA1", "r", "rn", "P"]"#, 0xA0, 0x36);
    run_to(&mut m, e + 10 + 181);
    assert_eq!(rd(&mut m, "SSP1CON1") & 0x10, 0, "a read address clears CKP after the ACK");
    assert_eq!(rd(&mut m, "SSP1STAT") & 4, 4, "R/W = read");
    assert!(sspif(&m));
    clear_sspif(&mut m);
    assert_eq!(rd(&mut m, "SSP1BUF"), 0xA1);
    w(&mut m, "SSP1BUF", 0x5A);
    assert_eq!(rd(&mut m, "SSP1CON1") & 0x80, 0);
    let c = m.cpu.cycles;
    w(&mut m, "SSP1CON1", 0x36);
    run_to(&mut m, c + 1 + 160 - 1);
    assert_eq!(rd(&mut m, "SSP1STAT") & 1, 1, "BF until the eighth falling edge");
    run_to(&mut m, c + 1 + 160 + 1);
    assert_eq!(rd(&mut m, "SSP1STAT") & 1, 0);
    assert_eq!(rd(&mut m, "SSP1STAT") & 0x20, 0x20, "D/A: data");
    run_to(&mut m, c + 1 + 170 + 1);
    assert_eq!(rd(&mut m, "SSP1CON2") & 0x40, 0, "ACKSTAT = 0: the host acknowledged");
    assert!(!sspif(&m));
    run_to(&mut m, c + 1 + 180 + 1);
    assert!(sspif(&m));
    assert_eq!(rd(&mut m, "SSP1CON1") & 0x10, 0, "after an ACK the clock is held again");
    clear_sspif(&mut m);
    w(&mut m, "SSP1BUF", 0x6B);
    w(&mut m, "SSP1CON1", 0x36);
    run_for(&mut m, 200);
    assert_eq!(rd(&mut m, "SSP1CON2") & 0x40, 0x40, "ACKSTAT = 1: NACK");
    assert!(sspif(&m));
    assert_eq!(rd(&mut m, "SSP1CON1") & 0x10, 0x10, "the clock is not held after a NACK");
    run_for(&mut m, 100);
    assert_eq!(ack_log(&m), [I2cEv::Start, I2cEv::Addr(0xA1, true), I2cEv::Read(0x5A, true), I2cEv::Read(0x6B, false), I2cEv::Stop]);
}

#[test]
fn client_transmit_wcol_while_bf_is_set() {
    let (mut m, e) = client(r#"["S", "w 0xA1", "r", "P"]"#, 0xA0, 0x36);
    run_to(&mut m, e + 10 + 181);
    // the address is still in SSPxBUF (BF = 1): a write is a collision
    w(&mut m, "SSP1BUF", 0x77);
    assert_eq!(rd(&mut m, "SSP1CON1") & 0x80, 0x80);
    assert_eq!(rd(&mut m, "SSP1BUF"), 0xA1, "SSPxBUF unchanged");
}

#[test]
fn client_address_and_data_hold() {
    let (mut m, e) = client(r#"["S", "w 0xA0", "w 0x12", "P"]"#, 0xA0, 0x36);
    w(&mut m, "SSP1CON3", 0x03); // AHEN, DHEN
    run_to(&mut m, e + 10 + 161);
    assert!(sspif(&m), "SSPxIF after the eighth falling edge");
    assert_eq!(rd(&mut m, "SSP1CON1") & 0x10, 0, "CKP cleared");
    assert_eq!(rd(&mut m, "SSP1CON3") & 0x80, 0x80, "ACKTIM");
    clear_sspif(&mut m);
    run_for(&mut m, 500);
    assert_eq!(ack_log(&m).len(), 1, "the host waits");
    assert_eq!(rd(&mut m, "SSP1BUF"), 0xA0);
    let c = m.cpu.cycles;
    w(&mut m, "SSP1CON2", 0x00); // ACKDT = 0: acknowledge
    w(&mut m, "SSP1CON1", 0x36);
    run_to(&mut m, c + 1 + 10 + 1);
    assert_eq!(rd(&mut m, "SSP1CON3") & 0x80, 0, "ACKTIM clears on the ninth rising edge");
    assert!(!sspif(&m));
    run_to(&mut m, c + 1 + 20 + 1);
    assert!(sspif(&m), "SSPxIF again after the acknowledge");
    clear_sspif(&mut m);
    // data byte: NACK by software
    run_for(&mut m, 170);
    assert_eq!(rd(&mut m, "SSP1CON1") & 0x10, 0, "DHEN clears CKP");
    assert_eq!(rd(&mut m, "SSP1BUF"), 0x12);
    clear_sspif(&mut m);
    w(&mut m, "SSP1CON2", 0x20); // ACKDT = 1
    w(&mut m, "SSP1CON1", 0x36);
    run_for(&mut m, 60);
    assert!(!sspif(&m), "no SSPxIF after a NACK");
    assert!(ack_log(&m).contains(&I2cEv::Write(0x12, false)));
}

#[test]
fn client_start_stop_interrupts() {
    let (mut m, e) = client(r#"["S", "w 0xA0", "P"]"#, 0xA0, 0x3E); // SSPM = 1110
    run_to(&mut m, e + 2);
    assert!(sspif(&m), "SSPM = 1110: interrupt on Start");
    clear_sspif(&mut m);
    run_for(&mut m, 500);
    assert_eq!(rd(&mut m, "SSP1STAT") & 0x10, 0x10);
    assert!(sspif(&m));
    // SSPM = 0110 with SCIE and PCIE
    let mut m = machine("[[i2c_host]]\nperiod = 20\nops = [\"S\", \"P\"]\n");
    w(&mut m, "SSP1CON3", 0x60);
    w(&mut m, "SSP1CON1", 0x36);
    run_for(&mut m, 5);
    assert!(sspif(&m), "SCIE");
}

#[test]
fn client_host_waits_for_enable_and_for_ckp() {
    let mut m = machine("[[i2c_host]]\nat = 100\nperiod = 20\nops = [\"S\", \"w 0xA0\"]\n");
    run_for(&mut m, 300);
    assert_eq!(ack_log(&m), []);
    w(&mut m, "SSP1ADD", 0xA0);
    w(&mut m, "SSP1CON1", 0x26); // CKP = 0
    run_for(&mut m, 500);
    assert_eq!(ack_log(&m), [I2cEv::Start], "CKP = 0 holds the clock, the byte never starts");
    w(&mut m, "SSP1CON1", 0x36);
    run_for(&mut m, 300);
    assert_eq!(ack_log(&m)[1], I2cEv::Addr(0xA0, true));
}

#[test]
fn i2c_host_stimulus_errors() {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    for bad in ["ops = [\"X\"]", "ops = [\"w 300\"]", "period = 1\nops = []", "port = 3\nops = []", "ops = [\"S\"]\nbogus = 1", "period = 20"] {
        let mut m = Machine::new(dev.clone()).unwrap();
        assert!(m.load_stimulus(&format!("[[i2c_host]]\n{bad}\n")).is_err(), "{bad}");
    }
}

// ---- I2C client mode: the bus on the pins (`[A164]`) ----

/// SCL = RC0, SDA = RC1, both pulled up; returns the machine and the cycle SSPEN was set. Reads give bit 0 = SCL, bit 1 = SDA.
fn client_pins(ops: &str, add: u8) -> (Machine, u64) { client_pins_with(ops, add, "") }

fn client_pins_with(ops: &str, add: u8, extra: &str) -> (Machine, u64) {
    let mut m = machine(&format!("[[pin]]\nport = \"C\"\nbit = 0\nlevel = 1\n[[pin]]\nport = \"C\"\nbit = 1\nlevel = 1\n{extra}[[i2c_host]]\nperiod = 20\nops = {ops}\n"));
    w(&mut m, "ANSELC", 0);
    w(&mut m, "RC0PPS", 0x0F);
    w(&mut m, "RC1PPS", 0x10);
    w(&mut m, "SSP1CLKPPS", 0x10);
    w(&mut m, "SSP1DATPPS", 0x11);
    w(&mut m, "SSP1ADD", add);
    let e = m.cpu.cycles;
    assert_eq!(rd(&mut m, "PORTC") & 3, 0b11, "idle: both pulled up");
    w(&mut m, "SSP1CON1", 0x36);
    (m, e)
}

fn bus(m: &mut Machine, at: u64) -> u8 {
    run_to(m, at);
    rd(m, "PORTC") & 3
}

#[test]
fn client_pins_show_start_bits_ack_and_stop() {
    let (mut m, e) = client_pins(r#"["S", "w 0xA0", "P"]"#, 0xA0);
    assert_eq!(bus(&mut m, e + 5), 0b01, "Start: SDA low while SCL high");
    assert_eq!(bus(&mut m, e + 15), 0b10, "SCL follows, SDA takes bit 7 (1)");
    assert_eq!(bus(&mut m, e + 25), 0b11, "bit 7 of 0xA0 (1): SCL high, SDA high");
    assert_eq!(bus(&mut m, e + 35), 0b00, "SCL low between bits, SDA already bit 6 (0)");
    assert_eq!(bus(&mut m, e + 45), 0b01, "bit 6 of 0xA0 is 0: SDA low, SCL high");
    assert_eq!(bus(&mut m, e + 185), 0b01, "ninth clock: SCL high, the client pulls SDA low (ACK)");
    assert_eq!(bus(&mut m, e + 192), 0b00, "SCL low, SDA pulled low ready for the Stop");
    assert_eq!(bus(&mut m, e + 197), 0b01, "Stop: SCL released while SDA is low");
    assert_eq!(bus(&mut m, e + 205), 0b11, "SDA rises while SCL is high");
}

#[test]
fn client_pins_show_no_ack_for_another_address() {
    let (mut m, e) = client_pins(r#"["S", "w 0xA2"]"#, 0xA0);
    assert_eq!(bus(&mut m, e + 185), 0b11, "no acknowledge: SDA stays high on the ninth clock");
}

#[test]
fn client_pins_show_a_stretched_clock_and_read_data() {
    // SEN = 1, so the address byte stretches the clock until CKP is set; then the host reads a byte.
    let (mut m, e) = client_pins(r#"["S", "w 0xA1", "r"]"#, 0xA0);
    w(&mut m, "SSP1CON2", 1);
    assert_eq!(bus(&mut m, e + 195), 0b10, "address acknowledged, then SCL held low by the client (CKP = 0 after a read address)");
    run_to(&mut m, e + 600);
    assert_eq!(bus(&mut m, e + 600) & 1, 0, "SCL stays low while CKP = 0");
    w(&mut m, "SSP1BUF", 0x80);
    w(&mut m, "SSP1CON1", 0x36);
    let t = m.cpu.cycles;
    // first data bit (1) goes out with the clock released a half period after the host resumes
    let mut seen_scl_high_sda_high = false;
    for k in 0..40 { if bus(&mut m, t + k) == 0b11 { seen_scl_high_sda_high = true; } }
    assert!(seen_scl_high_sda_high, "bit 7 of the byte the client sends appears on SDA");
}

#[test]
fn client_host_waits_while_an_outside_device_holds_scl_low() {
    // Without the hold the ninth clock is high at e + 185 (`client_pins_show_start_bits_ack_and_stop`). SCL is held low
    // from cycle 50 to 250: the host's clock stops for those 200 cycles.
    let hold = "[[pin]]\nport = \"C\"\nbit = 0\nlevel = 0\nat = 50\n[[pin]]\nport = \"C\"\nbit = 0\nlevel = 1\nat = 250\n";
    let (mut m, e) = client_pins_with(r#"["S", "w 0xA0"]"#, 0xA0, hold);
    assert!(e < 5, "the script is timed from about cycle 0");
    assert_eq!(bus(&mut m, e + 185) & 1, 0, "SCL is still low at the unstalled ninth clock");
    assert_eq!(bus(&mut m, e + 240) & 1, 0, "held low");
    assert_eq!(rd(&mut m, "SSP1STAT") & 1, 0, "no byte has arrived while the clock is held");
    assert_eq!(bus(&mut m, e + 385), 0b01, "ninth clock 200 cycles later: the client acknowledges");
    run_to(&mut m, e + 400);
    assert_eq!(rd(&mut m, "SSP1BUF"), 0xA0);
}

#[test]
fn client_host_is_not_delayed_by_a_hold_that_has_ended() {
    let hold = "[[pin]]\nport = \"C\"\nbit = 0\nlevel = 0\nat = 50\n[[pin]]\nport = \"C\"\nbit = 0\nlevel = 1\nat = 51\n";
    let (mut m, e) = client_pins_with(r#"["S", "w 0xA0"]"#, 0xA0, hold);
    assert_eq!(bus(&mut m, e + 186), 0b01, "a one-cycle hold costs about a cycle");
}

fn sda_pin(at: u64, level: u8) -> String { format!("[[pin]]\nport = \"C\"\nbit = 1\nlevel = {level}\nat = {at}\n") }

#[test]
fn client_data_bit_held_low_by_an_outside_device_reads_zero() {
    // 0xA0 = 1010 0000. Bit 5 (a 1) has its rising SCL edge at e + 60; SDA is held low from 55 to 65, so the client sees 0x80.
    let (mut m, e) = client_pins(r#"["S", "w 0xA0"]"#, 0xA0);
    run_to(&mut m, e + 200);
    assert_eq!(ack_log(&m)[1], I2cEv::Addr(0xA0, true), "control: no hold");
    let hold = sda_pin(55, 0) + &sda_pin(65, 1);
    let (mut m, e) = client_pins_with(r#"["S", "w 0xA0"]"#, 0xA0, &hold);
    run_to(&mut m, e + 200);
    assert_eq!(ack_log(&m)[1], I2cEv::Addr(0x80, false), "bit 5 reads 0: the address no longer matches");
}

#[test]
fn client_data_bit_held_low_outside_the_clock_high_time_is_not_seen() {
    // Bit 5's rising edge is at e + 60; a hold from 62 to 70 ends before the next edge at e + 80 and starts after bit 5 was sampled.
    let hold = sda_pin(62, 0) + &sda_pin(70, 1);
    let (mut m, e) = client_pins_with(r#"["S", "w 0xA0"]"#, 0xA0, &hold);
    run_to(&mut m, e + 200);
    assert_eq!(ack_log(&m)[1], I2cEv::Addr(0xA0, true));
}

/// The ACKSTAT after the client sends 0x00 and the host answers NACK, with `extra` stimulus; also the cycle the second byte was released.
fn nack_after_second_byte(extra: &str) -> (u8, bool, u64) {
    let (mut m, e) = client_pins_with(r#"["S", "w 0xA1", "r", "rn", "P"]"#, 0xA0, extra);
    run_to(&mut m, e + 10 + 181);
    clear_sspif(&mut m);
    w(&mut m, "SSP1BUF", 0x5A);
    w(&mut m, "SSP1CON1", 0x36);
    run_to(&mut m, e + 400);
    clear_sspif(&mut m);
    w(&mut m, "SSP1BUF", 0x00);
    w(&mut m, "SSP1CON1", 0x36);
    let c = m.cpu.cycles;
    run_to(&mut m, c + 200);
    (rd(&mut m, "SSP1CON2") >> 6 & 1, rd(&mut m, "SSP1CON1") & 0x10 != 0, c)
}

#[test]
fn client_host_nack_reads_as_ack_while_an_outside_device_holds_sda_low() {
    let (ackstat, ckp, c) = nack_after_second_byte("");
    assert_eq!((ackstat, ckp), (1, true), "control: NACK, the clock is released");
    let hold = sda_pin(c + 1, 0) + &sda_pin(c + 190, 1);
    let (ackstat, ckp, c2) = nack_after_second_byte(&hold);
    assert_eq!(c2, c, "the hold does not change the timing");
    assert_eq!((ackstat, ckp), (0, false), "SDA low on the ninth clock is an acknowledge: ACKSTAT = 0, CKP stays clear");
}
