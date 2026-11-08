//! A C program built with `xc8-cc -g -O1` that sets up CCP1 PWM on a PIC16F17146 (`data/xc8_ccp_p16.c`: `xc8-cc -mcpu=16F17146
//! -mdfp=DFP -g -O1 xc8_ccp_p16.c -o xc8_ccp_p16.elf`): Timer2 period 16 clocks, duty 20, CCP1 on RA0 through RA0PPS. The pin is
//! high for 6 cycles in 16 (`[A128]`).

use devdesc::Device;
use picsim::machine16::Machine16;
use std::path::Path;

#[test]
fn xc8_pwm_program_drives_ra0() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/16F17146.toml")).unwrap();
    let porta = dev.find_sfr("PORTA").unwrap().addr;
    let mut m = Machine16::new(dev).unwrap();
    m.load_hex(&std::fs::read_to_string(format!("{dir}/xc8_ccp_p16.hex")).unwrap()).unwrap();
    m.load_elf(Path::new(&format!("{dir}/xc8_ccp_p16.elf"))).unwrap();
    // a step is one or two cycles: give each cycle of it the level after the step
    let mut levels = Vec::new();
    while m.cpu.cycles < 1200 {
        let n = m.cpu.step();
        let hi = m.cpu.read(porta) & 1 != 0;
        levels.extend(std::iter::repeat_n(hi, n as usize));
    }
    let tail = &levels[levels.len() - 640..];
    let high = tail.iter().filter(|&&x| x).count();
    assert!((200..=280).contains(&high), "6 of 16 cycles are high (about 240 of 640), got {high}");
    assert!(tail.windows(2).filter(|w| !w[0] && w[1]).count() >= 35, "about 40 periods");
    assert!(m.debug.vars.iter().any(|v| v.name == "sink"));
}
