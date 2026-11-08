//! Floating-point, 64-bit and boolean variables built with MPLAB XC8 (`xc8-cc -g -O0`). Fixture `data/xc8_nums_q10.*`
//! (PIC18F27Q10) from `data/xc8_nums_q10.c`: `xc8-cc -mcpu=18F27Q10 -mdfp=DFP -g -O0 xc8_nums_q10.c -o xc8_nums_q10.elf`.

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

#[test]
fn numbers_show_their_values() {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let base = format!("{DATA}/xc8_nums_q10");
    let mut m = Machine::new(dev).unwrap();
    m.load_hex(&std::fs::read_to_string(format!("{base}.hex")).unwrap()).unwrap();
    m.load_elf(Path::new(&format!("{base}.elf"))).unwrap();
    m.run(None, 20_000);
    let mem: Vec<u8> = (0..0x1000).map(|a| m.cpu.peek(a)).collect();
    let show = |n: &str| m.debug.vars.iter().find(|v| v.name == n).unwrap_or_else(|| panic!("{n}")).show(&mem);
    assert_eq!(show("f"), "3.5 (0x40600000)");
    assert_eq!(show("d"), "-2.25 (0xC0100000)", "XC8 4.00 has 32-bit doubles only");
    assert_eq!(show("ll"), "-5000000000 (0xFFFFFFFED5FA0E00)", "a signed 64-bit integer keeps its sign");
    assert_eq!(show("ull"), "1234605616436508552 (0x1122334455667788)");
    assert_eq!(show("flag"), "true");
}
