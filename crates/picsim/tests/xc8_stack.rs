//! Locals on XC8's software stack (`-mstack=reentrant`, and the routines `-mstack=hybrid` puts there). Fixtures
//! `data/xc8_stack_q10.*` / `xc8_hybrid_q10.*` (PIC18F27Q10) and `xc8_stack_p16.*` / `xc8_hybrid_p16.*` (PIC16F17146) are
//! built from the same program (one copy of the source per tag) with
//! `xc8-cc -mcpu=PART -mdfp=DFP -g -O0 -mstack=reentrant|hybrid xc8_TAG.c -o xc8_TAG.elf`.
//! XC8 4.00 describes such a variable with `DW_OP_fbreg` from a frame base of FSR1 (`DW_AT_frame_base` = FSR1L); `rec`
//! recurses, so every level has its own frame and each shows its own values.
//! `data/xc8_tail_q10.*` / `xc8_tail_p16.*` are a tail call (`xc8-cc -mcpu=PART -mdfp=DFP -g -O3 -mstack=reentrant
//! xc8_tail_TAG.c -o xc8_tail_TAG.elf`): XC8 4.00 at -O3 and -Os (not -O1 or -O2) turns a CALL followed by RETURN into a
//! jump, here after pushing the callee's arguments.

use devdesc::Device;
use picsim::debuginfo::{DebugInfo, Frame, Shape, Var};
use picsim::machine16::Machine16;
use picsim::{Machine, Stop};
use std::path::Path;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");
const TAGS: [&str; 4] = ["stack_q10", "hybrid_q10", "stack_p16", "hybrid_p16"];

fn device(part: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap()
}

/// Either core with the fixture `tag` loaded (one at a time, so the size of the larger variant does not matter).
#[allow(clippy::large_enum_variant)]
enum Sim {
    P18(Machine),
    P16(Machine16),
}

impl Sim {
    fn new(tag: &str) -> Sim {
        let hex = std::fs::read_to_string(format!("{DATA}/xc8_{tag}.hex")).unwrap();
        let elf = format!("{DATA}/xc8_{tag}.elf");
        if tag.ends_with("p16") {
            let mut m = Machine16::new(device("16F17146")).unwrap();
            m.load_hex(&hex).unwrap();
            m.load_elf(Path::new(&elf)).unwrap();
            Sim::P16(m)
        } else {
            let mut m = Machine::new(device("18F27Q10")).unwrap();
            m.load_hex(&hex).unwrap();
            m.load_elf(Path::new(&elf)).unwrap();
            Sim::P18(m)
        }
    }

    fn d(&self) -> &DebugInfo {
        match self {
            Sim::P18(m) => &m.debug,
            Sim::P16(m) => &m.debug,
        }
    }

    fn pc(&self) -> u32 {
        match self {
            Sim::P18(m) => m.cpu.pc,
            Sim::P16(m) => m.cpu.pc,
        }
    }

    fn prog(&self) -> &[u8] {
        match self {
            Sim::P18(m) => &m.cpu.prog,
            Sim::P16(m) => &m.cpu.prog,
        }
    }

    fn frames(&self) -> Vec<Frame> {
        match self {
            Sim::P18(m) => m.frames(),
            Sim::P16(m) => m.frames(),
        }
    }

    fn data(&self) -> Vec<u8> {
        match self {
            Sim::P18(m) => m.debug.data_view(&m.cpu.m.mem).into_owned(),
            Sim::P16(m) => m.debug.data_view(&m.cpu.mem).into_owned(),
        }
    }

    /// Runs until routine `name` is entered (a call stops the run).
    fn run_to(&mut self, name: &str) {
        let a = self.d().funcs[fi(self.d(), name)].low;
        let stop = match self {
            Sim::P18(m) => {
                m.until_routine = true;
                m.run(Some(a), 100_000)
            }
            Sim::P16(m) => {
                m.until_routine = true;
                m.run(Some(a), 100_000)
            }
        };
        assert_eq!(stop, Stop::Halt, "to {name}");
    }

    fn step(&mut self) {
        match self {
            Sim::P18(m) => m.cpu.step(),
            Sim::P16(m) => m.cpu.step(),
        };
    }

