//! The deletes of a line that full mode guards (DF4): a delete, a move away, a
//! recursive change of modes or owners, or an `rsync --delete` of a tree that only the
//! user may risk.
//!
//! The guard reads the line through the same lenient split as [`predict`](super::predict)
//! and looks through the same wrappers (`sudo`, `env`, `nice`, `xargs`, `timeout`,
//! `setsid`, `nohup` and the rest), into the text of each substitution, into each
//! branch that may never run (`false && rm -rf ~`), and one level into the literal
//! string of `sh -c`, `bash -c`, `zsh -c` and `eval`. It reads only the line: a
//! delete inside a script or a program is not seen.
//!
//! A target is critical when it is `/`, the home directory or a dir above it, a system
//! dir outside home ([`is_system_dir`]), the turn's project root or a registered
//! project root, or a dir that holds what [`Envelope::holds`] counts. A target that efr
//! cannot read is critical too: a word with an expansion (but `$HOME` and `${HOME}` at
//! its start, which read as `~`), a pattern of a tree form whose literal dir is empty,
//! critical or unreadable, and a relative word where the dir of the line is not known.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::ptr;

use super::ExitInput;
use super::envelope::{Envelope, resolve};
use super::programs::{base_name, placed_commands};
use super::scan::{self, CdTarget, OPAQUE, Segment, is_assignment};
use crate::command::git;
use crate::path_class::normalize;
use crate::tables::is_system_dir;
use crate::{CriticalDelete, CriticalTarget, CriticalWhy, Locations};

/// The deletes of the line of `input` that full mode guards (DF4), in the order of the
/// line, each target of a simple command once. Pure: it reads the line, the locations
/// and the turn's roots, never the disk.
pub fn critical_deletes(input: &ExitInput<'_>) -> Vec<CriticalDelete> {
    let locations = input.locations;
    let in_home_form =
        |path: &Path| normalize(path).map(|path| locations.rehome(&path).into_owned());
    let mut guard = Guard {
        locations,
        envelope: Envelope::new(locations, input.project_root, input.scratch),
        project_root: input.project_root.and_then(in_home_form),
        found: Vec::new(),
    };
    let start = input.command_dir.and_then(in_home_form);
    guard.line(input.line, &[start], 0);
    guard.found
}

/// How deep the guard reads the literal string of `sh -c` or `eval`: the line itself
/// is depth 0, and its strings are read once more.
const NESTED: u8 = 1;

/// The shells whose `-c` string the guard reads as a line.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh"];

/// The programs that delete what `find -exec` gives them.
const FIND_DELETERS: &[&str] = &["rm", "shred", "srm", "wipe"];

/// The characters that make a word a pattern of the shell.
const PATTERN: &[char] = &['*', '?', '['];

/// At most this many words come from the brace expansion of one word; a word with more
/// counts as a pattern.
const MAX_BRACE_WORDS: usize = 64;

/// A word with more brace groups than this counts as a pattern, so the expansion stays
/// shallow.
const MAX_BRACE_GROUPS: usize = 8;

/// One word of a simple command: as the split reads it, and as the line wrote it.
#[derive(Debug, Clone, Copy)]
struct Word<'a> {
    text: &'a str,
    raw: &'a str,
}

/// How a target's place decides whether it is critical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// A tree form: the target and all it holds go.
    Tree,
    /// A tree form whose target may be the turn's project root, which the project
    /// snapshot covers: `git clean` (DF4, the narrower forms).
    TreeButProject,
    /// The pattern of a plain `rm`: only the files that the pattern names go, so only a
    /// pattern in `/`, `~`, a dir above `~` or a system dir is critical.
    Files,
}

struct Guard<'a> {
    locations: &'a Locations,
    envelope: Envelope<'a>,
    /// The turn's project root, in normal form under the home directory.
    project_root: Option<PathBuf>,
    found: Vec<CriticalDelete>,
}

