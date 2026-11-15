use devdesc::Device;
use picsim::debugger::{DataChange, Debugger, Signal};
use picsim::{Machine, Stop, machine16::Machine16};
use std::path::Path;

fn device(name: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{name}.toml")))
        .unwrap()
}

fn signal(addr: u32) -> Signal {
    Signal {
        name: "data byte".into(),
        addr,
        bit: None,
    }
}

#[test]
fn pic18_run_stops_on_change_and_can_resume() {
    let mut m = Machine::new(device("18F27Q10")).unwrap();
    // MOVLW 42; MOVWF 20h, access; INCF 20h, f, access; BRA -1
    m.cpu.prog[..8].copy_from_slice(&[0x42, 0x0e, 0x20, 0x6e, 0x20, 0x2a, 0xff, 0xd7]);
    m.debugger.watchpoints.push(0x20);
    m.debugger.signals.push(signal(0x20));
    m.debugger.recording = true;
    assert_eq!(
        m.run(None, 100),
        Stop::Watchpoint(DataChange {
            addr: 0x20,
            before: 0,
            after: 0x42
        })
    );
    assert_eq!(m.cpu.pc, 4);
    assert_eq!(
        m.run(None, 100),
        Stop::Watchpoint(DataChange {
            addr: 0x20,
            before: 0x42,
            after: 0x43
        })
    );
    assert_eq!(m.cpu.pc, 6);
    assert!(m.debugger.samples.last().unwrap().ns > 0);
    assert!(m.debugger.vcd().contains("b01000011 s0"));
}

#[test]
fn pic16_watchpoints_cover_instruction_and_source_steps() {
    let mut m = Machine16::new(device("16F17146")).unwrap();
    // MOVLW 42; MOVWF 20h; INCF 20h, f; BRA -1
    m.cpu.prog[..8].copy_from_slice(&[0x42, 0x30, 0xa0, 0x00, 0xa0, 0x0a, 0xff, 0x33]);
    m.debugger.watchpoints.push(0x20);
    assert_eq!(m.step(), None);
    assert_eq!(
        m.step_line(false, false, 20),
        Some(Stop::Watchpoint(DataChange {
            addr: 0x20,
            before: 0,
            after: 0x42
        }))
    );
    assert_eq!(m.cpu.pc, 2);
    assert_eq!(
        m.run(None, 100),
        Stop::Watchpoint(DataChange {
            addr: 0x20,
            before: 0x42,
            after: 0x43
        })
    );
    assert_eq!(m.cpu.pc, 3);
}

#[test]
fn same_value_writes_do_not_trigger_and_debug_edits_are_the_new_baseline() {
    let mut m = Machine::new(device("18F27Q10")).unwrap();
    m.cpu.prog[..4].copy_from_slice(&[0x20, 0x6e, 0xff, 0xd7]);
    m.cpu.write(m.cpu.m.regs.wreg, 0x42);
    m.cpu.write(0x20, 0x42);
    m.debugger.watchpoints.push(0x20);
    assert_eq!(m.run(None, 8), Stop::Cycles);
}

#[test]
fn reset_clears_capture_but_preserves_debugger_configuration() {
    let mut m = Machine16::new(device("16F17146")).unwrap();
    m.debugger.watchpoints.push(0x20);
    m.debugger.signals.push(signal(0x20));
    m.debugger.recording = true;
    m.debugger.sample(100, |_| 42);
    m.reset().unwrap();
    assert!(m.debugger.samples.is_empty());
    assert_eq!(m.debugger.watchpoints, [0x20]);
    assert_eq!(m.debugger.signals.len(), 1);
    assert!(m.debugger.recording);
}

#[test]
fn vcd_contains_initial_values_bits_transitions_and_relative_time() {
    let mut d = Debugger::default();
    d.signals = vec![
        signal(0x20),
        Signal {
            name: "RA0".into(),
            addr: 0x21,
            bit: Some(0),
        },
    ];
    d.recording = true;
    d.sample(100, |_| 0);
    d.sample(163, |_| 255);
    d.sample(225, |_| 255);
    assert_eq!(
        d.vcd(),
        "$version PicSim $end\n$timescale 1 ns $end\n$scope module picsim $end\n$var wire 8 s0 data_byte_0 $end\n$var wire 1 s1 RA0_1 $end\n$upscope $end\n$enddefinitions $end\n#0\nb00000000 s0\n0s1\n#63\nb11111111 s0\n1s1\n#125\n"
    );
}