    fn shape(&self, name: &str) -> Shape {
        self.d().shape(fi(self.d(), name), self.prog()).unwrap_or_else(|| panic!("shape of {name}"))
    }

    /// `n tag buf p` of every live `rec` call, innermost first, as the Variables tab shows them.
    fn rec_levels(&self) -> Vec<String> {
        let (d, data) = (self.d(), self.data());
        let rom = d.rom_image(self.prog()).into_owned();
        let rec = fi(d, "rec");
        self.frames().iter().filter(|f| f.func == Some(rec)).map(|f| ["n", "tag", "buf", "p"].map(|n| var(d, "rec", n).show_framed(&data, &rom, d, f.base)).join(" | ")).collect()
    }
}

fn fi(d: &DebugInfo, n: &str) -> usize {
    d.funcs.iter().position(|f| f.name == n).unwrap_or_else(|| panic!("routine {n}"))
}

fn var<'a>(d: &'a DebugInfo, f: &str, n: &str) -> &'a Var {
    let s = Some(fi(d, f));
    d.vars.iter().find(|v| v.scope == s && v.name == n).unwrap_or_else(|| panic!("{f}: {n}"))
}

/// One level of `rec(n, tag)` as `rec_levels` writes it (`p.x` is an `int8_t`, which XC8 calls `signed char`).
fn level(n: u8, tag: u8) -> String {
    format!(
        "{n} (0x{n:02X}) | {tag} (0x{tag:02X}) | {{{n} (0x{n:02X}), {tag} (0x{tag:02X}), 90 (0x5A), 13 (0x0D)}} | {{x = '.' ({}), y = {} (0x{:04X})}}",
        n.wrapping_neg(),
        1000 + u16::from(n),
        1000 + u16::from(n)
    )
}

#[test]
fn stack_variables_are_offsets_from_the_frame_base() {
    for tag in TAGS {
        let d = DebugInfo::from_elf(&std::fs::read(format!("{DATA}/xc8_{tag}.elf")).unwrap()).unwrap();
        let fsr1l = if tag.ends_with("p16") { 0x06 } else { 0xFE1 }; // FSR1L in devices/16F17146.toml and 18F27Q10.toml
        assert_eq!(d.funcs[fi(&d, "rec")].frame_reg, Some(fsr1l), "{tag}");
        for (n, off, param) in [("n", -8, true), ("tag", -9, true), ("buf", -7, false), ("p", -3, false)] {
            let v = var(&d, "rec", n);
            assert_eq!((v.frame, v.param, v.addr, v.rom), (Some(off), param, 0, false), "{tag}: {n}");
        }
        // sum4 is not recursive: under hybrid its locals have fixed addresses; reentrant puts `t` on the stack too.
        assert_eq!(var(&d, "sum4", "t").frame, tag.starts_with("stack").then_some(-2), "{tag}: t");
        assert_eq!(var(&d, "sum4", "u").frame, None, "{tag}: u is in a temporary");
        assert!(d.vars.iter().filter(|v| v.scope.is_none()).all(|v| v.frame.is_none()), "{tag}: globals are fixed");
    }
}

#[test]
fn the_prologue_gives_the_frame_size() {
    for tag in TAGS {
        let s = Sim::new(tag);
        let rec = s.shape("rec");
        assert_eq!(rec.auto, 7, "{tag}: rec's buf and p");
        let t = if tag.starts_with("stack") { 2 } else { 0 };
        for (f, auto) in [("sum4", t), ("done", 0), ("main", 0), ("nest", 3), ("leaf", 0), ("mix", 0)] {
            assert_eq!(s.shape(f).auto, auto, "{tag}: {f}");
        }
    }
}