impl Guard<'_> {
    /// Reads `line`, which starts in one of `starts` (`None`: a dir that efr does not
    /// know), at `depth`.
    fn line(&mut self, line: &str, starts: &[Option<PathBuf>], depth: u8) {
        let segments = scan::scan(line);
        // NOTE: the segments of a line see longer and longer lists of the same `cd`s,
        // so from one start the dirs grow by the new `cd`s alone. A line of 2,000 `cd`s
        // read each list again for each segment and took 41 s in full mode.
        let mut walk = match starts {
            [start] => Some(DirWalk::new(start.clone())),
            _ => None,
        };
        for (at, segment) in segments.iter().enumerate() {
            let read;
            let dirs: &[Option<PathBuf>] = match (&mut walk, segment.cds.as_deref()) {
                (Some(walk), Some(cds)) => walk.after(cds, self.locations),
                (_, cds) => {
                    read = dirs(starts, cds, self.locations);
                    &read
                }
            };
            let part = shown(&segment.raw.join(" ")).into_owned();
            if segment.too_deep {
                // NOTE: a delete may lie in the substitution that the split did not read.
                self.push(&part, CriticalTarget::Unreadable { word: part.clone() });
                continue;
            }
            let producer = at.checked_sub(1).and_then(|before| segments.get(before));
            let commands = placed_commands(&segment.words);
            for (index, (command, _)) in commands.iter().enumerate() {
                let words = with_raw(segment, command);
                let via_xargs =
                    commands[..index].iter().any(|(outer, _)| program(outer) == "xargs");
                let cx = Command { words: &words, starts, dirs, part: &part, depth };
                self.command(&cx, segment, via_xargs.then_some(producer));
            }
        }
    }

    /// Reads one simple command. `xargs` is `Some` when `xargs` runs it, with the
    /// command before it in the line, which may give `xargs` its input.
    fn command(&mut self, cx: &Command<'_>, segment: &Segment, xargs: Option<Option<&Segment>>) {
        let Some((first, args)) = cx.words.split_first() else { return };
        let program = base_name(first.text);
        let tree_form = match program {
            "rm" => {
                let (recursive, operands) = rm_operands(args);
                let reach = if recursive { Reach::Tree } else { Reach::Files };
                for word in operands {
                    if recursive || is_pattern(word.text) {
                        self.target(cx, word, reach);
                    }
                }
                recursive
            }
            "find" => {
                if let Some(roots) = find_roots(args) {
                    for word in roots {
                        self.target(cx, word, Reach::Tree);
                    }
                }
                false
            }
            "mv" => {
                for word in mv_sources(args) {
                    self.target(cx, word, Reach::Tree);
                }
                true
            }
            "rsync" => {
                if let Some(word) = rsync_deletes(args) {
                    self.target(cx, word, Reach::Tree);
                }
                false
            }
            "chmod" | "chown" | "chgrp" | "setfacl" => match recursive_operands(program, args) {
                Some(operands) => {
                    for word in operands {
                        self.target(cx, word, Reach::Tree);
                    }
                    true
                }
                None => false,
            },
            "shred" | "srm" | "wipe" => match wiped(program, args) {
                Some(operands) => {
                    for word in operands {
                        self.target(cx, word, Reach::Tree);
                    }
                    true
                }
                None => false,
            },
            "git" => {
                self.git_clean(cx, segment, args);
                false
            }
            "eval" if cx.depth < NESTED => {
                let line = match args {
                    [only] => nested_line(*only),
                    _ => args.iter().map(|word| word.text).collect::<Vec<_>>().join(" "),
                };
                self.line(&line, cx.dirs, cx.depth + 1);
                false
            }
            shell if SHELLS.contains(&shell) && cx.depth < NESTED => {
                if let Some(string) = shell_string(args) {
                    self.line(&nested_line(string), cx.dirs, cx.depth + 1);
                }
                false
            }
            _ => false,
        };
        if let (true, Some(producer)) = (tree_form, xargs)
            && !self.reads_literal_find(producer, cx.starts)
        {
            self.push(cx.part, CriticalTarget::Unreadable { word: cx.part.to_owned() });
        }
    }

    /// True when `producer`, the command before an `xargs` in a line that starts in one
    /// of `starts`, is a `find` whose roots efr reads and none of which is critical:
    /// what it prints lies below them.
    fn reads_literal_find(&self, producer: Option<&Segment>, starts: &[Option<PathBuf>]) -> bool {
        let Some(producer) = producer else { return false };
        let Some((command, _)) = placed_commands(&producer.words).into_iter().next() else {
            return false;
        };
        if program(command) != "find" {
            return false;
        }
        let words = with_raw(producer, command);
        let roots = find_roots_of(words.get(1..).unwrap_or_default());
        let dirs = dirs(starts, producer.cds.as_deref(), self.locations);
        roots.iter().all(|word| {
            let text = home_read(*word);
            !text.contains(OPAQUE)
                && !is_pattern(&text)
                && dirs.iter().all(|dir| {
                    resolve(&text, dir.as_deref(), self.locations)
                        .is_some_and(|path| self.why(&path, Reach::Tree).is_none())
                })
        })
    }

    /// `git clean` with `-f` and `-d`: the work tree of its dir, or its pathspecs.
    fn git_clean(&mut self, cx: &Command<'_>, segment: &Segment, args: &[Word<'_>]) {
        let Some(clean) = git_clean_args(args) else { return };
        if let Some(steer) = segment.words.iter().zip(&segment.raw).find(|(word, _)| {
            is_assignment(word)
                && (word.starts_with("GIT_DIR=") || word.starts_with("GIT_WORK_TREE="))
        }) {
            self.push(cx.part, CriticalTarget::Unreadable { word: shown(steer.1).into_owned() });
            return;
        }
        let mut dirs: Vec<Option<PathBuf>> = cx.dirs.to_vec();
        for word in clean.dirs {
            let text = home_read(word);
            dirs = dirs.iter().map(|dir| resolve(&text, dir.as_deref(), self.locations)).collect();
            if dirs.iter().any(Option::is_none) {
                self.push(
                    cx.part,
                    CriticalTarget::Unreadable { word: shown(word.raw).into_owned() },
                );
                return;
            }
        }
        if clean.pathspecs.is_empty() {
            for dir in &dirs {
                match dir {
                    Some(path) => self.path(cx.part, path, Reach::TreeButProject),
                    None => {
                        self.push(cx.part, CriticalTarget::Unreadable { word: cx.part.to_owned() });
                    }
                }
            }
            return;
        }
        let inner = Command { dirs: &dirs, ..*cx };
        for word in clean.pathspecs {
            self.target(&inner, word, Reach::TreeButProject);
        }
    }

    /// Reads the target `word` of a delete form.
    fn target(&mut self, cx: &Command<'_>, word: Word<'_>, reach: Reach) {
        // NOTE: `{}` is the place of `find -exec`, whose roots the `find` form reads.
        if word.text.is_empty() || word.text == "{}" {
            return;
        }
        let unreadable = CriticalTarget::Unreadable { word: shown(word.raw).into_owned() };
        let text = home_read(word);
        let Some(texts) = braces(&text) else {
            return self.pattern(cx, &text, unreadable, reach);
        };
        for text in texts {
            if text.contains(OPAQUE) {
                self.push(cx.part, unreadable.clone());
            } else if is_pattern(&text) {
                self.pattern(cx, &text, unreadable.clone(), reach);
            } else {
                for dir in cx.dirs {
                    match resolve(&operand(&text), dir.as_deref(), self.locations) {
                        Some(path) => self.path(cx.part, &path, reach),
                        None => self.push(cx.part, unreadable.clone()),
                    }
                }
            }
        }
    }

    /// Reads a pattern `text`, whose word is `unreadable` when efr cannot read it.
    fn pattern(&mut self, cx: &Command<'_>, text: &str, unreadable: CriticalTarget, reach: Reach) {
        let prefix = literal_prefix(text);
        if prefix.contains(OPAQUE) {
            return self.push(cx.part, unreadable);
        }
        let here = Path::new(&prefix).components().all(|part| part == std::path::Component::CurDir);
        if here && reach != Reach::Files {
            return self.push(cx.part, unreadable);
        }
        for dir in cx.dirs {
            let path = if here {
                dir.clone()
            } else {
                resolve(&operand(&prefix), dir.as_deref(), self.locations)
            };
            match path {
                Some(path) => self.path(cx.part, &path, reach),
                None => self.push(cx.part, unreadable.clone()),
            }
        }
    }

    /// Adds `path`, a target in normal form under the home directory, when a delete
    /// of `reach` there is critical.
    fn path(&mut self, part: &str, path: &Path, reach: Reach) {
        if let Some(why) = self.why(path, reach) {
            self.push(part, CriticalTarget::Path { path: path.to_path_buf(), why });
        }
    }

    /// Why a delete of `reach` at `path` is critical, or `None`.
    fn why(&self, path: &Path, reach: Reach) -> Option<CriticalWhy> {
        let locations = self.locations;
        let home = locations.home();
        let place = if path == Path::new("/") {
            Some(CriticalWhy::Root)
        } else if path == home {
            Some(CriticalWhy::Home)
        } else if locations.is_at_or_above_home(path) {
            Some(CriticalWhy::AboveHome)
        } else if !path.starts_with(home) && is_system_dir(path) {
            Some(CriticalWhy::SystemDir)
        } else {
            None
        };
        if place.is_some() || reach == Reach::Files {
            return place;
        }
        let turn_root = self.project_root.as_deref() == Some(path);
        if turn_root && reach == Reach::TreeButProject {
            return None;
        }
        if turn_root || locations.is_project_root(path) {
            return Some(CriticalWhy::ProjectRoot);
        }
        self.envelope.holds(path)
    }

    fn push(&mut self, part: &str, target: CriticalTarget) {
        let delete = CriticalDelete { part: part.to_owned(), target };
        if !self.found.contains(&delete) {
            self.found.push(delete);
        }
    }
}

