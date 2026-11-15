//! Q41 pin interrupts: IOCIF = PIR0.7, INT0/1/2IF = PIR1/6/10.0,
//! edge selection in INTCON0 and PPS routing. All six package/memory variants.
//! Exercises stimulus edges through flag generation, Sleep and both interrupt modes.

use devdesc::Device;
use picsim::{Machine, pic18::State};
use std::path::Path;

const PARTS: [&str; 6] = ["18F04Q41", "18F05Q41", "18F06Q41", "18F14Q41", "18F15Q41", "18F16Q41"];
const SOURCES: [(&str, u8, u8); 4] = [("IOC", 0, 7), ("INT0", 1, 0), ("INT1", 6, 0), ("INT2", 10, 0)];

fn rec(addr: u16, ty: u8, data: &[u8]) -> String {
    let mut b = vec![data.len() as u8, (addr >> 8) as u8, addr as u8, ty];
    b.extend_from_slice(data);
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

fn r(m: &mut Machine, n: &str) -> u8 {
    m.cpu.read(m.dev.find_sfr(n).unwrap().addr)
}

fn w(m: &mut Machine, n: &str, v: u8) {
    m.cpu.write(m.dev.find_sfr(n).unwrap().addr, v);
}

fn machine(part: &str, sleep: bool, vectored: bool, stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let mut m = Machine::new(dev).unwrap();
    // CONFIG3.MVECEN = selected mode; CONFIG5.WDTE = off. Execute at 0x200,
    // leaving room for all four vector table entries. Each ISR is RETFIE FAST.
    let mut hex = rec(0x200, 0, if sleep { &[3, 0, 0xFF, 0xD7] } else { &[0xFF, 0xD7] });
    hex += &rec(8, 0, &[0x11, 0]);
    hex += &rec(24, 0, &[0x11, 0]);
    if vectored {
        for (_, pir, bit) in SOURCES {
            let vector = 8 * u16::from(pir) + u16::from(bit);
            let handler = 0x100 + 4 * vector;
            hex += &rec(8 + 2 * vector, 0, &(handler / 4).to_le_bytes());
            hex += &rec(handler, 0, &[0x11, 0]);
        }
    }
    hex += &rec(0, 4, &[0, 0x30]);
    hex += &rec(2, 0, &[if vectored { 0xFF } else { 0xF7 }]);
    hex += &rec(4, 0, &[0x1F]);
    hex += &rec(0, 1, &[]);
    m.load_hex(&hex).unwrap();
    m.cpu.pc = 0x200;
    m.load_stimulus(stim).unwrap();
    w(&mut m, "ANSELA", 0);
    w(&mut m, "ANSELC", 0);
    m
}

fn edges(port: char, bit: u8) -> String {
    format!("[[pin]]\nport = \"{port}\"\nbit = {bit}\nlevel = 1\nat = 100\n[[pin]]\nport = \"{port}\"\nbit = {bit}\nlevel = 0\nat = 200\n")
}

fn run_to(m: &mut Machine, cycle: u64) {
    // Bounded even if an interrupt/reset unexpectedly restarts the cycle counter.
    for _ in 0..10_000 {
        if m.cpu.cycles >= cycle {
            return;
        }
        m.cpu.step();
    }
    panic!("{} did not reach cycle {cycle}", m.dev.device.name);
}

fn source(m: &mut Machine, name: &str) {
    if name == "IOC" {
        w(m, "IOCAP", 4); // RA2 exists on every Q41 package.
    } else {
        w(m, &format!("{name}PPS"), 2); // RA2, overriding the reset routing.
    }
}

#[test]
fn external_interrupts_select_either_edge_on_a_remapped_pin_without_ie() {
    for part in PARTS {
        for (name, pir, bit) in SOURCES.into_iter().skip(1) {
            for rising in [true, false] {
                // RC3 (PPS code 0x13) differs from every INTx reset selection.
                let mut m = machine(part, false, false, &edges('C', 3));
                w(&mut m, &format!("{name}PPS"), 0x13);
                let edg = 1 << (name.as_bytes()[3] - b'0');
                w(&mut m, "INTCON0", if rising { edg } else { 0 });
                let flag = format!("PIR{pir}");
                run_to(&mut m, 150);
                assert_eq!(r(&mut m, &flag) & (1 << bit) != 0, rising, "{part} {name} rising={rising}");
                w(&mut m, &flag, 0);
                run_to(&mut m, 250);
                assert_eq!(r(&mut m, &flag) & (1 << bit) != 0, !rising, "{part} {name} falling");
                assert_eq!(r(&mut m, &format!("PIE{pir}")), 0, "flag generation ignores IE");
                assert_eq!(m.cpu.stack_level(), 0);
            }
        }
    }
}

#[test]
fn ioc_edges_and_summary_flag_follow_all_ports_and_software_clears() {
    for part in PARTS {
        // Ports A and C exist on both package sizes; B exists on 20-pin parts.
        for (port, bit) in [('A', 0), ('B', 4), ('C', 0)] {
            let mut m = machine(part, false, false, "");
            if m.dev.find_sfr(&format!("IOC{port}P")).is_none() {
                continue;
            }
            m.load_stimulus(&edges(port, bit)).unwrap();
            let mask = 1 << bit;
            w(&mut m, &format!("ANSEL{port}"), 0);
            for rising in [true, false] {
                w(&mut m, &format!("IOC{port}P"), if rising { mask } else { 0 });
                w(&mut m, &format!("IOC{port}N"), if rising { 0 } else { mask });
                run_to(&mut m, if rising { 150 } else { 250 });
                assert_eq!(r(&mut m, &format!("IOC{port}F")) & mask, mask, "{part} port {port}");
                assert_eq!(r(&mut m, "PIR0") & 0x80, 0x80);
                w(&mut m, "PIR0", 0); // clearing the summary alone cannot acknowledge IOC
                m.cpu.step();
                assert_eq!(r(&mut m, "PIR0") & 0x80, 0x80);
                w(&mut m, &format!("IOC{port}F"), 0);
                m.cpu.step();
                assert_eq!(r(&mut m, "PIR0") & 0x80, 0);
            }
        }
    }
}

#[test]
fn ioc_without_edge_enables_ignores_both_edges() {
    for part in PARTS {
        let mut m = machine(part, false, false, &edges('A', 2));
        run_to(&mut m, 250);
        assert_eq!(r(&mut m, "IOCAF"), 0, "{part}");
        assert_eq!(r(&mut m, "PIR0") & 0x80, 0);
    }
}

#[test]
fn sleep_flags_wake_only_with_the_source_enable_even_with_gie_clear() {
    for part in PARTS {
        for (name, pir, bit) in SOURCES {
            for enabled in [false, true] {
                let mut m = machine(part, true, false, &edges('A', 2));
                source(&mut m, name);
                w(&mut m, &format!("PIE{pir}"), if enabled { 1 << bit } else { 0 });
                m.cpu.step();
                assert_eq!(m.cpu.state, State::Sleeping, "{part} {name}");
                run_to(&mut m, 100);
                assert_eq!(r(&mut m, &format!("PIR{pir}")) & (1 << bit), 1 << bit, "{part} {name}");
                m.cpu.step();
                assert_eq!(m.cpu.state, if enabled { State::Running } else { State::Sleeping }, "{part} {name} IE={enabled}");
                assert_eq!(m.cpu.pc, 0x202, "resume after SLEEP, no ISR");
                assert_eq!(r(&mut m, "STATUS") & 0x60, 0x40, "interrupt wake preserves TO=1, PD=0");
                if enabled {
                    assert!(m.cpu.cycles < 110, "woke at the edge");
                }
            }
        }
    }
}

#[test]
fn pin_edges_enter_fixed_and_vectored_handlers_at_both_priorities_and_return() {
    for part in PARTS {
        for (name, pir, bit) in SOURCES {
            for vectored in [false, true] {
                for high in [false, true] {
                    let mut m = machine(part, false, vectored, &edges('A', 2));
                    source(&mut m, name);
                    w(&mut m, &format!("IPR{pir}"), if high { 1 << bit } else { 0 });
                    w(&mut m, &format!("PIE{pir}"), 1 << bit);
                    w(&mut m, "INTCON0", 0xE7); // IPEN, GIEH, GIEL, rising edges
                    w(&mut m, "WREG", 0xA5);
                    let vector = 8 * u16::from(pir) + u16::from(bit);
                    let handler = if vectored {
                        u32::from(0x100 + 4 * vector)
                    } else if high {
                        8
                    } else {
                        24
                    };
                    for _ in 0..100 {
                        if m.cpu.pc == handler {
                            break;
                        }
                        m.cpu.step();
                    }
                    assert_eq!(m.cpu.pc, handler, "{part} {name} MVECEN={vectored} high={high}");
                    assert_eq!(r(&mut m, "INTCON1") >> 6, if high { 2 } else { 1 });
                    if vectored {
                        assert_eq!(r(&mut m, "WREG"), vector as u8);
                        assert_eq!(u16::from(r(&mut m, "IVTADL")) | u16::from(r(&mut m, "IVTADH")) << 8, 8 + 2 * vector);
                    }
                    if name == "IOC" {
                        w(&mut m, "IOCAF", 0);
                    } else {
                        w(&mut m, &format!("PIR{pir}"), 0);
                    }
                    m.cpu.step(); // RETFIE FAST
                    assert_eq!(m.cpu.pc, 0x200, "resume interrupted loop");
                    assert_eq!(m.cpu.stack_level(), 0);
                    assert_eq!(r(&mut m, "INTCON1") >> 6, 0);
                    assert_eq!(r(&mut m, "WREG"), 0xA5, "restore context");
                }
            }
        }
    }
}
