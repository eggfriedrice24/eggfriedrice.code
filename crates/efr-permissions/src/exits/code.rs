//! The code of the sandbox that a line would run or install outside the sandbox.
//!
//! The sandbox lets a command write every file of a write root. An exit that runs the
//! whole line outside the sandbox can then run such a file with the user's full rights,
//! and no reviewer reads a file. So efr finds what a line names that can be such code,
//! and an exit with such code stays the user's own question
//! ([`ExitNeed::unread_code`](super::ExitNeed::unread_code)).
//!
//! One walk over the line finds the items ([`CodeItem`]). The fact request reads the
//! paths and the program words of the walk ([`code_asks`]), so efrd says which files
//! exist, and [`sandbox_code`] reads the same walk with those facts and the real
//! envelope. The walk is the same on both sides, so each path that the check reads has
//! a fact.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::ptr;

use super::critical::DirWalk;
use super::envelope::{Envelope, resolve};
use super::facts::Facts;
use super::scan::{self, OPAQUE, Segment};
use super::{ExitInput, git_code, persist, programs};
use crate::path_class::normalize;
use crate::{Approver, Locations};

/// How a note ends for a file in a real write root.
const CAN_WRITE: &str = "which a sandboxed command can write";

/// The most notes of [`sandbox_code`]. One more note says how many it left out.
const MAX_NOTES: usize = 4;

/// The most dirs of one command that [`RunDirs`] gives. A line with more `cd`s than
/// this runs its later commands in a dir that efr does not know.
const MAX_RUN_DIRS: usize = 8;

/// How deep the walk reads the code of `sh -c`, `eval` or `su -c` as a line of its
/// own: the line is depth 0, and its strings are read once. A string in such a string
/// is code that efr does not read.
const NESTED: u8 = 1;

/// The characters that make a word without quotes a pattern of the shell.
const PATTERN: &[char] = &['*', '?', '[', '{'];

/// The programs that put a file in another place.
const COPIERS: &[&str] = &["cp", "mv", "install", "ln", "rsync", "tee"];

/// The dirs of the system where a file can run code later with the rights of root or
/// of every user: a unit, a cron table, a program on the `PATH`, a config that names a
/// program. A line that copies a file there installs it.
const SYSTEM_DIRS: &[&str] = &[
    "/etc",
    "/usr",
    "/opt",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/boot",
    "/srv",
    "/root",
    "/var",
    "/run/systemd",
];

/// One thing that a line names and that can be code of the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CodeItem {
    /// A program word, as the line wrote it: efrd resolves it where the line starts
    /// and says whether its file changed in this turn.
    Name(String),
    /// A program word that names a file by a path: each place where that file can be,
    /// one for each dir where the command may run.
    Program(Vec<PathBuf>),
    /// A file that the line names for an interpreter, or in a line that also names a
    /// place where a file runs code later.
    File(PathBuf),
    /// A pattern of the shell in the place of such a file, and the dir that it reads.
    Pattern { word: String, dir: PathBuf },
    /// A runner: its command, the dirs where it may run, and the paths that the line
    /// gives it.
    Runner { command: Vec<String>, dirs: Vec<Option<PathBuf>>, given: Vec<PathBuf> },
    /// A program that efr cannot find: a word that only the shell can read, a relative
    /// word in a dir that efr does not know, or code below [`NESTED`].
    Hidden,
}

/// What the fact request asks efrd for a line, besides what its own split finds.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CodeAsks {
    /// The files that the line can run or install, each once: efrd says which exist.
    pub(crate) paths: Vec<PathBuf>,
    /// The program words of the line and of the code in its `sh -c`, `eval` and
    /// `su -c` strings, each once.
    pub(crate) programs: Vec<String>,
}

