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

/// How a conditional breakpoint compares the watched value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// Bit `n` of the value is 1 / 0.
    BitSet(u8),
    BitClear(u8),
}

/// Stop when a data byte (or a 16-bit little-endian pair) satisfies a comparison.
///
/// Without `at` the condition is edge-triggered: it stops after the instruction that makes the comparison true, not on
/// every instruction while it stays true (so a condition that already holds when the run starts does not stop it until it
/// has become false and true again). With `at` it is a conditional code breakpoint: it stops before the instruction at
/// that program address executes, when the comparison holds then. A stop needs `after` hits (1 = the first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Condition {
    pub name: String,
    pub addr: u32,
    /// Two bytes, `addr` low and `addr + 1` high.
    pub wide: bool,
    pub cmp: Cmp,
    pub value: u32,
    pub at: Option<u32>,
    pub after: u32,
    pub hits: u32,
}

impl Condition {
    pub fn read(&self, peek: impl Fn(u32) -> u8) -> u32 {
        u32::from(peek(self.addr)) | if self.wide { u32::from(peek(self.addr + 1)) << 8 } else { 0 }
    }

    pub fn holds(&self, peek: impl Fn(u32) -> u8) -> bool {
        let v = self.read(peek);
        match self.cmp {
            Cmp::Eq => v == self.value,
            Cmp::Ne => v != self.value,
            Cmp::Lt => v < self.value,
            Cmp::Le => v <= self.value,
            Cmp::Gt => v > self.value,
            Cmp::Ge => v >= self.value,
            Cmp::BitSet(b) => v >> b.min(15) & 1 != 0,
            Cmp::BitClear(b) => v >> b.min(15) & 1 == 0,
        }
    }

    pub fn describe(&self) -> String {
        let w = if self.wide { format!("{}:2", self.name) } else { self.name.clone() };
        let test = match self.cmp {
            Cmp::Eq => format!("{w} == 0x{:X}", self.value),
            Cmp::Ne => format!("{w} != 0x{:X}", self.value),
            Cmp::Lt => format!("{w} < 0x{:X}", self.value),
            Cmp::Le => format!("{w} <= 0x{:X}", self.value),
            Cmp::Gt => format!("{w} > 0x{:X}", self.value),
            Cmp::Ge => format!("{w} >= 0x{:X}", self.value),
            Cmp::BitSet(b) => format!("{w} bit {b} set"),
            Cmp::BitClear(b) => format!("{w} bit {b} clear"),
        };
        let at = self.at.map_or(String::new(), |a| format!(" at 0x{a:X}"));
        let after = if self.after > 1 { format!(", hit {}", self.after) } else { String::new() };
        format!("{test}{at}{after}")
    }

    /// Counts a hit; true when it is time to stop.
    fn hit(&mut self) -> bool {
        self.hits = self.hits.saturating_add(1);
        self.hits >= self.after.max(1)
    }
}

#[derive(Default)]
pub struct Debugger {
    pub watchpoints: Vec<u32>,
    /// Watches on some bits of a byte (`mask` has them set): a change in the other bits does not stop.
    pub bit_watch: Vec<(u32, u8)>,
    pub conditions: Vec<Condition>,
    pub signals: Vec<Signal>,
    pub samples: Vec<Sample>,
    pub recording: bool,
    pub full: bool,
}

impl Debugger {
    pub const MAX_SAMPLES: usize = 100_000;

    /// The signal list can change only while no capture exists (a sample holds one value per signal).
    pub fn can_edit_signals(&self) -> bool {
        !self.recording && self.samples.is_empty()
    }

    /// Add `signal` to the capture; false when the list cannot change now or the signal is already there.
    pub fn add_signal(&mut self, signal: Signal) -> bool {
        if !self.can_edit_signals() || self.signals.iter().any(|s| s.addr == signal.addr && s.bit == signal.bit) {
            return false;
        }
        self.signals.push(signal);
        true
    }

    pub fn active(&self) -> bool {
        !self.watchpoints.is_empty() || !self.bit_watch.is_empty() || !self.conditions.is_empty() || self.recording
    }

    /// Whether each data condition (those without `at`) holds now: taken before an instruction, so that the stop is on the
    /// instruction that makes it true.
    pub fn cond_state(&self, peek: impl Fn(u32) -> u8) -> Vec<bool> {
        self.conditions.iter().map(|c| c.at.is_none() && c.holds(&peek)).collect()
    }

    /// After an instruction: the first data condition that was false before and holds now, and its value.
    pub fn cond_hit_data(&mut self, before: &[bool], peek: impl Fn(u32) -> u8) -> Option<(usize, u32)> {
        for (i, c) in self.conditions.iter_mut().enumerate() {
            if c.at.is_none() && !before.get(i).copied().unwrap_or(true) && c.holds(&peek) && c.hit() {
                return Some((i, c.read(&peek)));
            }
        }
        None
    }

    /// Before the instruction at `pc`: the first conditional code breakpoint there whose comparison holds, and its value.
    pub fn cond_hit_at(&mut self, pc: u32, peek: impl Fn(u32) -> u8) -> Option<(usize, u32)> {
        for (i, c) in self.conditions.iter_mut().enumerate() {
            if c.at == Some(pc) && c.holds(&peek) && c.hit() {
                return Some((i, c.read(&peek)));
            }
        }
        None
    }

