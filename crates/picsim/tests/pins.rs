//! Port pins, the `[[pin]]`/`[[port]]` stimulus and the Timer1 gate value (DS40002043F ch.17, 20.14.2; PIC18F27Q10).

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn machine(stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_stimulus(stim).unwrap();
    m
}

fn addr(m: &Machine, n: &str) -> u32 {
    m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr
}

fn run_to(m: &mut Machine, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

#[test]
fn input_pins_follow_the_stimulus_from_their_cycle() {
    let mut m = machine("[[pin]]\nport = \"B\"\nbit = 5\nlevel = 1\nat = 100\n[[port]]\nport = \"B\"\nvalue = 0x03\nmask = 0x0F\nat = 200\n");
    let (portb, ansb) = (addr(&m, "PORTB"), addr(&m, "ANSELB"));
    m.cpu.write(ansb, 0); // digital
    assert_eq!(m.cpu.read(portb), 0, "undriven pins read 0");
    run_to(&mut m, 100);
    assert_eq!(m.cpu.read(portb), 0x20);
    run_to(&mut m, 200);
    assert_eq!(m.cpu.read(portb), 0x23, "mask 0x0F touches only RB3:0");
}

#[test]
fn analog_pin_reads_zero_and_output_pin_shows_its_latch() {
    let mut m = machine("[[port]]\nport = \"B\"\nvalue = 0xFF\n");
    let (portb, ansb, trisb, latb) = (addr(&m, "PORTB"), addr(&m, "ANSELB"), addr(&m, "TRISB"), addr(&m, "LATB"));
    assert_eq!(m.cpu.read(portb), 0, "ANSELB resets to 1: the digital input buffer is off");
    m.cpu.write(ansb, 0);
    assert_eq!(m.cpu.read(portb), 0xFF);
    m.cpu.write(trisb, 0xF0); // RB3:0 outputs
    m.cpu.write(latb, 0x05);
    assert_eq!(m.cpu.read(portb), 0xF5);
    m.cpu.write(portb, 0x0A); // a write to PORTB goes to LATB
    assert_eq!(m.cpu.read(latb), 0x0A);
    assert_eq!(m.cpu.read(portb), 0xFA);
}

#[test]
fn timer1_gate_value_follows_the_t1g_pin() {
    let mut m = machine("[[pin]]\nport = \"B\"\nbit = 5\nlevel = 1\nat = 50\n");
    let (ansb, gcon, gate) = (addr(&m, "ANSELB"), addr(&m, "T1GCON"), addr(&m, "T1GATE"));
    m.cpu.write(ansb, 0);
    m.cpu.write(gate, 0); // T1GPPS resets to RB5
    m.cpu.write(gcon, 0x80); // GE, active low
    assert_eq!(m.cpu.read(gcon), 0x84, "pin low, active low: gate active");
    m.cpu.write(gcon, 0xC0); // GE, active high
    assert_eq!(m.cpu.read(gcon), 0xC0);
    run_to(&mut m, 50);
    assert_eq!(m.cpu.read(gcon), 0xC4);
    m.cpu.write(gcon, 0x04); // GVAL is read only; MDB reads 0 with GE = 0
    assert_eq!(m.cpu.read(gcon), 0x00);
}

#[test]
fn bad_stimulus_is_rejected() {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    for t in ["[[pin]]\nport = \"b\"\nbit = 0\nlevel = 1\n", "[[pin]]\nport = \"B\"\nbit = 8\nlevel = 1\n", "[[pin]]\nport = \"B\"\nbit = 1\nlevel = 2\n", "[[port]]\nport = \"B\"\nvalue = 256\n", "[[pin]]\nport = \"B\"\nbit = 1\nlevel = 1\nbogus = 1\n", "[[pin]]\nport = \"G\"\nbit = 1\nlevel = 1\n"] {
        assert!(Machine::new(dev.clone()).unwrap().load_stimulus(t).is_err(), "{t}");
    }
}

/// Timer1 on Fosc/4 (1 tick per cycle), gate from RB5 (T1GPPS reset), counting between cycles 0 and `run`.
fn gated(stim: &str, gcon: u8) -> (Machine, u32, u32, u32) {
    let mut m = machine(stim);
    let (ansb, gate, clk, con) = (addr(&m, "ANSELB"), addr(&m, "T1GATE"), addr(&m, "T1CLK"), addr(&m, "T1CON"));
    m.cpu.write(ansb, 0);
    m.cpu.write(gate, 0);
    m.cpu.write(clk, 1);
    m.cpu.write(addr(&m, "T1GCON"), gcon);
    m.cpu.write(con, 0x01);
    let (l, gcon_a, pir5) = (addr(&m, "TMR1L"), addr(&m, "T1GCON"), addr(&m, "PIR5"));
    (m, l, gcon_a, pir5)
}

const PULSE: &str = "[[pin]]\nport = \"B\"\nbit = 5\nlevel = 1\nat = 100\n[[pin]]\nport = \"B\"\nbit = 5\nlevel = 0\nat = 200\n";

#[test]
fn timer1_counts_only_while_the_gate_is_active() {
    // Active high: the pin is high for cycles 100..200.
    let (mut m, l, _, pir5) = gated(PULSE, 0xC0);
    run_to(&mut m, 300);
    assert_eq!(m.cpu.read(l), 100);
    assert_ne!(m.cpu.read(pir5) & 0x01, 0, "TMR1GIF on the falling edge of the gate");
    // Active low: it counts while the pin is low, 0..100 and from 200 on.
    let (mut m, l, ..) = gated(PULSE, 0x80);
    run_to(&mut m, 300);
    assert_eq!(m.cpu.read(l), 200);
    // GE = 0: the gate is ignored, but TMR1GIF still follows the pin.
    let (mut m, l, _, pir5) = gated(PULSE, 0x40);
    run_to(&mut m, 300);
    assert_eq!(m.cpu.read(l), 44);
    assert_eq!(m.cpu.read(l + 1), 1);
    assert_ne!(m.cpu.read(pir5) & 0x01, 0);
}

#[test]
fn timer1_gate_single_pulse_counts_one_pulse() {
    let two = format!("{PULSE}[[pin]]\nport = \"B\"\nbit = 5\nlevel = 1\nat = 300\n[[pin]]\nport = \"B\"\nbit = 5\nlevel = 0\nat = 400\n");
    let (mut m, l, g, _) = gated(&two, 0xD0); // GE, active high, GSPM
    m.cpu.write(g, 0xD8); // GGO
    run_to(&mut m, 500);
    assert_eq!(m.cpu.read(l), 100, "only the first pulse is counted");
    assert_eq!(m.cpu.read(g) & 8, 0, "GGO/DONE cleared on the falling edge");
}

#[test]
fn timer1_gate_toggle_measures_a_full_period() {
    // Pin high 100..200 and 300..400; toggle: the flip-flop is high from the first rising edge to the second.
    let two = format!("{PULSE}[[pin]]\nport = \"B\"\nbit = 5\nlevel = 1\nat = 300\n[[pin]]\nport = \"B\"\nbit = 5\nlevel = 0\nat = 400\n");
    let (mut m, l, ..) = gated(&two, 0xE0);
    run_to(&mut m, 500);
    assert_eq!(m.cpu.read(l), 200);
}
