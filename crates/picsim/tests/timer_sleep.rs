//! Timer0 and Timer1 on the clocks that go on in Sleep: the T0CKIPPS / T1CKIPPS pin, LFINTOSC (31 kHz) and SOSC (32.768 kHz)
//! (DS40002043F 19.2 p.296-297, 19.4 p.298, 20.2 p.306, 20.5 p.308, 20.9 p.314, 7.4 p.71; PIC18F27Q10). Fosc is 64 MHz, so the
//! instruction clock is 16 MHz and one LFINTOSC edge is 516.13 cycles, one SOSC edge 488.28.

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

const OFF: u8 = 0x1F; // WDTE = 00
const ON: u8 = 0x7F; // WDTE = 11: the watchdog wakes the part after 16517 cycles
const SLEEP_BRA: [u8; 4] = [0x03, 0x00, 0xFF, 0xD7];

fn machine(cfg3l: u8, stim: &str) -> Machine {
    machine_with(&SLEEP_BRA, cfg3l, stim)
}

/// A part that stays awake: `BRA $`.
fn awake(stim: &str) -> Machine {
    machine_with(&[0xFF, 0xD7], OFF, stim)
}

fn machine_with(prog: &[u8], cfg3l: u8, stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_hex(&(rec(0, 0, prog) + &rec(0, 4, &[0x00, 0x30]) + &rec(4, 0, &[cfg3l]) + &rec(0, 1, &[]))).unwrap();
    m.load_stimulus(stim).unwrap();
    w(&mut m, "ANSELA", 0);
    w(&mut m, "ANSELC", 0);
    w(&mut m, "WDTCON0", 0x00);
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

fn t1(m: &mut Machine) -> u16 {
    u16::from(r(m, "TMR1L")) | u16::from(r(m, "TMR1H")) << 8
}

fn run_to(m: &mut Machine, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

/// Run until the part is awake, at most `limit` steps.
fn wake(m: &mut Machine, limit: u32) {
    for _ in 0..limit {
        if m.cpu.state == State::Running {
            return;
        }
        m.cpu.step();
    }
    panic!("never woke (cycle {})", m.cpu.cycles);
}

/// Edges of an oscillator of `hz` in `cycles` instruction cycles at 16 MHz.
fn edges(cycles: u64, hz: u64) -> u64 {
    cycles * hz / 16_000_000
}

fn pulses(n: u64, first: u64, period: u64) -> String {
    (0..n)
        .map(|k| {
            let t = first + k * period;
            format!("[[pin]]\nport = \"C\"\nbit = 0\nlevel = 1\nat = {t}\n[[pin]]\nport = \"C\"\nbit = 0\nlevel = 0\nat = {}\n", t + period / 2)
        })
        .collect()
}

/// Awake, Timer1 on LFINTOSC (CS = 00100) and on SOSC (CS = 00110) counts the oscillator's edges, not instruction cycles.
#[test]
fn timer1_counts_lfintosc_and_sosc_awake() {
    for (cs, hz) in [(4u8, 31_000u64), (6, 32_768)] {
        let mut m = awake("");
        w(&mut m, "T1CLK", cs);
        w(&mut m, "T1CON", 0x01);
        run_to(&mut m, 100_000);
        let c = m.cpu.cycles;
        let n = u64::from(t1(&mut m));
        let want = edges(c, hz);
        assert!(n.abs_diff(want) <= 1, "CS={cs}: {n} counts, expected {want} at cycle {c}");
    }
}

/// The prescaler divides the oscillator edges.
#[test]
fn timer1_prescales_lfintosc() {
    let mut m = awake("");
    w(&mut m, "T1CLK", 4);
    w(&mut m, "T1CON", 0x31); // 1:8
    run_to(&mut m, 100_000);
    let c = m.cpu.cycles;
    let n = u64::from(t1(&mut m));
    assert!(n.abs_diff(edges(c, 31_000) / 8) <= 1, "{n} counts at cycle {c}");
}

/// Timer0 on LFINTOSC: T0CS = 100, 16-bit mode, 1:1.
#[test]
fn timer0_counts_lfintosc_awake() {
    let mut m = awake("");
    w(&mut m, "T0CON1", 0x80);
    w(&mut m, "T0CON0", 0x90);
    run_to(&mut m, 100_000);
    let c = m.cpu.cycles;
    let n = u64::from(r(&mut m, "TMR0L")) | u64::from(r(&mut m, "TMR0H")) << 8;
    assert!(n.abs_diff(edges(c, 31_000)) <= 1, "{n} counts at cycle {c}");
}

/// Timer1 SYNC = 1 on LFINTOSC goes on counting in Sleep and its overflow wakes the part on the edge that makes it: TMR1 starts 10
/// below the rollover, so the tenth edge, cycle ceil(10 * 516.129) = 5162 from the oscillator's start, sets TMR1IF.
#[test]
fn timer1_lfintosc_wakes_the_part() {
    let mut m = machine(OFF, "");
    w(&mut m, "T1CLK", 4);
    w(&mut m, "TMR1H", 0xFF);
    w(&mut m, "TMR1L", 0xF6);
    w(&mut m, "T1CON", 0x05); // ON, SYNC = 1
    w(&mut m, "PIE4", 0x01);
    w(&mut m, "INTCON", 0x40); // PEIE, GIE clear: the next instruction runs
    m.cpu.step(); // SLEEP
    assert_eq!(m.cpu.state, State::Sleeping);
    wake(&mut m, 100_000);
    assert_eq!(m.cpu.cycles, 5162, "woke on the tenth edge");
    assert_eq!(r(&mut m, "PIR4") & 1, 1, "TMR1IF");
    assert_eq!(t1(&mut m), 0);
}

/// SOSC: the same, TMR1 3 below the rollover, the third edge at ceil(3 * 488.28) = 1465.
#[test]
fn timer1_sosc_wakes_the_part() {
    let mut m = machine(OFF, "");
    w(&mut m, "T1CLK", 6);
    w(&mut m, "TMR1H", 0xFF);
    w(&mut m, "TMR1L", 0xFD);
    w(&mut m, "T1CON", 0x05);
    w(&mut m, "PIE4", 0x01);
    w(&mut m, "INTCON", 0x40);
    m.cpu.step();
    wake(&mut m, 100_000);
    assert_eq!(m.cpu.cycles, 1465);
    assert_eq!(r(&mut m, "PIR4") & 1, 1);
}

/// A long Sleep is not stepped cycle by cycle: 65536 LFINTOSC edges (2.1 s) are reached in few steps.
#[test]
fn a_long_sleep_on_an_oscillator_takes_few_steps() {
    let mut m = machine(OFF, "");
    w(&mut m, "T1CLK", 4);
    w(&mut m, "T1CON", 0x05);
    w(&mut m, "PIE4", 0x01);
    w(&mut m, "INTCON", 0x40);
    m.cpu.step();
    let mut steps = 0;
    while m.cpu.state != State::Running {
        m.cpu.step();
        steps += 1;
        assert!(steps < 200_000, "stuck");
    }
    assert_eq!(m.cpu.cycles, (65536u128 * 64_000_000).div_ceil(4 * 31_000) as u64);
}

/// SYNC = 0 needs the instruction clock to synchronise the input: Timer1 halts in Sleep (20.1 p.305 note 3, 20.9 p.314) and
/// goes on from its count after the wake-up, which here is the watchdog.
#[test]
fn timer1_without_sync_halts_in_sleep() {
    let mut m = machine(ON, "");
    w(&mut m, "T1CLK", 4);
    w(&mut m, "T1CON", 0x01);
    m.cpu.step();
    let before = t1(&mut m);
    wake(&mut m, 100_000);
    assert!(m.cpu.cycles > 16_000, "woke by the watchdog");
    let after = t1(&mut m);
    assert!(u64::from(after).abs_diff(u64::from(before)) <= 1, "{before} before, {after} after");
    let target = m.cpu.cycles + 2000;
    run_to(&mut m, target);
    assert!(t1(&mut m) > after + 2, "it counts again once awake");
}

/// SYNC = 1 on an oscillator counts through a Sleep that the watchdog ends, and the count is not moved by the wake-up.
#[test]
fn timer1_with_sync_counts_through_a_watchdog_sleep() {
    let mut m = machine(ON, "");
    w(&mut m, "T1CLK", 6);
    w(&mut m, "T1CON", 0x05);
    m.cpu.step();
    wake(&mut m, 100_000);
    let c = m.cpu.cycles;
    assert!(c > 16_000);
    let n = u64::from(t1(&mut m));
    assert!(n.abs_diff(edges(c, 32_768)) <= 1, "{n} counts at cycle {c}");
}

/// Timer0 on LFINTOSC with T0ASYNC = 1, 8-bit with TMR0H = 3: a match every 4 edges; the fourth edge is cycle 2065 and TMR0IF
/// (PIR0 bit 5, enabled by TMR0IE) wakes the part.
#[test]
fn timer0_async_lfintosc_wakes_the_part() {
    let mut m = machine(OFF, "");
    w(&mut m, "T0CON1", 0x90);
    w(&mut m, "TMR0H", 3);
    w(&mut m, "T0CON0", 0x80);
    w(&mut m, "PIE0", 0x20);
    m.cpu.step();
    assert_eq!(m.cpu.state, State::Sleeping);
    wake(&mut m, 100_000);
    assert_eq!(m.cpu.cycles, 2065);
    assert_eq!(r(&mut m, "PIR0") & 0x20, 0x20);
}

/// T0ASYNC = 0 on LFINTOSC: the input is synchronised to Fosc/4 and Timer0 halts in Sleep (19.2.2 p.296).
#[test]
fn timer0_sync_lfintosc_halts_in_sleep() {
    let mut m = machine(ON, "");
    w(&mut m, "T0CON1", 0x80);
    w(&mut m, "TMR0H", 3);
    w(&mut m, "T0CON0", 0x80);
    w(&mut m, "PIE0", 0x20);
    m.cpu.step();
    wake(&mut m, 100_000);
    assert!(m.cpu.cycles > 16_000, "the watchdog woke it, not Timer0");
    assert_eq!(r(&mut m, "PIR0") & 0x20, 0, "no TMR0IF in Sleep");
}

/// The pin of T1CKIPPS (RC0 by default) counts its rising edges, awake and in Sleep; the edge that overflows Timer1 wakes the part.
#[test]
fn timer1_pin_counts_rising_edges_and_wakes() {
    let mut m = machine(OFF, &pulses(6, 1000, 200));
    w(&mut m, "TRISC", 0xFF);
    w(&mut m, "T1CLK", 0);
    w(&mut m, "T1CON", 0x05);
    run_to(&mut m, 1500);
    assert_eq!(t1(&mut m), 3, "edges at 1000, 1200, 1400");
    // Back to the start with the SLEEP instruction: a new machine, TMR1 = 0xFFFE so the second edge overflows.
    let mut m = machine(OFF, &pulses(6, 1000, 200));
    w(&mut m, "TRISC", 0xFF);
    w(&mut m, "T1CLK", 0);
    w(&mut m, "TMR1H", 0xFF);
    w(&mut m, "TMR1L", 0xFE);
    w(&mut m, "T1CON", 0x05);
    w(&mut m, "PIE4", 0x01);
    w(&mut m, "INTCON", 0x40);
    m.cpu.step();
    assert_eq!(m.cpu.state, State::Sleeping);
    wake(&mut m, 10_000);
    assert_eq!(m.cpu.cycles, 1200, "the second rising edge");
    assert_eq!(r(&mut m, "PIR4") & 1, 1);
}

/// Timer0 on the inverted pin (T0CS = 001) counts falling edges; the T0CKIPPS pin is RA4 by default.
#[test]
fn timer0_inverted_pin_counts_falling_edges() {
    let stim: String = (0..4)
        .map(|k| {
            let t = 1000 + 200 * k;
            format!("[[pin]]\nport = \"A\"\nbit = 4\nlevel = 1\nat = {t}\n[[pin]]\nport = \"A\"\nbit = 4\nlevel = 0\nat = {}\n", t + 100)
        })
        .collect();
    let mut m = machine(OFF, &stim);
    w(&mut m, "TRISA", 0xFF);
    w(&mut m, "T0CON1", 0x30); // T0CS = 001, T0ASYNC = 1
    w(&mut m, "T0CON0", 0x90); // 16-bit
    run_to(&mut m, 1250);
    let lo = r(&mut m, "TMR0L");
    assert_eq!(lo, 1, "falling edge at 1100 only");
    run_to(&mut m, 1900);
    assert_eq!(r(&mut m, "TMR0L"), 4);
}

/// Idle keeps every clock running, so an oscillator timer needs no special rule there: the same wake-up as awake.
#[test]
fn idle_runs_the_oscillator_timer() {
    let mut m = machine(OFF, "");
    w(&mut m, "T1CLK", 4);
    w(&mut m, "TMR1H", 0xFF);
    w(&mut m, "TMR1L", 0xFD);
    w(&mut m, "T1CON", 0x01);
    w(&mut m, "PIE4", 0x01);
    w(&mut m, "INTCON", 0x40);
    w(&mut m, "CPUDOZE", 0x80);
    m.cpu.step();
    wake(&mut m, 100_000);
    assert_eq!(m.cpu.cycles, 1549, "third edge ceil(3 * 516.129)");
}

/// A CCP compare on Timer1 running from LFINTOSC in Sleep sets CCP1IF (PIR6 bit 0) on the edge where TMR1 reaches CCPR1: the
/// fifth edge, cycle ceil(5 * 516.129) = 2581, and wakes the part.
#[test]
fn ccp_compare_on_an_oscillator_timer_wakes_the_part() {
    let mut m = machine(OFF, "");
    w(&mut m, "T1CLK", 4);
    w(&mut m, "CCPR1L", 5);
    w(&mut m, "CCPR1H", 0);
    w(&mut m, "CCP1CON", 0x8A); // compare, flag only
    w(&mut m, "T1CON", 0x05);
    w(&mut m, "PIE6", 0x01);
    w(&mut m, "INTCON", 0x40);
    m.cpu.step();
    assert_eq!(m.cpu.state, State::Sleeping);
    wake(&mut m, 100_000);
    assert_eq!(m.cpu.cycles, 2581);
    assert_eq!(r(&mut m, "PIR6") & 1, 1, "CCP1IF");
}

/// The Q41 selects SOSC with other codes: Timer0 T0CS = 110, Timer1 CS = 00111 (DS40002214G p.360, p.377). Code 101 / 00110 are
/// MFINTOSC there, which is not modelled and does not count.
#[test]
fn q41_sosc_codes() {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F16Q41.toml")).unwrap();
    for (cs, counts) in [(7u8, true), (6, false)] {
        let mut m = Machine::new(dev.clone()).unwrap();
        m.load_hex(&(rec(0, 0, &[0xFF, 0xD7]) + &rec(0, 1, &[]))).unwrap();
        w(&mut m, "T1CLK", cs);
        w(&mut m, "T1CON", 0x01);
        run_to(&mut m, 50_000);
        let c = m.cpu.cycles;
        let n = u64::from(t1(&mut m));
        if counts {
            assert!(n.abs_diff(edges(c, 32_768)) <= 1, "{n} counts at cycle {c}");
        } else {
            assert_eq!(n, 0);
        }
    }
    let mut m = Machine::new(dev).unwrap();
    m.load_hex(&(rec(0, 0, &[0xFF, 0xD7]) + &rec(0, 1, &[]))).unwrap();
    w(&mut m, "T0CON1", 0xC0); // CS = 110
    w(&mut m, "T0CON0", 0x90);
    run_to(&mut m, 50_000);
    let c = m.cpu.cycles;
    let n = u64::from(r(&mut m, "TMR0L")) | u64::from(r(&mut m, "TMR0H")) << 8;
    assert!(n.abs_diff(edges(c, 32_768)) <= 1, "{n} counts at cycle {c}");
}
