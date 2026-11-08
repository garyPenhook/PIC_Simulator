//! C programs built with `xc8-cc -g -O2`, `-Os` and `-O3` (PIC18F27Q10 and PIC16F17146), from `data/xc8_opt.c`:
//! `xc8-cc -mcpu=PART -mdfp=DFP -g -O2|-Os|-O3 xc8_opt.c -o xc8_{o2,os,o3}_{q10,p16}.elf`. Every variable XC8 describes keeps a
//! fixed address, so globals and the locals of `main` read right; a variable the compiler folds away has no entry, and the
//! locals of a routine that is not running share storage with others (overlaid), so they are not checked.
use devdesc::Device;
use picsim::debuginfo::DebugInfo;
use picsim::machine16::Machine16;
use picsim::Machine;
use std::path::Path;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

fn device(part: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap()
}

fn run(level: &str, tag: &str) -> (DebugInfo, Vec<u8>) {
    let base = format!("{DATA}/xc8_{level}_{tag}");
    let hex = std::fs::read_to_string(format!("{base}.hex")).unwrap();
    if tag == "q10" {
        let mut m = Machine::new(device("18F27Q10")).unwrap();
        m.load_hex(&hex).unwrap();
        m.load_elf(Path::new(&format!("{base}.elf"))).unwrap();
        m.run(None, 20_000);
        (m.debug.clone(), (0..0x1000).map(|a| m.cpu.peek(a)).collect())
    } else {
        let mut m = Machine16::new(device("16F17146")).unwrap();
        m.load_hex(&hex).unwrap();
        m.load_elf(Path::new(&format!("{base}.elf"))).unwrap();
        m.run(None, 20_000);
        (m.debug.clone(), (0..0x1000).map(|a| m.cpu.peek(a)).collect())
    }
}

#[test]
fn higher_optimisation_levels_keep_globals_and_main_locals_right() {
    for level in ["o2", "os", "o3"] {
        for tag in ["q10", "p16"] {
            let (d, mem) = run(level, tag);
            let show = |scope: Option<&str>, n: &str| {
                d.vars
                    .iter()
                    .find(|v| v.name == n && v.scope.map(|i| d.funcs[i].name.as_str()) == scope)
                    .unwrap_or_else(|| panic!("{level} {tag}: {scope:?} {n}"))
                    .show(&mem)
            };
            let w = format!("{level} {tag}");
            assert_eq!(show(None, "sink"), "10 (0x0A)", "{w}");
            assert_eq!(show(None, "origin"), "{x = 7 (0x07), y = -300 (0xFED4)}", "{w}");
            assert_eq!(show(None, "tab"), "{1 (0x01), 2 (0x02), 3 (0x03), 4 (0x04)}", "{w}");
            assert_eq!(show(None, "d"), "10 (0x0A)", "{w}");
            assert_eq!(show(None, "e"), "13 (0x0D)", "{w}");
            assert_eq!(show(Some("main"), "local"), "{x = 10 (0x0A), y = -290 (0xFEDE)}", "{w}");
            // `total` is folded away at -Os and -O3 (no entry); where it is described it is right.
            if let Some(v) = d.vars.iter().find(|v| v.name == "total") {
                assert_eq!(v.show(&mem), "10 (0x000A)", "{w}");
            }
            // The compiler overlays the locals of routines that are not running, so only `main`'s and the globals are checked;
            // `c` is folded to a constant at every level.
            assert!(d.vars.iter().all(|v| v.name != "c"), "{w}");
        }
    }
}

#[test]
fn higher_optimisation_levels_have_routines_and_lines() {
    for level in ["o2", "os", "o3"] {
        for tag in ["q10", "p16"] {
            let d = DebugInfo::from_elf(&std::fs::read(format!("{DATA}/xc8_{level}_{tag}.elf")).unwrap()).unwrap();
            for n in ["main", "sq", "acc"] {
                let f = d.funcs.iter().find(|f| f.name == n).unwrap_or_else(|| panic!("{level} {tag}: {n}"));
                assert!(f.low < f.high, "{level} {tag}: {n}");
                assert!(d.lines.lookup(f.low).is_some(), "{level} {tag}: {n} has a line at its entry");
            }
        }
    }
}
