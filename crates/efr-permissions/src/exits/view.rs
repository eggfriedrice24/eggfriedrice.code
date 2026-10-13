//! The network view rule (phase D1, efr's auto spec 8.8): which lines run in the
//! sandbox with `Grant::NetView`, in the host's network namespace with no network
//! socket.
//!
//! A line gets the view when the machine runs NetView calls, at least one simple command
//! is a view command, and every simple command is a view command or a fixed text helper.
//! View commands are `ss` without the forms that kill or diagnose sockets or name another
//! namespace, the read forms of `ip`, `nstat` without its daemon mode, and a reader
//! (`cat`, `head`, `tail`, `grep`, `wc`) of a file below `/proc/net`.
//!
//! NOTE: the rule is a fit for `ss`-like work, never a security layer. A NetView call
//! can still run any code: a function or an alias named `ss` from the user's shell, a
//! zsh module, a bug in a helper. The seccomp profile, the Landlock rights and scopes,
//! and the refusal of `listen` carry the whole load. The narrow lists keep lines that
//! start local servers, and interpreters, out of the host's namespace.

use std::path::{Component, Path};

use super::programs;
use super::scan::{OPAQUE, Segment, is_assignment};
use crate::path_class::normalize;
use crate::policy::read_only_commands;
use efr_protocol::Grant;

use crate::{AutoSupport, CommandPattern};

/// The wrappers that a view command may run behind: `watch` repeats it, `timeout` ends
/// it. Any other wrapper, or a program that runs other programs, leaves the rule.
const VIEW_WRAPPERS: &[&str] = &["watch", "timeout"];

/// The programs that are a view command when an operand lies below a network dir of
/// `/proc` ([`below_proc_net`]), each matched by its row of the cautious read-only table.
const READERS: &[&str] = &["cat", "head", "tail", "grep", "wc"];

/// The text helpers that a view line may hold. Each must match its row of the cautious
/// read-only table where one exists, so its forbid list applies (`sort
/// --compress-program` runs a program). No interpreter and no program that runs another
/// one is here.
const HELPERS: &[&str] = &[
    "grep", "head", "tail", "wc", "sort", "uniq", "cut", "tr", "column", "cat", "echo", "printf",
    "date", "sleep", ":", "true", "tee",
];

/// The reserved words that close a compound command, such as `done` in
/// `while :; do ss -ti; sleep 5; done`. Alone in a simple command they run nothing.
const CLOSERS: &[&str] = &["done", "fi", "esac", "end", "}"];

/// What turns `ss` from a view into a change, or moves it to another namespace: `-K`
/// kills sockets, `-D` writes a diagnostic dump, `-N` enters a named namespace.
const SS_FORBID: &[&str] = &["-K", "--kill", "-D", "--diag", "-N", "--net"];

/// `nstat -d` (`--scan`) runs as a daemon that listens on a socket.
const NSTAT_FORBID: &[&str] = &["-d", "--scan"];

/// The objects of `ip` whose read forms the rule takes, with their short names.
const IP_OBJECTS: &[&str] = &[
    "addr",
    "address",
    "a",
    "link",
    "l",
    "route",
    "r",
    "ro",
    "neigh",
    "n",
    "rule",
    "ru",
    "maddr",
    "m",
    "addrlabel",
    "netconf",
];

/// The verbs of `ip` that only read. An object with no verb shows it too.
const IP_READ_VERBS: &[&str] = &["show", "list", "ls", "lst", "get"];

/// The options of `ip` that run other commands (`-batch`), enter another namespace
/// (`-netns`) or run a command in every namespace (`-all`). `ip` takes any prefix of an
/// option, so `-n` and `-b` are these too.
const IP_REFUSED: &[&str] = &["-batch", "-netns", "-all"];

/// The options of `ip` that take the next word as their value, as `ip` reads a prefix
/// of each. `-r` alone is `-resolve`, so `-rcvbuf` needs at least `-rc`.
const IP_VALUES: &[(&str, usize)] = &[("-loops", 2), ("-family", 2), ("-rcvbuf", 3)];

/// The characters that a shell reads as more than a word. `watch` joins its words with
/// spaces and gives them to a shell, so a word behind it must hold none of them.
const SH_SPECIAL: &[char] = &[
    ';', '&', '|', '<', '>', '(', ')', '$', '`', '\\', '"', '\'', '*', '?', '[', ']', '{', '}',
    '~', '#', '!', '\n', OPAQUE,
];

/// What one simple command is for the rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// It shows the host's network.
    View,
    /// It only moves text, or runs nothing.
    Helper,
}

/// The grants of the views that the line of `segments` gets: `Grant::NetView` when
/// [`net_view`] holds. A view is never an exit.
pub(super) fn grants(segments: &[Segment], support: &AutoSupport) -> Vec<Grant> {
    if net_view(segments, support) { vec![Grant::NetView] } else { Vec::new() }
}

