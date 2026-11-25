//! `dspic_dis`: disassembles instruction words given on stdin, one instruction per line as `W1` or `W1 W2` in hex.
//! Prints the assembler text, or `?` when the word is not an instruction. Used by `scripts/dspic_roundtrip.py`.
use picsim::dspic_isa::{decode, format};
use std::io::BufRead;

fn main() {
    let rel = std::env::args().any(|a| a == "--rel");
    for (i, line) in std::io::stdin().lock().lines().map_while(Result::ok).enumerate() {
        let v: Vec<u32> = line.split_whitespace().filter_map(|t| u32::from_str_radix(t, 16).ok()).collect();
        let (w, w2) = (v.first().copied().unwrap_or(0), v.get(1).copied().unwrap_or(0));
        match decode(w, w2) {
            Some(d) => println!("{}\t{}\t{}", d.words, i, format(&d.inst, 0, rel)),
            None => println!("0\t{i}\t?"),
        }
    }
}
