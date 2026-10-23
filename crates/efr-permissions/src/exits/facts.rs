//! The facts of one call with an index of the facts that the exits look up for each
//! path. [`CallFacts`] keeps its facts in lists, as the daemon collects them, and its
//! own look-ups scan a list; the exits of a line of thousands of paths look up a fact
//! for each path, so each list that is longer than a few entries gets a map, made at
//! its first look-up. The first fact of a key wins, as in [`CallFacts`].

use std::cell::OnceCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Deref;
use std::path::Path;

use crate::{CallFacts, CoverAsk, Coverage, TargetKind};

/// The longest list that a look-up scans. A longer one gets a map.
const SCANNED: usize = 8;

/// [`CallFacts`] with maps of its targets, its tracked counts and its coverage. It
/// derefs to the facts, so the facts that need no map read as before.
#[derive(Debug)]
pub(crate) struct Facts<'a> {
    facts: &'a CallFacts,
    targets: OnceCell<HashMap<&'a Path, Option<TargetKind>>>,
    tracked: OnceCell<HashMap<&'a Path, u32>>,
    covered: OnceCell<HashMap<&'a CoverAsk, Coverage>>,
}

impl<'a> Facts<'a> {
    pub(crate) fn new(facts: &'a CallFacts) -> Self {
        Facts {
            facts,
            targets: OnceCell::new(),
            tracked: OnceCell::new(),
            covered: OnceCell::new(),
        }
    }

    /// [`CallFacts::target`]: what `path` is, `Some(None)` when it does not exist, or
    /// `None` when no fact says.
    pub(crate) fn target(&self, path: &Path) -> Option<Option<TargetKind>> {
        let targets = &self.facts.targets;
        if targets.len() <= SCANNED {
            return self.facts.target(path);
        }
        let map = self
            .targets
            .get_or_init(|| first_wins(targets.iter().map(|(path, kind)| (path.as_path(), *kind))));
        map.get(path).copied()
    }

    /// [`CallFacts::tracked`]: how many tracked files lie below `dir`, when a fact says.
    pub(crate) fn tracked(&self, dir: &Path) -> Option<u32> {
        let counts = &self.facts.tracked_counts;
        if counts.len() <= SCANNED {
            return self.facts.tracked(dir);
        }
        let map = self
            .tracked
            .get_or_init(|| first_wins(counts.iter().map(|(dir, count)| (dir.as_path(), *count))));
        map.get(dir).copied()
    }

    /// [`CallFacts::coverage`]: whether efr's snapshot or its home store covers `ask`,
    /// or [`Coverage::Unknown`] when no fact says.
    pub(crate) fn coverage(&self, ask: &CoverAsk) -> Coverage {
        let covered = &self.facts.covered;
        if covered.len() <= SCANNED {
            return self.facts.coverage(ask);
        }
        let map = self
            .covered
            .get_or_init(|| first_wins(covered.iter().map(|(ask, coverage)| (ask, *coverage))));
        map.get(ask).copied().unwrap_or(Coverage::Unknown)
    }
}

impl Deref for Facts<'_> {
    type Target = CallFacts;

    fn deref(&self) -> &CallFacts {
        self.facts
    }
}

/// A map of `entries` in which the first entry of each key wins, as a scan from the
/// start of the list finds it.
fn first_wins<K: Hash + Eq, V>(entries: impl Iterator<Item = (K, V)>) -> HashMap<K, V> {
    let mut map = HashMap::new();
    for (key, value) in entries {
        map.entry(key).or_insert(value);
    }
    map
}

#[cfg(test)]
mod tests;