/// One simple command that the guard reads, with where it may run.
#[derive(Clone, Copy)]
struct Command<'c> {
    words: &'c [Word<'c>],
    /// The dirs where its line may start.
    starts: &'c [Option<PathBuf>],
    /// The dirs where it may run; `None` for one that efr does not know.
    dirs: &'c [Option<PathBuf>],
    /// The simple command of the line that holds it, for [`CriticalDelete::part`].
    part: &'c str,
    depth: u8,
}

/// The program of `command`, its base name.
fn program(command: &[String]) -> &str {
    command.first().map_or("", |first| base_name(first))
}

/// The words of `command`, a slice of `segment`'s words, each with its raw form.
fn with_raw<'s>(segment: &'s Segment, command: &'s [String]) -> Vec<Word<'s>> {
    let offset = command
        .first()
        .and_then(|first| segment.words.iter().position(|word| ptr::eq(word, first)))
        .unwrap_or(0);
    command
        .iter()
        .enumerate()
        .map(|(at, text)| Word {
            text,
            raw: segment.raw.get(offset + at).map_or(text.as_str(), String::as_str),
        })
        .collect()
}

/// The dirs where a command after the `cd`s `cds` may run, for a line that starts in
/// one of `starts`: each start, and the dir after each step of the `cd`s, because a
/// `cd` can fail, run in a subshell or in a branch that does not run. `None` stands
/// for a dir that efr does not know, and is the only dir when a `cd` goes where efr
/// cannot read.
fn dirs(
    starts: &[Option<PathBuf>],
    cds: Option<&[CdTarget]>,
    locations: &Locations,
) -> Vec<Option<PathBuf>> {
    let Some(cds) = cds else { return vec![None] };
    let mut found: Vec<Option<PathBuf>> = Vec::new();
    let mut add = |dir: Option<PathBuf>| {
        if !found.contains(&dir) {
            found.push(dir);
        }
    };
    for start in starts {
        let mut dir = start.clone();
        add(dir.clone());
        for cd in cds {
            dir = after_cd(cd, dir.as_deref(), locations);
            add(dir.clone());
        }
    }
    found
}

