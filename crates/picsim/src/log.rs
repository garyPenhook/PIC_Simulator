//! An observation log (bytes sent, bus events, pin changes) that keeps only its latest entries, so a long run does not
//! grow without limit. Entries have absolute positions: the `n`-th entry ever pushed is at `n` whether or not older ones
//! were dropped, so a reader that remembers a position (`total` at the time) stays right.

use std::ops::Deref;

/// Entries kept: when the log reaches this many, the older half is dropped (amortised O(1) per push).
pub const LOG_CAP: usize = 1 << 17;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Log<T> {
    v: Vec<T>,
    /// Entries dropped from the front since the start.
    dropped: u64,
    cap: usize,
}

impl<T> Default for Log<T> {
    fn default() -> Self {
        Log { v: Vec::new(), dropped: 0, cap: LOG_CAP }
    }
}

impl<T> Log<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// A log that keeps at most `cap` entries (at least 2).
    pub fn with_cap(cap: usize) -> Self {
        Log { v: Vec::new(), dropped: 0, cap: cap.max(2) }
    }

    pub fn push(&mut self, e: T) {
        if self.v.len() >= self.cap {
            let n = self.cap / 2;
            self.v.drain(..n);
            self.dropped += n as u64;
        }
        self.v.push(e);
    }

    /// Entries pushed since the start, kept or not: the position the next push gets.
    pub fn total(&self) -> u64 {
        self.dropped + self.v.len() as u64
    }

    /// Entries dropped from the front since the start: the position of the first entry kept.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The entry at absolute position `pos`, if it is still kept.
    pub fn get_abs_mut(&mut self, pos: u64) -> Option<&mut T> {
        let i = usize::try_from(pos.checked_sub(self.dropped)?).ok()?;
        self.v.get_mut(i)
    }

    /// The entries kept from absolute position `pos` on (all kept entries when `pos` was dropped).
    pub fn since(&self, pos: u64) -> &[T] {
        let i = usize::try_from(pos.saturating_sub(self.dropped)).unwrap_or(usize::MAX).min(self.v.len());
        &self.v[i..]
    }
}

/// The kept entries, oldest first.
impl<T> Deref for Log<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.v
    }
}

/// Compares the kept entries.
impl<T: PartialEq> PartialEq<Vec<T>> for Log<T> {
    fn eq(&self, o: &Vec<T>) -> bool {
        self.v == *o
    }
}

impl<'a, T> IntoIterator for &'a Log<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.v.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_latest_entries_with_absolute_positions() {
        let mut l = Log::with_cap(4);
        for i in 0..10u32 {
            l.push(i);
        }
        assert_eq!(l.total(), 10);
        assert!(l.len() <= 4 && *l.last().unwrap() == 9);
        assert_eq!(l.since(l.dropped()), &l[..]);
        assert_eq!(l.since(8), &[8, 9]);
        assert_eq!(l.since(0), &l[..], "a dropped position reads from the first entry kept");
        assert_eq!(l.since(10), &[] as &[u32]);
        assert_eq!(l.get_abs_mut(9).copied(), Some(9));
        assert_eq!(l.get_abs_mut(0), None);
    }
}
