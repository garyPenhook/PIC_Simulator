//! The Pins view's source: direction, level and analog selection of every pin, PIC18 and PIC16.
use devdesc::Device;
use picsim::machine16::Machine16;
use picsim::periph::PinInfo;
use picsim::Machine;
use std::path::Path;

fn dev(part: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap()
}

fn pin(v: &[PinInfo], letter: u8, bit: u8) -> PinInfo {
    *v.iter().find(|p| p.pin == (letter, bit)).unwrap_or_else(|| panic!("no R{}{bit}", letter as char))
}

#[test]
fn pic18_pin_direction_level_and_analog_follow_the_registers() {
    for part in ["18F27Q10", "18F16Q41", "18F26K22"] {
        let mut m = Machine::new(dev(part)).unwrap();
        let w = |m: &mut Machine, n: &str, v: u8| m.cpu.write(m.dev.find_sfr(n).unwrap().addr, v);
        w(&mut m, "ANSELC", 0);
        w(&mut m, "TRISC", 0xFE); // RC0 output
        w(&mut m, "LATC", 0x01);
        let v = m.cpu.p.pin_overview(&m.cpu.m, m.cpu.cycles);
        let (b0, b1) = (pin(&v, b'C', 0), pin(&v, b'C', 1));
        assert!(b0.output && b0.high && !b0.analog, "{part}: RC0 drives high");
        assert!(!b1.output && !b1.high, "{part}: RC1 is a floating input, read low");
        w(&mut m, "LATC", 0);
        assert!(!pin(&m.cpu.p.pin_overview(&m.cpu.m, m.cpu.cycles), b'C', 0).high, "{part}: RC0 drives low");
        w(&mut m, "ANSELC", 0x04); // RC1 has no analog function on the K22
        assert!(pin(&m.cpu.p.pin_overview(&m.cpu.m, m.cpu.cycles), b'C', 2).analog, "{part}: ANSELC selects analog");
        // The overview and the pins list agree on which pins exist.
        assert_eq!(m.cpu.p.pin_overview(&m.cpu.m, m.cpu.cycles).iter().map(|p| p.pin).collect::<Vec<_>>(), m.cpu.p.pins(&m.cpu.m), "{part}");
    }
}

#[test]
fn pic16_pin_direction_level_and_analog_follow_the_registers() {
    for part in ["16F17146", "16F18346"] {
        let mut m = Machine16::new(dev(part)).unwrap();
        let w = |m: &mut Machine16, n: &str, v: u8| m.cpu.write(m.dev.find_sfr(n).unwrap().addr, v);
        w(&mut m, "ANSELC", 0);
        w(&mut m, "TRISC", 0xFE);
        w(&mut m, "LATC", 0x01);
        let v = m.cpu.pin_overview();
        let (c0, c1) = (pin(&v, b'C', 0), pin(&v, b'C', 1));
        assert!(c0.output && c0.high && !c0.analog, "{part}: RC0 drives high");
        assert!(!c1.output && !c1.high, "{part}: RC1 is an input");
        w(&mut m, "ANSELC", 0x02);
        assert!(pin(&m.cpu.pin_overview(), b'C', 1).analog, "{part}");
        assert_eq!(m.cpu.pin_overview().iter().map(|p| p.pin).collect::<Vec<_>>(), m.cpu.pins(), "{part}");
    }
}

#[test]
fn a_stimulus_drive_shows_on_an_input_pin() {
    let mut m = Machine::new(dev("18F27Q10")).unwrap();
    m.load_stimulus("[[pin]]\nport = \"B\"\nbit = 4\nlevel = 1\n").unwrap();
    m.cpu.write(m.dev.find_sfr("ANSELB").unwrap().addr, 0);
    let p = pin(&m.cpu.p.pin_overview(&m.cpu.m, m.cpu.cycles), b'B', 4);
    assert!(!p.output && p.high);
}
