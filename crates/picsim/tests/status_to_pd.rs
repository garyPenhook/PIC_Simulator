//! STATUS TO and PD on the PIC18 core (`[A129]`): read only, 1 after a power-on reset, set by CLRWDT
//! (DS40002214G 7.7.7 p.40 for the PIC18F16Q41, whose pack gives STATUS as `-xxxxxxx`; DS40002043F 11.8.5 p.131 for the PIC18F27Q10).

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn rec(addr: u16, ty: u8, data: &[u8]) -> String {
    let mut b = vec![data.len() as u8, (addr >> 8) as u8, addr as u8, ty];
    b.extend_from_slice(data);
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

fn machine(part: &str, prog: &[u8]) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_hex(&(rec(0, 0, prog) + &rec(0, 1, &[]))).unwrap();
    m
}

fn status(m: &Machine) -> u8 {
    let a = m.dev.find_sfr("STATUS").unwrap().addr;
    m.cpu.m.mem[a as usize]
}

const PARTS: [&str; 2] = ["18F16Q41", "18F27Q10"];

#[test]
fn to_and_pd_are_set_at_power_up() {
    for part in PARTS {
        let m = machine(part, &[0xFF, 0xD7]);
        assert_eq!(status(&m) & 0x60, 0x60, "{part}");
    }
}

#[test]
fn clrwdt_sets_to_and_pd_and_a_write_does_not_change_them() {
    for part in PARTS {
        // clrwdt; bra self
        let mut m = machine(part, &[0x04, 0x00, 0xFF, 0xD7]);
        let a = m.dev.find_sfr("STATUS").unwrap().addr;
        m.cpu.m.mem[a as usize] &= !0x60; // as a WDT time-out in Sleep leaves them
        m.cpu.step();
        assert_eq!(status(&m) & 0x60, 0x60, "{part}: CLRWDT");
        m.cpu.write(a, 0);
        assert_eq!(status(&m) & 0x60, 0x60, "{part}: TO and PD are read only");
    }
}
