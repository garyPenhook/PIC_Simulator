//! Implemented masks of SFRs in the device descriptions, each confirmed against the data sheet (DS40002214G for the Q41 parts, DS40002043F
//! for the Q10, DS40002519D / DS40002195E / DS40002343F for the 16F parts). The descriptions once had masks that dropped real bits (PIR2, PIE2,
//! IPR2 and others as 0; PIR0 without bit 5; PMD0-PMD4 with half their bits), so software writes to them were lost.

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn load(part: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap()
}

fn mask(dev: &Device, n: &str) -> u32 {
    dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).implemented.unwrap_or(0xFF)
}

const Q41: [&str; 6] = ["18F04Q41", "18F05Q41", "18F06Q41", "18F14Q41", "18F15Q41", "18F16Q41"];

#[test]
fn q41_masks() {
    // PIR2/PIE2/IPR2 p.148 (7:4, 0); PIR0/PIE0/IPR0 p.144, 133, 163 (all 8); PMD0 p.293 (bit 2 absent); PMD1/2/4 p.294-297 (all 8);
    // WDTPSL/H p.243; MD1CARH p.513 (3:0); DAC1DATL/DAC2DATL p.765-766; CLCDATA p.349 (3:0).
    let want = [("PIR2", 0xF1), ("PIE2", 0xF1), ("IPR2", 0xF1), ("PIR0", 0xFF), ("PIE0", 0xFF), ("IPR0", 0xFF), ("PMD0", 0xFB), ("PMD1", 0xFF), ("PMD2", 0xFF), ("PMD4", 0xFF), ("WDTPSL", 0xFF), ("WDTPSH", 0xFF), ("MD1CARH", 0x0F), ("DAC1DATL", 0xFF), ("DAC2DATL", 0xFF), ("CLCDATA", 0x0F)];
    for part in Q41 {
        let dev = load(part);
        for (n, m) in want {
            assert_eq!(mask(&dev, n), m, "{part} {n}");
        }
    }
}

#[test]
fn q10_masks() {
    // PMD0 p.77, PMD2 p.80 (6, 5, 2:0), PMD3 p.81, PMD4 p.82 (7:4, 0), PMD5 p.83 (7:4, 0), PIE3 p.210 and IPR3 p.218 (all 8).
    let dev = load("18F27Q10");
    for (n, m) in [("PMD0", 0xFF), ("PMD2", 0x67), ("PMD3", 0xFF), ("PMD4", 0xF1), ("PMD5", 0xF1), ("PIE3", 0xFF), ("IPR3", 0xFF)] {
        assert_eq!(mask(&dev, n), m, "18F27Q10 {n}");
    }
}

#[test]
fn f16_masks() {
    // WDTPSL/H are full read-only bytes. PORTC has RC0-RC5 on the 14-pin parts and RC0-RC7 on the 20-pin ones (pin allocation tables).
    for part in ["16F13113", "16F13123", "16F13143", "16F15223", "16F17126", "16F17146"] {
        let dev = load(part);
        for n in ["WDTPSL", "WDTPSH"] {
            if dev.find_sfr(n).is_some() {
                assert_eq!(mask(&dev, n), 0xFF, "{part} {n}");
            }
        }
    }
    for (part, m) in [("16F13123", 0x3F), ("16F13124", 0x3F), ("16F13125", 0x3F), ("16F15223", 0x3F), ("16F15224", 0x3F), ("16F17126", 0x3F), ("16F13143", 0xFF), ("16F13144", 0xFF), ("16F13145", 0xFF), ("16F15243", 0xFF), ("16F15244", 0xFF), ("16F17146", 0xFF)] {
        assert_eq!(mask(&load(part), "PORTC"), m, "{part} PORTC");
    }
}

#[test]
fn q41_pir2_pie2_ipr2_keep_their_bits() {
    for part in Q41 {
        let mut m = Machine::new(load(part)).unwrap();
        for n in ["PIR2", "PIE2", "IPR2"] {
            let a = m.dev.find_sfr(n).unwrap().addr;
            m.cpu.write(a, 0xFF);
            assert_eq!(m.cpu.read(a), 0xF1, "{part} {n}: only bits 7:4 and 0 exist");
            m.cpu.write(a, 0x00);
            assert_eq!(m.cpu.read(a), 0x00, "{part} {n} clears");
        }
    }
}
