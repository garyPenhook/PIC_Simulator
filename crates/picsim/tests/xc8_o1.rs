//! C programs built with `xc8-cc -g -O1`: XC8 4.00 writes no location lists, so every variable it describes still has a fixed
//! address and reads correctly. A local the compiler folds into a constant (`c` in `main`) has no entry at all, at any level.
//! Fixtures `data/xc8_o1_q10.*` (PIC18F27Q10) and `data/xc8_o1_p16.*` (PIC16F17146) are built from the same program,
//! `data/xc8_o1_TAG.c`: `xc8-cc -mcpu=PART -mdfp=DFP -g -O1 xc8_o1_TAG.c -o xc8_o1_TAG.elf`. Both run to the final loop.

use devdesc::Device;
use picsim::debuginfo::DebugInfo;
use picsim::machine16::Machine16;
use picsim::Machine;
use std::path::Path;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

fn device(part: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap()
}

fn run(tag: &str) -> (DebugInfo, Vec<u8>) {
    let base = format!("{DATA}/xc8_o1_{tag}");
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
fn optimised_build_keeps_fixed_addresses_and_right_values() {
    for tag in ["q10", "p16"] {
        let (d, mem) = run(tag);
        let show = |scope: Option<&str>, n: &str| {
            d.vars
                .iter()
                .find(|v| v.name == n && v.scope.map(|i| d.funcs[i].name.as_str()) == scope)
                .unwrap_or_else(|| panic!("{tag}: {scope:?} {n}"))
                .show(&mem)
        };
        assert_eq!(show(None, "sink"), "10 (0x0A)", "{tag}");
        assert_eq!(show(None, "tab"), "{1 (0x01), 2 (0x02), 3 (0x03), 4 (0x04)}", "{tag}");
        assert_eq!(show(Some("main"), "d"), "10 (0x0A)", "{tag}");
        assert_eq!(show(Some("main"), "e"), "13 (0x0D)", "{tag}");
        assert_eq!(show(Some("main"), "total"), "10 (0x000A)", "{tag}");
        assert_eq!(show(Some("acc"), "s"), "10 (0x000A)", "{tag}");
        assert_eq!(show(Some("acc"), "j"), "4 (0x04)", "{tag}");
        assert!(d.vars.iter().all(|v| v.name != "c"), "{tag}: a folded constant has no entry");
    }
}

#[test]
fn optimised_build_has_routines_and_lines() {
    for tag in ["q10", "p16"] {
        let d = DebugInfo::from_elf(&std::fs::read(format!("{DATA}/xc8_o1_{tag}.elf")).unwrap()).unwrap();
        for n in ["main", "sq", "acc"] {
            let f = d.funcs.iter().find(|f| f.name == n).unwrap_or_else(|| panic!("{tag}: {n}"));
            assert!(f.low < f.high, "{tag}: {n}");
            assert!(d.lines.lookup(f.low).is_some(), "{tag}: {n} has a source line at its entry");
        }
    }
}
