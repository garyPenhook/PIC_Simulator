//! Interrupt-on-change and the INT pin on the PIC16 core, awake and in Sleep (PIC16F17146 DS40002343F 17.7 p.196-197,
//! 12.7-12.8 p.113; PIC16F18346 DS40001839E ch.15). The pin RA2 is driven by `[[pin]]` entries of the stimulus.

use devdesc::Device;
use picsim::machine16::Machine16;
use std::path::Path;

const PARTS: [&str; 2] = ["16F17146", "16F18346"];
const BRA_SELF: [u16; 1] = [0x33FF];
const SLEEP_BRA: [u16; 2] = [0x0063, 0x33FF];
/// RA2 rises at cycle 100 and falls at cycle 200.
const UP_DOWN: &str = "[[pin]]\nport = \"A\"\nbit = 2\nlevel = 1\nat = 100\n[[pin]]\nport = \"A\"\nbit = 2\nlevel = 0\nat = 200\n";

fn hexrec(b: &[u8]) -> String {
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

fn machine(part: &str, prog: &[u16]) -> Machine16 {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let mut m = Machine16::new(dev).unwrap();
    let mut text = String::new();
    for (i, &w) in prog.iter().enumerate() {
        let a = 2 * i as u32;
        for (a, b) in [(a, w as u8), (a + 1, (w >> 8) as u8)] {
            text += &hexrec(&[2, 0, 0, 4, (a >> 24) as u8, (a >> 16) as u8]);
            text += &hexrec(&[1, (a >> 8) as u8, a as u8, 0, b]);
        }
    }
    // the watchdog is off (WDTE = 0): unprogrammed it is on and would wake a sleeping part after 2 s
    for r in m.dev.config.clone() {
        if let Some(f) = r.fields.iter().find(|f| f.name == "WDTE") {
            let v = r.default & !(((1u32 << f.width) - 1) << f.pos);
            for (a, b) in [(2 * r.addr, v as u8), (2 * r.addr + 1, (v >> 8) as u8)] {
                text += &hexrec(&[2, 0, 0, 4, (a >> 24) as u8, (a >> 16) as u8]);
                text += &hexrec(&[1, (a >> 8) as u8, a as u8, 0, b]);
            }
        }
    }
    m.load_hex(&(text + ":00000001FF\n")).unwrap();
    m.load_stimulus(UP_DOWN).unwrap();
    w(&mut m, "ANSELA", 0); // digital inputs
    w(&mut m, "INTPPS", 0x02); // RA2
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

/// Register and mask of a bit that sits in one of the PIRx / PIEx / INTCON registers.
fn bit(m: &Machine16, reg_prefix: &str, name: &str) -> (u32, u8) {
    m.dev.sfr.iter().filter(|s| s.name.starts_with(reg_prefix)).find_map(|s| s.fields.iter().find(|f| f.name == name).map(|f| (s.addr, 1u8 << f.pos))).unwrap_or_else(|| panic!("no {name}"))
}

fn run_to(m: &mut Machine16, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

#[test]
fn positive_edge_sets_the_flag_and_iocif_until_cleared() {
    for part in PARTS {
        let mut m = machine(part, &BRA_SELF);
        let (ir, im) = bit(&m, "PIR", "IOCIF");
        w(&mut m, "IOCAP", 0x04);
        run_to(&mut m, 50);
        assert_eq!(r(&mut m, "IOCAF"), 0, "{part}");
        run_to(&mut m, 150);
        assert_eq!(r(&mut m, "IOCAF"), 0x04, "{part}");
        assert_eq!(m.cpu.read(ir) & im, im, "{part}: IOCIF");
        run_to(&mut m, 300); // the falling edge is not enabled
        assert_eq!(r(&mut m, "IOCAF"), 0x04, "{part}");
        w(&mut m, "IOCAF", 0);
        m.cpu.step();
        assert_eq!(m.cpu.read(ir) & im, 0, "{part}: IOCIF follows IOCAF");
    }
}

#[test]
fn negative_edge_only() {
    for part in PARTS {
        let mut m = machine(part, &BRA_SELF);
        w(&mut m, "IOCAN", 0x04);
        run_to(&mut m, 150);
        assert_eq!(r(&mut m, "IOCAF"), 0, "{part}: rising edge not enabled");
        run_to(&mut m, 250);
        assert_eq!(r(&mut m, "IOCAF"), 0x04, "{part}");
    }
}

#[test]
fn an_edge_with_no_enable_is_ignored() {
    for part in PARTS {
        let mut m = machine(part, &BRA_SELF);
        let (ir, im) = bit(&m, "PIR", "IOCIF");
        run_to(&mut m, 300);
        assert_eq!(r(&mut m, "IOCAF"), 0, "{part}");
        assert_eq!(m.cpu.read(ir) & im, 0, "{part}");
    }
}

#[test]
fn int_follows_the_edge_select() {
    for part in PARTS {
        let mut m = machine(part, &BRA_SELF);
        let (fr, fm) = bit(&m, "PIR", "INTF");
        let (er, em) = bit(&m, "INTCON", "INTEDG");
        assert_eq!(er, 0x0B);
        let ic = m.cpu.read(er);
        m.cpu.write(er, ic | em); // rising
        run_to(&mut m, 150);
        assert_eq!(m.cpu.read(fr) & fm, fm, "{part}: INTF on the rising edge");
        let mut m = machine(part, &BRA_SELF);
        let ic = m.cpu.read(er);
        m.cpu.write(er, ic & !em); // falling
        run_to(&mut m, 150);
        assert_eq!(m.cpu.read(fr) & fm, 0, "{part}");
        run_to(&mut m, 250);
        assert_eq!(m.cpu.read(fr) & fm, fm, "{part}: INTF on the falling edge");
    }
}

#[test]
fn ioc_wakes_the_part_from_sleep_when_iocie_is_set() {
    for part in PARTS {
        let mut m = machine(part, &SLEEP_BRA);
        let (pr, pm) = bit(&m, "PIE", "IOCIE");
        w(&mut m, "IOCAP", 0x04);
        m.cpu.write(pr, pm);
        m.cpu.step(); // SLEEP
        run_to(&mut m, 100);
        assert_eq!(r(&mut m, "IOCAF"), 0x04, "{part}: set before the first instruction after the wake-up");
        m.cpu.step();
        assert!(m.cpu.cycles < 110, "{part}: woke at the edge, cycle {}", m.cpu.cycles);
    }
}

#[test]
fn ioc_without_iocie_does_not_wake() {
    for part in PARTS {
        let mut m = machine(part, &SLEEP_BRA);
        w(&mut m, "IOCAP", 0x04);
        m.cpu.step();
        for _ in 0..150 {
            m.cpu.step();
        }
        assert_eq!(r(&mut m, "IOCAF"), 0x04, "{part}: the flag is set in Sleep");
        assert_eq!(m.cpu.pc, 1, "{part}: still asleep");
    }
}

#[test]
fn int_wakes_the_part_from_sleep() {
    for part in PARTS {
        let mut m = machine(part, &SLEEP_BRA);
        let (er, em) = bit(&m, "INTCON", "INTEDG");
        let ic = m.cpu.read(er);
        m.cpu.write(er, ic | em);
        let (pr, pm) = bit(&m, "PIE", "INTE");
        m.cpu.write(pr, pm);
        m.cpu.step();
        run_to(&mut m, 100);
        let (fr, fm) = bit(&m, "PIR", "INTF");
        assert_eq!(m.cpu.read(fr) & fm, fm, "{part}");
        m.cpu.step();
        assert!(m.cpu.cycles < 110, "{part}: woke at the edge, cycle {}", m.cpu.cycles);
    }
}

#[test]
fn a_pending_pin_change_keeps_a_sleeping_part_from_being_declared_dead() {
    let mut m = machine("16F17146", &SLEEP_BRA);
    m.cpu.step();
    assert!(!m.cpu.sleep_dead(), "the stimulus has edges to come");
    run_to(&mut m, 300);
    assert!(m.cpu.sleep_dead());
}

#[test]
fn a_sleep_without_watchdog_jumps_to_the_wake_up_edge() {
    // review P6: with the watchdog off, a Sleep waiting for a pin edge far away is not stepped cycle by cycle
    for part in PARTS {
        let mut m = machine(part, &SLEEP_BRA);
        // added to UP_DOWN, whose RA2 edges are not watched here: RA4 rises at cycle 10 M
        m.load_stimulus("[[pin]]\nport = \"A\"\nbit = 4\nlevel = 1\nat = 10000000\n").unwrap();
        let (pr, pm) = bit(&m, "PIE", "IOCIE");
        w(&mut m, "IOCAP", 0x10);
        m.cpu.write(pr, pm);
        m.cpu.step(); // SLEEP
        let mut steps = 0;
        while m.cpu.cycles < 10_000_000 {
            m.cpu.step();
            steps += 1;
        }
        assert!(steps < 10, "{part}: {steps} steps to the edge");
        assert_eq!(m.cpu.cycles, 10_000_000, "{part}: stopped at the edge");
        assert_eq!(r(&mut m, "IOCAF"), 0x10, "{part}: the edge is seen");
        m.cpu.step();
        m.cpu.step();
        assert!(m.cpu.cycles < 10_000_010, "{part}: woke at the edge, cycle {}", m.cpu.cycles);
    }
}
