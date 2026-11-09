//! Source lines on the PIC16 core: the ELF's line table (word addresses), the part named in the ELF, and Machine16's line stepping.
//! The fixture `data/p16lines.elf` is hand-written assembly with `#line` directives (see `data/p16lines.S`),
//! assembled with `pic-as -mcpu=16F17146 -mdfp=DFP -g p16lines.S -o p16lines.elf`.

use devdesc::Device;
use picsim::machine16::Machine16;
use picsim::Stop;
use std::path::Path;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

fn machine() -> Machine16 {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/16F17146.toml")).unwrap();
    let mut m = Machine16::new(dev).unwrap();
    m.load_hex(&std::fs::read_to_string(format!("{DATA}/p16lines.hex")).unwrap()).unwrap();
    m.load_elf(Path::new(&format!("{DATA}/p16lines.elf"))).unwrap();
    m.syms.insert("__halt".into(), 5);
    m
}

fn line(m: &Machine16) -> Option<u32> {
    m.debug.lines.lookup(m.cpu.pc).map(|(_, l)| l)
}

#[test]
fn the_part_comes_from_the_elf() {
    let elf = std::fs::read(format!("{DATA}/p16lines.elf")).unwrap();
    assert_eq!(picsim::debuginfo::device_of(&elf).as_deref(), Some("16F17146"));
    let q10 = std::fs::read(format!("{DATA}/zed_g.elf")).unwrap();
    assert!(picsim::debuginfo::device_of(&q10).is_some_and(|d| d.starts_with("18F")));
}

#[test]
fn the_line_table_is_in_words() {
    let m = machine();
    assert_eq!(m.debug.lines.source_files().len(), 1);
    assert_eq!(m.debug.lines.lookup(0).map(|(_, l)| l), Some(1));
    assert_eq!(m.debug.lines.lookup(1).map(|(_, l)| l), Some(2));
    assert_eq!(m.debug.lines.lookup(6).map(|(_, l)| l), Some(10));
    assert_eq!(m.debug.lines.lookup(9), None, "past the end of the program");
    let f = m.debug.lines.source_files()[0].clone();
    assert_eq!(m.debug.lines.first_addr(&f, 10), Some(6), "a breakpoint on line 10 goes at word 6");
}

#[test]
fn line_step_moves_to_the_next_line_and_into_a_call() {
    let mut m = machine();
    assert_eq!(line(&m), Some(1));
    assert_eq!(m.step_line(false, false, 100), None);
    assert_eq!(line(&m), Some(2));
    assert_eq!(m.step_line(false, false, 100), None);
    assert_eq!((m.cpu.pc, line(&m)), (3, Some(3)), "the MOVWF belongs to line 2 still");
    assert_eq!(m.step_line(false, false, 100), None);
    assert_eq!((m.cpu.pc, line(&m)), (6, Some(10)), "into the routine");
    assert_eq!(m.cpu.depth(), 1);
}

#[test]
fn line_over_runs_the_call_and_out_returns_from_it() {
    let mut m = machine();
    for _ in 0..2 {
        m.step_line(false, false, 100);
    }
    assert_eq!(m.cpu.pc, 3);
    assert_eq!(m.step_line(true, false, 100), None);
    assert_eq!((m.cpu.pc, line(&m), m.cpu.depth()), (4, Some(4), 0), "over the CALL");
    let mut m = machine();
    for _ in 0..3 {
        m.step_line(false, false, 100);
    }
    assert_eq!(m.cpu.pc, 6);
    assert_eq!(m.step_line(false, true, 100), None);
    assert_eq!((m.cpu.pc, m.cpu.depth()), (4, 0), "out: the instruction after the CALL");
}

#[test]
fn line_step_stops_at_halt_and_at_a_breakpoint() {
    let mut m = machine();
    m.cpu.pc = 4;
    assert_eq!(m.step_line(false, false, 100), Some(Stop::Halt));
    let mut m = machine();
    m.breakpoints.push(7);
    m.cpu.pc = 3;
    assert_eq!(m.step_line(true, false, 100), Some(Stop::Breakpoint(7)), "a breakpoint inside the call stops a step over it");
}

#[test]
fn a_step_that_does_not_finish_in_its_budget_can_be_continued() {
    let mut m = machine();
    m.cpu.pc = 3;
    let s = m.begin_line_step(true, false);
    assert_eq!(m.line_step(&s, 1), None, "the CALL alone has not reached a new line");
    assert_eq!(m.line_step(&s, 100), Some(None));
    assert_eq!(m.cpu.pc, 4);
}
