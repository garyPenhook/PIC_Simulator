//! C programs built with MPLAB XC8 (`xc8-cc -g -O0`): the line table (`.c` files are source, the generated `.s` is not), routines,
//! variables at fixed addresses (typedef/volatile types, no SFRs from the device header), `--until ROUTINE`, and line stepping.
//! Fixtures `data/xc8_q10.*` (PIC18F27Q10) and `data/xc8_p16.*` (PIC16F17146) are built from `data/xc8_q10.c` / `xc8_p16.c`
//! (the same program) with `xc8-cc -mcpu=PART -mdfp=DFP -g -O0 xc8_TAG.c -o xc8_TAG.elf`.

use devdesc::Device;
use picsim::debuginfo::{DebugInfo, Kind};
use picsim::machine16::Machine16;
use picsim::{Machine, Stop};
use std::path::Path;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

fn device(part: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap()
}

fn q10() -> Machine {
    let mut m = Machine::new(device("18F27Q10")).unwrap();
    m.load_hex(&std::fs::read_to_string(format!("{DATA}/xc8_q10.hex")).unwrap()).unwrap();
    m.load_elf(Path::new(&format!("{DATA}/xc8_q10.elf"))).unwrap();
    m
}

fn p16() -> Machine16 {
    let mut m = Machine16::new(device("16F17146")).unwrap();
    m.load_hex(&std::fs::read_to_string(format!("{DATA}/xc8_p16.hex")).unwrap()).unwrap();
    m.load_elf(Path::new(&format!("{DATA}/xc8_p16.elf"))).unwrap();
    m
}

fn info(tag: &str) -> DebugInfo {
    DebugInfo::from_elf(&std::fs::read(format!("{DATA}/xc8_{tag}.elf")).unwrap()).unwrap()
}

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

#[test]
fn c_files_are_source_and_generated_assembly_is_not() {
    for (tag, start) in [("q10", 0x1002), ("p16", 0x7cc)] {
        let d = info(tag);
        let files: Vec<String> = d.lines.source_files().iter().map(|f| name(f)).collect();
        assert_eq!(files, [format!("xc8_{tag}.c")], "only the C file; not t.s, startup.s or the device header");
        let (f, l) = d.lines.lookup(start).unwrap_or_else(|| panic!("{tag}: no line at the start of main"));
        assert_eq!(name(f), format!("xc8_{tag}.c"));
        assert_eq!(l, 10, "the first statement of main (the `{{` at line 8 has the same address)");
        assert_eq!(d.lines.lookup(0), None, "the reset vector is generated start-up code");
        let file = d.lines.source_files()[0].clone();
        let at12 = d.lines.first_addr(&file, 12).expect("the for loop has code");
        assert_eq!(d.lines.lookup(at12).map(|(_, l)| l), Some(12), "a breakpoint on a line lands on that line");
    }
}

#[test]
fn routines_have_ranges_and_a_routine_entry_is_known() {
    for (tag, main, add) in [("q10", 0x1002, 0x105c), ("p16", 0x7cc, 0x7c8)] {
        let d = info(tag);
        let fi = |n: &str| d.funcs.iter().position(|f| f.name == n).unwrap_or_else(|| panic!("{tag}: routine {n}"));
        assert_eq!(d.funcs[fi("main")].low, main);
        assert_eq!(d.funcs[fi("add")].low, add);
        assert!(d.funcs[fi("add")].low < d.funcs[fi("add")].high);
        assert_eq!(d.func_at(add), Some(fi("add")));
        assert!(d.is_routine_entry(main) && d.is_routine_entry(add));
        assert!(!d.is_routine_entry(main + 2));
        assert!(!d.funcs.iter().any(|f| f.name.starts_with("__")), "no C-library or assembler routines by accident");
    }
}

#[test]
fn variables_come_from_the_c_unit_only() {
    for (tag, counter, x, i, a, b) in [("q10", 0x06, 0x05, 0x03, 0x02, 0x01), ("p16", 0x77, 0x76, 0x74, 0x71, 0x70)] {
        let d = info(tag);
        let v = |n: &str, scope: Option<&str>| {
            let s = scope.map(|s| d.funcs.iter().position(|f| f.name == s).unwrap());
            d.vars.iter().find(|v| v.name == n && v.scope == s).unwrap_or_else(|| panic!("{tag}: variable {n} in {scope:?}"))
        };
        // `volatile unsigned int`: the qualifier is looked through.
        let c = v("counter", None);
        assert_eq!((c.addr, c.bytes, &c.kind), (counter, 2, &Kind::Unsigned));
        let xv = v("x", Some("main"));
        assert_eq!((xv.addr, xv.bytes, &xv.kind), (x, 1, &Kind::Unsigned), "unsigned char is a number, not text");
        let iv = v("i", Some("main"));
        assert_eq!((iv.addr, iv.bytes, &iv.kind), (i, 2, &Kind::Signed));
        assert_eq!((v("a", Some("add")).addr, v("a", Some("add")).param), (a, true));
        assert_eq!((v("b", Some("add")).addr, v("b", Some("add")).param), (b, true));
        // The device header's SFR "variables" (PORTB, LATB, TRISB ...) are not the program's.
        assert!(!d.vars.iter().any(|v| v.name.starts_with("LAT") || v.name.starts_with("TRIS") || v.name.starts_with("PORT")));
    }
}

