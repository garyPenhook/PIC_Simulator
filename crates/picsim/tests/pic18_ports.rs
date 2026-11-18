//! Weak pull-ups and open-drain pin resolution across PIC18 families.
use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn machine(part: &str, stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_stimulus(stim).unwrap();
    m
}
fn w(m: &mut Machine, name: &str, value: u8) {
    m.cpu.write(m.dev.find_sfr(name).unwrap().addr, value);
}
fn r(m: &mut Machine, name: &str) -> u8 {
    m.cpu.read(m.dev.find_sfr(name).unwrap().addr)
}

#[test]
fn weak_pullups_yield_to_external_low_and_are_disabled_for_outputs() {
    for part in ["18F27Q10", "18F16Q41", "18F26K22"] {
        let mut m = machine(part, "[[pin]]\nport = \"B\"\nbit = 4\nlevel = 0\n");
        w(&mut m, "ANSELB", 0);
        if part.ends_with("K22") {
            w(&mut m, "WPUB", 0x30);
            assert_eq!(r(&mut m, "PORTB") & 0x30, 0, "global disable at reset");
            w(&mut m, "INTCON2", 0);
        }
        w(&mut m, "WPUB", 0x30);
        assert_eq!(r(&mut m, "PORTB") & 0x30, 0x20, "{part}: explicit low beats WPU");
        w(&mut m, "TRISB", 0xCF);
        w(&mut m, "LATB", 0);
        assert_eq!(r(&mut m, "PORTB") & 0x30, 0, "{part}: outputs disable WPU");
        w(&mut m, "TRISB", 0xFF);
        w(&mut m, "ANSELB", 0x30);
        assert_eq!(r(&mut m, "PORTB") & 0x30, 0, "{part}: analog buffer disabled");
    }
}

#[test]
fn open_drain_releases_high_and_sinks_low() {
    for part in ["18F27Q10", "18F16Q41"] {
        let mut m = machine(part, "[[pin]]\nport = \"C\"\nbit = 0\nlevel = 1\n");
        w(&mut m, "ANSELC", 0);
        w(&mut m, "TRISC", 0xFC);
        w(&mut m, "WPUC", 3);
        w(&mut m, "ODCONC", 3);
        w(&mut m, "LATC", 3);
        assert_eq!(r(&mut m, "PORTC") & 3, 1, "{part}: released outputs use external drive, not WPU");
        assert_eq!(r(&mut m, "LATC") & 3, 3);
        w(&mut m, "LATC", 0);
        assert_eq!(r(&mut m, "PORTC") & 3, 0, "{part}: sink beats external high");
        w(&mut m, "ODCONC", 0);
        w(&mut m, "LATC", 3);
        assert_eq!(r(&mut m, "PORTC") & 3, 3, "{part}: push pull drives high");
        w(&mut m, "ANSELC", 3);
        assert_eq!(r(&mut m, "PORTC") & 3, 0, "{part}: ANSEL masks output readback");
    }
}

#[test]
fn open_drain_applies_after_pps_source_selection() {
    let mut m = machine("18F27Q10", "[[analog]]\npin = \"RA2\"\nvolts = 2.0\n[[analog]]\npin = \"RA0\"\nvolts = 1.0\n");
    w(&mut m, "ANSELC", 0);
    w(&mut m, "TRISC", 0xFE);
    w(&mut m, "RC0PPS", 0x0D); // CMP1
    w(&mut m, "CM1CON0", 0x80);
    assert_eq!(r(&mut m, "PORTC") & 1, 1, "comparator overrides low LAT");
    w(&mut m, "ODCONC", 1);
    assert_eq!(r(&mut m, "PORTC") & 1, 0, "high peripheral output releases floating pin");
    m.load_stimulus("[[pin]]\nport = \"C\"\nbit = 0\nlevel = 1\n").unwrap();
    assert_eq!(r(&mut m, "PORTC") & 1, 1);
    w(&mut m, "CM1CON0", 0x90); // invert comparator output
    w(&mut m, "LATC", 1);
    assert_eq!(r(&mut m, "PORTC") & 1, 0, "low peripheral output sinks despite high LAT");
}