/// What `line`, which starts in `command_dir`, can run or install, for
/// [`FactRequest::targets`](super::FactRequest::targets) and
/// [`FactRequest::programs`](super::FactRequest::programs):
///
/// - each program word, also one in the code of `sh -c '...'`, `eval` or `su -c`,
///   which the walk reads as a line of its own;
/// - each place of a program word that names a file by a path, such as `./setup.sh`
///   or `~/p/app/setup.sh`, in each dir where its command may run ([`RunDirs`]);
/// - each operand of an interpreter and of `source` or `.`
///   ([`programs::runs_file`]): a script, or a file that the script loads;
/// - each operand of a persistence form such as `crontab <file>`, and each operand of
///   each command of a line that names a place where a file runs code later: a floor
///   or a file in a floor dir, as an operand or as the target of a redirect, and a
///   system dir ([`SYSTEM_DIRS`]) as an operand of a copy or as the target of a
///   redirect. This is what such a line can install there, whatever its command is.
///
/// An operand is a word that is no option, the value that stands at a short option
/// (`-fproject.awk`) or the value after the `=` of a long one. A word that only the
/// shell can read names no path. No path is left out here: the dir of the line stands
/// in for the turn's project, so only [`sandbox_code`] knows the floors and masks.
pub(crate) fn code_asks(line: &str, command_dir: Option<&Path>, locations: &Locations) -> CodeAsks {
    let mut asks = CodeAsks::default();
    let mut paths: HashSet<PathBuf> = HashSet::new();
    let mut names: HashSet<String> = HashSet::new();
    for item in code_items(line, command_dir, locations) {
        match item {
            CodeItem::Name(word) => {
                if names.insert(word.clone()) {
                    asks.programs.push(word);
                }
            }
            CodeItem::Program(places) => {
                for path in places {
                    if paths.insert(path.clone()) {
                        asks.paths.push(path);
                    }
                }
            }
            CodeItem::File(path) => {
                if paths.insert(path.clone()) {
                    asks.paths.push(path);
                }
            }
            CodeItem::Pattern { .. } | CodeItem::Runner { .. } | CodeItem::Hidden => {}
        }
    }
    asks
}

