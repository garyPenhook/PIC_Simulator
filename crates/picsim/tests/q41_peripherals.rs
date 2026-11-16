//! Q41 shared CCP, comparators, CWG and four banked CLCs (DS40002214G ch.21/22/28/29/31/43).
use devdesc::Device;
use picsim::{Machine, periph::ResetKind};
use std::path::Path;
const PARTS: [&str; 6] = ["18F04Q41", "18F05Q41", "18F06Q41", "18F14Q41", "18F15Q41", "18F16Q41"];
fn machine(part: &str) -> Machine {
    let d = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let mut m = Machine::new(d).unwrap();
    m.cpu.prog[..2048].fill(0);
    m
}
fn w(m: &mut Machine, n: &str, v: u8) {
    m.cpu.write(m.dev.find_sfr(n).unwrap().addr, v);
}
fn r(m: &mut Machine, n: &str) -> u8 {
    m.cpu.read(m.dev.find_sfr(n).unwrap().addr)
}
fn steps(m: &mut Machine, n: usize) {
    for _ in 0..n {
        m.cpu.step();
    }
}
fn flag(m: &mut Machine, name: &str) -> bool {
    let (a, mask) = m
        .dev
        .sfr
        .iter()
        .filter(|s| s.name.starts_with("PIR"))
        .find_map(|s| {
            s.fields
                .iter()
                .chain(s.legacy.iter())
                .find(|f| f.name == name)
                .map(|f| (s.addr, 1 << f.pos))
        })
        .unwrap();
    m.cpu.read(a) & mask != 0
}
#[test]
fn compare_output_uses_q41_pps_and_timer3_selection() {
    for part in PARTS {
        let mut m = machine(part);
        w(&mut m, "ANSELA", 0);
        w(&mut m, "TRISA", 0xFE);
        w(&mut m, "RA0PPS", 9);
        w(&mut m, "CCPTMRS0", 2); // Timer3/4, not Timer1/2
        w(&mut m, "CCPR1L", 5);
        w(&mut m, "CCP1CON", 0x88);
        w(&mut m, "T3CLK", 1);
        w(&mut m, "T3CON", 1);
        steps(&mut m, 10);
        assert_eq!(r(&mut m, "CCP1CON") & 0x20, 0x20, "{part}");
        assert_eq!(r(&mut m, "PORTA") & 1, 1);
        assert!(flag(&mut m, "CCP1IF"));
        w(&mut m, "CCP1CON", 0);
        assert_eq!(r(&mut m, "PORTA") & 1, 0);
    }
}
#[test]
fn capture_and_pwm_have_real_engine_behavior() {
    let mut m = machine(PARTS[5]);
    m.load_stimulus("[[pin]]\nport='A'\nbit=2\nlevel=1\nat=20\n").unwrap();
    w(&mut m, "ANSELA", 0);
    w(&mut m, "CCP1PPS", 2);
    w(&mut m, "T1CLK", 1);
    w(&mut m, "T1CON", 1);
    w(&mut m, "CCP1CON", 0x85);
    steps(&mut m, 30);
    assert_eq!(r(&mut m, "CCPR1L"), 20);
    assert!(flag(&mut m, "CCP1IF"));
    w(&mut m, "TRISA", 0xFE);
    w(&mut m, "RA0PPS", 9);
    w(&mut m, "T2CLKCON", 1);
    w(&mut m, "T2PR", 7);
    w(&mut m, "CCPR1L", 12);
    w(&mut m, "CCPR1H", 0);
    w(&mut m, "CCP1CON", 0x8C);
    w(&mut m, "T2CON", 0x80);
    let mut levels = [false; 2];
    for _ in 0..40 {
        m.cpu.step();
        levels[(r(&mut m, "PORTA") & 1) as usize] = true;
    }
    assert_eq!(levels, [true, true]);
}
#[test]
fn comparator_q41_analog_inputs_polarity_interrupt_and_pps() {
    for part in PARTS {
        let mut m = machine(part);
        m.load_stimulus(
            "[[analog]]\npin='RA0'\nvolts=2.0\nat=0\n[[analog]]\npin='RA1'\nvolts=1.0\nat=0\n[[analog]]\npin='RC0'\nvolts=2.0\nat=0\n",
        )
        .unwrap();
        w(&mut m, "ANSELC", 0);
        w(&mut m, "TRISC", 0xCF);
        w(&mut m, "RC4PPS", 0x19);
        w(&mut m, "RC5PPS", 0x1A);
        w(&mut m, "CM1CON1", 3);
        w(&mut m, "CM2CON1", 3);
        w(&mut m, "CM1CON0", 0x80);
        w(&mut m, "CM2CON0", 0x80);
        steps(&mut m, 2);
        assert_eq!(r(&mut m, "CMOUT") & 3, 3, "{part}");
        assert_eq!(r(&mut m, "PORTC") & 0x30, 0x30);
        assert!(flag(&mut m, "C1IF"));
        assert!(flag(&mut m, "C2IF"));
        w(&mut m, "CM1CON0", 0x90);
        steps(&mut m, 1);
        assert_eq!(r(&mut m, "PORTC") & 0x10, 0);
    }
}
fn clc(m: &mut Machine, bank: u8, source: u8) {
    w(m, "CLCSELECT", bank);
    w(m, "CLCnSEL0", source);
    w(m, "CLCnGLS0", 2);
    w(m, "CLCnPOL", 2); // (D1 & constant 1)
    w(m, "CLCnCON", 0x98); // EN, rising/falling interrupts, AND-OR
}
#[test]
fn clc_banks_cascade_interrupts_and_pps() {
    for part in PARTS {
        let mut m = machine(part);
        w(&mut m, "ANSELA", 0);
        w(&mut m, "ANSELC", 0);
        w(&mut m, "TRISC", 0xF0);
        w(&mut m, "CLCIN0PPS", 0);
        for i in 0..4 {
            w(&mut m, &format!("RC{i}PPS"), i + 1);
            clc(&mut m, i, if i == 0 { 0 } else { 33 + i });
        }
        m.cpu.p.pin_feed(b'A', 10, 1, 1);
        steps(&mut m, 12);
        assert_eq!(r(&mut m, "CLCDATA") & 15, 15, "{part}");
        assert_eq!(r(&mut m, "PORTC") & 15, 15);
        for i in 0..4 {
            w(&mut m, "CLCSELECT", i);
            assert_eq!(r(&mut m, "CLCnCON"), 0xB8);
            assert!(flag(&mut m, &format!("CLC{}IF", i + 1)));
        }
        w(&mut m, "CLCSELECT", 0);
        w(&mut m, "CLCnCON", 0);
        steps(&mut m, 1);
        assert_eq!(r(&mut m, "CLCDATA") & 15, 0);
        m.cpu.reset_cond(ResetKind::Instruction);
        for i in 0..4 {
            w(&mut m, "CLCSELECT", i);
            assert_eq!(r(&mut m, "CLCnCON"), 0);
        }
    }
}
#[test]
fn cwg_follows_q41_clc_source_with_q41_output_codes() {
    let mut m = machine(PARTS[5]);
    w(&mut m, "ANSELA", 0);
    w(&mut m, "ANSELC", 0);
    w(&mut m, "TRISC", 0xFC);
    w(&mut m, "RC0PPS", 5);
    w(&mut m, "RC1PPS", 6);
    w(&mut m, "CLCIN0PPS", 0);
    clc(&mut m, 0, 0);
    w(&mut m, "CWG1ISM", 12);
    w(&mut m, "CWG1STR", 3);
    w(&mut m, "CWG1CON0", 0x80);
    m.cpu.p.pin_feed(b'A', 10, 1, 1);
    m.cpu.p.pin_feed(b'A', 20, 1, 0);
    steps(&mut m, 12);
    assert_eq!(r(&mut m, "PORTC") & 3, 3);
    steps(&mut m, 10);
    assert_eq!(r(&mut m, "PORTC") & 3, 0);
}
fn crc_word(m: &mut Machine, prefix: &str, value: u32) {
    for (i, suffix) in ["L", "H", "U", "T"].into_iter().enumerate() {
        w(m, &format!("{prefix}{suffix}"), (value >> (8 * i)) as u8);
    }
}
fn crc_result(m: &mut Machine) -> u32 {
    ["L", "H", "U", "T"]
        .into_iter()
        .enumerate()
        .fold(0, |v, (i, s)| v | u32::from(r(m, &format!("CRCOUT{s}"))) << (8 * i))
}
#[test]
fn crc_known_check_values_for_8_16_and_32_bits_all_parts() {
    for part in PARTS {
        for (bits, poly, seed, expected) in [
            (8, 7, 0, 0xF4),
            (16, 0x1021, 0xFFFF, 0x29B1),
            (32, 0x04C11DB7, 0xFFFFFFFF, 0x0376E6E7),
        ] {
            let mut m = machine(part);
            w(&mut m, "CRCCON0", 0xD8); // EN, GO, ACCM, SETUP=10 (polynomial)
            crc_word(&mut m, "CRCOUT", poly); // same physical addresses as CRCXOR
            w(&mut m, "CRCCON0", 0xD0); // SETUP=00 (accumulator)
            crc_word(&mut m, "CRCOUT", seed);
            w(&mut m, "CRCCON1", bits - 1);
            w(&mut m, "CRCCON2", 7);
            for &byte in b"123456789" {
                w(&mut m, "CRCDATAL", byte);
                steps(&mut m, 8);
            }
            assert_eq!(crc_result(&mut m), expected, "{part} CRC-{bits}");
            assert_eq!(r(&mut m, "CRCCON0") & 0x21, 0);
            assert!(flag(&mut m, "CRCIF"));
        }
    }
}
#[test]
fn crc_overlay_buffering_pause_and_full_width_word() {
    let mut m = machine(PARTS[5]);
    w(&mut m, "CRCCON0", 0x98);
    crc_word(&mut m, "CRCOUT", 0x04C11DB6);
    assert_eq!(r(&mut m, "CRCOUTL"), 0xB7, "polynomial constant term forced to one");
    w(&mut m, "CRCCON0", 0x90);
    w(&mut m, "CRCCON1", 31);
    w(&mut m, "CRCCON2", 31);
    // High bytes precede the low-byte write which starts the transfer.
    w(&mut m, "CRCDATAT", 0x12);
    w(&mut m, "CRCDATAU", 0x34);
    w(&mut m, "CRCDATAH", 0x56);
    w(&mut m, "CRCDATAL", 0x78);
    assert_eq!(r(&mut m, "CRCCON0") & 0x21, 0x21);
    steps(&mut m, 10);
    assert_eq!(crc_result(&mut m), 0);
    w(&mut m, "CRCCON0", 0xD4); // GO, shifter overlay
    assert_eq!(crc_result(&mut m), 0x12345678);
    crc_word(&mut m, "CRCOUT", 0xFFFFFFFF); // shift overlay is read only
    assert_eq!(crc_result(&mut m), 0x12345678);
    steps(&mut m, 32);
    w(&mut m, "CRCCON0", 0xD0);
    // Independent, bit-by-bit CRC-32 of bytes 12 34 56 78, seed zero.
    assert_eq!(crc_result(&mut m), 0x188E5750);
    w(&mut m, "CRCCON2", 7);
    w(&mut m, "CRCDATAL", 0x12);
    w(&mut m, "CRCDATAL", 0x34);
    assert_eq!(r(&mut m, "CRCCON0") & 0x21, 0x21);
    steps(&mut m, 16);
    assert_eq!(r(&mut m, "CRCCON0") & 0x21, 0);
}
#[test]
fn standalone_pwm_buffering_polarity_postscale_and_mirrors() {
    for part in PARTS {
        let mut m = machine(part);
        w(&mut m, "ANSELC", 0);
        w(&mut m, "TRISC", 0xFC);
        w(&mut m, "RC0PPS", 10);
        w(&mut m, "RC1PPS", 11);
        w(&mut m, "PWM1CLK", 2);
        w(&mut m, "PWM1CPRE", 3); // one timer count/instruction
        w(&mut m, "PWM1PRL", 7);
        w(&mut m, "PWM1S1P1L", 3);
        w(&mut m, "PWM1S1P2L", 5);
        w(&mut m, "PWM1PIPOS", 1);
        w(&mut m, "PWM1GIE", 3);
        w(&mut m, "PWMEN", 1);
        assert_eq!(r(&mut m, "PWM1CON") & 0x80, 0x80);
        assert_eq!(r(&mut m, "PORTC") & 3, 3);
        steps(&mut m, 3);
        assert_eq!(r(&mut m, "PORTC") & 3, 2);
        assert_eq!(r(&mut m, "PWM1GIR") & 1, 1);
        assert!(flag(&mut m, "PWM1IF"));
        steps(&mut m, 2);
        assert_eq!(r(&mut m, "PORTC") & 3, 0);
        steps(&mut m, 3);
        assert_eq!(r(&mut m, "PORTC") & 3, 3);
        assert!(!flag(&mut m, "PWM1PIF"));
        // Writes remain buffered until LD takes effect on the next period boundary.
        w(&mut m, "PWM1S1P1L", 1);
        steps(&mut m, 2);
        assert_eq!(r(&mut m, "PORTC") & 1, 1);
        w(&mut m, "PWMLOAD", 1);
        steps(&mut m, 6);
        assert!(flag(&mut m, "PWM1PIF"));
        assert_eq!(r(&mut m, "PWMLOAD"), 0);
        steps(&mut m, 1);
        assert_eq!(r(&mut m, "PORTC") & 1, 0);
        w(&mut m, "PWMEN", 0);
        assert_eq!(r(&mut m, "PORTC") & 3, 0);
        w(&mut m, "PWM1S1CFG", 0xC0);
        assert_eq!(r(&mut m, "PORTC") & 3, 3, "polarity applies while disabled");
        assert_eq!(r(&mut m, "PWMEN"), 0);
    }
}
#[test]
fn crc_matches_datasheet_both_input_orders_and_unaugmented_extension() {
    for (reverse, expected) in [(false, 0xC60D8323), (true, 0x843529CC)] {
        let mut m = machine(PARTS[5]);
        w(&mut m, "CRCCON0", 0xD8);
        crc_word(&mut m, "CRCOUT", 0x04C11DB7);
        w(&mut m, "CRCCON0", 0xD0 | (u8::from(reverse) * 2));
        crc_word(&mut m, "CRCOUT", 0);
        w(&mut m, "CRCCON1", 31);
        w(&mut m, "CRCCON2", 7);
        for byte in [0x55, 0x66, 0x77, 0x88] {
            w(&mut m, "CRCDATAL", byte);
            steps(&mut m, 8);
        }
        assert_eq!(crc_result(&mut m), expected, "DS40002214G Example 13-1");
    }
    let mut m = machine(PARTS[5]);
    w(&mut m, "CRCCON0", 0xC8);
    crc_word(&mut m, "CRCOUT", 0x1021);
    w(&mut m, "CRCCON0", 0xC0);
    w(&mut m, "CRCCON1", 15);
    w(&mut m, "CRCCON2", 7);
    for &byte in b"123456789\0\0" {
        w(&mut m, "CRCDATAL", byte);
        steps(&mut m, 8);
    }
    assert_eq!(crc_result(&mut m), 0x31C3, "unaugmented with appended zeros equals CRC-16/XMODEM");
}
#[test]
fn pwm_right_aligned_and_compare_modes_and_other_instances() {
    for mode in [1, 4, 5] {
        let mut m = machine(PARTS[5]);
        w(&mut m, "ANSELC", 0);
        w(&mut m, "TRISC", 0xF0);
        for (n, p1, p2) in [(2, 12, 13), (3, 14, 15)] {
            w(&mut m, &format!("RC{}PPS", (n - 2) * 2), p1);
            w(&mut m, &format!("RC{}PPS", (n - 2) * 2 + 1), p2);
            for (reg, v) in [
                ("CLK", 2),
                ("CPRE", 3),
                ("PRL", 7),
                ("S1P1L", 3),
                ("S1P2L", 3),
                ("S1CFG", mode),
                ("CON", 0x80),
            ] {
                w(&mut m, &format!("PWM{n}{reg}"), v);
            }
        }
        steps(&mut m, if mode == 1 { 5 } else { 3 });
        assert_eq!(r(&mut m, "PORTC") & 15, 15);
        steps(&mut m, if mode == 1 { 3 } else { 1 });
        assert_eq!(r(&mut m, "PORTC") & 15, if mode == 5 { 15 } else { 0 });
        if mode == 5 {
            steps(&mut m, 7);
            assert_eq!(r(&mut m, "PORTC") & 15, 0);
        }
    }
}
#[test]
fn unsupported_pwm_and_crc_scanner_are_identified() {
    let mut m = machine(PARTS[5]);
    w(&mut m, "PWM1S1CFG", 2);
    w(&mut m, "PWM1CLK", 7);
    w(&mut m, "PWM1CON", 0x80);
    w(&mut m, "SCANCON0", 0xA0);
    steps(&mut m, 1);
    assert!(m.cpu.notes.iter().any(|n| n.contains("PWM1 mode 2")));
    assert!(m.cpu.notes.iter().any(|n| n.contains("PWM1 clock 7")));
    assert!(m.cpu.m.unmodelled.contains("SCANCON0"));
}
#[test]
fn running_with_lazy_sync_matches_instruction_stepping() {
    fn configured() -> Machine {
        let mut m = machine(PARTS[5]);
        w(&mut m, "ANSELC", 0);
        w(&mut m, "TRISC", 0xFE);
        w(&mut m, "RC0PPS", 10);
        w(&mut m, "PWM1CLK", 2);
        w(&mut m, "PWM1CPRE", 3);
        w(&mut m, "PWM1PRL", 15);
        w(&mut m, "PWM1S1P1L", 5);
        w(&mut m, "PWM1CON", 0x80);
        clc(&mut m, 0, 21); // PWM1 P1 -> CLC
        w(&mut m, "CRCCON0", 0xD8);
        crc_word(&mut m, "CRCOUT", 0x1021);
        w(&mut m, "CRCCON0", 0xD0);
        w(&mut m, "CRCCON1", 15);
        w(&mut m, "CRCCON2", 7);
        w(&mut m, "CRCDATAL", 0x53);
        m
    }
    let mut stepped = configured();
    let mut running = configured();
    steps(&mut stepped, 80);
    running.run(None, 80);
    for n in ["PORTC", "CLCDATA", "PWM1GIR", "CRCCON0", "PIR0", "PIR4"] {
        assert_eq!(r(&mut stepped, n), r(&mut running, n), "{n}");
    }
    assert_eq!(crc_result(&mut stepped), crc_result(&mut running));
    assert_eq!(
        stepped.cpu.p.pin_log.iter().collect::<Vec<_>>(),
        running.cpu.p.pin_log.iter().collect::<Vec<_>>()
    );
}
#[test]
fn crc_pauses_during_sleep_and_resumes_on_ioc_wake() {
    let mut m = machine(PARTS[5]);
    m.cpu.prog[0..2].copy_from_slice(&[3, 0]); // SLEEP
    m.load_stimulus("[[pin]]\nport='A'\nbit=2\nlevel=1\nat=100\n").unwrap();
    w(&mut m, "ANSELA", 0);
    w(&mut m, "IOCAP", 4);
    w(&mut m, "PIE0", 0x80);
    w(&mut m, "CRCCON0", 0xD8);
    crc_word(&mut m, "CRCOUT", 0x04C11DB7);
    w(&mut m, "CRCCON0", 0xD0);
    w(&mut m, "CRCCON1", 31);
    w(&mut m, "CRCCON2", 31);
    for n in ["CRCDATAT", "CRCDATAU", "CRCDATAH", "CRCDATAL"] {
        w(&mut m, n, 0xFF);
    }
    steps(&mut m, 1);
    assert_eq!(m.cpu.state, picsim::pic18::State::Sleeping);
    let before = crc_result(&mut m);
    steps(&mut m, 1);
    assert_eq!(m.cpu.state, picsim::pic18::State::Running);
    assert_eq!(crc_result(&mut m), before);
    steps(&mut m, 1);
    assert_ne!(crc_result(&mut m), before);
}
