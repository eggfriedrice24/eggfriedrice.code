//! Where each simple command of a line runs, as the `under` of a command rule reads it
//! (efr's v0.0.4 live items plan, OQL9).
//!
//! The hidden shell's dir is known when the line starts. A `cd` in the line moves it, but
//! a `cd` can fail, run in a pipeline or in a branch that does not run, so a later
//! command may run in the dir before the `cd` or in its target. A `cd` whose target the
//! text does not show leaves the dir unknown. A rule that allows matches only in the
//! known dir; a rule that asks or denies matches in every dir where the command may run,
//! and everywhere when that is unknown, so it fails closed.

use std::path::{Path, PathBuf};

use crate::command::{self, DirChange, SimpleCommand};
use crate::path_class::normalize;

/// Where a simple command of a line runs, as rules read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place<'a> {
    /// In the dir where the line starts, in normal form: no change of directory comes
    /// before it in the line.
    Start(&'a Path),
    /// In one of these dirs, each in normal form, after a change of directory: the dir
    /// where the line starts and each target of a `cd` before it that efr can read.
    Among(&'a [PathBuf]),
    /// Where efr cannot tell: the line's dir is not known, or a change of directory
    /// before it goes where the text does not show.
    Unknown,
}

impl<'a> Place<'a> {
    /// The dir where the command runs, when efr knows it.
    pub(crate) fn known(self) -> Option<&'a Path> {
        match self {
            Place::Start(dir) => Some(dir),
            Place::Among(_) | Place::Unknown => None,
        }
    }
}

/// The most dirs that a line may be in after its changes of directory, the dir where it
/// starts included. Past it, where the rest of the line runs is unknown.
pub(crate) const MAX_DIRS: usize = 32;

/// The places of the simple commands of a line, which the parts borrow: the dirs that
/// the line may be in, in the order the line reaches them, and for each command how
/// many of them count, or `None` for unknown.
#[derive(Debug, Default)]
pub(crate) struct Places {
    dirs: Vec<PathBuf>,
    each: Vec<Option<usize>>,
}

impl Places {
    /// The places of `commands` for a line that starts in `start`, in normal form, by a
    /// user whose home is `home`. Before a change of directory each command runs in
    /// `start`; after one, in `start` or in any target of a `cd` so far; after one that
    /// goes where the text does not show ([`DirChange::Unknown`]), anywhere.
    pub(crate) fn walk(commands: &[SimpleCommand], start: Option<&Path>, home: &Path) -> Places {
        let mut places = Places::default();
        places.dirs.extend(start.map(Path::to_path_buf));
        // NOTE: `Some(0)` stands for the start alone, before any change of directory.
        let mut now = start.map(|_| 0);
        for command in commands {
            places.each.push(now);
            let Some(change) = command::directory_change(command) else { continue };
            let target = match change {
                DirChange::Home => normalize(home),
                DirChange::To(word) => match word.strip_prefix('~') {
                    Some(rest) => normalize(&home.join(rest.trim_start_matches('/'))),
                    None => normalize(Path::new(word)),
                },
                DirChange::Unknown => None,
            };
            now = now.zip(target).and_then(|(_, target)| {
                if !places.dirs.contains(&target) {
                    if places.dirs.len() == MAX_DIRS {
                        return None;
                    }
                    places.dirs.push(target);
                }
                Some(places.dirs.len())
            });
        }
        places
    }

    /// The place of the command at `index`.
    pub(crate) fn place(&self, index: usize) -> Place<'_> {
        match self.each.get(index).copied().flatten() {
            Some(0) => self.dirs.first().map_or(Place::Unknown, |dir| Place::Start(dir)),
            Some(count) => Place::Among(&self.dirs[..count]),
            None => Place::Unknown,
        }
    }
}