/// The code that the line of `input` would run or install outside the sandbox, and that
/// a sandboxed command wrote or can write: one note for the question about each such
/// file or place ([`ExitNeed::unread_code`](super::ExitNeed::unread_code), which is true
/// when a note comes). No reviewer reads a file, so an exit with such code stays the
/// user's own question.
///
/// Most notes come from a fact of efrd about a file that the line names:
///
/// - a program word, also one behind a wrapper such as `sudo` or `env`, whose file
///   lies in a real write root or changed in this turn;
/// - a program word that names a file by a path, when that file is there and a
///   sandboxed command can write it, in each dir where a `cd` of the line may lead:
///   `cd scripts && sudo ./setup.sh`, `sudo ~/p/app/setup.sh`. When the file is in
///   none of its places, the line makes it first, as in `gcc x.c -o x && ./x`, and a
///   place in a real write root gets the note too;
/// - a file that the line names for an interpreter or for `source`: `sh setup.sh`,
///   `bash -e ./deploy.sh`, `python3 build.py -c x`, `awk -fproject.awk`;
/// - a file that a line names while it also names a place where a file runs code
///   later, whatever the command: `cp`, `mv`, `install`, `cat x > unit`,
///   `crontab ./project.cron`, `sudo cp app.service /etc/systemd/system/`.
///
/// The code of `sh -c '...'`, `bash -lc '...'`, `eval` and `su -c` is a line of its
/// own, so `sudo sh -c './setup.sh --prod'` gets the note of its program. Inline code
/// of another interpreter, as in `python3 -c '...'`, names no file and gets no note: a
/// reviewer reads it in the line.
///
/// Some notes come from the place of a word, with no fact of a file:
///
/// - a runner ([`programs::runs_project_files`]), such as `make`, `just`, `npm` or
///   `cargo`, that may run in a real write root, or that the line gives a path there
///   (`make -C dir`): it runs what the files of that dir say, and the line names none
///   of them. A package publish is the exception ([`publishes`]);
/// - a pattern of the shell in the place of a file, such as `cp contrib/*.service
///   ~/.config/systemd/user/`, that reads a dir in a real write root;
/// - a program that efr cannot find: a word that only the shell can read, such as
///   `$PWD/deploy.sh` or `./set*.sh`, a relative one in a dir that efr does not know,
///   a runner in such a dir, and code in the code of `sh -c`. efr then cannot tell
///   what the line runs with the user's rights.
///
/// Without facts (no sandbox in efrd) there is nothing to say. A cache overlay and the
/// private tmp are no real write root, and a floor, a secret and a masked path are
/// files that no sandboxed command can write.
pub(super) fn sandbox_code(input: &ExitInput<'_>, envelope: &Envelope<'_>) -> Vec<String> {
    let Some(call_facts) = input.facts else {
        return Vec::new();
    };
    let facts = Facts::new(call_facts);
    let home = input.locations.home();
    let shown = |path: &Path| git_code::tilde(path, home);
    let writable = |path: &Path| {
        envelope.in_real_root(path)
            && !envelope.is_floor(path)
            && !envelope.is_secret(path)
            && !envelope.is_masked(path)
    };
    let there = |path: &Path| matches!(facts.target(path), Some(Some(_)));
    let mut known: HashMap<&str, (&Path, bool)> = HashMap::new();
    for (word, path, changed) in &call_facts.programs {
        if let Some(path) = path {
            known.entry(word.as_str()).or_insert((path.as_path(), *changed));
        }
    }
    let mut notes = Notes::default();
    for item in code_items(input.line, input.command_dir, input.locations) {
        match item {
            CodeItem::Name(word) => {
                let Some((path, changed)) = known.get(word.as_str()).copied() else {
                    continue;
                };
                if normalize(path).is_some_and(|path| envelope.in_real_root(&path)) {
                    notes.add(format!("the line runs {}, {CAN_WRITE}", shown(path)));
                } else if changed {
                    notes.add(format!("the line runs {}, which changed in this turn", shown(path)));
                }
            }
            CodeItem::Program(places) => {
                for path in places.iter().filter(|path| there(path) && writable(path)) {
                    notes.add(format!("the line runs {}, {CAN_WRITE}", shown(path)));
                }
                // NOTE: a fact for each place says that the file is in none of them,
                // so the line itself makes the file that it runs.
                let made = places.iter().all(|path| facts.target(path) == Some(None));
                if let Some(path) = places.iter().find(|path| made && writable(path)) {
                    notes.add(format!(
                        "the line runs {}, which is not there yet and {CAN_WRITE}",
                        shown(path)
                    ));
                }
            }
            CodeItem::File(path) => {
                // NOTE: the fact says that the file is there; a word that is no file,
                // such as the code after `python3 -c`, has none.
                if there(&path) && writable(&path) {
                    notes.add(format!("the line runs or installs {}, {CAN_WRITE}", shown(&path)));
                }
            }
            CodeItem::Pattern { word, dir } => {
                if envelope.in_real_root(&dir) {
                    notes.add(format!(
                        "the line names files by the pattern {word} in {}, where a sandboxed \
                         command can write",
                        shown(&dir)
                    ));
                }
            }
            CodeItem::Runner { command, dirs, given } => {
                if publishes(&command, input) {
                    continue;
                }
                let program = command.first().map_or("", |word| programs::base_name(word));
                let mut in_root = false;
                for dir in &dirs {
                    match dir {
                        Some(dir) if envelope.in_real_root(dir) => {
                            in_root = true;
                            notes.add(format!(
                                "the line runs {program} in {}, where a sandboxed command can \
                                 write the files that {program} runs",
                                shown(dir)
                            ));
                        }
                        Some(_) => {}
                        None => {
                            notes.add(format!("efr cannot read where the line runs {program}"));
                        }
                    }
                }
                // NOTE: a dir or a file that the line gives the runner, such as `make -C
                // dir` or `cargo run --manifest-path dir/Cargo.toml`. A word that is no
                // path resolves below the dir of the command, which has its note above.
                for path in given.iter().filter(|path| !in_root && envelope.in_real_root(path)) {
                    notes.add(format!("the line runs {program} with {}, {CAN_WRITE}", shown(path)));
                }
            }
            CodeItem::Hidden => {
                notes.add("efr cannot read which program the line runs".to_owned());
            }
        }
    }
    notes.capped()
}

/// The notes of [`sandbox_code`], each once, in the order in which they came.
#[derive(Debug, Default)]
struct Notes {
    listed: Vec<String>,
    seen: HashSet<String>,
}

impl Notes {
    fn add(&mut self, note: String) {
        if self.seen.insert(note.clone()) {
            self.listed.push(note);
        }
    }

    /// At most [`MAX_NOTES`] notes and the count of the rest, so a line of many files
    /// does not fill the question.
    fn capped(mut self) -> Vec<String> {
        if self.listed.len() > MAX_NOTES {
            let more = self.listed.len() - MAX_NOTES;
            self.listed.truncate(MAX_NOTES);
            self.listed.push(format!("the line runs or installs {more} more such files"));
        }
        self.listed
    }
}

/// True when `command` is a package publish ([`Approver::Guarded`]). It is a runner
/// that gets no note of [`sandbox_code`]: it ships the code of the project by its
/// nature, its own notes say what it runs, and the reviewer judges it only when a user
/// message names it.
fn publishes(command: &[String], input: &ExitInput<'_>) -> bool {
    programs::classify(command, input.support, input.hosts, &[])
        .iter()
        .any(|found| found.approver == Approver::Guarded)
}

