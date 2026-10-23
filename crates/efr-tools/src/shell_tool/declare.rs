//! The paths a command line declares: what each simple command reads and writes,
//! resolved lexically against the directory it runs in, as `cd` moves it.
//!
//! A relative path runs against the hidden shell's directory, which the call context
//! names, not against the user's. A `cd` inside the line adds its target as one more
//! directory the rest of the line may run in, because a `cd` in a pipeline or after
//! `||` may or may not take effect; a `cd` whose target the text does not show (`cd -`,
//! `popd`, `cd $DIR`) leaves the directory unknown, and a relative path then counts as
//! anything below `/`. Each relative `cd` adds its target below every directory so far,
//! so a chain of them doubles the count: past [`MAX_BASES`] directories the directory
//! is unknown as well, and the line asks or runs contained more often, never less.
//!
//! A path that an earlier `cp`, `ln` or `mv` of the same line writes may be a symbolic
//! link by the time a later command uses it, and the daemon resolves links before the
//! line runs, so a later path at or below it may reach anything: it counts as anything
//! below `/` as well. `ln -s ~ h && cat h/.ssh/id_ed25519` then asks.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::reads::{self, Depth, Named};
use super::words::{Line, Word};
use crate::paths::normalize;

/// The most directories that the rest of a line may run in. Past it, a relative path
/// counts as anything below `/`, as after `cd -`.
pub(super) const MAX_BASES: usize = 1024;

/// What a line reads and writes, each path once, in the order first named.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Declared {
    /// Paths read alone.
    pub(super) reads: Vec<PathBuf>,
    /// Paths read with everything below them.
    pub(super) trees: Vec<PathBuf>,
    /// Paths written by an output redirection or by a writer program, such as the
    /// operands of `rm` and the target of `cp`.
    pub(super) writes: Vec<PathBuf>,
}

/// The paths of `line`, run from `start` by a user whose home directory is `home`.
pub(super) fn declared(line: &Line, start: &Path, home: &Path) -> Declared {
    let start = normalize(start);
    let mut walk = Walk {
        bases: vec![start.clone()],
        lost: false,
        relinked: Vec::new(),
        home,
        out: Declared::default(),
        seen: Seen { bases: HashSet::from([start.clone().into_os_string()]), ..Seen::default() },
    };
    for command in &line.commands {
        for input in &command.inputs {
            walk.declare(&named(input, Depth::One), Access::Read);
        }
        for output in &command.outputs {
            walk.declare(&named(output, Depth::One), Access::Write);
        }
        let words = command.program_and_args();
        let Some(program) = words.first() else {
            continue;
        };
        match program.text.rsplit('/').next().unwrap_or(&program.text) {
            "cd" | "pushd" => walk.change_directory(&words[1..]),
            "popd" => walk.lost = true,
            _ => {
                let reads = reads::reads(words);
                for named in &reads.named {
                    walk.declare(named, Access::Read);
                }
                let mut written = Vec::new();
                for named in &reads.writes {
                    written.extend(walk.declare(named, Access::Write));
                }
                if reads.links {
                    walk.relinked.extend(written);
                }
                if let Some(depth) = reads.cwd {
                    walk.declare(
                        &Named { text: ".".to_owned(), tilde: false, pattern: None, depth },
                        Access::Read,
                    );
                }
            }
        }
    }
    walk.out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    Write,
}

struct Walk<'a> {
    /// The directories the next command may run in, each one normal.
    bases: Vec<PathBuf>,
    /// True once a `cd` went somewhere the text does not show.
    lost: bool,
    /// The paths that an earlier `cp`, `ln` or `mv` of the line writes, which may be
    /// symbolic links when a later command uses them.
    relinked: Vec<PathBuf>,
    home: &'a Path,
    out: Declared,
    seen: Seen,
}

/// The paths of `bases` and of each list of [`Declared`], so a path is added once
/// without a scan of its list. Each path is normal, so its bytes are the key: the hash
/// of a `Path` reads it name by name, and a line can name thousands of paths.
#[derive(Default)]
struct Seen {
    bases: HashSet<OsString>,
    reads: HashSet<OsString>,
    trees: HashSet<OsString>,
    writes: HashSet<OsString>,
}

