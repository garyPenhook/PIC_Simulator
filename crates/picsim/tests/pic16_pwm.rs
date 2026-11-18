//! Standalone PWM from the PIC16 data sheets: real waveforms, buffering, flags and PPS.
use devdesc::Device;
use picsim::machine16::Machine16;
use std::path::Path;
const NOP: u16 = 0;
const BRA_SELF: u16 = 0x33FF;
fn hexrec(b: &[u8]) -> String {
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(
        ":{}{:02X}\n",
        b.iter().map(|x| format!("{x:02X}")).collect::<String>(),
        sum.wrapping_neg()
    )
}

/// RA`bit` takes `level` at cycle `at`.
fn pins(list: &[(u8, u8, u64)]) -> String {
    list.iter()
        .map(|&(bit, level, at)| {
            format!("[[pin]]\nport = \"A\"\nbit = {bit}\nlevel = {level}\nat = {at}\n")
        })
        .collect()
}

/// A slide of NOPs (one cycle a step) that ends in a branch to itself.
fn machine(part: &str, stim: &str) -> Machine16 {
    machine_at(part, stim, None)
}

/// The same with a SLEEP at word `sleep`.
fn machine_at(part: &str, stim: &str, sleep: Option<u32>) -> Machine16 {
    let dev = Device::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml")),
    )
    .unwrap();
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
            put(
                r.addr,
                (r.default & !(((1u32 << f.width) - 1) << f.pos)) as u16,
            ); // the watchdog is off
        }
    }
    m.load_hex(&(text + ":00000001FF\n")).unwrap();
    m.load_stimulus(stim).unwrap();
    w(&mut m, "ANSELA", 0);
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