/// The step of [`dirs`] after one `cd`: the dir where `cd` goes from `dir`, or `None`.
fn after_cd(cd: &CdTarget, dir: Option<&Path>, locations: &Locations) -> Option<PathBuf> {
    let text = home_read(Word { text: &cd.word, raw: &cd.raw });
    if text.contains(OPAQUE) { None } else { resolve(&operand(&text), dir, locations) }
}

/// [`dirs`] from one start for the segments of one line, in their order: each segment
/// sees the `cd`s of the one before it and more, so the walk reads each `cd` once.
struct DirWalk {
    /// The dir after the `cd`s read so far.
    dir: Option<PathBuf>,
    /// How many `cd`s it read.
    read: usize,
    /// The dirs so far, each once, in the order of [`dirs`].
    found: Vec<Option<PathBuf>>,
    /// The dirs of `found`, to add each once without a scan of the list.
    seen: HashSet<Option<PathBuf>>,
}

impl DirWalk {
    fn new(start: Option<PathBuf>) -> Self {
        DirWalk {
            dir: start.clone(),
            read: 0,
            found: vec![start.clone()],
            seen: HashSet::from([start]),
        }
    }

    /// The dirs of [`dirs`] for a segment after the `cd`s `cds`, which start with the
    /// `cd`s that the walk read.
    fn after(&mut self, cds: &[CdTarget], locations: &Locations) -> &[Option<PathBuf>] {
        debug_assert!(self.read <= cds.len(), "a later segment sees fewer cds");
        for cd in &cds[self.read..] {
            self.dir = after_cd(cd, self.dir.as_deref(), locations);
            if self.seen.insert(self.dir.clone()) {
                self.found.push(self.dir.clone());
            }
        }
        self.read = cds.len();
        &self.found
    }
}