#[test]
fn run_stops_at_a_routine_reached_by_a_call_on_pic18() {
    let mut m = q10();
    let add = m.syms["add"];
    m.until_routine = true;
    assert_eq!(m.run(Some(add), 10_000), Stop::Halt);
    assert_eq!(m.cpu.pc, add);
    assert_eq!(m.cpu.peek(0x01), 2, "add@b was stored before the call");
    // Without the flag a call into the address is not "arriving" (the rule `__halt` needs), so the run goes on.
    let mut m = q10();
    assert_eq!(m.run(Some(add), 10_000), Stop::Cycles);
}

#[test]
fn run_stops_at_a_routine_reached_by_a_call_on_pic16() {
    let mut m = p16();
    let add = m.syms["add"];
    m.until_routine = true;
    assert_eq!(m.run(Some(add), 10_000), Stop::Halt);
    assert_eq!(m.cpu.pc, add);
    assert_eq!(m.cpu.peek(0x70), 2, "add@b was stored before the call");
    let mut m = p16();
    assert_eq!(m.run(Some(add), 10_000), Stop::Cycles);
}

#[test]
fn line_stepping_goes_through_the_c_source() {
    let mut m = q10();
    let main = m.syms["main"];
    let line = |m: &Machine| m.debug.lines.lookup(m.cpu.pc).map(|(_, l)| l);
    m.run(Some(main), 100); // to the first line of main (the call-exemption does not matter: main is jumped to)
    let first = line(&m).expect("main has a line");
    let mut seen = vec![first];
    for _ in 0..6 {
        assert_eq!(m.step_line(false, false, 1000), None);
        seen.push(line(&m).expect("on a C line"));
    }
    assert!(seen.windows(2).all(|w| w[0] != w[1]), "every step moves to a different line: {seen:?}");
    assert!(seen.contains(&6), "stepped into add (line 6): {seen:?}");
}

/// The Variables tab shows the locals of every routine with a pending call. Stopped on entry to `add` (first loop pass), the
/// return address on the stack lies in `main`, and `main`'s locals read from their fixed addresses: x = 1, i = 0.
#[test]
fn the_caller_of_a_routine_is_found_from_the_stack_and_its_locals_read() {
    let d = info("q10");
    let mut m = q10();
    let add = m.syms["add"];
    m.until_routine = true;
    assert_eq!(m.run(Some(add), 10_000), Stop::Halt);
    let sp = usize::from(m.cpu.peek(m.cpu.m.regs.stkptr) & 0x7F);
    assert_eq!(sp, 1, "one pending call");
    let caller = d.func_at(m.cpu.stack[sp] - 2).expect("return address is in a routine"); // the CALL is the instruction before it
    assert_eq!(d.funcs[caller].name, "main");
    let mem: Vec<u8> = (0..0x1000).map(|a| m.cpu.peek(a)).collect();
    let local = |d: &DebugInfo, n: &str| d.vars.iter().find(|v| v.name == n && v.scope == Some(caller)).unwrap_or_else(|| panic!("{n}")).show(&mem);
    assert_eq!(local(&d, "x"), "1 (0x01)");
    assert_eq!(local(&d, "i"), "0 (0x0000)");

    let d = info("p16");
    let mut m = p16();
    let add = m.syms["add"];
    m.until_routine = true;
    assert_eq!(m.run(Some(add), 10_000), Stop::Halt);
    assert_eq!(m.cpu.depth(), 1);
    let caller = d.func_at(m.cpu.stack_entry(0) - 1).expect("return address is in a routine");
    assert_eq!(d.funcs[caller].name, "main");
    let mem: Vec<u8> = (0..0x1000).map(|a| m.cpu.peek(a)).collect();
    let local = |d: &DebugInfo, n: &str| d.vars.iter().find(|v| v.name == n && v.scope == Some(caller)).unwrap_or_else(|| panic!("{n}")).show(&mem);
    assert_eq!(local(&d, "x"), "1 (0x01)");
    assert_eq!(local(&d, "i"), "0 (0x0000)");
}
