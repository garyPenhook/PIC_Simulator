//! Resolved PIC18 outputs feeding serial receivers, timer clocks, capture and IOC.
use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn machine(part: &str, stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_stimulus(stim).unwrap();
    w(&mut m, "ANSELC", 0);
    m
}
fn w(m: &mut Machine, n: &str, v: u8) { m.cpu.write(m.dev.find_sfr(n).unwrap().addr, v); }
fn r(m: &mut Machine, n: &str) -> u8 { m.cpu.read(m.dev.find_sfr(n).unwrap().addr) }
fn run(m: &mut Machine, n: u64) { let end = m.cpu.cycles + n; while m.cpu.cycles < end { m.cpu.step(); } }

#[test]
fn eusart_waveform_loopback_open_drain_and_remapping() {
    for od in [false, true] {
        let stim = if od { "[[pin]]\nport = \"C\"\nbit = 0\nlevel = 1\n" } else { "" };
        let mut m = machine("18F27Q10", stim);
        w(&mut m, "RC0PPS", 9);
        w(&mut m, "RX1PPS", 0x10);
        w(&mut m, "ODCONC", u8::from(od));
        w(&mut m, "RC1STA", 0x90);
        w(&mut m, "TX1STA", 0x24);
        run(&mut m, 2);
        assert_eq!(r(&mut m, "PORTC") & 1, 1);
        w(&mut m, "TX1REG", 0xA5);
        assert_eq!(r(&mut m, "PORTC") & 1, 0, "start bit");
        run(&mut m, 4);
        assert_eq!(r(&mut m, "PORTC") & 1, 1, "LSB first");
        run(&mut m, 38);
        assert_eq!(r(&mut m, "RC1REG"), 0xA5);
        w(&mut m, "WPUC", 2);
        w(&mut m, "RX1PPS", 0x11);
        w(&mut m, "TX1REG", 0x37);
        run(&mut m, 44);
        assert_eq!(r(&mut m, "RC1REG"), 0xA5, "unconnected receiver has no new byte");
    }
}

#[test]
fn eusart_tx_polarity_changes_physical_bits() {
    let mut m = machine("18F27Q10", "");
    w(&mut m, "RC0PPS", 9);
    w(&mut m, "RC1STA", 0x80);
    w(&mut m, "TX1STA", 0x24);
    w(&mut m, "BAUD1CON", 0x10);
    assert_eq!(r(&mut m, "PORTC") & 1, 0);
    w(&mut m, "TX1REG", 0);
    assert_eq!(r(&mut m, "PORTC") & 1, 1);
    run(&mut m, 40);
    assert_eq!(r(&mut m, "PORTC") & 1, 0);
}

#[test]
fn q41_uart_pin_loopback_including_inverted_polarity() {
    for inverted in [false, true] {
        let mut m = machine("18F16Q41", "");
        w(&mut m, "TRISC", 0xFE);
        w(&mut m, "RC0PPS", 0x10);
        w(&mut m, "U1RXPPS", 0x10);
        w(&mut m, "U1CON0", 0x30);
        w(&mut m, "U1CON2", if inverted { 0x44 } else { 0 });
        w(&mut m, "U1CON1", 0x80);
        run(&mut m, 2);
        assert_eq!(r(&mut m, "PORTC") & 1, u8::from(!inverted), "idle");
        w(&mut m, "U1TXB", 0x96);
        assert_eq!(r(&mut m, "PORTC") & 1, u8::from(inverted), "start");
        run(&mut m, 44);
        assert_eq!(r(&mut m, "U1RXB"), 0x96);
        assert_eq!(r(&mut m, "U1ERRIR") & 0x0A, 0);
    }
}

#[test]
fn gpio_edges_clock_timer_and_set_ioc() {
    let mut m = machine("18F27Q10", "");
    w(&mut m, "TRISC", 0xFE);
    w(&mut m, "T1CKIPPS", 0x10);
    w(&mut m, "T1CLK", 0);
    w(&mut m, "T1CON", 1);
    w(&mut m, "IOCCP", 1);
    run(&mut m, 2);
    for _ in 0..3 {
        w(&mut m, "LATC", 1); run(&mut m, 2);
        w(&mut m, "LATC", 0); run(&mut m, 2);
    }
    assert_eq!(r(&mut m, "TMR1L"), 3);
    assert_eq!(r(&mut m, "IOCCF") & 1, 1);
}

#[test]
fn mssp_spi_pin_loopback_and_tris() {
    for driven in [true, false] {
        let mut m = machine("18F27Q10", "");
        w(&mut m, "TRISC", if driven { 0xFC } else { 0xFF });
        w(&mut m, "RC0PPS", 0x10);
        w(&mut m, "RC1PPS", 0x0F);
        w(&mut m, "SSP1DATPPS", 0x10);
        w(&mut m, "SSP1STAT", 0x40);
        w(&mut m, "SSP1CON1", 0x21); // four cycles per bit
        run(&mut m, 2);
        w(&mut m, "SSP1BUF", 0xA6);
        run(&mut m, 32);
        assert_eq!(r(&mut m, "SSP1BUF"), if driven { 0xA6 } else { 0 });
    }
}

