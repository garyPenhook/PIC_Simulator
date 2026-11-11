//! An FSR operand that points at another INDF reads 0 and ignores writes; the core's internal "no target" value must not
//! show up as a stray address (review finding B7).

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn rec(addr: u16, ty: u8, data: &[u8]) -> String {
    let mut b = vec![data.len() as u8, (addr >> 8) as u8, addr as u8, ty];
    b.extend_from_slice(data);
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

#[test]
fn indf_through_indf_is_not_a_stray_address() {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let indf1 = dev.find_sfr("INDF1").unwrap().addr;
    let indf0 = dev.find_sfr("INDF0").unwrap().addr;
    let mut m = Machine::new(dev).unwrap();
    // LFSR 0, INDF1; INCF INDF0, f, ACCESS; BRA self
    let lfsr = [0xEE00 | (indf1 >> 8) as u16 & 0x3F, 0xF000 | (indf1 & 0xFF) as u16, 0x2A00 | (indf0 & 0xFF) as u16, 0xD7FF];
    let bytes: Vec<u8> = lfsr.iter().flat_map(|w| w.to_le_bytes()).collect();
    m.load_hex(&(rec(0, 0, &bytes) + &rec(0, 1, &[]))).unwrap();
    for _ in 0..4 {
        m.cpu.step();
    }
    assert!(!m.cpu.m.stray.contains(&u32::MAX), "stray: {:X?}", m.cpu.m.stray);
}
