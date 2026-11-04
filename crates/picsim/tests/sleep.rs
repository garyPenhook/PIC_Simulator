//! Peripherals in Sleep (DS40002043F 7.2 p.68, 7.3-7.4 p.71, 19.4 p.298, 29.4 p.523, 28.2.5 p.450; PIC18F27Q10): Fosc and Fosc/4 are off, so what
//! they clock stops and goes on where it was at the wake-up; in Idle everything runs.

use devdesc::Device;
use picsim::pic18::State;
use picsim::Machine;
use std::path::Path;

fn rec(addr: u16, ty: u8, data: &[u8]) -> String {
    let mut b = vec![data.len() as u8, (addr >> 8) as u8, addr as u8, ty];
    b.extend_from_slice(data);
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

const ON: u8 = 0x7F; // WDTE = 11: the watchdog runs in Sleep and wakes the part after 16517 cycles
const OFF: u8 = 0x1F;
const SLEEP: [u8; 2] = [0x03, 0x00];
const NOP: [u8; 2] = [0x00, 0x00];

fn machine(cfg3l: u8) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    let prog = [SLEEP, NOP, NOP, NOP].concat();
    let hex = rec(0, 0, &prog) + &rec(0, 4, &[0x00, 0x30]) + &rec(4, 0, &[cfg3l]) + &rec(0, 1, &[]);
    m.load_hex(&hex).unwrap();
    w(&mut m, "WDTCON0", 0x00); // 32 watchdog clocks: it times out after 16517 cycles
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

/// Timer0 on Fosc/4, 8-bit, period 11 cycles; its flag is TMR0IF = PIR0 bit 5.
fn timer0(m: &mut Machine) {
    w(m, "T0CON1", 0x40);
    w(m, "TMR0H", 0x0A);
    w(m, "T0CON0", 0x80);
}

fn wake(m: &mut Machine) {
    for _ in 0..40000 {
        if m.cpu.state == State::Running {
            return;
        }
        m.cpu.step();
    }
    panic!("never woke");
}

/// Timer0 on Fosc/4 does not count in Sleep and goes on from the same count after the wake-up; its flag stays clear.
#[test]
fn timer0_stops_in_sleep() {
    let mut m = machine(ON);
    timer0(&mut m);
    m.cpu.step(); // SLEEP
    assert_eq!(m.cpu.state, State::Sleeping);
    w(&mut m, "PIR0", 0);
    let before = r(&mut m, "TMR0L");
    wake(&mut m);
    assert!(m.cpu.cycles > 16000, "woke by the watchdog");
    assert_eq!(r(&mut m, "PIR0") & 0x20, 0, "no TMR0IF in Sleep");
    assert_eq!(r(&mut m, "TMR0L"), before, "TMR0L kept");
    for _ in 0..30 {
        m.cpu.step();
    }
    assert_ne!(r(&mut m, "PIR0") & 0x20, 0, "it counts again once awake");
}

/// An enabled Timer0 interrupt cannot wake the part when the timer has no clock: the Sleep goes on.
#[test]
fn stopped_timer_does_not_wake() {
    let mut m = machine(OFF);
    timer0(&mut m);
    w(&mut m, "PIE0", 0x20);
    m.cpu.step();
    for _ in 0..3000 {
        m.cpu.step();
    }
    assert_eq!(m.cpu.state, State::Sleeping);
    assert_eq!(r(&mut m, "PIR0") & 0x20, 0);
}

/// Idle (IDLEN = 1) leaves the peripheral clocks on: the same interrupt wakes the part within a few cycles.
#[test]
fn idle_keeps_peripherals_running() {
    let mut m = machine(OFF);
    timer0(&mut m);
    w(&mut m, "PIE0", 0x20);
    w(&mut m, "CPUDOZE", 0x80);
    m.cpu.step();
    assert_eq!(m.cpu.state, State::Sleeping);
    wake(&mut m);
    assert!(m.cpu.cycles < 100, "TMR0IF at {}", m.cpu.cycles);
    assert_ne!(r(&mut m, "PIR0") & 0x20, 0);
}

/// The EUSART transmitter stops in Sleep: a character written before SLEEP is still in the shift register at the wake-up and ends
/// 160 cycles later (BRGH = 0, SP1BRG = 0).
#[test]
fn uart_transmitter_waits_for_the_wake_up() {
    let mut m = machine(ON);
    w(&mut m, "RC1STA", 0x80);
    w(&mut m, "TX1STA", 0x20);
    w(&mut m, "TX1REG", 0x55);
    m.cpu.step(); // SLEEP
    assert_eq!(r(&mut m, "TX1STA") & 2, 0, "TRMT = 0: sending");
    wake(&mut m);
    assert!(m.cpu.cycles > 16000);
    let woke = m.cpu.cycles;
    assert_eq!(r(&mut m, "TX1STA") & 2, 0, "still sending after the wake-up");
    while r(&mut m, "TX1STA") & 2 == 0 {
        m.cpu.step();
    }
    let done = m.cpu.cycles - woke;
    assert!((140..=162).contains(&done), "the character ended {done} cycles after waking");
}
