//! Source-level stepping on a program built with `basil -g` (tests/data/i2c_q10_g.*).
//! The fixture is `i2c_q10_g.bas` copied to `main.bas` (the tests name that file) and built with basil 0.1.124
//! (Basic_Compiler commit 03709be): `basil -g main.bas`. A newer basil allocates RAM differently and moves addresses.
use devdesc::Device;
use picsim::debuginfo::DebugInfo;
use picsim::{Machine, Stop};
use std::path::Path;

fn machine() -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_hex(include_str!("data/i2c_q10_g.hex")).unwrap();
    m.load_debug(DebugInfo::from_elf(include_bytes!("data/i2c_q10_g.elf")).unwrap());
    m
}

fn line(m: &Machine) -> Option<(String, u32)> {
    m.debug.lines.lookup(m.cpu.pc).map(|(f, l)| (f.file_name().unwrap().to_string_lossy().into_owned(), l))
}

#[test]
fn symbols_come_from_the_elf() {
    let m = machine();
    assert!(m.syms.contains_key("__halt"));
    assert!(m.syms.contains_key("I2cSend"));
    assert_eq!(m.syms["iq_s0"], 0x0f);
}

#[test]
fn line_over_walks_the_main_program_in_order() {
    let mut m = machine();
    let mut seen = Vec::new();
    for _ in 0..80 {
        if m.step_line(true, false, 1_000_000).is_some() {
            break;
        }
        // Library routines the compiler inlined run at the caller's depth, so they show up as i2c.bas lines;
        // calls that were not inlined are skipped. The main.bas lines come in program order.
        if let Some((f, l)) = line(&m)
            && f == "main.bas"
        {
            seen.push(l);
        }
    }
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    assert!(seen.contains(&18) && seen.contains(&25) && seen.contains(&27), "{seen:?}");
}

#[test]
fn line_into_enters_the_library_and_out_returns() {
    let mut m = machine();
    // run to line 18 (iq_s0 = I2c.I2cStart())
    while line(&m).map(|(_, l)| l) != Some(18) {
        assert_eq!(m.step_line(true, false, 1_000_000), None);
    }
    let depth = m.stack_depth();
    let mut guard = 0;
    while line(&m).is_some_and(|(f, _)| f == "main.bas") {
        assert_eq!(m.step_line(false, false, 1_000_000), None);
        guard += 1;
        assert!(guard < 50);
    }
    let (f, _) = line(&m).unwrap();
    assert_eq!(f, "i2c.bas");
    let fi = m.debug.func_at(m.cpu.pc).unwrap();
    assert_eq!(m.debug.funcs[fi].name, "I2cStart");
    assert!(m.stack_depth() > depth);
    assert_eq!(m.step_line(false, true, 1_000_000), None);
    assert_eq!(m.stack_depth(), depth);
}

#[test]
fn stops_at_a_breakpoint_and_at_halt() {
    let mut m = machine();
    let a = m.debug.lines.first_addr(&m.debug.lines.files.iter().find(|f| f.ends_with("main.bas")).unwrap().clone(), 25).unwrap();
    m.breakpoints.push(a);
    let mut r = None;
    for _ in 0..40 {
        r = m.step_line(true, false, 1_000_000);
        if r.is_some() {
            break;
        }
    }
    assert_eq!(r, Some(Stop::Breakpoint(a)));
    m.breakpoints.clear();
    let mut r = None;
    for _ in 0..40 {
        r = m.step_line(true, false, 1_000_000);
        if r.is_some() {
            break;
        }
    }
    assert_eq!(r, Some(Stop::Halt));
}

/// A real program (tests/data/zed_g.*) whose first library routine starts exactly at `__halt`: calling it must not count as halting.
fn zed() -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_hex(include_str!("data/zed_g.hex")).unwrap();
    m.load_debug(DebugInfo::from_elf(include_bytes!("data/zed_g.elf")).unwrap());
    m
}

#[test]
fn calling_the_routine_at_halt_is_not_a_halt() {
    let mut m = zed();
    let halt = m.syms["__halt"];
    // a library routine starts right at __halt and is called during start-up
    assert!(m.debug.lines.lookup(halt).is_some_and(|(f, _)| f.ends_with("serial.bas")));
    let mut r = None;
    for _ in 0..200 {
        r = m.step_line(false, false, 5_000_000);
        if r.is_some() {
            break;
        }
    }
    assert_eq!(r, None, "stopped at {:#x} (halt {halt:#x})", m.cpu.pc);
    // and a plain run goes on into the main loop instead of stopping
    let mut m = zed();
    assert_eq!(m.run(Some(m.syms["__halt"]), 5_000_000), Stop::Cycles);
    // the main loop, `DelayMs(10)` at line 369
    assert_eq!(line(&m).map(|(f, _)| f).as_deref(), Some("main.bas"));
}
