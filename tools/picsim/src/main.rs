//! `basil-sim prog.hex|prog.elf --device 18F16Q41 [--asm prog.S] [--until __halt] [--dump T_R1:2,T_R2] [--bp SYM,SYM] [--bp-max N] [--config] [--stim FILE]`
//! `--speed max|R` limits the run to a fraction R of real time (`1` = real time, `0.5`, `1/10`...) at `--fosc MHZ` (default 64); the default is `max`.
//! `--bp` prints the cycle count each time one of the symbols is reached (at most `--bp-max`, default 20).

use picsim::pace::{parse_ratio, Pace};
use picsim::periph::I2cEv;
use picsim::{Machine, Stop};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("basil-sim: {e}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let mut args = std::env::args().skip(1);
    let mut trace: Option<(u64, u64)> = None;
    let (mut bps, mut bp_max) = (Vec::<String>::new(), 20usize);
    let mut show_cfg = false;
    let (mut speed, mut fosc) = (0.0f64, 64.0f64);
    let mut stim = None::<PathBuf>;
    let mut until_given = false;
    let (mut hex, mut device, mut asm, mut until, mut dump, mut max) =
        (None::<PathBuf>, String::from("18F27Q10"), None::<PathBuf>, Some(String::from("__halt")), Vec::new(), 2_000_000_000u64);
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../devices");
    while let Some(a) = args.next() {
        let mut val = |n: &str| args.next().ok_or(format!("{n} needs a value"));
        match a.as_str() {
            "--version" | "-V" => {
                println!("basil-sim {}", env!("CARGO_PKG_VERSION"));
                return Ok(ExitCode::SUCCESS);
            }
            "--device" => device = val("--device")?,
            "--devices" => root = PathBuf::from(val("--devices")?),
            "--asm" => asm = Some(PathBuf::from(val("--asm")?)),
            "--until" => {
                until = Some(val("--until")?);
                until_given = true;
            }
            "--dump" => dump = val("--dump")?.split(',').map(String::from).collect(),
            "--trace" => {
                let v = val("--trace")?;
                let (a, b) = v.split_once('-').ok_or("--trace FROM-TO (cycles)")?;
                trace = Some((a.parse().map_err(|_| "bad --trace")?, b.parse().map_err(|_| "bad --trace")?));
            }
            "--config" => show_cfg = true,
            "--speed" => speed = parse_ratio(&val("--speed")?)?,
            "--fosc" => {
                fosc = val("--fosc")?.parse().map_err(|_| "bad --fosc (MHz)")?;
                if !(fosc > 0.0 && fosc <= 64.0) {
                    return Err("--fosc must be in 0..64 MHz".into());
                }
            }
            "--stim" => stim = Some(PathBuf::from(val("--stim")?)),
            "--bp" => bps = val("--bp")?.split(',').map(String::from).collect(),
            "--bp-max" => bp_max = val("--bp-max")?.parse().map_err(|_| "bad --bp-max")?,
            "--max-cycles" => max = val("--max-cycles")?.parse().map_err(|_| "bad --max-cycles")?,
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => hex = Some(PathBuf::from(a)),
        }
    }
    let mut pace = if speed > 0.0 { Some(Pace::new(speed, fosc)) } else { None };
    let hex = hex.ok_or("usage: basil-sim prog.hex --device PART [--asm prog.S] [--until SYM] [--dump A,B:2] [--speed max|R] [--fosc MHZ]")?;
    let dev = devdesc::Device::load(&root.join(format!("{device}.toml"))).map_err(|e| e.to_string())?;
    if dev.device.family == "pic16e" {
        if show_cfg {
            return Err("--config is not supported for PIC16 parts yet".into());
        }
        return run16(dev, &hex, asm, stim, until, until_given, &dump, max, trace, &bps, bp_max, &mut pace);
    }
    let mut m = Machine::new(dev)?;
    let given = hex.clone();
    let hex = picsim::lines::program_hex(&hex)?;
    m.load_hex(&std::fs::read_to_string(&hex).map_err(|e| format!("{}: {e}", hex.display()))?)?;
    if show_cfg {
        let line: Vec<String> = m.cpu.cfg.fields().map(|(k, v)| format!("{k}={v}")).collect();
        println!("config: {}", line.join(" "));
        for n in ["OSCCON1", "OSCCON2", "OSCFRQ", "WDTCON0", "WDTCON1"] {
            if let Some(s) = m.dev.find_sfr(n) {
                println!("  {n} = {:#04x}", m.cpu.m.mem[s.addr as usize]);
            }
        }
    }
    if let Some(f) = &stim {
        m.load_stimulus(&std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?).map_err(|e| format!("{}: {e}", f.display()))?;
    }
    let asm = asm.unwrap_or_else(|| hex.with_extension("S"));
    if let Ok(t) = std::fs::read_to_string(&asm) {
        m.load_symbols_from_asm(&t);
    }
    // Routines, variables and `__halt` from the ELF when there is one (basil -g); the listing's symbols win.
    if let Some(e) = picsim::debuginfo::elf_for(&given) {
        if let Err(e) = m.load_elf(&e) {
            eprintln!("note: {e}");
        }
    }
    let until = default_until(until, until_given, m.syms.contains_key("__halt"));
    let stop_at = match &until {
        Some(s) => Some(sym(&m.syms, &m.debug, s)?.ok_or(format!("symbol {s} not found (no {})", asm.display()))?),
        None => None,
    };
    m.until_routine = until_given && stop_at.is_some_and(|a| m.debug.is_routine_entry(a));
    if let Some((from, to)) = trace {
        m.run(None, from);
        while m.cpu.cycles < to {
            let pc = m.cpu.pc;
            let (w, w2, w3) = (m.cpu.fetch(pc), m.cpu.fetch(pc + 2), m.cpu.fetch(pc + 4));
            let (text, _) = picsim::disasm::disasm(pc, w, w2, w3, true);
            let c0 = m.cpu.cycles;
            m.cpu.step();
            println!("{:>8} {:06X}  {:<28} +{}  TMR0L={} PIR0={:02X} INTCON={:02X}", c0, pc, text, m.cpu.cycles - c0,
                m.peek(m.dev.find_sfr("TMR0L").map_or(0, |x| x.addr), 1)[0],
                m.peek(m.dev.find_sfr("PIR0").map_or(0, |x| x.addr), 1)[0],
                m.peek(m.dev.find_sfr("INTCON").map_or(0, |x| x.addr), 1)[0]);
        }
        return Ok(ExitCode::SUCCESS);
    }
    for b in &bps {
        let a = sym(&m.syms, &m.debug, b)?.ok_or(format!("symbol {b} not found"))?;
        m.breakpoints.push(a);
    }
    check_dump(&m.debug, &dump)?;
    if let Some(p) = pace.as_mut() {
        p.restart(m.cpu.cycles);
    }
    let mut stop = paced_run(&mut pace, max, &mut m, |m| m.cpu.cycles, |m, n| m.run(stop_at, n));
    let mut n = 0;
    while let Stop::Breakpoint(pc) = stop {
        n += 1;
        let name = m.syms.iter().find(|(_, v)| **v == pc).map_or("?", |(k, _)| k.as_str());
        let r = |nm: &str| m.dev.find_sfr(nm).map_or(0, |x| m.cpu.m.mem[x.addr as usize]);
        println!("bp {n} {name} cycles {} tos {:#06X} TMR0L {} TMR0H {}", m.cpu.cycles, m.cpu.tos(), r("TMR0L"), r("TMR0H"));
        if n >= bp_max {
            break;
        }
        let left = max.saturating_sub(m.cpu.cycles);
        stop = paced_run(&mut pace, left, &mut m, |m| m.cpu.cycles, |m, n| m.run(stop_at, n));
    }
    println!("stop: {stop:?}  cycles: {}  instructions: {}", m.cpu.cycles, m.cpu.icount);
    for d in &dump {
        let (name, len) = dump_spec(d)?;
        let addr = sym(&m.syms, &m.debug, name)?.or_else(|| m.dev.find_sfr(name).map(|s| s.addr)).ok_or(format!("unknown symbol {name}"))?;
        let b = m.peek(addr, len);
        let v: u64 = b.iter().rev().fold(0, |a, x| a << 8 | u64::from(*x));
        println!("{name} = {v}  ({})", b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" "));
    }
    for n in 1..=2u8 {
        let bytes: Vec<u8> = m.cpu.p.tx.iter().filter(|t| t.0 == n).map(|t| t.2).collect();
        if !bytes.is_empty() {
            println!("tx{n}: {}", String::from_utf8_lossy(&bytes).escape_debug());
        }
    }
    for n in 1..=2u8 {
        let bytes: Vec<String> = m.cpu.p.spi_tx.iter().filter(|t| t.0 == n).map(|t| format!("{:02X}", t.2)).collect();
        if !bytes.is_empty() {
            println!("spi{n}: {}", bytes.join(" "));
        }
    }
    for n in 1..=2u8 {
        let ev: Vec<String> = m
            .cpu
            .p
            .i2c_log
            .iter()
            .filter(|t| t.0 == n)
            .map(|t| match t.2 {
                I2cEv::Start => "S".to_string(),
                I2cEv::Restart => "Sr".to_string(),
                I2cEv::Stop => "P".to_string(),
                I2cEv::Addr(b, ack) => format!("{:02X}{}{}", b >> 1, if b & 1 != 0 { 'R' } else { 'W' }, if ack { "" } else { "!" }),
                I2cEv::Write(b, ack) | I2cEv::Read(b, ack) => format!("{b:02X}{}", if ack { "" } else { "!" }),
            })
            .collect();
        if !ev.is_empty() {
            println!("i2c{n}: {}", ev.join(" "));
        }
    }
    if !m.cpu.p.pin_log.is_empty() {
        // Peripheral-driven pins (RxyPPS): time in Fosc ticks (4 per instruction cycle) and the new level.
        let ev: Vec<String> = m.cpu.p.pin_log.iter().map(|t| format!("R{}{}={}@{}", t.1 as char, t.2, u8::from(t.3), t.0)).collect();
        println!("pins: {}", ev.join(" "));
    }
    for l in &m.cpu.p.board.leds {
        let ev: Vec<String> = l.changes.iter().take(40).map(|(c, on)| format!("{}@{c}", u8::from(*on))).collect();
        println!(
            "led {} ({}, active {}): {}, on {} of {} cycles, {} changes{}{}",
            l.name,
            picsim::periph::pin_name(l.pin),
            if l.active_high { "high" } else { "low" },
            if l.on { "ON" } else { "off" },
            l.on_cycles,
            m.cpu.cycles,
            l.total_changes,
            if ev.is_empty() { "" } else { ": " },
            ev.join(" ")
        );
    }
    for (i, o) in m.cpu.p.board.oleds.iter().enumerate() {
        println!(
            "oled{} SSD1306 {:#04x} on SCL {} SDA {} ({}): {}, {} data bytes{}",
            i + 1,
            o.address,
            picsim::periph::pin_name(o.scl),
            picsim::periph::pin_name(o.sda),
            match o.bus {
                picsim::periph::Bus::Gpio => "bit-banged".to_string(),
                picsim::periph::Bus::I2c(n) => format!("I2C{n}"),
            },
            if o.on { "display on" } else { "display off" },
            o.writes,
            if o.status.is_empty() { String::new() } else { format!("; NOT WIRED: {}", o.status) }
        );
        if o.on {
            print!("{}", o.ascii());
        }
    }
    for n in &m.cpu.notes {
        eprintln!("note: {n}");
    }
    if !m.cpu.m.unmodelled.is_empty() {
        eprintln!("note: unmodelled SFRs touched: {}", m.cpu.m.unmodelled.iter().cloned().collect::<Vec<_>>().join(" "));
    }
    Ok(if stop == Stop::Halt || until.is_none() { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

/// The PIC16 enhanced mid-range path: the core, Timer0/1/2 and the EUSART (milestones S2 and S4 of docs/SIMULATOR-PLAN.md).
#[allow(clippy::too_many_arguments)]
fn run16(
    dev: devdesc::Device,
    hex: &std::path::Path,
    asm: Option<PathBuf>,
    stim: Option<PathBuf>,
    until: Option<String>,
    until_given: bool,
    dump: &[String],
    max: u64,
    trace: Option<(u64, u64)>,
    bps: &[String],
    bp_max: usize,
    pace: &mut Option<Pace>,
) -> Result<ExitCode, String> {
    let mut m = picsim::machine16::Machine16::new(dev)?;
    let given = hex.to_path_buf();
    let hex = picsim::lines::program_hex(hex)?;
    m.load_hex(&std::fs::read_to_string(&hex).map_err(|e| format!("{}: {e}", hex.display()))?)?;
    if let Some(f) = &stim {
        m.load_stimulus(&std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?).map_err(|e| format!("{}: {e}", f.display()))?;
    }
    let asm = asm.unwrap_or_else(|| hex.with_extension("S"));
    if let Ok(t) = std::fs::read_to_string(&asm) {
        m.load_symbols_from_asm(&t);
    }
    // Routines, variables and `__halt` from the ELF when there is one (basil -g, xc8-cc -g); the listing's symbols win.
    if let Some(e) = picsim::debuginfo::elf_for(&given) {
        if let Err(e) = m.load_elf(&e) {
            eprintln!("note: {e}");
        }
    }
    let until = default_until(until, until_given, m.syms.contains_key("__halt"));
    let stop_at = match &until {
        Some(s) => Some(sym(&m.syms, &m.debug, s)?.ok_or(format!("symbol {s} not found (no {})", asm.display()))?),
        None => None,
    };
    m.until_routine = until_given && stop_at.is_some_and(|a| m.debug.is_routine_entry(a));
    if let Some((from, to)) = trace {
        m.run(None, from);
        while m.cpu.cycles < to {
            let pc = m.cpu.pc;
            let c0 = m.cpu.cycles;
            let text = m.cpu.disasm(m.cpu.fetch(pc));
            m.cpu.step();
            println!("{:>8} {:04X}  {:<28} +{}  W={:02X} STATUS={:02X}", c0, pc, text, m.cpu.cycles - c0, m.cpu.mem[9], m.cpu.mem[3]);
        }
        return Ok(ExitCode::SUCCESS);
    }
    for b in bps {
        m.breakpoints.push(sym(&m.syms, &m.debug, b)?.ok_or(format!("symbol {b} not found"))?);
    }
    check_dump(&m.debug, dump)?;
    if let Some(p) = pace.as_mut() {
        p.restart(m.cpu.cycles);
    }
    let mut stop = paced_run(pace, max, &mut m, |m| m.cpu.cycles, |m, n| m.run(stop_at, n));
    let mut n = 0;
    while let Stop::Breakpoint(pc) = stop {
        n += 1;
        let name = m.syms.iter().find(|(_, v)| **v == pc).map_or("?", |(k, _)| k.as_str());
        println!("bp {n} {name} cycles {} tos {:#06X}", m.cpu.cycles, m.cpu.tos());
        if n >= bp_max {
            break;
        }
        let left = max.saturating_sub(m.cpu.cycles);
        stop = paced_run(pace, left, &mut m, |m| m.cpu.cycles, |m, n| m.run(stop_at, n));
    }
    println!("stop: {stop:?}  cycles: {}  instructions: {}", m.cpu.cycles, m.cpu.icount);
    for d in dump {
        let (name, len) = dump_spec(d)?;
        let addr = sym(&m.syms, &m.debug, name)?.or_else(|| m.dev.find_sfr(name).map(|s| s.addr)).ok_or(format!("unknown symbol {name}"))?;
        let b = m.peek(addr, len);
        let v: u64 = b.iter().rev().fold(0, |a, x| a << 8 | u64::from(*x));
        println!("{name} = {v}  ({})", b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" "));
    }
    for n in 1..=2u8 {
        let bytes: Vec<u8> = m.cpu.tx.iter().filter(|t| t.0 == n).map(|t| t.2).collect();
        if !bytes.is_empty() {
            println!("tx{n}: {}", String::from_utf8_lossy(&bytes).escape_debug());
        }
    }
    for n in &m.cpu.notes {
        eprintln!("note: {n}");
    }
    if !m.cpu.unmodelled.is_empty() {
        eprintln!("note: unmodelled SFRs touched: {}", m.cpu.unmodelled.iter().cloned().collect::<Vec<_>>().join(" "));
    }
    Ok(if stop == Stop::Halt || until.is_none() { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

/// The address of symbol `name`. `name@file.c` is the routine or global of that file the ELF describes; a name that is a
/// routine or global of more than one file (a `static` in each) is an error, rather than one of them picked silently.
fn sym(syms: &std::collections::BTreeMap<String, u32>, d: &picsim::debuginfo::DebugInfo, name: &str) -> Result<Option<u32>, String> {
    if let Some(e) = d.ambiguous(name) {
        return Err(e);
    }
    if picsim::debuginfo::split_file(name).1.is_some() {
        // A data address for --dump (not a constant in program memory, not a frame offset).
        return Ok(d.routine(name).map(|i| d.funcs[i].low).or_else(|| d.global(name, None).filter(|v| !v.rom && v.frame.is_none()).map(|v| v.addr)));
    }
    Ok(syms.get(name).copied())
}

/// Refuses, before the run, a `--dump` name that `sym` would refuse after it.
fn check_dump(d: &picsim::debuginfo::DebugInfo, dump: &[String]) -> Result<(), String> {
    for x in dump {
        dump_spec(x)?;
    }
    dump.iter().filter_map(|x| d.ambiguous(x.split_once(':').map_or(x.as_str(), |p| p.0))).next().map_or(Ok(()), Err)
}

/// Longest `--dump NAME:LEN`: more than any PIC data space, small enough to allocate (C1).
const DUMP_MAX: usize = 65536;

/// A `--dump` entry `NAME` or `NAME:LEN` as (name, length in bytes); a length that is not a number from 1 to `DUMP_MAX` is an error.
fn dump_spec(d: &str) -> Result<(&str, usize), String> {
    let Some((name, l)) = d.split_once(':') else { return Ok((d, 1)) };
    match l.parse::<usize>() {
        Ok(n) if (1..=DUMP_MAX).contains(&n) => Ok((name, n)),
        _ => Err(format!("--dump {d}: the length must be a number from 1 to {DUMP_MAX}")),
    }
}

/// The symbol to stop at. Unless `--until` named one, that is `__halt`, which a basil program has; a C program built with XC8 has none, so
/// it runs for `--max-cycles` instead (and `--until` can name any routine, such as a function the program calls when it is done).
fn default_until(until: Option<String>, given: bool, has_halt: bool) -> Option<String> {
    if !given && !has_halt {
        eprintln!("note: no __halt symbol (a C program?): running for --max-cycles; --until ROUTINE stops when a routine is entered");
        return None;
    }
    until
}

/// `run(budget)` limited to the `--speed` fraction of real time: the budget is spent in slices as the clock allows. Stops as `run` does
/// (halt, breakpoint, ...) or with `Stop::Cycles` when `budget` cycles have run.
fn paced_run<M>(pace: &mut Option<Pace>, budget: u64, m: &mut M, cycles: impl Fn(&M) -> u64, mut run: impl FnMut(&mut M, u64) -> Stop) -> Stop {
    let Some(p) = pace.as_mut() else {
        return run(m, budget);
    };
    let start = cycles(m);
    if p.is_max() {
        return run(m, budget);
    }
    loop {
        let left = budget.saturating_sub(cycles(m) - start);
        if left == 0 {
            return Stop::Cycles;
        }
        let due = p.due(cycles(m)).min(left).min(20_000);
        if due == 0 {
            std::thread::sleep(std::time::Duration::from_millis(1));
            continue;
        }
        match run(m, due) {
            Stop::Cycles => {}
            s => return s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_lengths_are_bounded() {
        assert_eq!(dump_spec("WREG"), Ok(("WREG", 1)));
        assert_eq!(dump_spec("buf:64"), Ok(("buf", 64)));
        assert_eq!(dump_spec("buf:65536"), Ok(("buf", DUMP_MAX)));
        for bad in ["WREG:99999999999", "WREG:65537", "WREG:0", "WREG:x", "WREG:"] {
            assert!(dump_spec(bad).is_err(), "{bad}");
        }
    }
}
