//! The project code that a line would run outside the sandbox.
//!
//! An exit that runs the whole line outside the sandbox runs its command with the
//! user's full rights. When that command runs project code, such as `sudo ./setup.sh`
//! or `sudo make install`, a sandboxed command may have written the code, and the
//! reviewer reads the line, never the file. So such an exit gets a note for each such
//! command and the strict review ([`Approver::User`]): its allow stands only with a
//! clear request of the user.
//!
//! A line with project code beside the exit never gets here: the one-command rule
//! ([`unsandboxed_line_problem`](super::unsandboxed_line_problem)) refuses a line that
//! would run outside the sandbox with more than one command that is no read-only
//! helper, so the model runs the project code in a call of its own, in the sandbox.
//!
//! The check reads the words of the line, the write roots of the envelope and the
//! facts of efrd about the program names of the line, never a file. Where efr cannot
//! tell what a command runs, it counts as project code: a wrong guess costs a stricter
//! review.

use std::collections::HashSet;

use super::envelope::{Envelope, resolve};
use super::scan::{self, OPAQUE, Segment};
use super::{ExitInput, ExitNeed, git_code, programs};
use crate::Approver;
use crate::path_class::normalize;

/// The most notes for one exit. One more note says how many it left out.
const MAX_NOTES: usize = 4;

/// How deep the check reads the code of `sh -c`, `eval` or `su -c` as a line of its
/// own: the line is depth 0, and its strings are read once. A string in such a string
/// is code that efr does not read.
const NESTED: u8 = 1;

/// The programs that copy a file that they name to another place. `rsync` is not one:
/// its `-t`, `-o` and `-g` take no value, so [`copy`] would misread its words.
const COPIERS: &[&str] = &["cp", "mv", "install", "ln"];

/// The dirs of the system where a file can run code later with the rights of root or
/// of every user: a unit, a cron table, a program on the `PATH`, a config that names a
/// program. A floor of the user's home is no such dir here: its exit has its own route,
/// and the reviewer reads what a copy or a link puts there (`ExitNeed::installs`).
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

/// The awk programs, whose first operand is inline code and whose file comes after
/// `-f`.
const AWKS: &[&str] = &["awk", "gawk", "mawk", "nawk"];

/// The options of a copy that take a value, so the next word is no file: the mode,
/// the owner, the group and the suffix of `install`, `cp`, `mv` and `ln`, and the
/// target dir.
const VALUE_OPTIONS: &[&str] = &[
    "-m",
    "-o",
    "-g",
    "-S",
    "-t",
    "--mode",
    "--owner",
    "--group",
    "--suffix",
    "--target-directory",
];

/// Where the code of an interpreter or of `source` comes from.
#[derive(Debug, PartialEq, Eq)]
enum Source<'a> {
    /// A file, or the module of `python3 -m`.
    File(&'a str),
    /// Inline code, which the line shows, as in `python3 -c '...'`.
    Inline,
    /// The input of the command: a pipe, a redirect or a here-document.
    Input,
}

/// Gives each exit of `needs` that runs the line of `input` outside the sandbox a note
/// for each project code that the line runs or installs, and [`Approver::User`]:
///
/// - a program by a path in a write root, also behind a wrapper such as `sudo`, or by a
///   relative path, as `./setup.sh` or `scripts/build`;
/// - a runner ([`programs::runs_project_files`]), such as `make`, `just`, `npm` or
///   `cargo`, which runs what the files of its dir say;
/// - a program on the `PATH` whose file, by a fact of efrd, lies in a real write root or
///   changed in this turn;
/// - an interpreter or `source` on a file by a relative path or in a write root, as
///   `python3 build.py` or `. ./env.sh`, `python3 -m`, and an interpreter that reads
///   its code from its input, as in `cat ./setup.sh | sudo bash`; inline code such as
///   `python3 -c '...'` is no file, and the reviewer reads it in the line;
/// - the code of `sh -c '...'`, `eval` and `su -c`, read as a line of its own once;
/// - a program or a script that only the shell can read, such as `"$TOOL"` or
///   `~/p/app/set*.sh`;
/// - a copy of a file by a relative path or in a write root into a system dir, as
///   `sudo cp app.service /etc/systemd/system/`, also with `-t` and after a `cd`.
///
/// A package publish is the exception: its notes name the code that it runs, and the
/// reviewer judges it only when a user message names it.
pub(super) fn apply(
    needs: &mut [ExitNeed],
    segments: &[Segment],
    input: &ExitInput<'_>,
    envelope: &Envelope<'_>,
) {
    let check = Check { input, envelope };
    let mut notes = Notes::default();
    for segment in segments {
        check.segment(segment, 0, &mut notes);
    }
    let notes = notes.capped();
    if notes.is_empty() {
        return;
    }
    for need in needs.iter_mut().filter(|need| need.runs_unsandboxed()) {
        need.raise_to_user(&notes);
    }
}

/// The notes of [`apply`], each once, at most [`MAX_NOTES`] and a count of the rest.
#[derive(Debug, Default)]
struct Notes {
    listed: Vec<String>,
    seen: HashSet<String>,
    more: usize,
}