fn steps(m: &mut Machine16, count: usize) {
    for _ in 0..count {
        m.cpu.step();
    }
}
fn route(m: &mut Machine16, first: u8) {
    w(m, "ANSELA", 0);
    w(m, "RA0PPS", first);
    w(m, "RA1PPS", first + 1);
    w(m, "TRISA", 0xFC);
}
fn field(m: &Machine16, name: &str) -> (u32, u8) {
    m.dev
        .sfr
        .iter()
        .find_map(|r| {
            r.fields
                .iter()
                .find(|f| f.name == name)
                .map(|f| (r.addr, 1 << f.pos))
        })
        .unwrap()
}
fn set_field(m: &mut Machine16, name: &str, value: bool) {
    let (a, mask) = field(m, name);
    let old = m.cpu.read(a);
    m.cpu.write(a, if value { old | mask } else { old & !mask });
}
fn has_field(m: &mut Machine16, name: &str) -> bool {
    let (a, b) = field(m, name);
    m.cpu.read(a) & b != 0
}
fn basic(m: &mut Machine16, n: u8, duty: u16, pr: u8, prescale: u8) {
    w(m, &format!("PWM{n}DCH"), (duty >> 2) as u8);
    w(m, &format!("PWM{n}DCL"), (duty << 6) as u8);
    if m.dev.find_sfr("T2CLKCON").is_some() {
        w(m, "T2CLKCON", 1);
        w(m, "T2PR", pr);
        w(m, "T2CON", 0x80 | prescale << 4);
    } else {
        w(m, "PR2", pr);
        w(m, "T2CON", 4 | prescale);
    }
    w(m, &format!("PWM{n}CON"), 0x80);
}
fn trace(m: &mut Machine16, n: u8, cycles: usize) -> Vec<bool> {
    (0..cycles)
        .map(|_| {
            m.cpu.step();
            r(m, &format!("PWM{n}CON")) & 0x20 != 0
        })
        .collect()
}
#[test]
fn ten_bit_pwm_period_duty_and_pps_on_all_basic_families() {
    for (part, n, code) in [
        ("16F13145", 1, 0x2C),
        ("16F15244", 3, 3),
        ("16F18056", 3, 11),
        ("16F18076", 3, 11),
        ("16F18346", 5, 2),
        ("16F18313", 5, 2),
        ("16F18325", 5, 2),
    ] {
        let mut m = machine(part, "");
        route(&mut m, code);
        basic(&mut m, n, 8, 7, 0); // two cycles high, eight-cycle period
        let values = trace(&mut m, n, 50);
        let rises: Vec<_> = values
            .windows(2)
            .enumerate()
            .filter_map(|(i, w)| (!w[0] && w[1]).then_some(i + 1))
            .collect();
        assert!(rises.len() >= 4, "{part}: {values:?}");
        assert!(rises.windows(2).all(|w| w[1] - w[0] == 8), "{part}");
        for &at in &rises[..rises.len() - 1] {
            assert_eq!(&values[at..at + 4], &[true, true, false, false], "{part}");
        }
        for _ in 0..20 {
            m.cpu.step();
            assert_eq!(
                r(&mut m, "PORTA") & 1 != 0,
                r(&mut m, &format!("PWM{n}CON")) & 0x20 != 0,
                "{part}"
            );
        }
        assert!(
            m.cpu.unmodelled.is_empty(),
            "{part}: {:?}",
            m.cpu.unmodelled
        );
    }
}
#[test]
fn basic_zero_full_polarity_read_only_and_disable() {
    let mut m = machine("16F13145", "");
    route(&mut m, 0x2C);
    basic(&mut m, 1, 0, 7, 0);
    assert!(trace(&mut m, 1, 20).iter().all(|&v| !v));
    w(&mut m, "PWM1CON", 0xA0);
    assert_eq!(r(&mut m, "PWM1CON") & 0x20, 0, "OUT is read only");
    w(&mut m, "PWM1DCH", 8); // full 32/32
    steps(&mut m, 16);
    assert!(trace(&mut m, 1, 20).iter().all(|&v| v));
    w(&mut m, "PWM1CON", 0x90);
    assert_eq!(r(&mut m, "PORTA") & 1, 0);
    w(&mut m, "PWM1CON", 0x10);
    assert_eq!(r(&mut m, "PWM1CON") & 0x20, 0);
}
#[test]
fn basic_duty_is_buffered_until_period_and_prescaler_scales_pulse() {
    let mut m = machine("16F13145", "");
    basic(&mut m, 1, 8, 7, 1);
    while r(&mut m, "PWM1CON") & 0x20 == 0 {
        m.cpu.step();
        assert!(m.cpu.cycles < 50);
    }
    w(&mut m, "PWM1DCH", 0); // does not truncate the in-flight pulse
    assert_eq!(trace(&mut m, 1, 4), [true, true, true, false]);
    assert!(trace(&mut m, 1, 40).iter().all(|&v| !v));
}
#[test]
fn basic_pmd_and_reserved_timer_selection_stop_pwm() {
    let mut m = machine("16F13145", "");
    basic(&mut m, 1, 8, 7, 0);
    steps(&mut m, 20);
    set_field(&mut m, "PWM1MD", true);
    assert!(trace(&mut m, 1, 20).iter().all(|&v| !v));
    set_field(&mut m, "PWM1MD", false);
    w(&mut m, "PWMTMRS0", 0);
    assert!(trace(&mut m, 1, 20).iter().all(|&v| !v));
    w(&mut m, "PWMTMRS0", 1);
    assert!(trace(&mut m, 1, 20).iter().any(|&v| v));
}
#[test]
fn basic_second_module_and_timer4_selection() {
    let mut m = machine("16F18346", "");
    route(&mut m, 2);
    basic(&mut m, 5, 8, 7, 0);
    w(&mut m, "PWM6DCH", 4);
    w(&mut m, "PWM6CON", 0x80);
    w(&mut m, "PWMTMRS", 9); // PWM5 Timer2, PWM6 Timer4
    w(&mut m, "PR4", 15);
    w(&mut m, "T4CON", 4);
    let mut edges = [Vec::new(), Vec::new()];
    let mut previous = 0;
    for i in 0..100 {
        m.cpu.step();
        let v = r(&mut m, "PORTA") & 3;
        for (b, list) in edges.iter_mut().enumerate() {
            if v & (1 << b) != 0 && previous & (1 << b) == 0 {
                list.push(i);
            }
        }
        previous = v;
    }
    assert!(edges[0].windows(2).all(|w| w[1] - w[0] == 8));
    assert!(edges[1].len() > 3);
    assert!(edges[1].windows(2).all(|w| w[1] - w[0] == 16));
}
fn word(m: &mut Machine16, stem: &str, value: u16) {
    w(m, &format!("{stem}L"), value as u8);
    w(m, &format!("{stem}H"), (value >> 8) as u8);
}
fn wide(m: &mut Machine16, n: u8, mode: u8) {
    w(m, &format!("PWM{n}CLK"), 2);
    word(m, &format!("PWM{n}PR"), 31);
    word(m, &format!("PWM{n}S1P1"), 8);
    word(m, &format!("PWM{n}S1P2"), 16);
    w(m, &format!("PWM{n}S1CFG"), mode);
    w(m, &format!("PWM{n}CON"), 0x80);
}
#[test]
fn sixteen_bit_both_outputs_and_modules_on_171_and_132() {
    for part in ["16F17126", "16F17146", "16F13214", "16F13245"] {
        for n in 1..=2 {
            let mut m = machine(part, "");
            route(&mut m, 11 + 2 * (n - 1));
            wide(&mut m, n, 0);
            let mut values = Vec::new();
            for _ in 0..24 {
                values.push(r(&mut m, "PORTA") & 3);
                m.cpu.step();
            }
            assert_eq!(&values[..8], &[3, 3, 2, 2, 0, 0, 0, 0], "{part} PWM{n}");
            assert_eq!(&values[..8], &values[8..16], "{part}");
            assert!(has_field(&mut m, &format!("PWM{n}PIF")));
            assert_eq!(r(&mut m, &format!("PWM{n}GIR")) & 3, 3);
            assert!(
                m.cpu.unmodelled.is_empty(),
                "{part}: {:?}",
                m.cpu.unmodelled
            );
        }
    }
}
#[test]
fn sixteen_bit_load_buffer_postscale_and_group_interrupt() {
    let mut m = machine("16F17146", "");
    route(&mut m, 11);
    w(&mut m, "PWM1PIPOS", 1);
    wide(&mut m, 1, 0);
    steps(&mut m, 4);
    assert!(!has_field(&mut m, "PWM1PIF"));
    assert!(!has_field(&mut m, "PWM1IF"));
    w(&mut m, "PWM1GIE", 1);
    assert!(has_field(&mut m, "PWM1IF"));
    w(&mut m, "PWM1GIR", 0);
    assert!(!has_field(&mut m, "PWM1IF"));
    word(&mut m, "PWM1S1P1", 24); // held until LD
    steps(&mut m, 4);
    assert!(!has_field(&mut m, "PWM1PIF"));
    steps(&mut m, 2);
    assert_eq!(r(&mut m, "PORTA") & 1, 0);
    w(&mut m, "PWM1CON", 0x84);
    steps(&mut m, 6);
    assert_eq!(r(&mut m, "PWM1CON") & 4, 0);
    assert!(has_field(&mut m, "PWM1PIF"));
    steps(&mut m, 4);
    assert_eq!(r(&mut m, "PORTA") & 1, 1, "new duty lasts six cycles");
}
#[test]
fn sixteen_bit_modes_polarity_prescaler_pmd_and_reset() {
    for mode in [0, 1, 4, 5, 8] {
        let mut m = machine("16F17146", "");
        route(&mut m, 11);
        w(&mut m, "PWM1CPRE", 3);
        wide(&mut m, 1, mode);
        let mut values = Vec::new();
        for _ in 0..80 {
            values.push(r(&mut m, "PORTA") & 3);
            m.cpu.step();
        }
        assert!(values.iter().any(|&v| v != 0), "mode {mode}");
        assert!(values.iter().any(|&v| v == 0), "mode {mode}");
    }
    let mut m = machine("16F17146", "");
    route(&mut m, 11);
    wide(&mut m, 1, 0x40);
    assert_eq!(r(&mut m, "PORTA") & 3, 2);
    set_field(&mut m, "PWM1MD", true);
    steps(&mut m, 2);
    assert_eq!(
        r(&mut m, "PORTA") & 3,
        1,
        "disabled inactive level respects polarity"
    );
    m.reset().unwrap();
    assert_eq!(r(&mut m, "PWM1CON"), 0);
}
#[test]
fn unsupported_wide_clock_mode_reset_load_are_diagnosed() {
    let mut m = machine("16F17146", "");
    wide(&mut m, 1, 2);
    w(&mut m, "PWM1CLK", 7);
    w(&mut m, "PWM1ERS", 1);
    w(&mut m, "PWM1LDS", 1);
    steps(&mut m, 2);
    let notes = m.cpu.notes.iter().cloned().collect::<Vec<_>>().join("\n");
    assert!(notes.contains("mode 2"));
    assert!(notes.contains("clock 7"));
    assert!(notes.contains("external reset"));
}
#[test]
fn pwm_outputs_hold_in_sleep_and_resume_without_consuming_pulse_time() {
    let stim = pins(&[(2, 1, 100)]);
    for (part, n, code, is_wide) in [
        ("16F13145", 1, 0x2C, false),
        ("16F18346", 5, 2, false),
        ("16F17146", 1, 11, true),
        ("16F13245", 1, 11, true),
    ] {
        for sleep in [18, 20, 23] {
            let mut m = machine_at(part, &stim, Some(sleep));
            route(&mut m, code);
            if is_wide {
                wide(&mut m, n, 0);
            } else {
                basic(&mut m, n, 8, 7, 0);
            }
            w(&mut m, "IOCAP", 4);
            set_field(&mut m, "IOCIE", true);
            let mut values = vec![r(&mut m, "PORTA") & 1 != 0];
            for _ in 0..180 {
                let count = m.cpu.icount;
                m.cpu.step();
                if count != m.cpu.icount {
                    values.push(r(&mut m, "PORTA") & 1 != 0);
                }
            }
            let mut runs = Vec::new();
            let mut high = 0;
            for v in values {
                if v {
                    high += 1;
                } else if high > 0 {
                    runs.push(high);
                    high = 0;
                }
            }
            assert!(runs.len() > 10, "{part}");
            assert!(
                runs.iter().all(|&v| v == 2),
                "{part} sleep {sleep}: {runs:?}"
            );
        }
    }
}
#[test]
fn wide_period_interrupt_dispatches_to_pic16_vector() {
    let mut m = machine("16F17146", "");
    wide(&mut m, 1, 0);
    set_field(&mut m, "PWM1PIE", true);
    w(&mut m, "INTCON", 0xC0);
    m.cpu.pc = 100;
    for _ in 0..12 {
        m.cpu.step();
        if m.cpu.pc == 4 {
            break;
        }
    }
    assert_eq!(m.cpu.pc, 4);
    assert!(has_field(&mut m, "PWM1PIF"));
}
#[test]
fn stopped_basic_timer_holds_the_output_and_remaining_pulse() {
    let mut m = machine("16F13145", "");
    basic(&mut m, 1, 16, 15, 0);
    while r(&mut m, "PWM1CON") & 0x20 == 0 {
        m.cpu.step();
        assert!(m.cpu.cycles < 40);
    }
    w(&mut m, "T2CON", 0);
    assert!(trace(&mut m, 1, 20).iter().all(|&v| v));
    w(&mut m, "T2CON", 0x80);
    let levels = trace(&mut m, 1, 8);
    assert!(levels[..3].iter().all(|&v| v));
    assert!(levels[5..].iter().all(|&v| !v));
}

