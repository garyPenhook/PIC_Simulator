//! Instruction-boundary data watchpoints and bounded digital waveform capture.
//! Peeks never consume peripheral data. Writes of the same value and changes entirely
//! within an instruction are deliberately not reported as data changes.
use std::fmt::Write;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataChange {
    pub addr: u32,
    pub before: u8,
    pub after: u8,
}

#[derive(Clone, Debug)]
pub struct Signal {
    pub name: String,
    pub addr: u32,
    /// A selected PORT bit, or all eight bits of a data byte.
    pub bit: Option<u8>,
}

#[derive(Clone, Debug)]
pub struct Sample {
    pub ns: u64,
    pub values: Vec<u8>,
}

#[derive(Default)]
pub struct Debugger {
    pub watchpoints: Vec<u32>,
    pub signals: Vec<Signal>,
    pub samples: Vec<Sample>,
    pub recording: bool,
    pub full: bool,
}

impl Debugger {
    pub const MAX_SAMPLES: usize = 100_000;

    pub fn active(&self) -> bool {
        !self.watchpoints.is_empty() || self.recording
    }

    /// Clear the capture on a power-on reset, retaining signal and watchpoint choices.
    pub fn reset(&mut self) {
        self.samples.clear();
        self.full = false;
    }

    pub fn snapshot(&self, peek: impl Fn(u32) -> u8) -> Vec<u8> {
        self.watchpoints.iter().map(|&a| peek(a)).collect()
    }

    pub fn changed(&self, before: &[u8], peek: impl Fn(u32) -> u8) -> Option<DataChange> {
        self.watchpoints
            .iter()
            .zip(before)
            .find_map(|(&addr, &before)| {
                let after = peek(addr);
                (before != after).then_some(DataChange {
                    addr,
                    before,
                    after,
                })
            })
    }

    pub fn sample(&mut self, ns: u64, peek: impl Fn(u32) -> u8) {
        if !self.recording {
            return;
        }
        let values: Vec<u8> = self
            .signals
            .iter()
            .map(|s| {
                let v = peek(s.addr);
                s.bit.map_or(v, |b| (v >> b.min(7)) & 1)
            })
            .collect();
        // Keep unchanged endpoints too, so the visible/exported duration includes idle time.
        if self.samples.last().is_some_and(|s| s.values == values)
            && self.samples.len() > 1
            && self.samples[self.samples.len() - 2].values == values
        {
            self.samples.last_mut().unwrap().ns = ns;
            return;
        }
        if self.samples.len() >= Self::MAX_SAMPLES {
            self.recording = false;
            self.full = true;
            return;
        }
        self.samples.push(Sample { ns, values });
    }

    /// VCD timestamps are simulated nanoseconds relative to the start of this capture.
    pub fn vcd(&self) -> String {
        let mut out =
            String::from("$version PicSim $end\n$timescale 1 ns $end\n$scope module picsim $end\n");
        for (i, s) in self.signals.iter().enumerate() {
            let name: String = s
                .name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            writeln!(
                out,
                "$var wire {} s{i} {name}_{i} $end",
                if s.bit.is_some() { 1 } else { 8 }
            )
            .unwrap();
        }
        out.push_str("$upscope $end\n$enddefinitions $end\n");
        let base = self.samples.first().map_or(0, |s| s.ns);
        let mut last: Option<&[u8]> = None;
        for s in &self.samples {
            writeln!(out, "#{}", s.ns.saturating_sub(base)).unwrap();
            for (i, (&v, signal)) in s.values.iter().zip(&self.signals).enumerate() {
                if last.is_some_and(|p| p.get(i) == Some(&v)) {
                    continue;
                }
                if signal.bit.is_some() {
                    writeln!(out, "{}s{i}", v & 1).unwrap();
                } else {
                    writeln!(out, "b{v:08b} s{i}").unwrap();
                }
            }
            last = Some(&s.values);
        }
        out
    }
}