impl Notes {
    fn add(&mut self, note: String) {
        if self.listed.len() == MAX_NOTES {
            self.more += usize::from(!self.listed.contains(&note) && self.seen.insert(note));
        } else if self.seen.insert(note.clone()) {
            self.listed.push(note);
        }
    }

    fn capped(mut self) -> Vec<String> {
        if self.more > 0 {
            self.listed.push(format!("the line runs or installs {} more such files", self.more));
        }
        self.listed
    }
}

/// The check of [`apply`].
struct Check<'a, 'b> {
    input: &'a ExitInput<'b>,
    envelope: &'a Envelope<'b>,
}

impl Check<'_, '_> {
    /// Adds a note for each project code that `segment` runs or installs, at `depth`.
    fn segment(&self, segment: &Segment, depth: u8, notes: &mut Notes) {
        for command in programs::commands(&segment.words) {
            self.command(command, depth, notes);
            let Some((target, sources)) = copy(command) else { continue };
            // NOTE: after a `cd`, or with no known start dir, efr does not know the dir
            // of a relative target, so it counts as a system dir.
            let unknown = (segment.after_cd || self.input.command_dir.is_none())
                && !target.starts_with(['/', '~']);
            if unknown || self.installs_at(target) {
                for source in sources {
                    if unreadable(source) {
                        notes.add("the line installs a file that efr cannot read".to_owned());
                    } else if let Some(shown) = self.project_file(source) {
                        notes.add(format!("the line installs {shown}"));
                    }
                }
            }
        }
    }

    /// Adds a note for the project code that `command`, one command, runs.
    fn command(&self, command: &[String], depth: u8, notes: &mut Notes) {
        let Some(word) = command.first() else { return };
        if let Some(note) = self.program(word) {
            notes.add(note);
        }
        if let Some(code) = programs::inline_code(command) {
            if depth < NESTED {
                for segment in scan::scan(&code) {
                    self.segment(&segment, depth + 1, notes);
                }
            } else {
                notes.add("efr cannot read the code that the line runs".to_owned());
            }
            return;
        }
        let runner = programs::runs_project_files(command);
        // NOTE: only a runner or an interpreter can publish. A publish names its own
        // code in its notes, and its own route judges it.
        if (!runner && !programs::runs_file(command)) || self.publishes(command) {
            return;
        }
        let program = programs::base_name(word);
        // NOTE: a runner such as `bun` or `deno` is an interpreter too; its note says
        // enough, and its first word is a subcommand, no file.
        if runner {
            notes.add(format!("the line runs {program}, which runs what the files of its dir say"));
            return;
        }
        match source(command) {
            Source::Inline => {}
            Source::Input => notes.add(format!("the line runs code from the input of {program}")),
            Source::File(file) if unreadable(file) => {
                notes.add(format!("efr cannot read which file {program} runs"));
            }
            Source::File(file) => {
                if let Some(shown) = self.project_file(file) {
                    notes.add(format!("the line runs {shown} with {program}"));
                }
            }
        }
    }

    /// The note for the program word `word` when it runs project code: a word that only
    /// the shell can read, a relative path, a path in a write root, or a program whose
    /// file a fact of efrd places in a write root or shows changed in this turn.
    fn program(&self, word: &str) -> Option<String> {
        if unreadable(word) {
            return Some("efr cannot read which program the line runs".to_owned());
        }
        if !word.contains('/') && !word.starts_with('~') {
            return self.by_facts(word);
        }
        self.project_file(word)
            .map(|shown| format!("the line runs {shown}"))
            .or_else(|| self.by_facts(word))
    }

    /// The note for the program word `word` by the facts of efrd, which name its file
    /// and whether it changed in this turn. No fact, no note.
    fn by_facts(&self, word: &str) -> Option<String> {
        let facts = self.input.facts?;
        let (_, path, changed) = facts.programs.iter().find(|(name, _, _)| name == word)?;
        let path = path.as_deref()?;
        let shown = git_code::tilde(path, self.input.locations.home());
        if normalize(path).is_some_and(|path| self.envelope.in_real_root(&path)) {
            Some(format!("the line runs {shown}, which a sandboxed command can write"))
        } else if *changed {
            Some(format!("the line runs {shown}, which changed in this turn"))
        } else {
            None
        }
    }

    /// `word` as the line shows it when it names a file that may be project code: a
    /// relative path, which may lie in the project after any `cd`, a path in a write
    /// root that a sandboxed command can write, or a path that only the shell can read.
    /// `None` for any other path, and for a word that holds an expansion, whose note
    /// each caller words itself ([`unreadable`]).
    fn project_file(&self, word: &str) -> Option<String> {
        if word.is_empty() || word.starts_with('-') || unreadable(word) {
            return None;
        }
        // NOTE: a brace expansion, as in `/usr/{a,../home/u/p/app/x}`, can lead into a
        // write root from a path that reads as one outside it.
        if !word.starts_with(['/', '~']) || word.contains('{') {
            return Some(word.to_owned());
        }
        // NOTE: a pattern or a form such as `~+/x` only the shell can read: it may name
        // a file of a write root.
        let Some(path) = resolve(word, None, self.input.locations) else {
            return Some(word.to_owned());
        };
        let envelope = self.envelope;
        let writable = envelope.in_real_root(&path)
            && !envelope.is_floor(&path)
            && !envelope.is_secret(&path)
            && !envelope.is_masked(&path);
        writable.then(|| git_code::tilde(&path, self.input.locations.home()))
    }

