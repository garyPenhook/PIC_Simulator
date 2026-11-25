//! `dspic_run`: runs a dsPIC33 hex file in picsim's core for the differential tests against MDB (`scripts/dspic_diff.py`).
//!
//! `dspic_run PROG.hex DEVICE.toml --stops A,B,C --until A --dump ADDR:LEN [--max-cycles N]`
//! prints `STOP <pc> <cycles since the previous stop>` for each stop address reached and finally `PC`, `CYCLES` and `DUMP` lines.
use picsim::dspic::Dspic;

fn num(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or_else(|_| s.parse().unwrap())
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let hex = std::fs::read_to_string(&a[1]).unwrap();
    let dev = devdesc::Device::load(std::path::Path::new(&a[2])).unwrap();
    let mut stops: Vec<u32> = Vec::new();
    let mut until = u32::MAX;
    let mut dumps: Vec<(u32, usize)> = Vec::new();
    let mut max = 50_000_000u64;
    let mut i = 3;
    while i < a.len() {
        match a[i].as_str() {
            "--stops" => {
                stops = a[i + 1].split(',').filter(|s| !s.is_empty()).map(num).collect();
                i += 1;
            }
            "--until" => {
                until = num(&a[i + 1]);
                i += 1;
            }
            "--dump" => {
                let (x, y) = a[i + 1].split_once(':').unwrap();
                dumps.push((num(x), y.parse().unwrap()));
                i += 1;
            }
            "--max-cycles" => {
                max = a[i + 1].parse().unwrap();
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    let mut c = Dspic::new(&dev).unwrap();
    if let Some(i) = a.iter().position(|x| x == "--stim") {
        picsim::stimulus::Stimulus::parse(&std::fs::read_to_string(&a[i + 1]).unwrap()).unwrap().apply_dspic(&dev, &mut c).unwrap();
    }
    c.load(&picsim::hex::Image::parse(&hex).unwrap());
    let mut last = 0u64;
    let mut first = true;
    while c.cycles < max {
        if (stops.contains(&c.pc) && !first) || c.pc == until {
            println!("STOP {:X} {}", c.pc, c.cycles - last);
            last = c.cycles;
            if c.pc == until {
                break;
            }
        }
        first = false;
        c.step();
    }
    println!("PC {:X}", c.pc);
    println!("CYCLES {}", c.cycles);
    for (addr, len) in dumps {
        let bytes: Vec<String> = (0..len).map(|k| format!("{:02x}", c.peek(addr + k as u32))).collect();
        println!("DUMP {:X} {}", addr, bytes.join(" "));
    }
    for n in &c.unmodelled {
        eprintln!("unmodelled {n}");
    }
}