impl Walk<'_> {
    fn change_directory(&mut self, args: &[Word]) {
        let target = args.iter().find(|word| word.text == "-" || !word.text.starts_with('-'));
        match target {
            None => self.add_base(normalize(self.home)),
            Some(word)
                if word.text == "-"
                    || word.text.starts_with('+')
                    || word.expansion
                    || word.pattern.is_some() =>
            {
                self.lost = true;
            }
            Some(word) => {
                let target = named(word, Depth::One);
                let declared = self.declare(&target, Access::Read);
                if self.relinked.iter().any(|link| self.reaches(&target, link)) {
                    self.lost = true;
                    return;
                }
                // NOTE: `declare` reads a word such as `~user` without its tilde, so only
                // a word without one reuses what it resolved.
                let moved = if target.tilde {
                    self.resolve(&target.text, target.tilde)
                } else {
                    Some(declared)
                };
                for base in moved.into_iter().flatten() {
                    self.add_base(base);
                }
            }
        }
    }

    /// Adds `base` as one more directory the rest of the line may run in. Past
    /// [`MAX_BASES`] the directory is unknown.
    fn add_base(&mut self, base: PathBuf) {
        if self.lost {
            return;
        }
        // NOTE: bases are only added, so without the cap a chain of relative cds
        // doubles them with each cd, and efrd runs out of time or memory.
        if self.bases.len() == MAX_BASES {
            self.lost = !self.seen.bases.contains(base.as_os_str());
            return;
        }
        if self.seen.bases.insert(base.clone().into_os_string()) {
            self.bases.push(base);
        }
    }

    /// Declares `named`: a pattern as everything below its fixed directory, anything
    /// else as itself. Returns the paths it declared.
    fn declare(&mut self, named: &Named, access: Access) -> Vec<PathBuf> {
        let (text, depth) = match named.pattern {
            Some(at) => {
                let fixed = &named.text[..at];
                let directory = match fixed.rfind('/') {
                    Some(0) => "/",
                    Some(slash) => &fixed[..slash],
                    None if named.tilde => "~",
                    None => ".",
                };
                (directory, Depth::Tree)
            }
            None => (named.text.as_str(), named.depth),
        };
        let tilde = named.tilde && (text == "~" || text.starts_with("~/"));
        let paths = match self.resolve(text, tilde) {
            Some(paths) => paths,
            // NOTE: the directory is unknown, so a relative path may be anywhere.
            None => {
                self.push(PathBuf::from("/"), access, Depth::Tree);
                return Vec::new();
            }
        };
        for path in &paths {
            // NOTE: a link that this line made may lead anywhere, and the daemon cannot
            // resolve it before the line runs.
            if self.relinked.iter().any(|link| path.starts_with(link)) {
                self.push(PathBuf::from("/"), access, Depth::Tree);
            }
            self.push(path.clone(), access, depth);
        }
        paths
    }

    /// True when `target` resolves to `link` or below it from one of the bases.
    fn reaches(&self, target: &Named, link: &Path) -> bool {
        self.resolve(&target.text, target.tilde)
            .is_some_and(|paths| paths.iter().any(|path| path.starts_with(link)))
    }

    /// The absolute forms of `text`: under the home directory for `~`, itself when
    /// absolute, under every base otherwise; `None` when the base is unknown.
    fn resolve(&self, text: &str, tilde: bool) -> Option<Vec<PathBuf>> {
        let paths = if tilde {
            let below = text.strip_prefix('~').unwrap_or(text).trim_start_matches('/');
            vec![self.home.join(below)]
        } else if text.starts_with('/') {
            vec![PathBuf::from(text)]
        } else if self.lost {
            return None;
        } else if plain(text) {
            // NOTE: a normal base joined with plain names is normal, so this skips the
            // normalize of each path: a line can run in up to 1024 bases.
            return Some(self.bases.iter().map(|base| base.join(text)).collect());
        } else {
            self.bases.iter().map(|base| base.join(text)).collect()
        };
        Some(paths.iter().map(|path| normalize(path)).collect())
    }

    fn push(&mut self, path: PathBuf, access: Access, depth: Depth) {
        let (list, seen) = match (access, depth) {
            (Access::Write, _) => (&mut self.out.writes, &mut self.seen.writes),
            (Access::Read, Depth::One) => (&mut self.out.reads, &mut self.seen.reads),
            (Access::Read, Depth::Tree) => (&mut self.out.trees, &mut self.seen.trees),
        };
        if seen.insert(path.clone().into_os_string()) {
            list.push(path);
        }
    }
}

/// True when `text` is names joined by single slashes, none of them `.` or `..`, with
/// no slash at either end.
fn plain(text: &str) -> bool {
    !text.is_empty() && text.split('/').all(|name| !matches!(name, "" | "." | ".."))
}

fn named(word: &Word, depth: Depth) -> Named {
    Named { text: word.text.clone(), tilde: word.tilde, pattern: word.pattern, depth }
}

#[cfg(test)]
mod tests;