    /// Clear the capture on a power-on reset, retaining signal and watchpoint choices.
    pub fn reset(&mut self) {
        self.samples.clear();
        self.full = false;
        for c in &mut self.conditions {
            c.hits = 0;
        }
    }

    pub fn snapshot(&self, peek: impl Fn(u32) -> u8) -> Vec<u8> {
        self.watchpoints.iter().map(|&a| peek(a)).chain(self.bit_watch.iter().map(|&(a, m)| peek(a) & m)).collect()
    }

    /// The first watched byte, or watched bits of a byte, that differs from `before` (as `snapshot` took it). For a bit watch
    /// `before` and `after` are the byte with only the watched bits kept.
    pub fn changed(&self, before: &[u8], peek: impl Fn(u32) -> u8) -> Option<DataChange> {
        let bytes = self.watchpoints.iter().map(|&a| (a, 0xFF));
        bytes.chain(self.bit_watch.iter().copied()).zip(before).find_map(|((addr, mask), &before)| {
            let after = peek(addr) & mask;
            (before != after).then_some(DataChange { addr, before, after })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(addr: u32, bit: Option<u8>) -> Signal {
        Signal { name: "x".into(), addr, bit }
    }

    #[test]
    fn a_signal_is_added_once_and_only_while_no_capture_exists() {
        let mut d = Debugger::default();
        assert!(d.add_signal(sig(0x10, Some(1))));
        assert!(!d.add_signal(sig(0x10, Some(1))), "the same pin twice");
        assert!(d.add_signal(sig(0x10, None)), "the whole byte is another signal");
        d.recording = true;
        assert!(!d.add_signal(sig(0x11, None)), "not while recording");
        d.recording = false;
        d.samples.push(Sample { ns: 0, values: vec![0, 0] });
        assert!(!d.add_signal(sig(0x11, None)), "not while a capture exists");
        d.reset();
        assert!(d.add_signal(sig(0x11, None)));
        assert_eq!(d.signals.len(), 3);
    }

    fn cond(cmp: Cmp, value: u32) -> Condition {
        Condition { name: "X".into(), addr: 0, wide: false, cmp, value, at: None, after: 1, hits: 0 }
    }

    #[test]
    fn comparisons_read_a_byte_or_a_little_endian_pair() {
        let mem = [0x34u8, 0x12];
        let peek = |a: u32| mem[a as usize];
        assert!(cond(Cmp::Eq, 0x34).holds(peek) && cond(Cmp::Ne, 0).holds(peek));
        assert!(cond(Cmp::Gt, 0x33).holds(peek) && !cond(Cmp::Gt, 0x34).holds(peek) && cond(Cmp::Ge, 0x34).holds(peek));
        assert!(cond(Cmp::Lt, 0x35).holds(peek) && cond(Cmp::Le, 0x34).holds(peek));
        assert!(cond(Cmp::BitSet(2), 0).holds(peek) && cond(Cmp::BitClear(0), 0).holds(peek));
        let wide = Condition { wide: true, ..cond(Cmp::Eq, 0x1234) };
        assert!(wide.holds(peek));
        assert!(Condition { wide: true, ..cond(Cmp::BitSet(12), 0) }.holds(peek), "bit 12 of 0x1234");
        assert_eq!(wide.describe(), "X:2 == 0x1234");
    }

    #[test]
    fn a_data_condition_stops_on_the_change_that_makes_it_true_and_not_while_it_stays_true() {
        let mut d = Debugger::default();
        d.conditions.push(cond(Cmp::Eq, 5));
        let before = d.cond_state(|_| 4);
        assert_eq!(d.cond_hit_data(&before, |_| 5), Some((0, 5)));
        // Already true before the instruction: no stop, however long it stays true.
        let before = d.cond_state(|_| 5);
        assert_eq!(d.cond_hit_data(&before, |_| 5), None);
        // False, then true again: stops again.
        let before = d.cond_state(|_| 6);
        assert_eq!(d.cond_hit_data(&before, |_| 5), Some((0, 5)));
        assert!(d.active());
    }

    #[test]
    fn a_code_condition_needs_its_address_and_its_hit_count() {
        let mut d = Debugger::default();
        d.conditions.push(Condition { at: Some(0x40), after: 3, ..cond(Cmp::Ne, 0) });
        assert_eq!(d.cond_hit_at(0x42, |_| 1), None, "another address");
        assert_eq!(d.cond_hit_at(0x40, |_| 0), None, "the comparison is false");
        assert_eq!(d.cond_hit_at(0x40, |_| 1), None, "hit 1 of 3");
        assert_eq!(d.cond_hit_at(0x40, |_| 1), None, "hit 2 of 3");
        assert_eq!(d.cond_hit_at(0x40, |_| 1), Some((0, 1)), "hit 3 of 3");
        d.reset();
        assert_eq!(d.conditions[0].hits, 0, "a reset starts the count again");
        // A condition with an address is never an edge-triggered data condition.
        assert_eq!(d.cond_state(|_| 1), vec![false]);
    }
}