/// `word` with a `$HOME` or `${HOME}` at its start, also in double quotes, read as `~`,
/// as the shell tool reads it. Any other expansion stays [`OPAQUE`].
fn home_read(word: Word<'_>) -> Cow<'_, str> {
    let raw = word.raw.strip_prefix('"').unwrap_or(word.raw);
    let opaque_home = format!("{OPAQUE}HOME");
    if raw.starts_with("${HOME}")
        && let Some(rest) = word.text.strip_prefix(OPAQUE)
    {
        return Cow::Owned(format!("~{rest}"));
    }
    if let Some(after) = raw.strip_prefix("$HOME")
        && !after.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
        && let Some(rest) = word.text.strip_prefix(opaque_home.as_str())
    {
        return Cow::Owned(format!("~{rest}"));
    }
    Cow::Borrowed(word.text)
}

/// `text` as a path word for [`resolve`]: a relative word that starts with `-` (after
/// `--`) gets `./`, so it is not read as an option.
fn operand(text: &str) -> Cow<'_, str> {
    if text.starts_with('-') { Cow::Owned(format!("./{text}")) } else { Cow::Borrowed(text) }
}

/// True when the shell expands `text` as a pattern.
fn is_pattern(text: &str) -> bool {
    text.contains(PATTERN)
}

/// The words of the brace expansion of `text`, such as `~/p` and `~/x` for `~/{p,x}`,
/// or `text` alone when it has none. `None` when efr does not expand it: a range such
/// as `{1..9}`, nested braces, or too many words; the caller reads it as a pattern.
fn braces(text: &str) -> Option<Vec<String>> {
    let Some(open) = text.find('{') else { return Some(vec![text.to_owned()]) };
    if text.matches('{').count() > MAX_BRACE_GROUPS {
        return None;
    }
    let Some(close) = text[open..].find('}').map(|at| open + at) else {
        return Some(vec![text.to_owned()]);
    };
    let inner = &text[open + 1..close];
    if inner.contains('{') || inner.contains("..") {
        return None;
    }
    // NOTE: the rest is expanded once, so the work grows with the length of the word
    // and never with the product of its groups.
    let rest = braces(&text[close + 1..])?;
    if !inner.contains(',') {
        // NOTE: `{x}` is no expansion; the rest may still hold one.
        return Some(rest.into_iter().map(|rest| format!("{}{rest}", &text[..=close])).collect());
    }
    let mut words = Vec::new();
    for choice in inner.split(',') {
        for rest in &rest {
            words.push(format!("{}{choice}{rest}", &text[..open]));
            if words.len() > MAX_BRACE_WORDS {
                return None;
            }
        }
    }
    Some(words)
}

/// The dir part of a pattern before its first component with a pattern character or
/// a brace: `build` for `build/*`, `~` for `~/*`, `/` for `/*`, empty for `*` and
/// `*.o`.
fn literal_prefix(text: &str) -> String {
    let special = |part: &&str| part.contains(PATTERN) || part.contains('{');
    let parts: Vec<&str> = text.split('/').take_while(|part| !special(part)).collect();
    let prefix = parts.join("/");
    if prefix.is_empty() && text.starts_with('/') { "/".to_owned() } else { prefix }
}

/// `text` for the user: what only the shell knows shows as `...`.
fn shown(text: &str) -> Cow<'_, str> {
    if text.contains(OPAQUE) {
        Cow::Owned(text.replace(OPAQUE, "..."))
    } else {
        Cow::Borrowed(text)
    }
}

