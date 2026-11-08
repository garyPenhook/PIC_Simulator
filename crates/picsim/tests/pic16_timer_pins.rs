//! Timer0 and Timer1 on the PIC16 core with their clock and gate on pins (`[A127]`): Timer0 CS = 000 (rising edges) and 001
//! (inverted: falling edges) of T0CKIPPS (PIC16F17146 DS40002343F 22.5.2 p.248, PIC16F18346 DS40001839E Register 26-4 p.281);
//! Timer1 CS = 0 of TxCLK (DS40002343F Table 23-4 p.264) or TMRxCS = 10 with TxSOSC = 0 (DS40001839E Register 27-1 p.293); the
//! gate enable mode on the T1GPPS pin (GE, GPOL, GSS = pin: DS40002343F 23.13.2 p.262, Table 23-5 p.265); asynchronous operation
//! in Sleep (22.4 p.246, 23.5 p.255). RA2 is the clock pin and RA4 the gate pin; both are driven by `[[pin]]` entries.

use devdesc::Device;
use picsim::machine16::Machine16;
use std::path::Path;

const PARTS: [&str; 2] = ["16F17146", "16F18346"];
const NOP: u16 = 0;
const SLEEP: u16 = 0x0063;
const BRA_SELF: u16 = 0x33FF;