/// The items of `line`, which starts in `command_dir`, in the order of the line.
fn code_items(line: &str, command_dir: Option<&Path>, locations: &Locations) -> Vec<CodeItem> {
    let start = command_dir.and_then(normalize);
    // NOTE: the turn's project is not known to the fact request. The registered
    // project that holds the dir of the line stands in for it, or that dir itself, so
    // the walk finds the floors of the project also from a dir below its root.
    let root = start
        .as_deref()
        .map(|dir| dir.ancestors().find(|above| locations.is_project_root(above)).unwrap_or(dir));
    let mut walk =
        CodeWalk { locations, floors: Envelope::new(locations, root, None), items: Vec::new() };
    walk.line(line, std::slice::from_ref(&start), 0);
    walk.items
}

/// The walk of [`code_items`].
struct CodeWalk<'a> {
    locations: &'a Locations,
    /// The envelope that says which paths are floors: see [`code_items`].
    floors: Envelope<'a>,
    items: Vec<CodeItem>,
}

impl CodeWalk<'_> {
    /// Reads `line`, which starts in one of `starts`, at `depth`.
    fn line(&mut self, line: &str, starts: &[Option<PathBuf>], depth: u8) {
        let segments = scan::scan(line);
        for start in starts.iter().take(MAX_RUN_DIRS) {
            let mut run_dirs = RunDirs::new(start.clone());
            let installs = segments.iter().any(|segment| {
                let dirs = run_dirs.of(segment, self.locations);
                self.installs(segment, dirs)
            });
            let mut run_dirs = RunDirs::new(start.clone());
            for segment in &segments {
                let dirs = run_dirs.of(segment, self.locations);
                for command in programs::commands(&segment.words) {
                    self.command(segment, command, dirs, installs, depth);
                }
            }
        }
    }

    /// True when `segment`, which may run in `dirs`, names a place where a file runs
    /// code later: a floor or a file in a floor dir, as an operand or as the target of
    /// a redirect, or a system dir as an operand of a copy or as the target of a
    /// redirect.
    fn installs(&self, segment: &Segment, dirs: &[Option<PathBuf>]) -> bool {
        let home = self.locations.home();
        let floor =
            |path: &Path| self.floors.is_floor(path) || self.floors.is_floor(&path.join("x"));
        let system = |path: &Path| {
            !path.starts_with(home)
                && !path.starts_with("/var/tmp")
                && SYSTEM_DIRS.iter().any(|dir| path.starts_with(dir))
        };
        let named = |word: &str, copied: bool| {
            dirs.iter()
                .filter_map(|dir| resolve(word, dir.as_deref(), self.locations))
                .any(|path| floor(&path) || (copied && system(&path)))
        };
        segment.redirects.iter().any(|redirect| named(&redirect.target, true))
            || programs::commands(&segment.words).into_iter().any(|command| {
                let copied = command
                    .first()
                    .is_some_and(|word| COPIERS.contains(&programs::base_name(word)));
                command
                    .iter()
                    .skip(1)
                    .flat_map(|word| programs::operand_forms(word))
                    .any(|word| named(word, copied))
            })
    }

    /// Reads one command of `segment`, which may run in `dirs`. `installs` is true
    /// when its line names a place where a file runs code later.
    fn command(
        &mut self,
        segment: &Segment,
        command: &[String],
        dirs: &[Option<PathBuf>],
        installs: bool,
        depth: u8,
    ) {
        let Some(word) = command.first() else { return };
        self.program(word, dirs);
        if let Some(code) = programs::inline_code(command) {
            // NOTE: the other words of the command are the arguments of that code.
            if depth < NESTED {
                self.line(&code, dirs, depth + 1);
            } else {
                self.items.push(CodeItem::Hidden);
            }
            return;
        }
        // NOTE: `systemctl enable app.service` finds the unit by its name in the unit
        // dirs, never in the dir of the line; only a path names a file there.
        let by_name = programs::base_name(word) == "systemctl";
        let own_files =
            by_name || programs::runs_file(command) || !persist::covers(command).is_empty();
        let names_files = installs || own_files;
        // NOTE: a pattern counts where the command itself takes files: an interpreter,
        // a persistence form, and in a line that installs, a copy or a command with a
        // redirect. `ls *.txt` beside an install reads names only.
        let copies = COPIERS.contains(&programs::base_name(word)) || !segment.redirects.is_empty();
        let takes_patterns = own_files || (installs && copies);
        let runner = programs::runs_project_files(command);
        let raw = raw_words(segment, command);
        let mut given: Vec<PathBuf> = Vec::new();
        for (at, operand) in command.iter().enumerate().skip(1) {
            for form in programs::operand_forms(operand) {
                if by_name && !form.contains('/') {
                    continue;
                }
                let quoted = raw.get(at).is_some_and(|raw| raw.contains(['\'', '"', '\\']));
                if form.contains(PATTERN) && !quoted {
                    if takes_patterns {
                        self.pattern(form, dirs);
                    }
                    continue;
                }
                for dir in dirs {
                    let Some(path) = resolve(form, dir.as_deref(), self.locations) else {
                        continue;
                    };
                    if runner {
                        given.push(path.clone());
                    }
                    if names_files {
                        self.items.push(CodeItem::File(path));
                    }
                }
            }
        }
        if runner {
            self.items.push(CodeItem::Runner {
                command: command.to_vec(),
                dirs: dirs.to_vec(),
                given,
            });
        }
    }

    /// The items of the program word `word` of a command that may run in `dirs`.
    fn program(&mut self, word: &str, dirs: &[Option<PathBuf>]) {
        if word.contains([OPAQUE, '$']) {
            self.items.push(CodeItem::Hidden);
            return;
        }
        self.items.push(CodeItem::Name(word.to_owned()));
        // NOTE: a bare name comes from the `PATH`, and its fact says all about it.
        if !word.contains('/') && !word.starts_with('~') {
            return;
        }
        let mut places: Vec<PathBuf> = Vec::new();
        for dir in dirs.iter().filter(|_| !word.contains(PATTERN)) {
            if let Some(path) = resolve(word, dir.as_deref(), self.locations)
                && !places.contains(&path)
            {
                places.push(path);
            }
        }
        // NOTE: no place for a pattern or a form such as `~+/x`, which only the shell
        // can read; a relative word in a dir that efr does not know has a place more.
        let relative = !word.starts_with(['/', '~']);
        if places.is_empty() || (relative && dirs.iter().any(Option::is_none)) {
            self.items.push(CodeItem::Hidden);
        }
        if !places.is_empty() {
            self.items.push(CodeItem::Program(places));
        }
    }

    /// The items of `word`, a pattern of the shell in the place of a file: the dir
    /// that the pattern reads, in each of `dirs` that efr knows.
    fn pattern(&mut self, word: &str, dirs: &[Option<PathBuf>]) {
        let fixed = word.split(PATTERN).next().unwrap_or_default();
        let dir_word = match fixed.rfind('/') {
            Some(0) => "/",
            Some(end) => &fixed[..end],
            None if fixed == "~" => "~",
            None => ".",
        };
        for dir in dirs {
            if let Some(dir) = resolve(dir_word, dir.as_deref(), self.locations) {
                self.items.push(CodeItem::Pattern { word: word.to_owned(), dir });
            }
        }
    }
}

