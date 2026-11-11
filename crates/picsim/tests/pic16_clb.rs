//! The CLB of the PIC16F132xx (DS40002686B ch.29 p.359-383) and PIC16F131xx (DS40002519D ch.29): `periph/clb.rs`.
//! The netlist comes from the `[clb]` section of the stimulus file (the Synthesizer's bitstream is not published).
use devdesc::Device;
use picsim::machine16::Machine16;
use std::path::Path;

const NOP: u16 = 0;

fn movlw(k: u16) -> u16 { 0x3000 | k }
fn movwf(f: u16) -> u16 { 0x0080 | f }
fn movlb(k: u16) -> u16 { 0x0140 | k }

fn hexrec(b: &[u8]) -> String {
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

struct Part {
    dev: Device,
}

impl Part {
    fn new(part: &str) -> Part {
        Part { dev: Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap() }
    }

    fn a(&self, name: &str) -> usize {
        self.dev.find_sfr(name).unwrap_or_else(|| panic!("no {name}")).addr as usize
    }

    /// Load each (register, value).
    fn set(&self, regs: &[(&str, u16)]) -> Vec<u16> {
        let mut p = Vec::new();
        for &(n, k) in regs {
            let r = self.a(n);
            p.extend([movlb((r >> 7) as u16), movlw(k), movwf((r & 0x7F) as u16)]);
        }
        p
    }

    fn machine(&self, prog: &[u16], stim: &str) -> Machine16 {
        let mut m = Machine16::new(Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{}.toml", &self.dev.device.name[3..]))).unwrap()).unwrap();
        let mut text = String::new();
        for (i, &w) in prog.iter().enumerate() {
            for (a, b) in [(2 * i as u32, w as u8), (2 * i as u32 + 1, (w >> 8) as u8)] {
                text += &hexrec(&[2, 0, 0, 4, (a >> 24) as u8, (a >> 16) as u8]);
                text += &hexrec(&[1, (a >> 8) as u8, a as u8, 0, b]);
            }
        }
        m.load_hex(&(text + ":00000001FF\n")).unwrap();
        m.load_stimulus(stim).unwrap();
        m
    }
}

fn run(m: &mut Machine16, n: u64) {
    let end = m.cpu.cycles + n;
    while m.cpu.cycles < end {
        m.cpu.step();
    }
}

/// Run `n` instructions of the program, then NOPs forever.
fn pad(mut p: Vec<u16>) -> Vec<u16> {
    p.extend([NOP; 3000]);
    p
}

#[test]
fn software_inputs_are_locked_while_busy_and_reach_the_outputs_after_one_ble_clk() {
    // CLBSWIN write sequence and BUSY (DS40002686B 29.4.2 p.366-367); a BLE whose LUT input A is CLBSWIN[0], read in CLBSWOUTL
    let p = Part::new("16F13245");
    let stim = "[clb]\ndivider = 64\n[[clb.ble]]\nn = 0\na = 12\nlut = 0xAAAA\n";
    let mut prog = p.set(&[("CLBCLK", 5), ("CLBCON", 0x80), ("CLBSWINU", 0), ("CLBSWINH", 0), ("CLBSWINM", 0), ("CLBSWINL", 1)]);
    let n = prog.len();
    prog.extend(p.set(&[("CLBSWINU", 0xFF)])); // ignored: locked
    let n2 = prog.len();
    let mut m = p.machine(&pad(prog), stim);
    let (con, swin_u, swout_l) = (p.a("CLBCON"), p.a("CLBSWINU"), p.a("CLBSWOUTL"));
    for _ in 0..n {
        m.cpu.step();
    }
    assert_eq!(m.cpu.mem[con] & 1, 1, "BUSY right after CLBSWINL");
    assert_eq!(m.cpu.mem[swout_l] & 1, 0, "not through yet");
    for _ in n..n2 {
        m.cpu.step();
    }
    assert_eq!(m.cpu.mem[swin_u], 0, "CLBSWINU is locked while BUSY");
    run(&mut m, 40); // BLE_clk = 64 Fosc = 16 cycles
    assert_eq!(m.cpu.mem[con] & 1, 0, "BUSY cleared");
    assert_eq!(m.cpu.mem[swout_l] & 1, 1, "CLBSWIN[0] reached BLE[0]");
}

#[test]
fn a_flop_that_inverts_itself_toggles_every_tick_sets_its_interrupt_flag_and_drives_a_pin() {
    // BLE0: flop, D = not Q. CLB1IF0 on its rising edge (29.7 p.370); CLBPPSOUT0 = RxyPPS 36h (Table 18-2 p.207)
    let p = Part::new("16F13245");
    let stim = "[clb]\ndivider = 32\noutput = [0]\nirq = [0]\n[[clb.ble]]\nn = 0\na = 0\nlut = 0x5555\nflop = true\n";
    let prog = pad([p.set(&[("RA0PPS", 0x36), ("TRISA", 0xFE), ("CLBCLK", 5), ("CLBCON", 0x80)])].concat());
    let mut m = p.machine(&prog, stim);
    let (swout_l, porta, pir7) = (p.a("CLBSWOUTL"), p.a("PORTA"), p.a("PIR7"));
    run(&mut m, 12);
    assert_eq!(m.cpu.mem[pir7] & 1, 0, "no rising edge yet");
    let mut seen = [false; 2];
    let mut edges = 0;
    let mut last = m.cpu.mem[swout_l] & 1;
    for _ in 0..200 {
        m.cpu.step();
        let v = m.cpu.mem[swout_l] & 1;
        seen[usize::from(v)] = true;
        edges += u32::from(v != last);
        last = v;
        assert_eq!(m.cpu.peek(porta as u32) & 1, v, "the pin shows BLE[0]");
    }
    assert!(seen[0] && seen[1], "toggles");
    assert!(edges >= 10, "one toggle per BLE_clk of 8 cycles: {edges}");
    assert_eq!(m.cpu.mem[pir7] & 1, 1, "CLB1IF0");
}

#[test]
fn a_pin_input_with_a_synchronised_edge_detector_makes_a_one_period_pulse() {
    // CLBIN0PPS = RA2; positive edge detector, synchronised (29.4.1 Fig. 29-8 p.366); BLE1 latches the pulse: CLB1IF1
    let p = Part::new("16F13245");
    let stim = "[clb]\ndivider = 16\nirq = [-1, 1]\n[[clb.input]]\nn = 0\nsource = 0\nedge = \"pos\"\nsync = true\n\
                [[clb.ble]]\nn = 1\na = 8\nlut = 0xAAAA\nflop = true\n[[pin]]\nport = \"A\"\nbit = 2\nlevel = 1\nat = 400\n";
    let prog = pad(p.set(&[("ANSELA", 0), ("CLBIN0PPS", 2), ("CLBCLK", 5), ("CLBCON", 0x80)]));
    let mut m = p.machine(&prog, stim);
    let (swout_l, pir7) = (p.a("CLBSWOUTL"), p.a("PIR7"));
    run(&mut m, 390);
    assert_eq!(m.cpu.mem[pir7] & 2, 0, "no edge yet");
    let mut high = 0;
    for _ in 0..60 {
        m.cpu.step();
        high += u32::from(m.cpu.mem[swout_l] >> 1 & 1);
    }
    assert_eq!(m.cpu.mem[pir7] & 2, 2, "CLB1IF1 after the edge");
    assert!(high > 0 && high < 20, "the flop holds the pulse for about one BLE_clk (4 cycles), {high} steps");
}

#[test]
fn the_counter_steps_on_ble_clk_and_its_decoded_value_feeds_a_ble() {
    // Counter[A1] = count equals 3 `[A135]`; the counter runs on BLE_clk (29.3 p.361)
    let p = Part::new("16F13245");
    let stim = "[clb]\ndivider = 8\n[clb.counter]\nenable = true\na1 = 3\n[[clb.ble]]\nn = 2\na = 20\nlut = 0xAAAA\n";
    let prog = pad(p.set(&[("CLBCLK", 5), ("CLBCON", 0x80)]));
    let mut m = p.machine(&prog, stim);
    let swout_l = p.a("CLBSWOUTL");
    run(&mut m, 20);
    let (mut rises, mut last, mut high) = (0, m.cpu.mem[swout_l] >> 2 & 1, 0);
    for _ in 0..64 {
        m.cpu.step();
        let v = m.cpu.mem[swout_l] >> 2 & 1;
        rises += u32::from(v == 1 && last == 0);
        high += u32::from(v);
        last = v;
    }
    // a count period is 8 ticks of 2 cycles: 16 cycles, 4 in 64
    assert!((3..=5).contains(&rises), "rises {rises}");
    assert!(high >= 3 * 2 && high <= 5 * 3, "high for 1 of 8 counts, {high} steps");
}

#[test]
fn a_disabled_block_and_a_block_held_by_clbmd_drive_nothing() {
    let p = Part::new("16F13245");
    let stim = "[clb]\n[[clb.ble]]\nn = 0\nlut = 0xFFFF\n";
    let mut m = p.machine(&pad(p.set(&[("CLBCLK", 5)])), stim);
    run(&mut m, 30);
    assert_eq!(m.cpu.mem[p.a("CLBSWOUTL")], 0, "EN = 0");
    let prog = pad(p.set(&[("CLBCLK", 5), ("CLBCON", 0x80)]));
    let mut m = p.machine(&prog, stim);
    run(&mut m, 30);
    assert_eq!(m.cpu.mem[p.a("CLBSWOUTL")] & 1, 1, "an all-ones LUT drives 1");
    let prog = pad(p.set(&[("PMD4", 0x80), ("CLBCLK", 5), ("CLBCON", 0x80)]));
    let mut m = p.machine(&prog, stim);
    run(&mut m, 30);
    assert_eq!(m.cpu.mem[p.a("CLBSWOUTL")], 0, "CLBMD = 1 holds the block in Reset (29.2 p.361)");
}

#[test]
fn rdhold_freezes_the_software_outputs_and_debug_shows_one_ble() {
    let p = Part::new("16F13245");
    let stim = "[clb]\ndivider = 32\n[[clb.ble]]\nn = 0\na = 0\nlut = 0x5555\nflop = true\n";
    // CLBDEBUG0 = 0 selects BLE[0]: DB0 (CLBCON bit 2) follows it
    let prog = pad(p.set(&[("CLBCLK", 5), ("CLBDEBUG0", 0), ("CLBCON", 0x80)]));
    let mut m = p.machine(&prog, stim);
    let (con, swout_l) = (p.a("CLBCON"), p.a("CLBSWOUTL"));
    run(&mut m, 14);
    let mut db_matches = 0;
    for _ in 0..40 {
        m.cpu.step();
        db_matches += u32::from(m.cpu.mem[con] >> 2 & 1 == m.cpu.mem[swout_l] & 1);
    }
    assert_eq!(db_matches, 40, "DB0 is BLE[0]");
    // freeze
    let mut prog = p.set(&[("CLBCLK", 5), ("CLBCON", 0x80)]);
    prog.extend(p.set(&[("CLBCON", 0x82)]));
    let mut m = p.machine(&pad(prog), stim);
    run(&mut m, 14);
    let v = m.cpu.mem[swout_l];
    run(&mut m, 40);
    assert_eq!(m.cpu.mem[swout_l], v, "RDHOLD = 1: stable");
}

#[test]
fn enabling_the_clb_without_a_netlist_says_so() {
    let p = Part::new("16F13245");
    let mut m = p.machine(&pad(p.set(&[("CLBCON", 0x80)])), "");
    run(&mut m, 10);
    assert!(m.cpu.notes.iter().any(|n| n.contains("no [clb] netlist")), "{:?}", m.cpu.notes);
}

#[test]
fn a_part_without_a_clb_rejects_the_section() {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/16F15244.toml")).unwrap();
    let mut m = Machine16::new(dev).unwrap();
    assert!(m.load_stimulus("[clb]\ndivider = 2\n").is_err());
}

#[test]
fn a_bad_netlist_is_an_error() {
    let p = Part::new("16F13245");
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/16F13245.toml")).unwrap();
    let _ = p;
    for bad in ["[clb]\nbogus = 1\n", "[clb]\n[[clb.ble]]\nn = 32\nlut = 0\n", "[clb]\n[[clb.ble]]\nn = 0\nlut = 70000\n", "[clb]\n[[clb.input]]\nn = 0\nsource = 0\nedge = \"up\"\n"] {
        let mut m = Machine16::new(Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/16F13245.toml")).unwrap()).unwrap();
        assert!(m.load_stimulus(bad).is_err(), "{bad}");
    }
    drop(dev);
}

#[test]
fn pic16f131xx_clb_outputs_use_codes_24h_to_2bh_and_clbppscon_takes_over_the_pin_direction() {
    // DS40002519D Table 18-2 p.187: CLBPPSOUT0 = 24h; OESELn = 1000 selects BLE[3] as the output enable (Table 29-3 p.326)
    let p = Part::new("16F13145");
    let stim = "[clb]\noutput = [0]\n[[clb.ble]]\nn = 0\nlut = 0xFFFF\n[[clb.ble]]\nn = 3\nlut = 0xFFFF\n";
    // RA0 stays an input (TRISA0 = 1): without OESEL it does not drive
    let prog = pad(p.set(&[("RA0PPS", 0x24), ("CLBCLK", 5), ("CLBCON", 0x80)]));
    let mut m = p.machine(&prog, stim);
    run(&mut m, 30);
    assert_eq!(m.cpu.peek(p.a("PORTA") as u32) & 1, 0, "TRISA0 = 1: the pin is an input");
    let prog = pad(p.set(&[("RA0PPS", 0x24), ("CLBPPSCON1", 0x08), ("CLBCLK", 5), ("CLBCON", 0x80)]));
    let mut m = p.machine(&prog, stim);
    run(&mut m, 30);
    assert_eq!(m.cpu.peek(p.a("PORTA") as u32) & 1, 1, "BLE[3] high makes the pin an output driving BLE[0]");
    assert!(!m.cpu.notes.iter().any(|n| n.contains("RxyPPS output source")), "{:?}", m.cpu.notes);
}

#[test]
fn pic16f132xx_trisxypps_hands_the_direction_to_the_clb() {
    // Register 18.9.3 p.213: TRISxnPPS = 0001 selects CLBPPS_OE[0] (BLE[3], `[A137]`)
    let p = Part::new("16F13245");
    let stim = "[clb]\noutput = [0]\n[[clb.ble]]\nn = 0\nlut = 0xFFFF\n[[clb.ble]]\nn = 3\nlut = 0xFFFF\n";
    let prog = pad(p.set(&[("RC0PPS", 0x36), ("TRISC0PPS", 1), ("CLBCLK", 5), ("CLBCON", 0x80)]));
    let mut m = p.machine(&prog, stim);
    run(&mut m, 30);
    assert_eq!(m.cpu.peek(p.a("PORTC") as u32) & 1, 1, "TRISC0 = 1 but the CLB owns the direction");
    let prog = pad(p.set(&[("RC0PPS", 0x36), ("CLBCLK", 5), ("CLBCON", 0x80)]));
    let mut m = p.machine(&prog, stim);
    run(&mut m, 30);
    assert_eq!(m.cpu.peek(p.a("PORTC") as u32) & 1, 0, "without TRISC0PPS the input pin does not drive");
}

#[test]
fn a_quiet_clb_still_sees_a_pin_the_firmware_changes_and_a_clb_output_looped_to_its_own_input() {
    // The CLB settles (nothing changes) and then the program drives CLBIN0's pin (RA1) from LATA: BLE[0] = the input follows
    let p = Part::new("16F13245");
    let stim = "[clb]\n[[clb.input]]\nn = 0\nsource = 0\n[[clb.ble]]\nn = 0\na = 8\nlut = 0xAAAA\n";
    let mut prog = p.set(&[("ANSELA", 0), ("TRISA", 0xFD), ("CLBIN0PPS", 1), ("CLBCLK", 5), ("CLBCON", 0x80)]);
    prog.extend([NOP; 60]);
    prog.extend(p.set(&[("LATA", 2)]));
    let mut m = p.machine(&pad(prog), stim);
    let swout_l = p.a("CLBSWOUTL");
    run(&mut m, 40);
    assert_eq!(m.cpu.mem[swout_l] & 1, 0);
    run(&mut m, 120);
    assert_eq!(m.cpu.mem[swout_l] & 1, 1, "RA1 went high by firmware");
    // loop-back: BLE[4] (CLBPPSOUT1) drives RA1 and CLBIN0 reads RA1; BLE[4] = not input
    let stim = "[clb]\noutput = [0, 0]\n[[clb.input]]\nn = 0\nsource = 0\n[[clb.ble]]\nn = 4\na = 8\nlut = 0x5555\n";
    let prog = pad(p.set(&[("ANSELA", 0), ("RA1PPS", 0x37), ("TRISA", 0xFD), ("CLBIN0PPS", 1), ("CLBCLK", 5), ("CLBCON", 0x80)]));
    let mut m = p.machine(&prog, stim);
    let mut seen = [false; 2];
    for _ in 0..80 {
        m.cpu.step();
        seen[usize::from(m.cpu.peek(p.a("PORTA") as u32) >> 1 & 1)] = true;
    }
    assert!(seen[0] && seen[1], "an inverter looped through its pin oscillates (the engine sees the pin it drives)");
}

#[test]
fn clearing_en_zeroes_the_outputs_at_once() {
    let p = Part::new("16F13245");
    let stim = "[clb]\n[[clb.ble]]\nn = 0\nlut = 0xFFFF\n";
    let mut prog = p.set(&[("CLBCLK", 5), ("CLBCON", 0x80)]);
    prog.extend([NOP; 20]);
    prog.extend(p.set(&[("CLBCON", 0x00)]));
    let mut m = p.machine(&pad(prog), stim);
    run(&mut m, 22);
    assert_eq!(m.cpu.mem[p.a("CLBSWOUTL")] & 1, 1);
    run(&mut m, 40);
    assert_eq!(m.cpu.mem[p.a("CLBSWOUTL")], 0);
}
