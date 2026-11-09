//! A C program of two files: `data/xc8_multi_main.c` and `xc8_multi_util.c`, built for the PIC18F27Q10 and the PIC16F17146
//! with `xc8-cc -mcpu=PART -mdfp=DFP -g -O0 xc8_multi_main.c xc8_multi_util.c -o xc8_multi_TAG.elf`. XC8 writes a compile
//! unit per file. `shared` is defined in util and declared `extern` in main (only the definition is described); each file
//! has its own `static count` and `static helper`; main calls `bump` in util.

use devdesc::Device;
use picsim::debuginfo::DebugInfo;
use picsim::machine16::Machine16;
use picsim::{Machine, Stop};
use std::path::Path;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");
const MAIN: &str = "xc8_multi_main.c";
const UTIL: &str = "xc8_multi_util.c";

fn device(part: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap()
}

/// What a stop shows: the debug information, data memory as variables are read from it, the PC and the call stack.
struct At {
    d: DebugInfo,
    data: Vec<u8>,
    pc: u32,
    calls: Vec<String>,
}

/// Runs the fixture `tag` to the `n`th entry of the routine at `entry(debug)` and returns what it shows there.
fn run(tag: &str, entry: impl Fn(&DebugInfo) -> u32, n: usize) -> At {
    let hex = std::fs::read_to_string(format!("{DATA}/xc8_multi_{tag}.hex")).unwrap();
    let elf = format!("{DATA}/xc8_multi_{tag}.elf");
    let names = |d: &DebugInfo, fr: Vec<picsim::debuginfo::Frame>| fr.iter().map(|f| f.func.map_or("?".into(), |i| d.func_label(i))).collect();
    if tag == "p16" {
        let mut m = Machine16::new(device("16F17146")).unwrap();
        m.load_hex(&hex).unwrap();
        m.load_elf(Path::new(&elf)).unwrap();
        m.until_routine = true;
        let a = entry(&m.debug);
        for i in 0..n {
            if i > 0 {
                m.cpu.step(); // off the entry, so the run goes on to the next one
            }
            assert_eq!(m.run(Some(a), 100_000), Stop::Halt, "{tag}");
        }
        At { calls: names(&m.debug, m.frames()), data: m.debug.data_view(&m.cpu.mem).into_owned(), pc: m.cpu.pc, d: m.debug }
    } else {
        let mut m = Machine::new(device("18F27Q10")).unwrap();
        m.load_hex(&hex).unwrap();
        m.load_elf(Path::new(&elf)).unwrap();
        m.until_routine = true;
        let a = entry(&m.debug);
        for i in 0..n {
            if i > 0 {
                m.cpu.step();
            }
            assert_eq!(m.run(Some(a), 100_000), Stop::Halt, "{tag}");
        }
        At { calls: names(&m.debug, m.frames()), data: m.debug.data_view(&m.cpu.m.mem).into_owned(), pc: m.cpu.pc, d: m.debug }
    }
}

fn routine(d: &DebugInfo, name: &str, file: &str) -> usize {
    d.funcs.iter().position(|f| f.name == name && d.unit_file(f.unit) == file).unwrap_or_else(|| panic!("{name} in {file}"))
}

fn entry_of(name: &'static str, file: &'static str) -> impl Fn(&DebugInfo) -> u32 {
    move |d| d.funcs[routine(d, name, file)].low
}

/// The value of global `name` as C sees it from the routine containing `pc`.
fn global(at: &At, name: &str, pc: Option<u32>) -> String {
    at.d.global(name, pc).unwrap_or_else(|| panic!("global {name}")).show(&at.data)
}