#[test]
fn every_level_of_the_recursion_shows_its_own_values() {
    for tag in TAGS {
        let mut s = Sim::new(tag);
        s.run_to("done"); // called from rec(0, 13), inside rec(1, 12), rec(2, 11) and rec(3, 10)
        let want: Vec<String> = [(0, 13), (1, 12), (2, 11), (3, 10)].map(|(n, t)| level(n, t)).to_vec();
        assert_eq!(s.rec_levels(), want, "{tag}");
        let fr = s.frames();
        let names: Vec<&str> = fr.iter().map(|f| f.func.map_or("?", |i| s.d().funcs[i].name.as_str())).collect();
        assert_eq!(names, ["done", "rec", "rec", "rec", "rec", "main"], "{tag}");
        // Each rec frame is 7 bytes of locals above the 2 bytes of parameters. done and main have a frame base (an empty
        // frame) only when reentrant; hybrid keeps them on the compiled stack, without DW_AT_frame_base.
        let b: Vec<u32> = fr[1..5].iter().map(|f| f.base.unwrap()).collect();
        assert!(b.windows(2).all(|w| w[0] == w[1] + 9), "{tag}: {b:?}");
        let reentrant = tag.starts_with("stack");
        assert_eq!(fr[0].base, reentrant.then_some(b[0]), "{tag}: done's empty frame is where rec's ends");
        assert_eq!(fr[5].base.is_some(), reentrant, "{tag}: main");
        // gp = &buf[1] of the innermost rec: a pointer into the stack (a linear address on the PIC16) reads its byte.
        let (d, data) = (s.d(), s.data());
        let gp = d.vars.iter().find(|v| v.name == "gp").unwrap().show_with(&data, &d.rom_image(s.prog()), d);
        let at = b[0] - 7 + 1; // buf is at fbreg -7
        assert_eq!(gp, format!("0x{at:04X} -> 13 (0x0D)"), "{tag}");
        if tag.ends_with("p16") {
            assert!(at >= 0x2000, "{tag}: FSR1 holds a linear address");
        }
    }
}

#[test]
fn at_a_routine_entry_the_parameters_are_already_readable() {
    for tag in TAGS {
        let mut s = Sim::new(tag);
        s.run_to("rec"); // rec(3, 10) from main, before its prologue
        assert_eq!(s.pc(), s.d().funcs[fi(s.d(), "rec")].low);
        let (d, data) = (s.d(), s.data());
        let base = s.frames()[0].base;
        assert!(base.is_some(), "{tag}");
        assert_eq!(var(d, "rec", "n").show_framed(&data, &[], d, base), "3 (0x03)", "{tag}");
        assert_eq!(var(d, "rec", "tag").show_framed(&data, &[], d, base), "10 (0x0A)", "{tag}");
        let main = s.frames()[1].base;
        assert_eq!(main, base.filter(|_| tag.starts_with("stack")).map(|b| b - 9), "{tag}: main's base is below rec's frame and parameters");
    }
}

#[test]
fn every_instruction_of_the_returns_keeps_the_frames_right() {
    for tag in TAGS {
        let mut s = Sim::new(tag);
        s.run_to("done");
        let want: Vec<String> = [(0, 13), (1, 12), (2, 11), (3, 10)].map(|(n, t)| level(n, t)).to_vec();
        let main = fi(s.d(), "main");
        let mut steps = 0;
        // Out of done and back through every rec epilogue and RETURN (and the additions between them) to main.
        while s.d().func_at(s.pc()) != Some(main) {
            let got = s.rec_levels();
            assert_eq!(got, want[want.len() - got.len()..], "{tag}: at {:#x}", s.pc());
            s.step();
            steps += 1;
            assert!(steps < 5000, "{tag}: rec did not return");
        }
        assert!(s.rec_levels().is_empty(), "{tag}");
    }
}

#[test]
fn a_caller_that_pushed_arguments_for_an_outer_call_keeps_its_frame() {
    for tag in TAGS {
        let mut s = Sim::new(tag);
        s.run_to("mark"); // in leaf, called from nest while mix(loc[0], ...) has its first argument pushed
        let nest = fi(s.d(), "nest");
        let shown = |s: &Sim| -> Vec<String> {
            let (d, data) = (s.d(), s.data());
            let f = s.frames().into_iter().find(|f| f.func == Some(nest)).expect("nest is live");
            ["n", "loc"].map(|n| var(d, "nest", n).show_framed(&data, &[], d, f.base)).to_vec()
        };
        let want = ["4 (0x04)", "{4 (0x04), 119 (0x77), 5 (0x05)}"];
        assert_eq!(shown(&s), want, "{tag}: in mark");
        // On through leaf's return, the pushes for mix, mix itself and back to nest's own epilogue.
        while s.d().func_at(s.pc()) != Some(nest) || s.frames().len() > 2 {
            assert_eq!(shown(&s), want, "{tag}: at {:#x}", s.pc());
            s.step();
        }
        while s.d().func_at(s.pc()) == Some(nest) {
            assert_eq!(shown(&s), want, "{tag}: in nest at {:#x}", s.pc());
            s.step();
        }
    }
}