#[test]
fn lazy_run_preserves_pwm_flags_phase_and_pins() {
    for (part, n, code, wide_pwm) in [
        ("16F13145", 1, 0x2C, false),
        ("16F18346", 5, 2, false),
        ("16F17146", 1, 11, true),
        ("16F13245", 1, 11, true),
    ] {
        let configure = || {
            let mut m = machine(part, "");
            route(&mut m, code);
            if wide_pwm {
                wide(&mut m, n, 0);
            } else {
                basic(&mut m, n, 8, 7, 1);
            }
            m
        };
        let mut stepped = configure();
        let mut running = configure();
        steps(&mut stepped, 99);
        running.run(None, 99);
        for name in ["PORTA", "PIR3", &format!("PWM{n}CON")] {
            if stepped.dev.find_sfr(name).is_some() {
                assert_eq!(
                    r(&mut stepped, name),
                    r(&mut running, name),
                    "{part} {name}"
                );
            }
        }
        for _ in 0..24 {
            stepped.cpu.step();
            running.cpu.step();
            assert_eq!(r(&mut stepped, "PORTA"), r(&mut running, "PORTA"), "{part}");
        }
    }
}

#[test]
fn every_pic16_device_description_runs_each_standalone_pwm_instance() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices");
    let mut parts = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_stem().unwrap().to_string_lossy();
        if path.extension().is_none_or(|e| e != "toml")
            || !name.starts_with("16F")
            || name.contains('.')
        {
            continue;
        }
        let dev = Device::load(&path).unwrap();
        let mut instances = 0;
        for n in 1..=6 {
            if dev.find_sfr(&format!("PWM{n}CON")).is_none() {
                continue;
            }
            let is_wide = dev.find_sfr(&format!("PWM{n}S1P1L")).is_some();
            let code = if is_wide {
                11 + 2 * (n - 1)
            } else if name.starts_with("16F131") {
                0x2B + n
            } else if name.starts_with("16F152") {
                n
            } else if name.starts_with("16F180") {
                8 + n
            } else {
                n - 3
            };
            let mut m = Machine16::new(dev.clone()).unwrap();
            m.cpu.prog[..256].fill(0);
            route(&mut m, code);
            if is_wide {
                wide(&mut m, n, 0);
            } else {
                basic(&mut m, n, 8, 7, 0);
            }
            let mut states = [false; 2];
            for _ in 0..40 {
                m.cpu.step();
                states[usize::from(r(&mut m, "PORTA") & 1 != 0)] = true;
            }
            assert_eq!(states, [true, true], "{name} PWM{n}");
            assert!(
                m.cpu.unmodelled.is_empty(),
                "{name}: {:?}",
                m.cpu.unmodelled
            );
            instances += 1;
        }
        if instances > 0 {
            parts += 1;
        }
    }
    assert!(parts >= 33, "only exercised {parts} parts");
}
