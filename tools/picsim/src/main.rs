//! `basil-sim prog.hex --device 18F16Q41 [--asm prog.S] [--until __halt] [--dump T_R1:2,T_R2] [--cycles N] [--trace]`

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
    let (mut hex, mut device, mut asm, mut until, mut dump, mut max) =
        (None::<PathBuf>, String::from("18F27Q10"), None::<PathBuf>, Some(String::from("__halt")), Vec::new(), 2_000_000_000u64);
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../devices");
    while let Some(a) = args.next() {
        let mut val = |n: &str| args.next().ok_or(format!("{n} needs a value"));
        match a.as_str() {
            "--device" => device = val("--device")?,
            "--devices" => root = PathBuf::from(val("--devices")?),
            "--asm" => asm = Some(PathBuf::from(val("--asm")?)),
            "--until" => until = Some(val("--until")?),
            "--dump" => dump = val("--dump")?.split(',').map(String::from).collect(),
            "--max-cycles" => max = val("--max-cycles")?.parse().map_err(|_| "bad --max-cycles")?,
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => hex = Some(PathBuf::from(a)),
        }
    }
    let hex = hex.ok_or("usage: basil-sim prog.hex --device PART [--asm prog.S] [--until SYM] [--dump A,B:2]")?;
    let dev = devdesc::Device::load(&root.join(format!("{device}.toml"))).map_err(|e| e.to_string())?;
    let mut m = Machine::new(dev)?;
    m.load_hex(&std::fs::read_to_string(&hex).map_err(|e| format!("{}: {e}", hex.display()))?)?;
    let asm = asm.unwrap_or_else(|| hex.with_extension("S"));
    if let Ok(t) = std::fs::read_to_string(&asm) {
        m.load_symbols_from_asm(&t);
    }
    let stop_at = match &until {
        Some(s) => Some(*m.syms.get(s).ok_or(format!("symbol {s} not found (no {})", asm.display()))?),
        None => None,
    };
    let stop = m.run(stop_at, max);
    println!("stop: {stop:?}  cycles: {}  instructions: {}", m.cpu.cycles, m.cpu.icount);
    for d in &dump {
        let (name, len) = d.split_once(':').map_or((d.as_str(), 1), |(n, l)| (n, l.parse().unwrap_or(1)));
        let addr = m.syms.get(name).copied().or_else(|| m.dev.find_sfr(name).map(|s| s.addr)).ok_or(format!("unknown symbol {name}"))?;
        let b = m.peek(addr, len);
        let v: u64 = b.iter().rev().fold(0, |a, x| a << 8 | u64::from(*x));
        println!("{name} = {v}  ({})", b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" "));
    }
    for n in &m.cpu.notes {
        eprintln!("note: {n}");
    }
    if !m.cpu.m.unmodelled.is_empty() {
        eprintln!("note: unmodelled SFRs touched: {}", m.cpu.m.unmodelled.iter().cloned().collect::<Vec<_>>().join(" "));
    }
    Ok(if stop == Stop::Halt || until.is_none() { ExitCode::SUCCESS } else { ExitCode::from(1) })
}