#[test]
fn capture_is_bounded_and_unchanged_periods_keep_their_duration() {
    let mut d = Debugger::default();
    d.signals.push(signal(0));
    d.recording = true;
    for ns in 0..1000 {
        d.sample(ns, |_| 0);
    }
    assert_eq!(d.samples.len(), 2);
    assert_eq!(d.samples.last().unwrap().ns, 999);
    d.reset();
    for ns in 0..=Debugger::MAX_SAMPLES as u64 {
        d.sample(ns, |_| (ns & 1) as u8);
    }
    assert_eq!(d.samples.len(), Debugger::MAX_SAMPLES);
    assert!(!d.recording);
    assert!(d.full);
}

#[test]
fn adc_input_and_peripheral_watchpoint_stop_at_conversion_completion() {
    let mut m = Machine::new(device("18F27Q10")).unwrap();
    m.cpu.prog.fill(0); // NOPs
    let addr = |n| m.dev.find_sfr(n).unwrap().addr;
    let (con, result, clk, acq, pch) = (
        addr("ADCON0"),
        addr("ADRESL"),
        addr("ADCLK"),
        addr("ADACQ"),
        addr("ADPCH"),
    );
    assert!(m.cpu.p.adc_feed(13, m.cpu.cycles, 0x2a5));
    m.cpu.write(clk, 3);
    m.cpu.write(acq, 2);
    m.cpu.write(pch, 13);
    m.cpu.write(con, 0x84);
    m.cpu.write(con, 0x85);
    m.debugger.watchpoints.push(result);
    m.debugger.signals.push(signal(result));
    m.debugger.recording = true;
    assert_eq!(
        m.run(None, 100),
        Stop::Watchpoint(DataChange {
            addr: result,
            before: 0,
            after: 0xa5
        })
    );
    assert_eq!(m.cpu.cycles, 27);
    assert_eq!(m.debugger.samples.last().unwrap().values, [0xa5]);
}

#[test]
fn pin_input_is_recorded_by_both_cores() {
    let mut m = Machine::new(device("18F27Q10")).unwrap();
    let ansel = m.dev.find_sfr("ANSELA").unwrap().addr;
    let port = m.dev.find_sfr("PORTA").unwrap().addr;
    m.cpu.write(ansel, 0);
    m.cpu.prog.fill(0);
    m.debugger.signals.push(Signal {
        name: "RA0".into(),
        addr: port,
        bit: Some(0),
    });
    m.debugger.recording = true;
    assert!(m.cpu.p.pin_feed(b'A', 0, 1, 0));
    m.step();
    assert!(m.cpu.p.pin_feed(b'A', m.cpu.cycles, 1, 1));
    m.step();
    assert_eq!(m.debugger.samples.first().unwrap().values, [0]);
    assert_eq!(m.debugger.samples.last().unwrap().values, [1]);

    let mut m = Machine16::new(device("16F17146")).unwrap();
    let ansel = m.dev.find_sfr("ANSELA").unwrap().addr;
    let port = m.dev.find_sfr("PORTA").unwrap().addr;
    m.cpu.write(ansel, 0);
    m.cpu.prog.fill(0);
    m.debugger.signals.push(Signal {
        name: "RA0".into(),
        addr: port,
        bit: Some(0),
    });
    m.debugger.recording = true;
    assert!(m.cpu.pin_feed(b'A', 0, 1, 0));
    m.step();
    assert!(m.cpu.pin_feed(b'A', m.cpu.cycles, 1, 1));
    m.step();
    assert_eq!(m.debugger.samples.first().unwrap().values, [0]);
    assert_eq!(m.debugger.samples.last().unwrap().values, [1]);
}
