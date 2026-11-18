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