    /// True when `word`, the target of a copy, lies in a system dir ([`SYSTEM_DIRS`]).
    fn installs_at(&self, word: &str) -> bool {
        let Some(path) = resolve(word, self.input.command_dir, self.input.locations) else {
            return false;
        };
        !path.starts_with(self.input.locations.home())
            && !path.starts_with("/var/tmp")
            && SYSTEM_DIRS.iter().any(|dir| path.starts_with(dir))
    }

    /// True when `command` is a package publish ([`Approver::Guarded`]): it ships the
    /// code of the project by its nature, and the reviewer judges it only when a user
    /// message names it.
    fn publishes(&self, command: &[String]) -> bool {
        programs::classify(command, self.input.support, self.input.hosts, &[])
            .iter()
            .any(|found| found.approver == Approver::Guarded)
    }
}

/// True when `word` holds an expansion, whose text only the shell knows.
fn unreadable(word: &str) -> bool {
    word.contains([OPAQUE, '$'])
}

/// The target and the sources of `command` when it is a copy ([`COPIERS`]): the value
/// of `-t` or `--target-directory`, else the last operand. The values of
/// [`VALUE_OPTIONS`] are no file.
fn copy(command: &[String]) -> Option<(&str, Vec<&str>)> {
    let (first, args) = command.split_first()?;
    if !COPIERS.contains(&programs::base_name(first)) {
        return None;
    }
    let mut target: Option<&str> = None;
    let mut operands: Vec<&str> = Vec::new();
    let mut args = args.iter();
    while let Some(word) = args.next() {
        let attached = word
            .strip_prefix("--target-directory=")
            .or_else(|| word.strip_prefix("-t").filter(|dir| !dir.is_empty()));
        if let Some(dir) = attached {
            target = Some(dir);
        } else if word == "-t" || word == "--target-directory" {
            target = args.next().map(String::as_str);
        } else if VALUE_OPTIONS.contains(&word.as_str()) {
            args.next();
        } else if !word.starts_with('-') {
            operands.push(word);
        }
    }
    let target = match target {
        Some(target) => target,
        None => operands.pop()?,
    };
    Some((target, operands))
}

/// Where the code of `command`, an interpreter or `source`, comes from: the first
/// operand, the module of `python3 -m`, the file of `awk -f` (or gawk's `-E`), the
/// inline code of an option such as `python3 -c`, or the input when no operand names
/// a file.
fn source(command: &[String]) -> Source<'_> {
    let Some((first, args)) = command.split_first() else { return Source::Input };
    let name = programs::base_name(first).to_lowercase();
    let name = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    let mut args = args.iter();
    if AWKS.contains(&name) {
        while let Some(word) = args.next() {
            if matches!(word.as_str(), "-f" | "--file" | "-E" | "--exec") {
                return args.next().map_or(Source::Input, |file| Source::File(file));
            }
            let file = word.strip_prefix("--file=").or_else(|| word.strip_prefix("-f"));
            if let Some(file) = file.filter(|file| !file.is_empty()) {
                return Source::File(file);
            }
        }
        // NOTE: the first operand of awk is its program, in the line.
        return Source::Inline;
    }
    while let Some(word) = args.next() {
        if inline_option(name, word) {
            return Source::Inline;
        }
        if ((name == "python" || name == "pypy") && word == "-m") || word == "--" {
            return args.next().map_or(Source::Input, |file| Source::File(file));
        }
        // NOTE: a shell option that takes a value, as in `bash -o errexit ./x.sh`.
        if matches!(word.as_str(), "-o" | "+o" | "-O" | "+O") {
            args.next();
            continue;
        }
        if word == "-" {
            return Source::Input;
        }
        if !word.starts_with(['-', '+']) {
            return Source::File(word);
        }
    }
    Source::Input
}

/// True when `word` is an option of the interpreter `name` (its base name without a
/// version) whose value is inline code. A shell's `-c` is no such option here:
/// [`programs::inline_code`] reads that code first.
fn inline_option(name: &str, word: &str) -> bool {
    match name {
        "python" | "pypy" => word == "-c",
        "node" | "nodejs" => matches!(word, "-e" | "--eval" | "-p" | "--print"),
        "perl" => matches!(word, "-e" | "-E"),
        "ruby" | "lua" | "luajit" | "julia" | "rscript" | "osascript" => word == "-e",
        "php" => word == "-r",
        "pwsh" => word.eq_ignore_ascii_case("-c") || word.eq_ignore_ascii_case("-command"),
        _ => false,
    }
}