#[test]
fn a_reset_forgets_the_calls_and_main_is_watched_again() {
    for tag in TAGS {
        let mut s = Sim::new(tag);
        s.run_to("done");
        match &mut s {
            Sim::P18(m) => m.reset(),
            Sim::P16(m) => m.reset().unwrap(),
        }
        assert_eq!(s.frames(), [Frame { func: None, base: None }], "{tag}: at the reset vector");
        s.run_to("done");
        let want: Vec<String> = [(0, 13), (1, 12), (2, 11), (3, 10)].map(|(n, t)| level(n, t)).to_vec();
        assert_eq!(s.rec_levels(), want, "{tag}: after the reset");
        let main_noted = match &s {
            Sim::P18(m) => m.cpu.entry_sp[0],
            Sim::P16(m) => m.cpu.entry_sp[0],
        };
        assert!(main_noted.is_some(), "{tag}: main's entry was noted again");
    }
}

#[test]
fn start_up_code_has_no_frame() {
    for tag in TAGS {
        let s = Sim::new(tag);
        let fr = s.frames();
        assert_eq!(fr, [Frame { func: None, base: None }], "{tag}: at the reset vector");
    }
}

/// A PIC16 routine `ADDFSR 1,2; NOP; ADDFSR 1,-2; RETURN; NOP; ADDFSR 1,-2; RETURN` at word 0x100 (encodings as in
/// `pic16.rs` `disasm_for`): the frame base is FSR1 at the call plus the prologue's 2 bytes, wherever the PC is.
#[test]
fn the_frame_base_is_fsr1_at_the_call_plus_the_prologue() {
    let words: [u16; 7] = [0x3142, 0x0000, 0x317E, 0x0008, 0x0000, 0x317E, 0x0008];
    let mut prog = vec![0xFFu8; 0x200 * 2];
    for (i, w) in words.iter().enumerate() {
        prog[2 * (0x100 + i)] = *w as u8;
        prog[2 * (0x100 + i) + 1] = (*w >> 8) as u8;
    }
    let f = picsim::debuginfo::Func { name: "f".into(), low: 0x100, high: 0x107, frame_reg: Some(6), unit: 0 };
    let mut d = DebugInfo { funcs: vec![f], word_addressed: true, fsr1l: Some(6), ..DebugInfo::default() };
    assert_eq!(d.shape(0, &prog), Some(Shape { auto: 2 }));
    for pc in 0x100..0x107 {
        assert_eq!(d.frames(pc, &[], &[Some(0x2012)], &prog)[0].base, Some(0x2014), "at {pc:#x}");
    }
    assert_eq!(d.frames(0x101, &[], &[None], &prog)[0].base, None, "no call noted");
    assert_eq!(d.frames(0x101, &[0x180], &[Some(0x2012), Some(0x2000)], &prog)[1], Frame { func: None, base: None }, "unknown caller");
    // A frame base that is not FSR1L, or none at all: no base, rather than one read from FSR1.
    d.funcs[0].frame_reg = Some(0x86);
    assert_eq!(d.frames(0x101, &[], &[Some(0x2012)], &prog)[0].base, None);
    d.funcs[0].frame_reg = None;
    assert_eq!(d.frames(0x101, &[], &[Some(0x2012)], &prog)[0].base, None);
}