/// The literal string of a shell's `-c`, as a line to read: the text of a word in
/// double quotes as the line wrote it, so the nested read sees `$HOME` and `$(...)`
/// as the shell does; else the word with its quotes removed.
fn nested_line(word: Word<'_>) -> String {
    let inner = word.raw.strip_prefix('"').and_then(|rest| rest.strip_suffix('"'));
    match inner {
        Some(inner) if !inner.contains('"') => inner.to_owned(),
        _ => word.text.to_owned(),
    }
}

/// True for an option word: a dash and at least one more character.
fn is_option(text: &str) -> bool {
    text.starts_with('-') && text.len() > 1
}

/// Whether `rm` with `args` is recursive, and its operands.
fn rm_operands<'w>(args: &[Word<'w>]) -> (bool, Vec<Word<'w>>) {
    let mut recursive = false;
    let mut operands = Vec::new();
    let mut options = true;
    for word in args {
        if options && word.text == "--" {
            options = false;
        } else if options && word.text.starts_with("--") {
            recursive |= word.text == "--recursive";
        } else if options && is_option(word.text) {
            recursive |= word.text.contains(['r', 'R']);
        } else {
            operands.push(*word);
        }
    }
    (recursive, operands)
}

/// The roots of `find` with `args` when it deletes what it finds: `-delete`, or an
/// `-exec`, `-execdir`, `-ok` or `-okdir` that runs a program of [`FIND_DELETERS`].
fn find_roots<'w>(args: &[Word<'w>]) -> Option<Vec<Word<'w>>> {
    let mut deletes = false;
    let mut at = 0;
    while let Some(word) = args.get(at) {
        match word.text {
            "-delete" => deletes = true,
            "-exec" | "-execdir" | "-ok" | "-okdir" => {
                let runs = args.get(at + 1).map(|program| base_name(program.text));
                deletes |= runs.is_some_and(|runs| FIND_DELETERS.contains(&runs));
            }
            _ => {}
        }
        at += 1;
    }
    deletes.then(|| find_roots_of(args))
}

/// The roots of `find` with `args`, past its leading options: `.` when it names none.
fn find_roots_of<'w>(args: &[Word<'w>]) -> Vec<Word<'w>> {
    let mut at = 0;
    while let Some(word) = args.get(at) {
        match word.text {
            "-H" | "-L" | "-P" => at += 1,
            "-D" => at += 2,
            text if text.starts_with("-O") => at += 1,
            _ => break,
        }
    }
    let roots: Vec<Word<'w>> = args
        .get(at..)
        .unwrap_or_default()
        .iter()
        .take_while(|word| !word.text.starts_with('-') && !matches!(word.text, "(" | "!" | ","))
        .copied()
        .collect();
    if roots.is_empty() { vec![Word { text: ".", raw: "." }] } else { roots }
}

/// The sources of `mv` with `args`: every operand but the last, or all of them with
/// `-t`.
fn mv_sources<'w>(args: &[Word<'w>]) -> Vec<Word<'w>> {
    let mut operands = Vec::new();
    let mut target_given = false;
    let mut options = true;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let text = word.text;
        if options && text == "--" {
            options = false;
        } else if options && matches!(text, "-t" | "--target-directory" | "-S" | "--suffix") {
            target_given |= matches!(text, "-t" | "--target-directory");
            words.next();
        } else if options && text.starts_with("--") {
            target_given |= text.starts_with("--target-directory=");
        } else if options && is_option(text) {
            // NOTE: in a cluster such as `-ft`, the dir is the rest of the cluster or
            // the next word.
            if let Some(at) = text.find('t') {
                target_given = true;
                if at + 1 == text.len() {
                    words.next();
                }
            }
        } else {
            operands.push(*word);
        }
    }
    if !target_given {
        operands.pop();
    }
    operands
}

/// The options of `rsync` that take the next word as their value.
const RSYNC_VALUES: &[&str] = &[
    "-e",
    "--rsh",
    "-f",
    "--filter",
    "--exclude",
    "--include",
    "--exclude-from",
    "--include-from",
    "--files-from",
    "-B",
    "--block-size",
    "-T",
    "--temp-dir",
    "-M",
    "--remote-option",
    "--partial-dir",
    "--backup-dir",
    "--suffix",
    "--chmod",
    "--chown",
    "--usermap",
    "--groupmap",
    "--compare-dest",
    "--copy-dest",
    "--link-dest",
    "--password-file",
    "--log-file",
    "--out-format",
    "--timeout",
    "--contimeout",
    "--bwlimit",
    "--max-size",
    "--min-size",
    "--port",
    "--info",
    "--debug",
];