fn hexrec(b: &[u8]) -> String {
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

/// `[[pin]]` entries: RA`bit` takes `level` at cycle `at`.
fn pins(list: &[(u8, u8, u64)]) -> String {
    list.iter().map(|&(bit, level, at)| format!("[[pin]]\nport = \"A\"\nbit = {bit}\nlevel = {level}\nat = {at}\n")).collect()
}

/// RA2 rises at 100, 200, 300 and falls at 150, 250.
fn clock_pulses() -> String {
    pins(&[(2, 1, 100), (2, 0, 150), (2, 1, 200), (2, 0, 250), (2, 1, 300)])
}

fn machine(part: &str, prog: &[u16], stim: &str) -> Machine16 {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let mut m = Machine16::new(dev).unwrap();
    let mut text = String::new();
    let mut put = |w: u32, v: u16| {
        for (a, b) in [(2 * w, v as u8), (2 * w + 1, (v >> 8) as u8)] {
            text += &hexrec(&[2, 0, 0, 4, (a >> 24) as u8, (a >> 16) as u8]);
            text += &hexrec(&[1, (a >> 8) as u8, a as u8, 0, b]);
        }
    };
    for (i, &w) in prog.iter().enumerate() {
        put(i as u32, w);
    }
    for r in m.dev.config.clone() {
        if let Some(f) = r.fields.iter().find(|f| f.name == "WDTE") {
            put(r.addr, (r.default & !(((1u32 << f.width) - 1) << f.pos)) as u16); // the watchdog is off
        }
    }
    m.load_hex(&(text + ":00000001FF\n")).unwrap();
    m.load_stimulus(stim).unwrap();
    w(&mut m, "ANSELA", 0);
    w(&mut m, "T0CKIPPS", 0x02); // RA2
    w(&mut m, "T1CKIPPS", 0x02);
    w(&mut m, "T1GPPS", 0x04); // RA4
    m
}

fn a(m: &Machine16, n: &str) -> u32 {
    m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr
}

fn r(m: &mut Machine16, n: &str) -> u8 {
    let x = a(m, n);
    m.cpu.read(x)
}

fn w(m: &mut Machine16, n: &str, v: u8) {
    let x = a(m, n);
    m.cpu.write(x, v);
}

fn run_to(m: &mut Machine16, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

/// Register and mask of a bit that sits in one of the registers whose name starts with `prefix`.
fn bit(m: &Machine16, prefix: &str, name: &str) -> (u32, u8) {
    m.dev.sfr.iter().filter(|s| s.name.starts_with(prefix)).find_map(|s| s.fields.iter().find(|f| f.name == name).map(|f| (s.addr, 1u8 << f.pos))).unwrap_or_else(|| panic!("no {name}"))
}

/// Timer1 on the pin clock: T1CLK = 0 or, on the PIC16F183xx, TMR1CS = 10 in T1CON.
fn timer1_on_pin(m: &mut Machine16, part: &str, extra: u8) {
    if part == "16F18346" {
        w(m, "T1CON", 0x81 | extra);
    } else {
        w(m, "T1CLK", 0);
        w(m, "T1CON", 0x01 | extra);
    }
}

#[test]
fn timer0_counts_the_rising_edges_of_its_pin_and_the_inverted_pin_the_falling_ones() {
    for part in PARTS {
        let mut m = machine(part, &[BRA_SELF], &clock_pulses());
        w(&mut m, "T0CON1", 0x00); // CS = 000: T0CKIPPS, ASYNC = 0, 1:1
        w(&mut m, "T0CON0", 0x90); // 16-bit, enabled
        run_to(&mut m, 98); // the loop takes two cycles a step: 98 or 99 stops before the edge at 100
        assert_eq!(r(&mut m, "TMR0L"), 0, "{part}");
        run_to(&mut m, 120);
        assert_eq!(r(&mut m, "TMR0L"), 1, "{part}: the rise at 100");
        run_to(&mut m, 400);
        assert_eq!(r(&mut m, "TMR0L"), 3, "{part}: rises at 100, 200, 300");

        let mut m = machine(part, &[BRA_SELF], &clock_pulses());
        w(&mut m, "T0CON1", 0x20); // CS = 001: inverted, so the falling edges count
        w(&mut m, "T0CON0", 0x90);
        run_to(&mut m, 400);
        assert_eq!(r(&mut m, "TMR0L"), 2, "{part}: falls at 150, 250");
    }
}

#[test]
fn timer0_prescaler_divides_the_pin_edges() {
    for part in PARTS {
        let mut m = machine(part, &[BRA_SELF], &clock_pulses());
        w(&mut m, "T0CON1", 0x01); // 1:2
        w(&mut m, "T0CON0", 0x90);
        run_to(&mut m, 400);
        assert_eq!(r(&mut m, "TMR0L"), 1, "{part}: 3 edges, 1:2");
    }
}

#[test]
fn timer1_counts_the_rising_edges_of_its_pin() {
    for part in PARTS {
        let mut m = machine(part, &[BRA_SELF], &clock_pulses());
        timer1_on_pin(&mut m, part, 0);
        run_to(&mut m, 98);
        assert_eq!(r(&mut m, "TMR1L"), 0, "{part}");
        run_to(&mut m, 400);
        assert_eq!(r(&mut m, "TMR1L"), 3, "{part}");
        assert_eq!(r(&mut m, "TMR1H"), 0, "{part}");
    }
}

#[test]
fn timer1_gate_pin_lets_the_timer_count_only_while_it_is_active() {
    // RA4 is high from cycle 100 to 199: an active-high gate lets Fosc/4 count for 100 cycles, an active-low one for the rest
    let stim = pins(&[(4, 1, 100), (4, 0, 200)]);
    for part in PARTS {
        let (gcon_hi, gcon_lo) = (0xC0u8, 0x80u8); // GE with GPOL = 1 / 0 (GSS = 0: the pin)
        for (gcon, lo, hi) in [(gcon_hi, 99u16, 101u16), (gcon_lo, 299, 301)] {
            let mut m = machine(part, &[BRA_SELF], &stim);
            w(&mut m, "T1GCON", gcon);
            if part == "16F18346" {
                w(&mut m, "T1CON", 0x01); // Fosc/4
            } else {
                w(&mut m, "T1GATE", 0);
                w(&mut m, "T1CLK", 1);
                w(&mut m, "T1CON", 0x01);
            }
            run_to(&mut m, 400);
            let n = u16::from(r(&mut m, "TMR1H")) << 8 | u16::from(r(&mut m, "TMR1L"));
            assert!((lo..=hi).contains(&n), "{part}: GCON {gcon:#04x}: {n}");
        }
    }
}

#[test]
fn timer1_gate_with_a_source_other_than_the_pin_holds_the_count_and_says_so() {
    let mut m = machine("16F17146", &[BRA_SELF], "");
    w(&mut m, "T1GATE", 1); // TMR0_overflow_OUT
    w(&mut m, "T1GCON", 0x80);
    w(&mut m, "T1CLK", 1);
    w(&mut m, "T1CON", 0x01);
    run_to(&mut m, 50);
    assert_eq!(r(&mut m, "TMR1L"), 0);
    assert!(m.cpu.notes.iter().any(|n| n.contains("Timer1 gate")));
}

#[test]
fn an_asynchronous_timer0_on_its_pin_counts_in_sleep_and_wakes_the_part() {
    for part in PARTS {
        let mut m = machine(part, &[SLEEP, NOP, BRA_SELF], &clock_pulses());
        w(&mut m, "T0CON1", 0x10); // CS = 000, ASYNC = 1
        w(&mut m, "TMR0H", 1); // 8-bit mode: the flag is set when the second edge matches
        w(&mut m, "T0CON0", 0x80);
        let (pie, mask) = bit(&m, "PIE", "TMR0IE");
        let v = r(&mut m, "PIE0") | mask;
        w(&mut m, "PIE0", v);
        assert_eq!(pie, a(&m, "PIE0"), "{part}");
        m.cpu.step(); // SLEEP
        while m.cpu.pc == 1 && m.cpu.cycles < 1000 {
            m.cpu.step();
        }
        assert_eq!(m.cpu.pc, 2, "{part}: the part woke and ran the NOP after SLEEP");
        assert!((200..=203).contains(&m.cpu.cycles), "{part}: woke at {}, the second rise is at 200", m.cpu.cycles);
    }
}

#[test]
fn a_synchronous_timer0_halts_in_sleep() {
    for part in PARTS {
        let mut m = machine(part, &[SLEEP, NOP, BRA_SELF], &clock_pulses());
        w(&mut m, "T0CON1", 0x00); // ASYNC = 0
        w(&mut m, "TMR0H", 1);
        w(&mut m, "T0CON0", 0x80);
        let (_, mask) = bit(&m, "PIE", "TMR0IE");
        let v = r(&mut m, "PIE0") | mask;
        w(&mut m, "PIE0", v);
        for _ in 0..400 {
            m.cpu.step();
            if m.cpu.sleep_dead() {
                break;
            }
        }
        assert!(m.cpu.sleep_dead(), "{part}: nothing wakes it");
        assert_eq!(m.cpu.pc, 1, "{part}");
    }
}

#[test]
fn timer1_with_sync_set_counts_in_sleep_and_wakes_on_the_overflow() {
    for part in PARTS {
        let mut m = machine(part, &[SLEEP, NOP, BRA_SELF], &clock_pulses());
        w(&mut m, "TMR1H", 0xFF);
        w(&mut m, "TMR1L", 0xFE); // two edges to the roll-over
        timer1_on_pin(&mut m, part, 0x04); // SYNC = 1
        let (pie, mask) = bit(&m, "PIE", "TMR1IE");
        let reg = m.dev.sfr.iter().find(|s| s.addr == pie).unwrap().name.clone();
        let v = r(&mut m, &reg) | mask;
        w(&mut m, &reg, v);
        m.cpu.step();
        while m.cpu.pc == 1 && m.cpu.cycles < 1000 {
            m.cpu.step();
        }
        assert_eq!(m.cpu.pc, 2, "{part}");
        assert!((200..=203).contains(&m.cpu.cycles), "{part}: woke at {}", m.cpu.cycles);
    }
}
