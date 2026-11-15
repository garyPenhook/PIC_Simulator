//! The pin changes of a stimulus, kept sorted by cycle, with the level each port is driven to after every change
//! (REVIEW-2026-11-11 P3: the cores used to walk and re-sort the whole list on every lookup).

/// One change: from cycle `at`, the pins `mask` of port `port` are driven to `val`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub at: u64,
    pub port: usize,
    pub mask: u8,
    pub val: u8,
}

/// A periodic pin driver. `until` excludes edges at and after that cycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clock {
    pub pin: (u8, u8),
    pub at: u64,
    pub period: u64,
    pub high: u64,
    pub until: Option<u64>,
}

impl Clock {
    fn latest(&self, t: u64) -> Option<(u64, bool)> {
        if t < self.at { return None; }
        let t = self.until.map_or(t, |end| t.min(end - 1));
        let offset = (t - self.at) % self.period;
        let base = t - offset;
        Some(if offset < self.high { (base, true) } else { (base + self.high, false) })
    }

    fn next(&self, t: u64) -> Option<u64> {
        let next = if t < self.at { self.at } else {
            let offset = (t - self.at) % self.period;
            t.checked_add(if offset < self.high { self.high - offset } else { self.period - offset })?
        };
        self.until.is_none_or(|end| next < end).then_some(next)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Drive {
    /// Every change, sorted by `at`; changes at the same cycle keep the order they were added in.
    edges: Vec<Edge>,
    clocks: Vec<(usize, Clock)>,
    /// Per port: `(at, mask, val, level after it, driven mask after it)` for each of that port's changes, same order as `edges`.
    levels: Vec<Vec<(u64, u8, u8, u8, u8)>>,
}

impl Drive {
    pub fn push(&mut self, at: u64, port: usize, mask: u8, val: u8) {
        let i = self.edges.partition_point(|e| e.at <= at);
        self.edges.insert(i, Edge { at, port, mask, val });
        if self.levels.len() <= port {
            self.levels.resize(port + 1, Vec::new());
        }
        let l = &mut self.levels[port];
        let j = l.partition_point(|e| e.0 <= at);
        l.insert(j, (at, mask, val, 0, 0));
        // this change and the later ones of the port apply on top of the one before
        let mut prev = if j == 0 { 0 } else { l[j - 1].3 };
        let mut driven = if j == 0 { 0 } else { l[j - 1].4 };
        for e in &mut l[j..] {
            e.3 = prev & !e.1 | e.2 & e.1;
            prev = e.3;
            driven |= e.1;
            e.4 = driven;
        }
    }

    /// The level the outside world drives on port `port` at cycle `t` (0 before its first change).
    pub fn level(&self, port: usize, t: u64) -> u8 {
        let Some(l) = self.levels.get(port) else { return 0 };
        let mut value = match l.partition_point(|e| e.0 <= t) { 0 => 0, j => l[j - 1].3 };
        for (p, c) in &self.clocks {
            if *p != port { continue; }
            if let Some((at, high)) = c.latest(t) {
                let mask = 1 << c.pin.1;
                let end = l.partition_point(|e| e.0 <= t);
                let start = l.partition_point(|e| e.0 < at);
                if !l[start..end].iter().any(|e| e.1 & mask != 0) {
                    value = value & !mask | if high { mask } else { 0 };
                }
            }
        }
        value
    }

    pub fn clock(&mut self, port: usize, clock: Clock) {
        if self.levels.len() <= port { self.levels.resize(port + 1, Vec::new()); }
        self.clocks.push((port, clock));
    }

    /// Pins explicitly driven at `t`; an unspecified pin floats rather than driving zero.
    pub fn mask(&self, port: usize, t: u64) -> u8 {
        let Some(l) = self.levels.get(port) else { return 0 };
        self.clocks.iter().filter(|(p, c)| *p == port && t >= c.at).fold(
            match l.partition_point(|e| e.0 <= t) { 0 => 0, j => l[j - 1].4 }, |mask, (_, c)| mask | 1 << c.pin.1)
    }

    /// Cycles of the changes with `from < at <= to`, in order (a cycle repeats when several changes share it).
    pub fn times(&self, from: u64, to: u64) -> impl Iterator<Item = u64> + '_ {
        let mut i = self.edges.partition_point(|e| e.at <= from);
        let mut clocks: Vec<_> = self.clocks.iter().map(|(_, c)| c.next(from)).collect();
        std::iter::from_fn(move || {
            let edge = self.edges.get(i).map(|e| e.at).filter(|at| *at <= to);
            let clock = clocks.iter().enumerate().filter_map(|(i, t)| t.filter(|t| *t <= to).map(|t| (i, t))).min_by_key(|(_, t)| *t);
            match (edge, clock) {
                (Some(at), c) if c.is_none_or(|(_, t)| at <= t) => { i += 1; Some(at) }
                (_, Some((j, at))) => { clocks[j] = self.clocks[j].1.next(at); Some(at) }
                _ => None,
            }
        })
    }

    /// The first change after cycle `t`.
    pub fn next_after(&self, t: u64) -> Option<u64> {
        self.edges.get(self.edges.partition_point(|e| e.at <= t)).map(|e| e.at)
            .into_iter().chain(self.clocks.iter().filter_map(|(_, c)| c.next(t))).min()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty() && self.clocks.is_empty()
    }
}

/// Insert `e` into `v`, kept sorted by `key`, after the entries with an equal key (what a stable sort gives).
pub fn insert_sorted<T>(v: &mut Vec<T>, e: T, key: impl Fn(&T) -> u64) {
    let k = key(&e);
    let i = v.partition_point(|x| key(x) <= k);
    v.insert(i, e);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_clock_merges_edges_and_static_overrides_without_expansion() {
        let mut d = Drive::default();
        d.clock(1, Clock { pin: (b'B', 0), at: 10, period: 10, high: 3, until: Some(34) });
        assert_eq!(d.mask(1, 9), 0);
        assert_eq!(d.mask(1, 10), 1);
        assert_eq!(d.level(1, 10), 1);
        assert_eq!(d.level(1, 13), 0);
        assert_eq!(d.next_after(13), Some(20));
        d.push(20, 1, 1, 0); // explicit drive wins at the same cycle
        d.push(22, 1, 1, 1);
        assert_eq!(d.level(1, 20), 0);
        assert_eq!(d.level(1, 22), 1);
        assert_eq!(d.level(1, 23), 0); // next clock edge resumes the clock
        assert_eq!(d.level(1, 30), 1);
        assert_eq!(d.level(1, 100), 0); // holds final level after until
        assert_eq!(d.next_after(33), None);
        assert_eq!(d.times(9, 100).collect::<Vec<_>>(), [10, 13, 20, 20, 22, 23, 30, 33]);
        assert_eq!(d.times(30, 20).count(), 0);
        d.clock(0, Clock { pin: (b'A', 1), at: 0, period: 2, high: 1, until: None });
        assert_eq!(d.times(0, u64::MAX).take(3).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(d.next_after(u64::MAX), None);
        assert_eq!(d.level(0, u64::MAX), 0);
    }

    /// The old linear walk, for comparison.
    fn walk(edges: &[(u64, usize, u8, u8)], port: usize, t: u64) -> u8 {
        let mut s = edges.to_vec();
        s.sort_by_key(|d| d.0);
        let mut ext = 0;
        for &(at, p, m, v) in &s {
            if at <= t && p == port {
                ext = ext & !m | v & m;
            }
        }
        ext
    }

    #[test]
    fn matches_the_linear_walk_in_any_insertion_order() {
        let edges = [(50, 0, 0x01, 0x01), (10, 0, 0x03, 0x02), (50, 0, 0x01, 0x00), (30, 1, 0xFF, 0xAA), (20, 0, 0x02, 0x00), (10, 1, 0x0F, 0x05), (70, 0, 0xFF, 0x80)];
        let mut d = Drive::default();
        for (n, &(at, p, m, v)) in edges.iter().enumerate() {
            d.push(at, p, m, v);
            for port in 0..3 {
                for t in [0, 9, 10, 15, 20, 29, 30, 49, 50, 51, 70, 100] {
                    assert_eq!(d.mask(port, t), edges[..=n].iter().filter(|e| e.0 <= t && e.1 == port).fold(0, |mask, e| mask | e.2));
                    assert_eq!(d.level(port, t), walk(&edges[..=n], port, t), "after {n} edges, port {port}, t {t}");
                }
            }
        }
        assert_eq!(d.times(10, 50).collect::<Vec<_>>(), [20, 30, 50, 50]);
        assert_eq!(d.times(0, 10).collect::<Vec<_>>(), [10, 10]);
        assert_eq!(d.times(70, 1000).count(), 0);
        assert_eq!(d.times(50, 20).count(), 0);
        assert_eq!(d.next_after(50), Some(70));
        assert_eq!(d.next_after(70), None);
        assert_eq!(d.next_after(0), Some(10));
    }
}
