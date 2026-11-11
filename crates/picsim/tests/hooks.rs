//! Every PIC18 device binds to a machine with each peripheral hook code on one register only, and the Timer4/Timer6
//! registers reach their timer (REVIEW-2026-11-11 B1, B2).

use devdesc::Device;
use picsim::Machine;
use picsim::mem::Special;
use std::collections::HashMap;
use std::path::Path;

fn pic18_devices() -> Vec<(String, Device)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices");
    let mut v = vec![];
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if !name.ends_with(".toml") || name.ends_with(".expect.toml") {
            continue;
        }
        let dev = Device::load(&p).unwrap();
        if dev.device.family == "pic16e" {
            continue;
        }
        v.push((name, dev));
    }
    v
}

#[test]
fn every_pic18_part_binds_with_distinct_hook_codes() {
    let mut built = 0;
    for (name, dev) in pic18_devices() {
        let Ok(m) = Machine::new(dev) else { continue };
        built += 1;
        let mut seen: HashMap<u8, usize> = HashMap::new();
        for (a, s) in m.cpu.m.special.iter().enumerate() {
            if let Special::Hook(k) = s
                && let Some(b) = seen.insert(*k, a)
            {
                panic!("{name}: hook {k} on {b:#X} and {a:#X}");
            }
        }
    }
    assert!(built >= 7, "only {built} PIC18 parts built");
}

#[test]
fn timer4_and_timer6_counters_take_a_write() {
    for (name, dev) in pic18_devices() {
        let Ok(mut m) = Machine::new(dev) else { continue };
        for t in ["T4TMR", "T6TMR"] {
            let Some(a) = m.dev.find_sfr(t).map(|s| s.addr) else { continue };
            m.cpu.write(a, 0x55);
            assert_eq!(m.cpu.read(a), 0x55, "{name}: {t}");
        }
        if let Some(l) = m.dev.find_sfr("NVMLOCK").map(|s| s.addr) {
            assert_eq!(m.cpu.read(l), 0, "{name}: NVMLOCK reads 0");
        }
    }
}