/// The words of `command`, a slice of `segment`'s words, as the line wrote them, from
/// its program on.
fn raw_words<'s>(segment: &'s Segment, command: &[String]) -> &'s [String] {
    let offset = command
        .first()
        .and_then(|first| segment.words.iter().position(|word| ptr::eq(word, first)))
        .unwrap_or(0);
    segment.raw.get(offset..).unwrap_or_default()
}

/// The dirs where each segment of one line may run: the dir where the line starts and
/// the dir after each `cd` before the segment, because a `cd` can fail, run in a
/// subshell or in a branch that does not run. `None` stands for a dir that efr does
/// not know: after a `cd` that goes where efr cannot read (`cd "$X"`). After `cd -` or
/// `popd`, and for a line with more than [`MAX_RUN_DIRS`] dirs, the dirs are the one
/// where the line starts and `None`. Ask for the segments in their order.
struct RunDirs {
    walk: DirWalk,
    /// The dirs of a segment whose `cd`s efr cannot follow.
    unknown: Vec<Option<PathBuf>>,
}

impl RunDirs {
    fn new(start: Option<PathBuf>) -> Self {
        let unknown = match &start {
            Some(_) => vec![start.clone(), None],
            None => vec![None],
        };
        RunDirs { walk: DirWalk::new(start), unknown }
    }

    fn of(&mut self, segment: &Segment, locations: &Locations) -> &[Option<PathBuf>] {
        let Some(cds) = segment.cds.as_deref() else {
            return &self.unknown;
        };
        let dirs = self.walk.after(cds, locations);
        if dirs.len() > MAX_RUN_DIRS { &self.unknown } else { dirs }
    }
}