#[test]
fn each_c_file_is_a_unit_with_its_own_statics() {
    for tag in ["q10", "p16"] {
        let d = DebugInfo::from_elf(&std::fs::read(format!("{DATA}/xc8_multi_{tag}.elf")).unwrap()).unwrap();
        // One unit per .c file of the program (not the .s or startup units); the others are XC8's library sources.
        let (lib, mut files): (Vec<&String>, Vec<&String>) = d.units.iter().partition(|u| u.contains("/pic/sources/"));
        files.sort();
        assert_eq!(files.iter().map(|u| u.rsplit('/').next().unwrap()).collect::<Vec<_>>(), [MAIN, UTIL], "{tag}");
        assert!(lib.iter().all(|u| u.ends_with(".c")), "{tag}: {lib:?}");
        // shared: once, external, from its definition in util (main's extern declaration is not described).
        let shared: Vec<_> = d.vars.iter().filter(|v| v.name == "shared").collect();
        assert_eq!(shared.len(), 1, "{tag}");
        assert_eq!((d.unit_file(shared[0].unit), shared[0].file_static), (UTIL, false), "{tag}");
        // count: a static in each file, at different addresses.
        let count: Vec<_> = d.vars.iter().filter(|v| v.name == "count").collect();
        assert_eq!(count.len(), 2, "{tag}");
        assert!(count.iter().all(|v| v.file_static && v.scope.is_none()), "{tag}");
        assert_ne!(count[0].unit, count[1].unit, "{tag}");
        assert_ne!(count[0].addr, count[1].addr, "{tag}");
        // helper: a routine in each file, with ranges of its own; main and bump are in their files.
        let (hm, hu) = (routine(&d, "helper", MAIN), routine(&d, "helper", UTIL));
        assert_ne!(d.funcs[hm].low, d.funcs[hu].low, "{tag}");
        assert_eq!(d.func_at(d.funcs[hm].low), Some(hm), "{tag}");
        assert_eq!(d.func_at(d.funcs[hu].low), Some(hu), "{tag}");
        routine(&d, "main", MAIN);
        routine(&d, "bump", UTIL);
        // bump's parameter and local belong to bump, not to either file's globals.
        let bump = Some(routine(&d, "bump", UTIL));
        assert!(d.vars.iter().filter(|v| v.scope == bump).all(|v| !v.file_static && d.unit_file(v.unit) == UTIL), "{tag}");
        // Labels: a name two files share gets its file; the others stay as they are.
        assert_eq!(d.var_label(count[0]), format!("count ({})", d.unit_file(count[0].unit)));
        assert_eq!(d.var_label(shared[0]), "shared");
        assert_eq!(d.func_label(hm), format!("helper ({MAIN})"));
        assert_eq!(d.func_label(routine(&d, "bump", UTIL)), "bump");
        // A symbol by name outside any routine (basil-sim --until, --bp, --dump) is refused when two files define it.
        let e = d.ambiguous("helper").unwrap_or_default();
        assert!(e.starts_with("helper is a routine in more than one file (") && e.contains(MAIN) && e.contains(UTIL), "{tag}: {e}");
        assert!(d.ambiguous("count").is_some_and(|e| e.starts_with("count is a variable in more than one file")), "{tag}");
        assert_eq!(d.ambiguous("shared"), None);
        assert_eq!(d.ambiguous("bump"), None);
    }
}

#[test]
fn a_static_name_means_the_one_of_the_current_file() {
    for tag in ["q10", "p16"] {
        // stop() called from bump: main's helper has run (its count 0x11 + 1), util's helper too (0x22 + 2).
        let at = run(tag, entry_of("stop", UTIL), 1);
        assert_eq!(at.calls, ["stop", "bump", "main"], "{tag}");
        let d = &at.d;
        let (main_count, util_count) = ("18 (0x12)", "36 (0x24)");
        let by_file = |file: &str| d.vars.iter().find(|v| v.name == "count" && d.unit_file(v.unit) == file).unwrap().show(&at.data);
        assert_eq!((by_file(MAIN).as_str(), by_file(UTIL).as_str()), (main_count, util_count), "{tag}");
        // From a routine of util (stop, bump) `count` is util's; from main it is main's.
        assert_eq!(global(&at, "count", Some(at.pc)), util_count, "{tag}: in stop");
        let main_pc = d.funcs[routine(d, "main", MAIN)].low;
        assert_eq!(global(&at, "count", Some(main_pc)), main_count, "{tag}: in main");
        assert_eq!(d.path_ref_at("count", Some(main_pc)).map(|r| r.0), d.vars.iter().find(|v| v.name == "count" && d.unit_file(v.unit) == MAIN).map(|v| v.addr));
        // shared has one definition, the same from either file; bump's own variables are read as usual.
        assert_eq!(global(&at, "shared", Some(main_pc)), "4660 (0x1234)", "{tag}");
        assert_eq!(global(&at, "shared", Some(at.pc)), "4660 (0x1234)", "{tag}");
        let bump = Some(routine(d, "bump", UTIL));
        let local = |n: &str| d.vars.iter().find(|v| v.scope == bump && v.name == n).unwrap().show(&at.data);
        assert_eq!((local("k").as_str(), local("loc").as_str()), ("3 (0x03)", "37 (0x25)"), "{tag}: loc = 3 + util's count before its helper ran");
        // The Variables tab of stop lists both statics, each named with its file.
        let mut shown: Vec<String> = d.visible(d.func_at(at.pc)).map(|v| d.var_label(v)).collect();
        shown.sort();
        assert_eq!(shown, [format!("count ({MAIN})"), format!("count ({UTIL})"), "shared".to_string()], "{tag}");
    }
}

