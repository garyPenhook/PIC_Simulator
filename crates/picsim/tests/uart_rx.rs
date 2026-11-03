//! EUSART receiver and the stimulus file (DS40002043F 29.1.2, PIC18F27Q10).

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn machine(stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_stimulus(stim).unwrap();
    m
}

fn addr(m: &Machine, n: &str) -> u32 {
    m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr
}

/// 4 instruction cycles per bit (BRG16 = 1, BRGH = 1, n = 3); returns (RC1STA, RC1IF mask/addr).
fn setup(m: &mut Machine, rcsta: u8) -> (u32, (u32, u8)) {
    let (ra, ta, ba) = (addr(m, "RC1STA"), addr(m, "TX1STA"), addr(m, "BAUD1CON"));
    let (l, h) = (addr(m, "SP1BRGL"), addr(m, "SP1BRGH"));
    m.cpu.write(ba, 8);
    m.cpu.write(ta, 4);
    m.cpu.write(l, 3);
    m.cpu.write(h, 0);
    m.cpu.write(ra, rcsta);
    for s in m.dev.sfr.iter().filter(|s| s.name.starts_with("PIR")) {
        if let Some(f) = s.fields.iter().chain(s.legacy.iter()).find(|f| f.name == "RC1IF") {
            return (ra, (s.addr, 1 << f.pos));
        }
    }
    panic!("no RC1IF")
}

fn run_to(m: &mut Machine, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

#[test]
fn byte_arrives_at_the_middle_of_the_stop_bit() {
    let mut m = machine("[[uart]]\nat = 100\nbytes = \"A\"\n");
    let (ra, (pa, pm)) = setup(&mut m, 0x90);
    run_to(&mut m, 100 + 4 * 19 / 2 - 2);
    assert_eq!(m.cpu.m.mem[pa as usize] & pm, 0, "RCIF early");
    assert_eq!(m.cpu.m.mem[addr(&m, "BAUD1CON") as usize] & 0x40, 0, "RCIDL low while the character comes in");
    run_to(&mut m, 100 + 4 * 19 / 2 + 2);
    assert_ne!(m.cpu.m.mem[pa as usize] & pm, 0, "RCIF");
    assert_eq!(m.cpu.m.mem[addr(&m, "BAUD1CON") as usize] & 0x40, 0x40);
    assert_eq!(m.cpu.read(ra), 0x90);
    assert_eq!(m.cpu.read(addr(&m, "RC1REG")), b'A');
    assert_eq!(m.cpu.m.mem[pa as usize] & pm, 0, "reading RCREG clears RCIF");
}

#[test]
fn third_character_overruns_until_cren_is_cleared() {
    let mut m = machine("[[uart]]\nat = 10\nbytes = \"abc\"\n");
    let (ra, (pa, pm)) = setup(&mut m, 0x90);
    run_to(&mut m, 400);
    assert_eq!(m.cpu.read(ra) & 2, 2, "OERR");
    assert_ne!(m.cpu.m.mem[pa as usize] & pm, 0);
    let r = addr(&m, "RC1REG");
    assert_eq!((m.cpu.read(r), m.cpu.read(r)), (b'a', b'b'));
    assert_eq!(m.cpu.m.mem[pa as usize] & pm, 0);
    assert_eq!(m.cpu.read(ra) & 2, 2, "reading does not clear OERR");
    m.cpu.write(ra, 0x80);
    assert_eq!(m.cpu.read(ra) & 2, 0);
    m.cpu.write(ra, 0x90);
    run_to(&mut m, 500);
    assert_eq!(m.cpu.m.mem[pa as usize] & pm, 0, "the third character is gone");
}

#[test]
fn framing_error_follows_the_top_character() {
    let mut m = machine("[[uart]]\nat = 10\ndata = [0x55]\nbad_stop = true\n[[uart]]\ndata = [0x56]\n");
    let (ra, _) = setup(&mut m, 0x90);
    run_to(&mut m, 200);
    assert_eq!(m.cpu.read(ra) & 4, 4, "FERR on the first");
    let r = addr(&m, "RC1REG");
    assert_eq!(m.cpu.read(r), 0x55);
    assert_eq!(m.cpu.read(ra) & 4, 0, "the second has a good Stop bit");
    assert_eq!(m.cpu.read(r), 0x56);
}

#[test]
fn character_started_while_the_receiver_is_off_is_lost() {
    let mut m = machine("[[uart]]\nat = 0\nbytes = \"x\"\n");
    let (ra, (pa, pm)) = setup(&mut m, 0x80);
    run_to(&mut m, 20);
    m.cpu.write(ra, 0x90);
    run_to(&mut m, 200);
    assert_eq!(m.cpu.m.mem[pa as usize] & pm, 0);
}

#[test]
fn nine_bit_address_detect() {
    let mut m = machine("[[uart]]\nat = 10\nnine = true\ndata = [0x041, 0x142]\n");
    let (ra, _) = setup(&mut m, 0xD8); // SPEN RX9 CREN ADDEN
    run_to(&mut m, 300);
    assert_eq!(m.cpu.read(ra) & 1, 1, "RX9D of the address byte");
    assert_eq!(m.cpu.read(addr(&m, "RC1REG")), 0x42, "the data byte (ninth bit 0) was ignored");
}

#[test]
fn stimulus_errors() {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    for bad in ["[[uart]]\nat = 1\n", "[[uart]]\nbytes = \"a\"\ncolour = 1\n", "[[uart]]\ndata = [256]\n", "[adc]\n", "[[uart]]\nport = 9\nbytes = \"a\"\n"] {
        assert!(m.load_stimulus(bad).is_err(), "{bad}");
    }
}

#[test]
fn stimulus_survives_reset() {
    let mut m = machine("[[uart]]\nat = 100\nbytes = \"A\"\n");
    m.reset();
    let (_, (pa, pm)) = setup(&mut m, 0x90);
    run_to(&mut m, 100 + 4 * 19 / 2 + 2);
    assert_ne!(m.cpu.m.mem[pa as usize] & pm, 0, "RCIF after reset");
    assert_eq!(m.cpu.read(addr(&m, "RC1REG")), b'A');
}

#[test]
fn huge_timing_values_are_rejected() {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    for k in ["at", "bit", "gap"] {
        assert!(m.load_stimulus(&format!("[[uart]]\n{k} = 9000000000\nbytes = \"a\"\n")).is_err(), "{k}");
    }
    assert!(m.load_stimulus("[[uart]]\nat = 4294967296\nbit = 4294967296\ngap = 4294967296\nbytes = \"abc\"\n").is_ok());
}