/// The destination of `rsync` with `args` when a `--del*` option deletes in it, and it
/// is a local path.
fn rsync_deletes<'w>(args: &[Word<'w>]) -> Option<Word<'w>> {
    let mut deletes = false;
    let mut operands = Vec::new();
    let mut options = true;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let text = word.text;
        if options && text == "--" {
            options = false;
        } else if options && RSYNC_VALUES.contains(&text) {
            words.next();
        } else if options && is_option(text) {
            deletes |= text.starts_with("--del");
        } else {
            operands.push(*word);
        }
    }
    let destination = operands.pop()?;
    (deletes && !is_remote(destination.text)).then_some(destination)
}

/// True for an operand of `rsync` on another host: `host:path`, `host::module` or an
/// `rsync://` URL.
fn is_remote(text: &str) -> bool {
    text.starts_with("rsync://")
        || text.split_once(':').is_some_and(|(before, _)| !before.contains('/'))
}

/// The operands of `chmod`, `chown`, `chgrp` or `setfacl` with `args` that are paths,
/// when it works recursively; `None` when it does not.
fn recursive_operands<'w>(program: &str, args: &[Word<'w>]) -> Option<Vec<Word<'w>>> {
    let (flags, values): (&str, &[&str]) = match program {
        "chmod" => ("cfvR", &[]),
        "setfacl" => (
            "RbkndLPtvh",
            &[
                "-m",
                "-M",
                "-x",
                "-X",
                "--modify",
                "--modify-file",
                "--remove",
                "--remove-file",
                "--set",
                "--set-file",
            ],
        ),
        _ => ("cfvhRHLP", &[]),
    };
    let mut recursive = false;
    let mut first_given = program == "setfacl";
    let mut operands = Vec::new();
    let mut options = true;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let text = word.text;
        if options && text == "--" {
            options = false;
        } else if options && values.contains(&text) {
            words.next();
        } else if options && text.starts_with("--") {
            recursive |= text == "--recursive";
            first_given |= text.starts_with("--reference");
        } else if options && is_option(text) && text[1..].chars().all(|c| flags.contains(c)) {
            recursive |= text.contains('R');
        } else if options && program == "setfacl" && is_option(text) {
            // NOTE: a cluster that ends with a letter that takes a value, such as `-Rm`.
            recursive |= text.contains('R');
            if text.ends_with(['m', 'M', 'x', 'X']) {
                words.next();
            }
        } else if !first_given {
            // NOTE: the mode, owner or group, also a mode such as `-w` for `chmod`.
            first_given = true;
        } else {
            operands.push(*word);
        }
    }
    recursive.then_some(operands)
}

/// The operands of `shred` with `args` when it removes the files (`-u`, `--remove`),
/// and every operand of `srm` and `wipe`; `None` for a `shred` that only overwrites.
fn wiped<'w>(program: &str, args: &[Word<'w>]) -> Option<Vec<Word<'w>>> {
    let mut removes = program != "shred";
    let mut operands = Vec::new();
    let mut options = true;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let text = word.text;
        if options && text == "--" {
            options = false;
        } else if options
            && program == "shred"
            && matches!(text, "-n" | "-s" | "--iterations" | "--size" | "--random-source")
        {
            words.next();
        } else if options && is_option(text) {
            removes |= text.starts_with("--remove")
                || (!text.starts_with("--") && text.contains(['u', 'r']));
        } else {
            operands.push(*word);
        }
    }
    removes.then_some(operands)
}

/// What `git clean` deletes.
struct GitClean<'w> {
    /// The dirs of `-C` and `--work-tree`, in order.
    dirs: Vec<Word<'w>>,
    /// The paths that limit the clean.
    pathspecs: Vec<Word<'w>>,
}

/// `git` with `args` when it is a `git clean` that deletes untracked dirs: `-f` and
/// `-d`, and no `-n`. The positions of git's own options come from the one table
/// ([`git::exact`]), so `git --config-env a.b=HOME -C ~ clean -fdx` is a clean in `~`
/// (DL28).
fn git_clean_args<'w>(args: &[Word<'w>]) -> Option<GitClean<'w>> {
    git_clean_at(args, 0)
}