#[test]
fn mssp_i2c_start_stop_and_open_drain_bus() {
    let mut m = machine("18F27Q10", "[[pin]]\nport = \"C\"\nbit = 0\nlevel = 1\n[[pin]]\nport = \"C\"\nbit = 1\nlevel = 1\n");
    w(&mut m, "RC0PPS", 0x0F);
    w(&mut m, "RC1PPS", 0x10);
    w(&mut m, "SSP1CLKPPS", 0x10);
    w(&mut m, "SSP1DATPPS", 0x11);
    w(&mut m, "SSP1ADD", 7);
    w(&mut m, "SSP1CON1", 0x28);
    assert_eq!(r(&mut m, "PORTC") & 3, 3);
    w(&mut m, "SSP1CON2", 1);
    run(&mut m, 4);
    assert_eq!(r(&mut m, "PORTC") & 3, 1, "SDA falls while SCL high");
    run(&mut m, 4);
    assert_eq!(r(&mut m, "PORTC") & 3, 0);
    w(&mut m, "SSP1CON2", 4);
    run(&mut m, 8);
    assert_eq!(r(&mut m, "PORTC") & 3, 3, "STOP releases both wires");
}

#[test]
fn uart_edges_reach_timer_and_capture_inputs() {
    let mut m = machine("18F27Q10", "");
    w(&mut m, "RC0PPS", 9);
    w(&mut m, "T1CKIPPS", 0x10);
    w(&mut m, "T1CLK", 0);
    w(&mut m, "T1CON", 1);
    w(&mut m, "CCP1PPS", 0x10);
    w(&mut m, "CCP1CON", 0x85); // capture every rising edge
    w(&mut m, "RC1STA", 0x80);
    w(&mut m, "TX1STA", 0x24);
    run(&mut m, 2);
    let initial = r(&mut m, "TMR1L");
    w(&mut m, "TX1REG", 0x55);
    run(&mut m, 44);
    assert_eq!(r(&mut m, "TMR1L").wrapping_sub(initial), 5);
    assert_eq!(r(&mut m, "CCPR1L"), r(&mut m, "TMR1L"));
    assert_eq!(r(&mut m, "PIR6") & 1, 1);
}

#[test]
fn spi_all_clock_polarities_and_edges_include_the_fastest_clock() {
    for ckp in [0, 0x10] {
        for cke in [0, 0x40] {
            let mut m = machine("18F27Q10", "");
            w(&mut m, "TRISC", 0xFC);
            w(&mut m, "RC0PPS", 0x10);
            w(&mut m, "RC1PPS", 0x0F);
            w(&mut m, "SSP1DATPPS", 0x10);
            w(&mut m, "SSP1STAT", cke);
            w(&mut m, "SSP1CON1", 0x20 | ckp);
            run(&mut m, 2);
            let start = m.cpu.cycles;
            w(&mut m, "SSP1BUF", 0xA6);
            run(&mut m, 8);
            assert_eq!(r(&mut m, "SSP1BUF"), 0xA6, "CKP={ckp} CKE={cke}");
            let clocks = m.cpu.p.pin_log.iter().filter(|e| e.0 > 4 * start && e.1 == b'C' && e.2 == 1).count();
            assert_eq!(clocks, 16, "half-cycle SCK edges are logged");
        }
    }
}

#[test]
fn k22_fixed_uart_and_spi_pins() {
    for part in ["18F26K22", "18F46K22"] {
        let mut m = machine(part, "");
        w(&mut m, "TRISC", 0xD7); // SCK RC3 and SDO RC5 outputs; SDI RC4 input
        w(&mut m, "LATC", 0x10);
        w(&mut m, "TRISC", 0xC7); // drive SDI high to receive FF
        w(&mut m, "SSP1CON1", 0x21);
        run(&mut m, 2);
        w(&mut m, "SSP1BUF", 0xA6);
        run(&mut m, 32);
        assert_eq!(r(&mut m, "SSP1BUF"), 0xFF, "{part}: fixed SDI RC4");
        w(&mut m, "RC1STA", 0x80);
        w(&mut m, "TX1STA", 0x24);
        assert_eq!(r(&mut m, "PORTC") & 0x40, 0x40, "{part}: TX idle on RC6");
        w(&mut m, "TX1REG", 0);
        assert_eq!(r(&mut m, "PORTC") & 0x40, 0, "{part}: TX start");
    }
}

#[test]
fn scheduled_compare_output_reaches_another_timer_and_ioc() {
    let mut m = machine("18F27Q10", "");
    w(&mut m, "TRISC", 0xFE);
    w(&mut m, "RC0PPS", 5);
    w(&mut m, "T3CKIPPS", 0x10);
    w(&mut m, "T3CLK", 0);
    w(&mut m, "T3CON", 1);
    w(&mut m, "IOCCP", 1);
    w(&mut m, "T1CLK", 1);
    w(&mut m, "CCPR1L", 4);
    w(&mut m, "CCP1CON", 0x82);
    w(&mut m, "T1CON", 1);
    run(&mut m, 8);
    assert_eq!(r(&mut m, "TMR3L"), 1);
    assert_eq!(r(&mut m, "IOCCF") & 1, 1);
}

#[test]
fn internally_driven_timer_gate_follows_the_uart_waveform() {
    let mut m = machine("18F27Q10", "");
    w(&mut m, "RC0PPS", 9);
    w(&mut m, "RC1STA", 0x80);
    w(&mut m, "TX1STA", 0x24);
    w(&mut m, "T1GPPS", 0x10);
    w(&mut m, "T1GATE", 0);
    w(&mut m, "T1GCON", 0xC0);
    w(&mut m, "T1CLK", 1);
    w(&mut m, "T1CON", 1);
    run(&mut m, 2);
    let before = r(&mut m, "TMR1L");
    run(&mut m, 8);
    assert_eq!(r(&mut m, "TMR1L").wrapping_sub(before), 8);
    let before = r(&mut m, "TMR1L");
    w(&mut m, "TX1REG", 0);
    run(&mut m, 32);
    assert_eq!(r(&mut m, "TMR1L").wrapping_sub(before), 0, "gate is closed from the following clock");
}
