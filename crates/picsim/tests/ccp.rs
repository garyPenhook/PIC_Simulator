//! CCP1 capture, compare and PWM and the PPS output pin log (DS40002043F ch.22 p.345-359, 18.2 p.286-287; PIC18F27Q10).

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

fn w(m: &mut Machine, n: &str, v: u8) {
    let a = addr(m, n);
    m.cpu.write(a, v);
}

fn r(m: &mut Machine, n: &str) -> u8 {
    let a = addr(m, n);
    m.cpu.read(a)
}

fn run_to(m: &mut Machine, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

#[test]
fn pwm_sets_at_the_period_match_and_clears_after_the_pulse_width() {
    let mut m = machine("");
    w(&mut m, "RC2PPS", 0x05); // CCP1 on RC2
    w(&mut m, "TRISC", 0xFB);
    w(&mut m, "T2PR", 0x3F); // period (63+1) * 4 Tosc = 64 cycles at 1:1
    w(&mut m, "T2CLKCON", 1); // Fosc/4
    w(&mut m, "CCPR1L", 0x20); // right aligned: 32 Tosc = 8 cycles
    w(&mut m, "CCP1CON", 0x8C);
    w(&mut m, "T2CON", 0x80);
    let start = m.cpu.cycles;
    run_to(&mut m, start + 400);
    let log = m.cpu.p.pin_log.clone();
    assert!(log.len() >= 8, "{log:?}");
    for pair in log.chunks(2).take(4) {
        assert_eq!((pair[0].1, pair[0].2, pair[0].3), (b'C', 2, true));
        assert_eq!((pair[1].1, pair[1].2, pair[1].3), (b'C', 2, false));
        assert_eq!(pair[1].0 - pair[0].0, 32, "pulse width CCPR * TOSC");
    }
    for k in 1..4 {
        assert_eq!(log[2 * k].0 - log[2 * k - 2].0, 256, "period (T2PR+1) * 4 * TOSC");
    }
    }

#[test]
fn pwm_left_aligned_duty_and_zero_duty() {
    let mut m = machine("");
    w(&mut m, "RC2PPS", 0x05);
    w(&mut m, "TRISC", 0xFB);
    w(&mut m, "T2PR", 0xFF);
    w(&mut m, "T2CLKCON", 1);
    w(&mut m, "CCPR1H", 0x10); // FMT = 1: duty = 0x10 << 2 = 64 Tosc
    w(&mut m, "CCP1CON", 0x9C);
    w(&mut m, "T2CON", 0x80);
    let start = m.cpu.cycles;
    run_to(&mut m, start + 600);
    let log = m.cpu.p.pin_log.clone();
    assert!(log.len() >= 2);
    assert_eq!(log[1].0 - log[0].0, 64);
    // Duty 0: the pin is not set.
    let mut z = machine("");
    w(&mut z, "RC2PPS", 0x05);
    w(&mut z, "TRISC", 0xFB);
    w(&mut z, "T2PR", 0x0F);
    w(&mut z, "T2CLKCON", 1);
    w(&mut z, "CCP1CON", 0x8C);
    w(&mut z, "T2CON", 0x80);
    run_to(&mut z, 300);
    assert!(z.cpu.p.pin_log.is_empty());
}

#[test]
fn compare_toggle_sets_the_flag_and_drives_the_pin() {
    let mut m = machine("");
    w(&mut m, "RC2PPS", 0x05);
    w(&mut m, "TRISC", 0xFB);
    w(&mut m, "T1CLK", 1);
    w(&mut m, "CCPR1H", 0);
    w(&mut m, "CCPR1L", 100);
    w(&mut m, "CCP1CON", 0x82); // toggle on match
    w(&mut m, "T1CON", 0x01);
    let start = m.cpu.cycles;
    run_to(&mut m, start + 98);
    assert_eq!(r(&mut m, "PIR6") & 0x01, 0, "no match yet");
    assert!(m.cpu.p.pin_log.is_empty());
    run_to(&mut m, start + 110);
    assert!(m.cpu.p.pin_log.len() == 1, "{:?}", m.cpu.p.pin_log);
    let (tick, port, bit, level) = m.cpu.p.pin_log[0];
    assert_eq!((port, bit, level), (b'C', 2, true));
    assert!((start + 99..=start + 101).contains(&(tick / 4)), "match near cycle 100 after the start: {}", tick / 4 - start);
    assert_eq!(r(&mut m, "CCP1CON") & 0x20, 0x20, "OUT follows the latch");
    assert_eq!(r(&mut m, "PORTC") & 4, 4, "the pin shows the output level");
    // The next match comes after TMR1 wraps (65536 clocks): not yet.
    run_to(&mut m, start + 1000);
    assert_eq!(m.cpu.p.pin_log.len(), 1);
}

#[test]
fn compare_with_clear_resets_timer1_each_period() {
    let mut m = machine("");
    w(&mut m, "RC2PPS", 0x05);
    w(&mut m, "TRISC", 0xFB);
    w(&mut m, "T1CLK", 1);
    w(&mut m, "CCPR1L", 49);
    w(&mut m, "CCP1CON", 0x81); // toggle + clear TMR1
    w(&mut m, "T1CON", 0x01);
    let start = m.cpu.cycles;
    run_to(&mut m, start + 520);
    let log = m.cpu.p.pin_log.clone();
    assert!(log.len() >= 9, "{log:?}");
    for k in 1..log.len() {
        assert_eq!(log[k].0 - log[k - 1].0, 4 * 50, "TMR1 counts 0..49 and is cleared on the next clock: CCPR + 1 clocks apart ([A61])");
        assert_ne!(log[k].3, log[k - 1].3, "the output toggles");
    }
    }

#[test]
fn clearing_ccpxcon_forces_the_output_low() {
    let mut m = machine("");
    w(&mut m, "RC2PPS", 0x05);
    w(&mut m, "TRISC", 0xFB);
    w(&mut m, "T1CLK", 1);
    w(&mut m, "CCPR1L", 10);
    w(&mut m, "CCP1CON", 0x88); // set on match
    w(&mut m, "T1CON", 0x01);
    let s = m.cpu.cycles;
    run_to(&mut m, s + 50);
    assert_eq!(m.cpu.p.pin_log.len(), 1);
    w(&mut m, "CCP1CON", 0);
    assert_eq!(m.cpu.p.pin_log.len(), 2);
    assert!(!m.cpu.p.pin_log[1].3);
    assert_eq!(r(&mut m, "CCP1CON") & 0x20, 0);
}

#[test]
fn capture_copies_timer1_at_the_edge_and_prescales() {
    let mut m = machine("[[pin]]\nport = \"C\"\nbit = 2\nlevel = 1\nat = 300\n[[pin]]\nport = \"C\"\nbit = 2\nlevel = 0\nat = 340\n");
    w(&mut m, "ANSELC", 0);
    w(&mut m, "TRISC", 0xFF);
    w(&mut m, "T1CLK", 1);
    w(&mut m, "CCP1CON", 0x85); // every rising edge
    w(&mut m, "T1CON", 0x01);
    let start = m.cpu.cycles;
    run_to(&mut m, 299);
    assert_eq!(r(&mut m, "PIR6") & 0x01, 0);
    run_to(&mut m, 400);
    let cap = u16::from(r(&mut m, "CCPR1H")) << 8 | u16::from(r(&mut m, "CCPR1L"));
    let expect = 300 - start;
    assert!((expect.saturating_sub(2)..=expect + 2).contains(&u64::from(cap)), "captured {cap}, edge {expect} cycles after the start");
    assert_eq!(r(&mut m, "PIR6") & 0x01, 0x01, "CCP1IF");
}

#[test]
fn capture_falling_edge_and_every_fourth_rising() {
    let mut s = String::new();
    for k in 0..8u64 {
        s += &format!("[[pin]]\nport = \"C\"\nbit = 2\nlevel = 1\nat = {}\n[[pin]]\nport = \"C\"\nbit = 2\nlevel = 0\nat = {}\n", 200 + 40 * k, 220 + 40 * k);
    }
    let mut m = machine(&s);
    w(&mut m, "ANSELC", 0);
    w(&mut m, "TRISC", 0xFF);
    w(&mut m, "T1CLK", 1);
    w(&mut m, "CCP1CON", 0x86); // every 4th rising edge
    w(&mut m, "T1CON", 0x01);
    let start = m.cpu.cycles;
    run_to(&mut m, 200 + 40 * 3 - 5);
    assert_eq!(r(&mut m, "PIR6") & 1, 0, "three rising edges so far");
    run_to(&mut m, 200 + 40 * 3 + 10);
    assert_eq!(r(&mut m, "PIR6") & 1, 1, "fourth rising edge at cycle 320");
    let cap = u16::from(r(&mut m, "CCPR1H")) << 8 | u16::from(r(&mut m, "CCPR1L"));
    assert!((u64::from(cap)).abs_diff(320 - start) <= 2, "{cap}");
}

#[test]
fn unmodelled_pps_sources_keep_showing_the_latch() {
    let mut m = machine("");
    w(&mut m, "RC6PPS", 0x09); // EUSART1 TX is not an output source of the pin layer
    w(&mut m, "TRISC", 0x00);
    w(&mut m, "LATC", 0x40);
    assert_eq!(r(&mut m, "PORTC") & 0x40, 0x40);
    assert!(m.cpu.p.pin_log.is_empty());
}

#[test]
fn q41_is_unchanged() {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F16Q41.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    let a = m.dev.find_sfr("CCP1CON").unwrap().addr;
    m.cpu.write(a, 0x8C);
    assert_eq!(m.cpu.read(a), 0x8C, "no CCP model on the Q41: a plain register");
}

// ---- Timers 3-6 (DS40002043F ch.20, 21, 22.1; CCPTMRS p.359) ----

#[test]
fn timer3_and_timer5_roll_over_and_set_their_own_flags() {
    for n in [3, 5] {
        let mut m = machine("");
        w(&mut m, &format!("T{n}CLK"), 1);
        w(&mut m, &format!("TMR{n}H"), 0xFF);
        w(&mut m, &format!("TMR{n}L"), 0xF0);
        w(&mut m, &format!("T{n}CON"), 0x01);
        let start = m.cpu.cycles;
        run_to(&mut m, start + 10);
        assert_eq!(r(&mut m, &format!("TMR{n}H")), 0xFF);
        run_to(&mut m, start + 40);
        assert_eq!(r(&mut m, &format!("TMR{n}H")), 0x00, "Timer{n} wrapped");
        assert!(r(&mut m, &format!("TMR{n}L")) < 40);
        assert_eq!(r(&mut m, "TMR1L"), 0, "Timer1 is untouched");
    }
}

#[test]
fn timer4_and_timer6_use_their_own_period_register() {
    for n in [4, 6] {
        let mut m = machine("");
        w(&mut m, &format!("T{n}PR"), 9);
        w(&mut m, &format!("T{n}CLKCON"), 1);
        w(&mut m, &format!("T{n}CON"), 0x80);
        let start = m.cpu.cycles;
        run_to(&mut m, start + 25);
        let t = r(&mut m, &format!("T{n}TMR"));
        assert!(t <= 9, "Timer{n} counts 0..PR: {t}");
        assert_eq!(r(&mut m, "T2TMR"), 0, "Timer2 is off");
    }
}

#[test]
fn compare_on_timer3_through_ccptmrs() {
    let mut m = machine("");
    w(&mut m, "RC2PPS", 0x05);
    w(&mut m, "TRISC", 0xFB);
    w(&mut m, "CCPTMRS", 0x56); // C1TSEL = 10: Timer3
    w(&mut m, "T3CLK", 1);
    w(&mut m, "CCPR1L", 49);
    w(&mut m, "CCP1CON", 0x81); // toggle + clear the timer
    w(&mut m, "T3CON", 0x01);
    let start = m.cpu.cycles;
    run_to(&mut m, start + 520);
    let log = m.cpu.p.pin_log.clone();
    assert!(log.len() >= 9, "{log:?}");
    for k in 1..log.len() {
        assert_eq!(log[k].0 - log[k - 1].0, 4 * 50, "Timer3 period CCPR + 1 clocks");
    }
    assert_eq!(r(&mut m, "TMR1L"), 0, "Timer1 stays off");
    assert!(m.cpu.p.notes.iter().all(|n| !n.contains("CCP1")), "{:?}", m.cpu.p.notes);
}

#[test]
fn pwm_on_timer4_through_ccptmrs() {
    let mut m = machine("");
    w(&mut m, "RC2PPS", 0x05);
    w(&mut m, "TRISC", 0xFB);
    w(&mut m, "CCPTMRS", 0x56); // C1TSEL = 10: PWM on Timer4
    w(&mut m, "T4PR", 0x3F);
    w(&mut m, "T4CLKCON", 1);
    w(&mut m, "CCPR1L", 0x20);
    w(&mut m, "CCP1CON", 0x8C);
    w(&mut m, "T4CON", 0x80);
    let start = m.cpu.cycles;
    run_to(&mut m, start + 400);
    let log = m.cpu.p.pin_log.clone();
    assert!(log.len() >= 8, "{log:?}");
    assert_eq!(log[1].0 - log[0].0, 32, "pulse width");
    assert_eq!(log[2].0 - log[0].0, 256, "period");
    // Timer2 running does not drive a CCP that selected Timer4.
    let mut m = machine("");
    w(&mut m, "RC2PPS", 0x05);
    w(&mut m, "TRISC", 0xFB);
    w(&mut m, "CCPTMRS", 0x56);
    w(&mut m, "T2PR", 0x3F);
    w(&mut m, "T2CLKCON", 1);
    w(&mut m, "CCPR1L", 0x20);
    w(&mut m, "CCP1CON", 0x8C);
    w(&mut m, "T2CON", 0x80);
    let s = m.cpu.cycles;
    run_to(&mut m, s + 400);
    assert!(m.cpu.p.pin_log.is_empty());
}

#[test]
fn capture_reads_timer5() {
    let mut m = machine("[[pin]]\nport = \"C\"\nbit = 2\nlevel = 1\nat = 300\n");
    w(&mut m, "ANSELC", 0);
    w(&mut m, "TRISC", 0xFF);
    w(&mut m, "CCPTMRS", 0x57); // C1TSEL = 11: Timer5
    w(&mut m, "T5CLK", 1);
    w(&mut m, "CCP1CON", 0x85);
    w(&mut m, "T5CON", 0x01);
    let start = m.cpu.cycles;
    run_to(&mut m, 400);
    let cap = u64::from(r(&mut m, "CCPR1H")) << 8 | u64::from(r(&mut m, "CCPR1L"));
    let expect = 300 - start;
    assert!((expect.saturating_sub(2)..=expect + 2).contains(&cap), "captured {cap}, edge {expect}");
}