/// [`git_clean_args`] for an alias of the line `depth` levels deep.
///
/// NOTE: when the line sets an alias for its subcommand with `-c alias.<name>=...`,
/// the guard reads the alias as git does: its value in place of the subcommand, after
/// the line's own options. An alias that efr cannot read (a shell command, a value
/// from `--config-env`, a quote) or that is too deep fails closed: it counts as a
/// clean of the whole work tree.
fn git_clean_at<'w>(args: &[Word<'w>], depth: usize) -> Option<GitClean<'w>> {
    let texts: Vec<&str> = args.iter().map(|word| word.text).collect();
    let parse = git::exact(&texts);
    let mut dirs = Vec::new();
    for global in parse.globals.iter().filter(|g| g.is("-C") || g.is("--work-tree")) {
        match global.option.map(|option| option.form) {
            Some(git::Form::Next) => dirs.extend(args.get(global.at + 1).copied()),
            Some(git::Form::Joined) => {
                if let Some((_, value)) = args[global.at].text.split_once('=') {
                    dirs.push(Word { text: value, raw: value });
                }
            }
            Some(git::Form::Flag) | None => {}
        }
    }
    let at = parse.sub?;
    if args[at].text != "clean" {
        let value = line_alias(args, &parse, args[at].text)?;
        return match value {
            Some(value) if depth < git::MAX_ALIAS_DEPTH => {
                let expanded: Vec<Word<'w>> = args[..at]
                    .iter()
                    .copied()
                    .chain(value.split_whitespace().map(|text| Word { text, raw: text }))
                    .chain(args[at + 1..].iter().copied())
                    .collect();
                git_clean_at(&expanded, depth + 1)
            }
            _ => Some(GitClean { dirs, pathspecs: Vec::new() }),
        };
    }
    let (mut force, mut directories, mut dry) = (false, false, false);
    let mut pathspecs = Vec::new();
    let mut options = true;
    let mut words = args.get(at + 1..).unwrap_or_default().iter();
    while let Some(word) = words.next() {
        let text = word.text;
        if options && text == "--" {
            options = false;
        } else if options && matches!(text, "-e" | "--exclude") {
            words.next();
        } else if options && text.starts_with("--") {
            force |= text == "--force";
            dry |= text == "--dry-run";
        } else if options && is_option(text) {
            force |= text.contains('f');
            directories |= text.contains('d');
            dry |= text.contains('n');
            if text.ends_with('e') {
                words.next();
            }
        } else {
            pathspecs.push(*word);
        }
    }
    (force && directories && !dry).then_some(GitClean { dirs, pathspecs })
}

/// The alias `name` that git's own options in `args` set (the last one wins, and git
/// compares the name without case): `Some(Some(value))` for a value that efr reads,
/// `Some(None)` for one that it cannot read, and `None` when the line sets no alias of
/// that name.
fn line_alias<'w>(args: &[Word<'w>], parse: &git::GitParse, name: &str) -> Option<Option<&'w str>> {
    let mut found = None;
    for global in parse.globals.iter().filter(|g| g.is("-c") || g.is("--config-env")) {
        let value = match global.option.map(|option| option.form) {
            Some(git::Form::Next) => args.get(global.at + 1).map(|word| word.text),
            Some(git::Form::Joined) => args[global.at].text.split_once('=').map(|(_, value)| value),
            Some(git::Form::Flag) | None => None,
        };
        let Some(value) = value else { continue };
        if let Some((alias, text)) = git::alias_setting(value, global.is("--config-env"))
            && alias.eq_ignore_ascii_case(name)
        {
            found = Some(text);
        }
    }
    found
}

/// The literal string of a shell with `args` and `-c`, or `None` when it runs a file
/// or its input.
fn shell_string<'w>(args: &[Word<'w>]) -> Option<Word<'w>> {
    let mut command = false;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let text = word.text;
        if text == "--" {
            return command.then(|| words.next().copied()).flatten();
        }
        if matches!(text, "-o" | "+o" | "-O" | "+O") {
            words.next();
        } else if (text.starts_with('-') || text.starts_with('+')) && text.len() > 1 {
            command |= text.starts_with('-') && text.contains('c');
        } else {
            return command.then_some(*word);
        }
    }
    None
}

#[cfg(test)]
mod tests;