#[test]
fn a_call_into_the_other_file_has_its_lines_there() {
    for tag in ["q10", "p16"] {
        let at = run(tag, entry_of("bump", UTIL), 1);
        assert_eq!(at.calls, ["bump", "main"], "{tag}");
        let file = |a: u32| at.d.lines.lookup(a).map(|(f, l)| (f.file_name().unwrap().to_string_lossy().into_owned(), l));
        assert_eq!(file(at.pc).map(|f| f.0).as_deref(), Some(UTIL), "{tag}: bump starts on a line of util");
        let main_pc = at.d.funcs[routine(&at.d, "main", MAIN)].low;
        assert_eq!(file(main_pc).map(|f| f.0).as_deref(), Some(MAIN), "{tag}");
        // Each helper is a routine of its own file: the call stack and the lines say which one runs.
        let hm = run(tag, entry_of("helper", MAIN), 1);
        assert_eq!(hm.calls, [format!("helper ({MAIN})"), "main".into()], "{tag}");
        assert_eq!(file(hm.pc).map(|f| f.0).as_deref(), Some(MAIN), "{tag}");
        let hu = run(tag, entry_of("helper", UTIL), 1);
        assert_eq!(hu.calls, [format!("helper ({UTIL})"), "bump".into(), "main".into()], "{tag}");
        assert_eq!(file(hu.pc).map(|f| f.0).as_deref(), Some(UTIL), "{tag}");
    }
}

#[test]
fn the_extern_global_is_updated_across_files() {
    for tag in ["q10", "p16"] {
        // The second stop() is main's own call, after shared += bump(3) (0x1234 + 0x25).
        let at = run(tag, entry_of("stop", UTIL), 2);
        assert_eq!(at.calls, ["stop", "main"], "{tag}");
        assert_eq!(global(&at, "shared", Some(at.pc)), "4697 (0x1259)", "{tag}");
    }
}

#[test]
fn name_at_file_picks_the_copy_of_that_file() {
    for tag in ["q10", "p16"] {
        let at = run(tag, entry_of("stop", UTIL), 1);
        let d = &at.d;
        assert_eq!(d.global(&format!("count@{MAIN}"), Some(at.pc)).unwrap().show(&at.data), "18 (0x12)", "{tag}: from util, main's by name");
        assert_eq!(d.global(&format!("count@{UTIL}"), None).unwrap().show(&at.data), "36 (0x24)", "{tag}");
        assert!(d.global("count@other.c", None).is_none());
        assert_eq!(d.routine(&format!("helper@{MAIN}")), Some(routine(d, "helper", MAIN)), "{tag}");
        assert_eq!(d.routine(&format!("helper@{UTIL}")), Some(routine(d, "helper", UTIL)), "{tag}");
        assert_eq!(d.routine("bump"), Some(routine(d, "bump", UTIL)));
        // A qualified name is not ambiguous; the message for a bare one says how to qualify it.
        assert!(!d.is_ambiguous(&format!("helper@{MAIN}")) && d.is_ambiguous("helper") && !d.is_ambiguous("bump"));
        let e = d.ambiguous("count").unwrap();
        assert!(e.contains(": name one as count@xc8_multi_"), "{tag}: {e}");
        let p = d.path_addr(&format!("count@{MAIN}")).unwrap();
        assert_eq!(p.0, d.global(&format!("count@{MAIN}"), None).unwrap().addr);
    }
}

/// `path_addr` reads data memory: of two globals of one name, a `const` in program memory in one file and a variable in
/// RAM in another, it finds the RAM one.
#[test]
fn path_addr_skips_a_constant_of_the_same_name() {
    use picsim::debuginfo::{Form, Kind, Ty, Var};
    let layout = Ty { bytes: 1, elem: 1, kind: Kind::Unsigned, array: false, name: "char".into(), form: Form::Scalar };
    let var = |addr, unit, rom| Var { name: "tab".into(), addr, bytes: 1, kind: Kind::Unsigned, elem: 1, array: false, ty: "char".into(), scope: None, param: false, rom, layout: layout.clone(), frame: None, unit, file_static: true };
    let d = DebugInfo { units: vec!["a.c".into(), "b.c".into()], vars: vec![var(0x1F00, 0, true), var(0x30, 1, false)], ..DebugInfo::default() };
    assert_eq!(d.path_addr("tab"), Some((0x30, 1)));
    assert_eq!(d.path_ref("tab"), Some((0x1F00, 1, true)), "path_ref takes the first, a constant");
}
