//! LEDs and the SSD1306 OLED on the pins of a PIC18F27Q10 (periph/board.rs).

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn machine(stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_stimulus(stim).unwrap();
    m
}

fn w(m: &mut Machine, n: &str, v: u8) {
    let a = m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr;
    m.cpu.write(a, v);
}

fn rd(m: &mut Machine, n: &str) -> u8 {
    let a = m.dev.find_sfr(n).unwrap().addr;
    m.cpu.read(a)
}

fn run_for(m: &mut Machine, n: u64) {
    let c = m.cpu.cycles + n;
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

/// Let the board look at the pins at a later cycle without running code.
fn poll(m: &mut Machine, dt: u64) {
    m.cpu.cycles += dt;
    let now = m.cpu.cycles;
    m.cpu.p.board_poll(&m.cpu.m, now);
}

#[test]
fn led_follows_its_pin_and_polarity() {
    let mut m = machine("[[led]]\npin = \"RB0\"\n[[led]]\npin = \"B1\"\nactive = \"low\"\nname = \"STATUS\"\n");
    poll(&mut m, 1);
    assert!(!m.cpu.p.board.leds[0].on && !m.cpu.p.board.leds[1].on, "pins are inputs: floating, dark");
    w(&mut m, "TRISB", 0xFC);
    poll(&mut m, 1);
    assert!(!m.cpu.p.board.leds[0].on, "LATB0 = 0");
    assert!(m.cpu.p.board.leds[1].on, "active low LED lights with the pin low");
    w(&mut m, "LATB", 0x01);
    poll(&mut m, 10);
    assert!(m.cpu.p.board.leds[0].on);
    w(&mut m, "LATB", 0x03);
    poll(&mut m, 10);
    assert!(!m.cpu.p.board.leds[1].on);
    assert_eq!(m.cpu.p.board.leds[0].changes.len(), 1);
    assert_eq!(m.cpu.p.board.leds[1].name, "STATUS");
}

#[test]
fn led_brightness_is_the_on_time_share() {
    let mut m = machine("[[led]]\npin = \"RA2\"\n");
    w(&mut m, "TRISA", 0xFB);
    poll(&mut m, 1);
    m.cpu.p.board.take_window();
    for _ in 0..10 {
        w(&mut m, "LATA", 0x04);
        poll(&mut m, 25);
        w(&mut m, "LATA", 0x00);
        poll(&mut m, 75);
    }
    let d = m.cpu.p.board.take_window()[0];
    assert!((d - 0.75).abs() < 0.02 || (d - 0.25).abs() < 0.02, "duty {d}");
}

// ---- OLED on the hardware I2C bus ------------------------------------------------------------------------------

const HW: &str = "[[oled]]\nbus = \"i2c1\"\nscl = \"RC3\"\nsda = \"RC4\"\n";

fn hw(stim: &str) -> Machine {
    let mut m = machine(stim);
    w(&mut m, "SSP1ADD", 9);
    w(&mut m, "SSP1CON1", 0x28);
    m
}

fn wire(m: &mut Machine, scl: u8, sda: u8) {
    w(m, "SSP1CLKPPS", scl);
    w(m, "SSP1DATPPS", sda);
    w(m, "RC3PPS", 0x0F);
    w(m, "RC4PPS", 0x10);
}

fn hw_send(m: &mut Machine, bytes: &[u8]) {
    w(m, "SSP1CON2", 1);
    run_for(m, 10);
    for &b in bytes {
        w(m, "SSP1BUF", b);
        run_for(m, 190);
    }
    w(m, "SSP1CON2", 4);
    run_for(m, 15);
}

#[test]
fn hardware_i2c_oled_shows_what_the_code_writes() {
    let mut m = hw(HW);
    wire(&mut m, 0x13, 0x14); // RC3, RC4
    run_for(&mut m, 5);
    hw_send(&mut m, &[0x78, 0x00, 0x8D, 0x14, 0xAF]); // charge pump on, display on
    hw_send(&mut m, &[0x78, 0x40, 0xFF, 0x01]);
    let o = &m.cpu.p.board.oleds[0];
    assert!(o.on, "status {:?}", o.status);
    assert!((0..8).all(|y| o.pixel(0, y)), "column 0 page 0 all set");
    assert!(o.pixel(1, 0) && !o.pixel(1, 1), "second byte 0x01");
    assert!(!o.pixel(2, 0));
    assert_eq!(o.status, "");
    assert!(o.ascii().contains('█'));
}

#[test]
fn hardware_i2c_oled_ignores_a_bus_wired_to_other_pins() {
    let mut m = hw(HW);
    wire(&mut m, 0x13, 0x15); // SDA input on RC5: not where the OLED is
    run_for(&mut m, 5);
    hw_send(&mut m, &[0x78, 0x00, 0xAF]);
    let o = &m.cpu.p.board.oleds[0];
    assert!(!o.on);
    assert!(o.status.contains("SSP1DATPPS selects RC5"), "{}", o.status);
    assert!(m.cpu.p.i2c_log.iter().any(|e| matches!(e.2, picsim::periph::I2cEv::Addr(0x78, false))), "address not acknowledged");
    // The output side must match too.
    let mut m = hw(HW);
    wire(&mut m, 0x13, 0x14);
    w(&mut m, "RC4PPS", 0x00);
    run_for(&mut m, 5);
    assert!(m.cpu.p.board.oleds[0].status.contains("RC4PPS"), "{}", m.cpu.p.board.oleds[0].status);
}

// ---- OLED on bit-banged pins -----------------------------------------------------------------------------------

const BB: &str = "[[oled]]\nbus = \"gpio\"\nscl = \"RC0\"\nsda = \"RC1\"\naddress = 0x3C\nheight = 32\n";

struct Bb(Machine);

impl Bb {
    fn set(&mut self, pin: u8, high: bool) {
        let mut t = rd(&mut self.0, "TRISC");
        if high { t |= 1 << pin } else { t &= !(1 << pin) }
        w(&mut self.0, "TRISC", t);
        w(&mut self.0, "LATC", 0);
        poll(&mut self.0, 2);
    }
    fn start(&mut self) {
        self.set(1, true);
        self.set(0, true);
        self.set(1, false);
        self.set(0, false);
    }
    fn stop(&mut self) {
        self.set(1, false);
        self.set(0, true);
        self.set(1, true);
    }
    fn byte(&mut self, b: u8) -> bool {
        for i in (0..8).rev() {
            self.set(1, b >> i & 1 != 0);
            self.set(0, true);
            self.set(0, false);
        }
        self.set(1, true); // release for the acknowledge
        self.set(0, true);
        let ack = rd(&mut self.0, "PORTC") & 2 == 0;
        self.set(0, false);
        ack
    }
}

#[test]
fn bit_banged_oled_acknowledges_and_draws() {
    let mut b = Bb(machine(BB));
    b.start();
    assert!(b.byte(0x78), "address acknowledged");
    assert!(b.byte(0x00));
    assert!(b.byte(0x8D));
    assert!(b.byte(0x14)); // charge pump on
    assert!(b.byte(0xAF));
    assert!(b.byte(0xA1)); // segment remap
    b.stop();
    b.start();
    assert!(b.byte(0x78));
    assert!(b.byte(0x40));
    assert!(b.byte(0x80));
    b.stop();
    let o = &b.0.cpu.p.board.oleds[0];
    assert!(o.on);
    assert!(o.pixel(127, 7) && !o.pixel(127, 6) && !o.pixel(0, 7), "remapped: RAM column 0 shows at x = 127");
    assert_eq!(o.ascii().lines().count(), 16, "32 rows, two per line");
}

#[test]
fn bit_banged_oled_does_not_answer_another_address() {
    let mut b = Bb(machine(BB));
    b.start();
    assert!(!b.byte(0x7A), "address 0x3D is nobody's");
    b.stop();
    b.start();
    assert!(!b.0.cpu.p.board.oleds[0].on);
    assert!(b.byte(0x78));
}

#[test]
fn released_bus_pins_read_high_with_the_pull_ups() {
    let mut m = machine(BB);
    w(&mut m, "TRISC", 0xFF);
    assert_eq!(rd(&mut m, "PORTC") & 3, 3);
    let mut m = machine("");
    w(&mut m, "TRISC", 0xFF);
    assert_eq!(rd(&mut m, "PORTC") & 3, 0, "no OLED: inputs read 0 until driven");
}

#[test]
fn bad_board_entries_are_errors() {
    let dev = || Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    for bad in ["[[led]]\n", "[[led]]\npin = \"RZ9\"\n", "[[led]]\npin = \"RB0\"\nactive = \"mid\"\n", "[[oled]]\nbus = \"spi\"\nscl = \"RC0\"\nsda = \"RC1\"\n", "[[oled]]\nbus = \"gpio\"\nscl = \"RC0\"\n", "[[oled]]\nbus = \"gpio\"\nscl = \"RC0\"\nsda = \"RC1\"\nheight = 48\n", "[[led]]\npin = \"RH0\"\n"] {
        let mut m = Machine::new(dev()).unwrap();
        assert!(m.load_stimulus(bad).is_err(), "{bad}");
    }
}

#[test]
fn board_survives_a_reset_with_a_fresh_state() {
    let mut b = Bb(machine(BB));
    b.start();
    b.byte(0x78);
    b.byte(0x00);
    b.byte(0xAF);
    b.stop();
    assert!(b.0.cpu.p.board.oleds[0].on);
    b.0.reset();
    assert_eq!(b.0.cpu.p.board.oleds.len(), 1);
    assert!(!b.0.cpu.p.board.oleds[0].on);
}

#[test]
fn bit_banged_read_is_acknowledged_but_returns_ff_with_a_note() {
    let mut b = Bb(machine(BB));
    b.start();
    assert!(b.byte(0x79), "read address acknowledged (8.1.5 a)");
    b.set(1, true); // the host releases SDA
    let mut v = 0;
    for _ in 0..8 {
        b.set(0, true);
        v = v << 1 | (rd(&mut b.0, "PORTC") >> 1 & 1);
        b.set(0, false);
    }
    b.set(1, true); // NACK: end of the read
    b.set(0, true);
    b.set(0, false);
    b.stop();
    assert_eq!(v, 0xFF, "no data read is provided in serial mode: SDA stays high");
    assert!(b.0.cpu.notes.iter().chain(b.0.cpu.p.notes.iter()).any(|n| n.contains("SSD1306 read")), "{:?}", b.0.cpu.notes);
}

#[test]
fn hardware_i2c_read_returns_ff() {
    let mut m = hw(HW);
    wire(&mut m, 0x13, 0x14);
    run_for(&mut m, 5);
    w(&mut m, "SSP1CON2", 1);
    run_for(&mut m, 10);
    w(&mut m, "SSP1BUF", 0x79);
    run_for(&mut m, 190);
    w(&mut m, "SSP1CON2", 0x08); // RCEN
    run_for(&mut m, 120);
    assert_eq!(rd(&mut m, "SSP1BUF"), 0xFF);
}

#[test]
fn display_stays_dark_without_the_charge_pump_unless_the_module_has_its_own_vcc() {
    for (stim, lit) in [(HW.to_string(), false), (format!("{HW}external_vcc = true\n"), true)] {
        let mut m = hw(&stim);
        wire(&mut m, 0x13, 0x14);
        run_for(&mut m, 5);
        hw_send(&mut m, &[0x78, 0x00, 0xAF]);
        hw_send(&mut m, &[0x78, 0x40, 0xFF]);
        let o = &m.cpu.p.board.oleds[0];
        assert!(o.on);
        assert_eq!(o.pixel(0, 0), lit);
        assert_eq!(m.cpu.notes.iter().any(|n| n.contains("charge pump")), !lit, "{:?}", m.cpu.notes);
    }
}

#[test]
fn scrolling_runs_on_the_pic_clock_time() {
    // 4 MHz Fosc: one instruction cycle is 1 us. 26h, pages 0..7, C = 111b (2 frames), then 2Fh.
    let mut m = hw(&format!("fosc_mhz = 4\n{HW}external_vcc = true\n"));
    wire(&mut m, 0x13, 0x14);
    run_for(&mut m, 5);
    hw_send(&mut m, &[0x78, 0x00, 0xAF]);
    hw_send(&mut m, &[0x78, 0x40, 0x01]);
    hw_send(&mut m, &[0x78, 0x00, 0x26, 0, 0, 7, 7, 0, 0xFF, 0x2F]);
    assert!(m.cpu.p.board.oleds[0].is_scrolling());
    assert!(m.cpu.p.board.oleds[0].pixel(0, 0));
    // A step is 2 frames of 54 x 64 clocks at 370 kHz = 18.7 ms = 18 683 cycles.
    poll(&mut m, 18_000);
    assert!(m.cpu.p.board.oleds[0].pixel(0, 0), "not yet");
    poll(&mut m, 1_000);
    let o = &m.cpu.p.board.oleds[0];
    assert!(!o.pixel(0, 0) && o.pixel(1, 0), "one column to the right");
    // The same time at 64 MHz is 16 times fewer ms: far more steps.
    m.cpu.p.board.fosc_mhz = 64.0;
    poll(&mut m, 18_683);
    let o = &m.cpu.p.board.oleds[0];
    assert!(o.pixel(2, 0) || o.pixel(1, 0), "{}", o.ascii());
}