#[test]
fn a_stack_variable_has_no_fixed_address_to_show() {
    let d = DebugInfo::from_elf(&std::fs::read(format!("{DATA}/xc8_stack_q10.elf")).unwrap()).unwrap();
    let mem = vec![0x55u8; 0x1000];
    assert_eq!(var(&d, "rec", "n").show(&mem), "(no frame here)");
    assert_eq!(var(&d, "rec", "n").show_with(&mem, &[], &d), "(no frame here)");
    assert_eq!(var(&d, "rec", "n").place(Some(0x80)), Some(0x78));
}

/// XC8's `high_pc` is a routine's last address, so `from_elf` adds one: in every XC8 fixture each routine has a range and
/// no two overlap.
#[test]
fn routine_ranges_of_every_xc8_fixture_do_not_overlap() {
    let mut seen = 0;
    for e in std::fs::read_dir(DATA).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("xc8_") && name.ends_with(".elf")) {
            continue;
        }
        let d = DebugInfo::from_elf(&std::fs::read(e.path()).unwrap()).unwrap();
        // XC8 writes a one-instruction routine with high_pc = low_pc; it still has a (one-instruction) range.
        assert!(d.funcs.iter().all(|f| f.high > f.low), "{name}: an empty routine range");
        let mut r: Vec<(u32, u32, &str)> = d.funcs.iter().map(|f| (f.low, f.high, f.name.as_str())).collect();
        r.sort();
        for w in r.windows(2) {
            assert!(w[0].1 <= w[1].0, "{name}: {} {:#x}..{:#x} overlaps {} at {:#x}", w[0].2, w[0].0, w[0].1, w[1].2, w[1].0);
        }
        seen += 1;
    }
    assert!(seen >= 20, "only {seen} XC8 fixtures");
}

/// `n m l` of `inner` in the tail-call fixture, in each live call of it, innermost first.
fn inner_calls(s: &Sim) -> Vec<String> {
    let (d, data) = (s.d(), s.data());
    let inner = fi(d, "inner");
    s.frames().iter().filter(|f| f.func == Some(inner)).map(|f| ["n", "m", "l"].map(|n| var(d, "inner", n).show_framed(&data, &[], d, f.base)).join(" | ")).collect()
}

fn call_names(s: &Sim) -> Vec<String> {
    s.frames().iter().map(|f| f.func.map_or("?".into(), |i| s.d().funcs[i].name.clone())).collect()
}

#[test]
fn a_routine_entered_by_a_tail_jump_has_its_own_frame() {
    for tag in ["tail_q10", "tail_p16"] {
        let mut s = Sim::new(tag);
        // hop's last instruction is the jump (GOTO on the PIC18, LJMP = MOVLP + GOTO on the PIC16), not CALL; RETURN.
        let hop = fi(s.d(), "hop");
        s.run_to("hop");
        while s.d().func_at(s.pc()) == Some(hop) {
            s.step();
        }
        assert_eq!(s.d().func_at(s.pc()), Some(fi(s.d(), "inner")), "{tag}: hop went on into inner");
        assert_eq!(call_names(&s), ["inner", "main"], "{tag}: hop has no pending return");
        let (d, data) = (s.d(), s.data());
        let base = s.frames()[0].base;
        let at_entry = ["n", "m"].map(|n| var(d, "inner", n).show_framed(&data, &[], d, base));
        assert_eq!(at_entry, ["3 (0x03)", "165 (0xA5)"], "{tag}: at inner's first instruction");
        s.run_to("mark");
        assert_eq!(call_names(&s), ["mark", "inner", "main"], "{tag}");
        let want = ["3 (0x03) | 165 (0xA5) | {3 (0x03), 165 (0xA5)}"];
        assert_eq!(inner_calls(&s), want, "{tag}: in mark");
        // Back out of mark and through inner's epilogue and RETURN to main.
        let main = fi(s.d(), "main");
        while s.d().func_at(s.pc()) != Some(main) {
            let got = inner_calls(&s);
            assert_eq!(got, want[..got.len()], "{tag}: at {:#x}", s.pc());
            s.step();
        }
        // The second hop(4) reuses the same stack bytes.
        s.run_to("mark");
        assert_eq!(inner_calls(&s), ["4 (0x04) | 165 (0xA5) | {4 (0x04), 165 (0xA5)}"], "{tag}: the second call");
    }
}