/// True when the line of `segments` gets the host's network view: the machine runs
/// NetView calls (`support.netview`), and every simple command is a view command or a
/// helper, with at least one view command.
pub(super) fn net_view(segments: &[Segment], support: &AutoSupport) -> bool {
    if !support.netview {
        return false;
    }
    let rows = read_only_commands();
    let mut view = false;
    for segment in segments {
        match part(segment, &rows) {
            Some(Part::View) => view = true,
            Some(Part::Helper) => {}
            None => return false,
        }
    }
    view
}

/// What `segment` is for the rule, or `None` when it leaves the rule.
fn part(segment: &Segment, rows: &[CommandPattern]) -> Option<Part> {
    let commands = programs::commands(&segment.words);
    let Some((inner, wrappers)) = commands.split_last() else {
        // NOTE: only reserved words, such as `do` alone on a line, run nothing; an
        // assignment, such as `PATH=/x`, changes what a later command runs.
        let neutral = !segment.words.is_empty()
            && segment.words.iter().all(|word| !is_assignment(word) && !word.contains(OPAQUE));
        return neutral.then_some(Part::Helper);
    };
    // NOTE: an assignment before the program, or between a wrapper and its program, can
    // change what runs (`LD_PRELOAD=`).
    let first = commands.first().map_or(0, |command| segment.words.len() - command.len());
    if segment.words[..first].iter().any(|word| is_assignment(word)) {
        return None;
    }
    let mut under_shell = false;
    for (wrapper, next) in wrappers.iter().zip(commands.iter().skip(1)) {
        let program = wrapper.first()?;
        if !VIEW_WRAPPERS.contains(&program.as_str()) {
            return None;
        }
        let between = &wrapper[1..wrapper.len() - next.len()];
        if between.iter().any(|word| is_assignment(word)) {
            return None;
        }
        under_shell |= program == "watch";
    }
    if under_shell && inner.iter().any(|word| word.contains(SH_SPECIAL)) {
        return None;
    }
    command_part(inner, rows)
}

/// What the simple command `words` is for the rule, program first.
fn command_part(words: &[String], rows: &[CommandPattern]) -> Option<Part> {
    let (program, args) = words.split_first()?;
    let pattern = words.iter().any(|word| word.contains(['*', '?', '[']));
    let row_allows = || {
        let mut own = rows.iter().filter(|row| row.program == *program).peekable();
        own.peek().is_none() || own.any(|row| row.matches_command(words, pattern))
    };
    match program.as_str() {
        "ss" => CommandPattern::new("ss")
            .with_forbid(SS_FORBID.iter().copied())
            .matches_command(words, pattern)
            .then_some(Part::View),
        "ip" => ip_reads(args).then_some(Part::View),
        "nstat" => CommandPattern::new("nstat")
            .with_forbid(NSTAT_FORBID.iter().copied())
            .matches_command(words, pattern)
            .then_some(Part::View),
        program if READERS.contains(&program) && args.iter().any(|arg| below_proc_net(arg)) => {
            row_allows().then_some(Part::View)
        }
        program if HELPERS.contains(&program) => row_allows().then_some(Part::Helper),
        program if CLOSERS.contains(&program) && args.is_empty() => Some(Part::Helper),
        _ => None,
    }
}

/// True when `ip` with `args` is a read form: options first, none of
/// [`IP_REFUSED`], then an object of [`IP_OBJECTS`], then no verb or one of
/// [`IP_READ_VERBS`].
fn ip_reads(args: &[String]) -> bool {
    if args.iter().any(|word| word.contains([OPAQUE, '$'])) {
        return false;
    }
    let mut at = 0;
    while let Some(word) = args.get(at) {
        let Some(rest) = word.strip_prefix('-') else { break };
        // NOTE: `ip` reads `--netns` as `-netns`, and `-` or `--` alone is a prefix of
        // every option.
        let option = if rest.starts_with('-') { rest } else { word.as_str() };
        if IP_REFUSED.iter().any(|refused| refused.starts_with(option)) {
            return false;
        }
        let takes_value = IP_VALUES
            .iter()
            .any(|(name, shortest)| option.len() >= *shortest && name.starts_with(option));
        at += if takes_value { 2 } else { 1 };
    }
    let Some(object) = args.get(at) else { return false };
    if !IP_OBJECTS.contains(&object.as_str()) {
        return false;
    }
    args.get(at + 1).is_none_or(|verb| IP_READ_VERBS.contains(&verb.as_str()))
}

/// True when `word` names a path strictly below a network dir of `/proc`: `/proc/net`,
/// `/proc/self/net`, `/proc/thread-self/net` or `/proc/<pid>/net`.
fn below_proc_net(word: &str) -> bool {
    if word.starts_with('-') || word.contains(OPAQUE) {
        return false;
    }
    let Some(path) = normalize(Path::new(word)) else { return false };
    let Ok(rest) = path.strip_prefix("/proc") else { return false };
    let names: Vec<&str> = rest
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    let process = |name: &str| {
        name == "self"
            || name == "thread-self"
            || (!name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit()))
    };
    match names.as_slice() {
        ["net", _, ..] => true,
        [owner, "net", _, ..] => process(owner),
        _ => false,
    }
}
