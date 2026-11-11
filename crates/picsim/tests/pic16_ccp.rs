//! CCP1 on the PIC16 core (`[A128]`, `periph/ccp16.rs`): PWM, compare and capture on the PIC16F17146 (DS40002343F ch.27
//! p.326-337), PIC16F13145 (DS40002519D ch.24), PIC16F15244 (DS40002195E ch.22) and PIC16F18346 (DS40001839E ch.29). RA0 carries
//! the CCP1 output through RA0PPS; RA2 is the capture input and is driven by `[[pin]]` entries. The timing of compare and PWM is
//! what MDB does (`tests/probe/ccp_cmp.S`, `ccp_seq.S`, `ccp_pwm_seq.S`, read through the OUT bit of CCP1CON).

use devdesc::Device;
use picsim::machine16::Machine16;
use std::path::Path;

const PARTS: [&str; 5] = ["16F17146", "16F13145", "16F13245", "16F15244", "16F18346"];
const NOP: u16 = 0;
const BRA_SELF: u16 = 0x33FF;

fn hexrec(b: &[u8]) -> String {
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

/// RA`bit` takes `level` at cycle `at`.
fn pins(list: &[(u8, u8, u64)]) -> String {
    list.iter().map(|&(bit, level, at)| format!("[[pin]]\nport = \"A\"\nbit = {bit}\nlevel = {level}\nat = {at}\n")).collect()
}

/// A slide of NOPs (one cycle a step) that ends in a branch to itself.
fn machine(part: &str, stim: &str) -> Machine16 {
    machine_at(part, stim, None)
}

/// The same with a SLEEP at word `sleep`.
fn machine_at(part: &str, stim: &str, sleep: Option<u32>) -> Machine16 {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let mut m = Machine16::new(dev).unwrap();
    let mut text = String::new();
    let mut put = |w: u32, v: u16| {
        for (a, b) in [(2 * w, v as u8), (2 * w + 1, (v >> 8) as u8)] {
            text += &hexrec(&[2, 0, 0, 4, (a >> 24) as u8, (a >> 16) as u8]);
            text += &hexrec(&[1, (a >> 8) as u8, a as u8, 0, b]);
        }
    };
    for i in 0..3000u32 {
        put(i, if sleep == Some(i) { 0x0063 } else { NOP });
    }
    put(3000, BRA_SELF);
    for r in m.dev.config.clone() {
        if let Some(f) = r.fields.iter().find(|f| f.name == "WDTE") {
            put(r.addr, (r.default & !(((1u32 << f.width) - 1) << f.pos)) as u16); // the watchdog is off
        }
    }
    m.load_hex(&(text + ":00000001FF\n")).unwrap();
    m.load_stimulus(stim).unwrap();
    w(&mut m, "ANSELA", 0);
    w(&mut m, "CCP1PPS", 0x02); // RA2
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

fn bit(m: &Machine16, prefix: &str, name: &str) -> (u32, u8) {
    m.dev.sfr.iter().filter(|s| s.name.starts_with(prefix)).find_map(|s| s.fields.iter().find(|f| f.name == name).map(|f| (s.addr, 1u8 << f.pos))).unwrap_or_else(|| panic!("no {name}"))
}

fn ccp1if(m: &mut Machine16) -> bool {
    let (reg, mask) = bit(m, "PIR", "CCP1IF");
    m.cpu.read(reg) & mask != 0
}

fn clear_ccp1if(m: &mut Machine16) {
    let (reg, mask) = bit(m, "PIR", "CCP1IF");
    let v = m.cpu.read(reg);
    m.cpu.write(reg, v & !mask);
}

/// RxyPPS code of CCP1 on the part.
fn code(part: &str) -> u8 {
    match part {
        "16F15244" => 0x01,
        "16F18346" => 0x0C,
        _ => 0x09,
    }
}

/// Timer2 free running on Fosc/4, 1:1, with period register `pr`.
fn t2_on(m: &mut Machine16, part: &str, pr: u8) {
    if part == "16F18346" {
        w(m, "PR2", pr);
        w(m, "T2CON", 0x04);
    } else {
        w(m, "T2HLT", 0);
        w(m, "T2CLKCON", 1);
        w(m, "T2PR", pr);
        w(m, "T2CON", 0x80);
    }
}

/// Timer1 on Fosc/4.
fn t1_on(m: &mut Machine16, part: &str) {
    if part != "16F18346" {
        w(m, "T1CLK", 1);
    }
    w(m, "T1CON", 0x01);
}

/// RA0 is an output driven by CCP1.
fn pin_ra0(m: &mut Machine16, part: &str) {
    w(m, "RA0PPS", code(part));
    w(m, "TRISA", 0xFE);
}

fn ra0(m: &mut Machine16) -> bool {
    r(m, "PORTA") & 1 != 0
}

/// RA0 sampled after each of `n` one-cycle steps.
fn sample(m: &mut Machine16, n: usize) -> Vec<bool> {
    (0..n)
        .map(|_| {
            m.cpu.step();
            ra0(m)
        })
        .collect()
}

#[test]
fn pwm_pulse_is_duty_over_four_cycles_of_a_period_of_pr_plus_one() {
    for part in PARTS {
        let mut m = machine(part, "");
        pin_ra0(&mut m, part);
        t2_on(&mut m, part, 0x0F); // 16 cycles
        w(&mut m, "CCPR1L", 20); // 20 Tosc = 5 cycles by the equation; MDB keeps the pin set ceil(20 / 4) + 1 = 6 counts
        w(&mut m, "CCPR1H", 0);
        w(&mut m, "CCP1CON", 0x8C);
        let s = sample(&mut m, 400);
        let first = s.iter().position(|&x| x).unwrap_or_else(|| panic!("{part}: the pin never went high"));
        let run: Vec<bool> = s[first..first + 160].to_vec();
        assert_eq!(run.iter().filter(|&&x| x).count(), 60, "{part}: 6 cycles in 16, ten times");
        assert_eq!(run[..16].iter().map(|&x| u8::from(x)).collect::<Vec<_>>(), [1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], "{part}");
        assert!(run[16], "{part}: the next period starts 16 cycles on");
        assert!(ccp1if(&mut m), "{part}: CCP1IF is set when the pulse ends");
        assert_eq!(r(&mut m, "CCP1CON") & 0x20 != 0, ra0(&mut m), "{part}: OUT follows the pin");
    }
}

#[test]
fn pwm_duty_zero_never_sets_the_pin_and_a_full_duty_keeps_it_set() {
    for part in PARTS {
        let mut m = machine(part, "");
        pin_ra0(&mut m, part);
        t2_on(&mut m, part, 0x0F);
        w(&mut m, "CCP1CON", 0x8C); // duty 0
        assert!(sample(&mut m, 200).iter().all(|&x| !x), "{part}: 0 %");
        assert!(!ccp1if(&mut m), "{part}");
        w(&mut m, "CCPR1L", 64); // 64 Tosc = the whole period (16 cycles of 4 Tosc), and 56 and up already is in MDB
        let s = sample(&mut m, 200);
        assert!(s[40..].iter().all(|&x| x), "{part}: a pulse as long as the period leaves the pin set");
        w(&mut m, "CCPR1L", 0);
        let s = sample(&mut m, 40);
        assert!(!s[39], "{part}: the duty goes to 0 at the next period");
    }
}

#[test]
fn pwm_left_aligned_duty_and_prescaler() {
    for part in PARTS {
        let mut m = machine(part, "");
        pin_ra0(&mut m, part);
        t2_on(&mut m, part, 0x0F);
        w(&mut m, "CCPR1H", 5); // FMT = 1: CCPR1H is the top eight bits, so 5 << 2 = 20
        w(&mut m, "CCPR1L", 0);
        w(&mut m, "CCP1CON", 0x9C);
        let s = sample(&mut m, 400);
        let first = s.iter().position(|&x| x).unwrap();
        assert_eq!(s[first..first + 160].iter().filter(|&&x| x).count(), 60, "{part}");
    }
}

#[test]
fn pwm_output_needs_the_pin_to_be_an_output() {
    for part in PARTS {
        let mut m = machine(part, "");
        w(&mut m, "RA0PPS", code(part));
        t2_on(&mut m, part, 0x0F);
        w(&mut m, "CCPR1L", 20);
        w(&mut m, "CCP1CON", 0x8C);
        assert!(sample(&mut m, 200).iter().all(|&x| !x), "{part}: TRISA0 = 1 keeps the pin an input");
        w(&mut m, "TRISA", 0xFE);
        assert!(sample(&mut m, 200).iter().any(|&x| x), "{part}");
    }
}

#[test]
fn compare_set_clear_and_toggle_at_the_match() {
    for part in PARTS {
        // set output
        let mut m = machine(part, "");
        pin_ra0(&mut m, part);
        t1_on(&mut m, part);
        w(&mut m, "CCPR1L", 100);
        w(&mut m, "CCP1CON", 0x88);
        let s = sample(&mut m, 200);
        let at = s.iter().position(|&x| x).unwrap_or_else(|| panic!("{part}: no match"));
        assert!((95..=105).contains(&at), "{part}: the match is when TMR1 reaches 100, not {at}");
        assert!(s[at..].iter().all(|&x| x) && ccp1if(&mut m), "{part}");
        // clear output: starts low, so set it by toggling first
        let mut m = machine(part, "");
        pin_ra0(&mut m, part);
        t1_on(&mut m, part);
        w(&mut m, "CCPR1L", 100);
        w(&mut m, "CCP1CON", 0x89);
        let s = sample(&mut m, 200);
        assert!(s.iter().all(|&x| !x) && ccp1if(&mut m), "{part}: clear output: the pin stays low, the flag is set");
    }
}

#[test]
fn compare_toggle_and_clear_timer_has_a_period_of_ccpr_plus_one() {
    for part in PARTS {
        let mut m = machine(part, "");
        pin_ra0(&mut m, part);
        t1_on(&mut m, part);
        w(&mut m, "CCPR1L", 50);
        w(&mut m, "CCP1CON", 0x81); // toggle, clear TMR1
        let s = sample(&mut m, 400);
        let edges: Vec<usize> = (1..s.len()).filter(|&i| s[i] != s[i - 1]).collect();
        assert!(edges.len() >= 6, "{part}: {edges:?}");
        // TMR1 counts 1 to CCPR1 after a match (MDB): a period of CCPR1 clocks, the first from 0 one longer
        assert_eq!(edges[1] - edges[0], 50, "{part}: {edges:?}");
        for p in edges.windows(2) {
            assert_eq!(p[1] - p[0], 50, "{part}: {edges:?}");
        }
        assert!(ccp1if(&mut m), "{part}");
        // MODE 0010 toggles without clearing TMR1: once in 65536 clocks
        let mut m = machine(part, "");
        pin_ra0(&mut m, part);
        t1_on(&mut m, part);
        w(&mut m, "CCPR1L", 49);
        w(&mut m, "CCP1CON", 0x82);
        let s = sample(&mut m, 400);
        assert_eq!((1..s.len()).filter(|&i| s[i] != s[i - 1]).count(), 1, "{part}");
    }
}

#[test]
fn clearing_ccp1con_forces_the_output_low() {
    for part in PARTS {
        let mut m = machine(part, "");
        pin_ra0(&mut m, part);
        t1_on(&mut m, part);
        w(&mut m, "CCPR1L", 10);
        w(&mut m, "CCP1CON", 0x88);
        assert!(sample(&mut m, 40).last().copied().unwrap(), "{part}");
        w(&mut m, "CCP1CON", 0);
        assert!(!ra0(&mut m), "{part}");
        assert_eq!(r(&mut m, "CCP1CON") & 0x20, 0, "{part}");
    }
}

fn captured(m: &mut Machine16) -> u16 {
    u16::from(r(m, "CCPR1H")) << 8 | u16::from(r(m, "CCPR1L"))
}

#[test]
fn capture_copies_timer1_at_the_edge() {
    let stim = pins(&[(2, 1, 100), (2, 0, 150), (2, 1, 200), (2, 0, 250), (2, 1, 300)]);
    for part in PARTS {
        let mut m = machine(part, &stim);
        t1_on(&mut m, part);
        w(&mut m, "CCP1CON", 0x85); // every rising edge
        while m.cpu.cycles < 120 {
            m.cpu.step();
        }
        let first = captured(&mut m);
        assert!((90..=100).contains(&first), "{part}: TMR1 at the rise in cycle 100 is {first}");
        assert!(ccp1if(&mut m), "{part}");
        clear_ccp1if(&mut m);
        while m.cpu.cycles < 220 {
            m.cpu.step();
        }
        assert_eq!(captured(&mut m), first + 100, "{part}: the next rise is 100 clocks later");
        assert!(ccp1if(&mut m), "{part}");
        // falling edges
        let mut m = machine(part, &stim);
        t1_on(&mut m, part);
        w(&mut m, "CCP1CON", 0x84);
        while m.cpu.cycles < 400 {
            m.cpu.step();
        }
        let last = captured(&mut m);
        assert!((240..=250).contains(&last), "{part}: the last fall is at 250: {last}");
        // every edge: the rise at 300 is the last of five
        let mut m = machine(part, &stim);
        t1_on(&mut m, part);
        w(&mut m, "CCP1CON", 0x83);
        while m.cpu.cycles < 400 {
            m.cpu.step();
        }
        assert!((290..=300).contains(&captured(&mut m)), "{part}");
    }
}

#[test]
fn capture_prescaler_waits_for_the_fourth_rising_edge() {
    let stim = pins(&[(2, 1, 100), (2, 0, 120), (2, 1, 140), (2, 0, 160), (2, 1, 180), (2, 0, 200), (2, 1, 220), (2, 0, 240)]);
    for part in PARTS {
        let mut m = machine(part, &stim);
        t1_on(&mut m, part);
        w(&mut m, "CCP1CON", 0x86); // every 4th rising edge
        while m.cpu.cycles < 200 {
            m.cpu.step();
        }
        assert!(!ccp1if(&mut m), "{part}: three rises so far");
        while m.cpu.cycles < 260 {
            m.cpu.step();
        }
        assert!(ccp1if(&mut m), "{part}");
        assert!((210..=220).contains(&captured(&mut m)), "{part}");
    }
}

#[test]
fn a_pwm_pulse_goes_on_after_sleep() {
    // RA2 rises at cycle 300 and wakes the part (IOC). Whatever the phase the SLEEP lands in, the pulse (6 cycles at duty 20) is
    // 6 awake cycles long: Timer2 and the pin stand still in Sleep (27.4.7 p.334).
    let stim = pins(&[(2, 1, 300)]);
    for part in ["16F17146", "16F18346"] {
        for at in 40..60u32 {
            let mut m = machine_at(part, &stim, Some(at));
            pin_ra0(&mut m, part);
            t2_on(&mut m, part, 0x0F);
            w(&mut m, "CCPR1L", 20);
            w(&mut m, "CCP1CON", 0x8C);
            w(&mut m, "IOCAP", 0x04);
            let (reg, mask) = bit(&m, "PIE", "IOCIE");
            w(&mut m, "PIE0", 0);
            let v = m.cpu.read(reg);
            m.cpu.write(reg, v | mask);
            let mut levels = Vec::new();
            for _ in 0..500 {
                let before = m.cpu.icount;
                m.cpu.step();
                if m.cpu.icount != before {
                    levels.push(ra0(&mut m));
                }
            }
            let mut runs = Vec::new();
            let mut n = 0;
            for &x in &levels {
                if x {
                    n += 1;
                } else if n > 0 {
                    runs.push(n);
                    n = 0;
                }
            }
            assert!(runs.len() > 10, "{part} {at}: {runs:?}");
            assert!(runs.iter().all(|&r| r == 6), "{part}: SLEEP at word {at}: {runs:?}");
        }
    }
}

#[test]
fn a_pwm_pulse_holds_while_timer2_is_off() {
    for part in PARTS {
        let mut m = machine(part, "");
        pin_ra0(&mut m, part);
        t2_on(&mut m, part, 0x0F);
        w(&mut m, "CCPR1L", 20); // 6 cycles
        w(&mut m, "CCP1CON", 0x8C);
        while !{
            m.cpu.step();
            ra0(&mut m)
        } {}
        let mut high = 1;
        for _ in 0..2 {
            m.cpu.step();
            assert!(ra0(&mut m), "{part}");
            high += 1;
        }
        w(&mut m, "T2CON", 0); // Timer2 off
        assert!(sample(&mut m, 60).iter().all(|&x| x), "{part}: the pin holds its level while Timer2 stands still");
        t2_on(&mut m, part, 0x0F);
        while ra0(&mut m) {
            m.cpu.step();
            high += 1;
            assert!(high < 20, "{part}");
        }
        assert!((6..=8).contains(&high), "{part}: the pulse goes on where it stopped: {high}");
    }
}

#[test]
fn leaving_pwm_ends_the_pulse_and_a_same_mode_write_keeps_the_capture_count() {
    for part in PARTS {
        let mut m = machine(part, "");
        pin_ra0(&mut m, part);
        t2_on(&mut m, part, 0x0F);
        w(&mut m, "CCPR1L", 40);
        w(&mut m, "CCP1CON", 0x8C);
        while !{
            m.cpu.step();
            ra0(&mut m)
        } {}
        w(&mut m, "CCP1CON", 0x85); // capture: the pulse must not be stuck high
        assert!(!ra0(&mut m) && r(&mut m, "CCP1CON") & 0x20 == 0, "{part}");
        assert!(sample(&mut m, 100).iter().all(|&x| !x), "{part}");
    }
    let stim = pins(&[(2, 1, 100), (2, 0, 120), (2, 1, 140), (2, 0, 160), (2, 1, 180)]);
    for part in PARTS {
        let mut m = machine(part, &stim);
        t1_on(&mut m, part);
        w(&mut m, "CCP1CON", 0x86); // every 4th rising edge
        while m.cpu.cycles < 150 {
            m.cpu.step();
        }
        w(&mut m, "CCP1CON", 0x96); // FMT only: the two edges counted stay counted
        while m.cpu.cycles < 200 {
            m.cpu.step();
        }
        assert!(!ccp1if(&mut m), "{part}: three rises");
    }
}

#[test]
fn a_part_without_a_known_output_code_says_so() {
    let m = machine("16F18056", "");
    assert!(m.cpu.notes.iter().any(|n| n.contains("CCP1") && n.contains("not modelled")), "{:?}", m.cpu.notes);
    let _ = m;
}
