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

#[derive(Clone, Debug, Default)]
pub struct Drive {
    /// Every change, sorted by `at`; changes at the same cycle keep the order they were added in.
    edges: Vec<Edge>,
    /// Per port: `(at, mask, val, level after it)` for each of that port's changes, same order as `edges`.
    levels: Vec<Vec<(u64, u8, u8, u8)>>,
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
        l.insert(j, (at, mask, val, 0));
        // this change and the later ones of the port apply on top of the one before
        let mut prev = if j == 0 { 0 } else { l[j - 1].3 };
        for e in &mut l[j..] {
            e.3 = prev & !e.1 | e.2 & e.1;
            prev = e.3;
        }
    }

    /// The level the outside world drives on port `port` at cycle `t` (0 before its first change).
    pub fn level(&self, port: usize, t: u64) -> u8 {
        let Some(l) = self.levels.get(port) else { return 0 };
        match l.partition_point(|e| e.0 <= t) {
            0 => 0,
            j => l[j - 1].3,
        }
    }

    /// Cycles of the changes with `from < at <= to`, in order (a cycle repeats when several changes share it).
    pub fn times(&self, from: u64, to: u64) -> impl Iterator<Item = u64> + '_ {
        let a = self.edges.partition_point(|e| e.at <= from);
        let b = self.edges.partition_point(|e| e.at <= to).max(a);
        self.edges[a..b].iter().map(|e| e.at)
    }

    /// The first change after cycle `t`.
    pub fn next_after(&self, t: u64) -> Option<u64> {
        self.edges.get(self.edges.partition_point(|e| e.at <= t)).map(|e| e.at)
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
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
