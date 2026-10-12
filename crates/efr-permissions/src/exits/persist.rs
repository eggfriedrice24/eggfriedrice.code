//! Named persistence that is not a plain file edit (efr's auto spec 9.1 and 10.8, plan
//! PB19, PB19a, PB20): a change of a user unit, a link into a unit dir, a copy into a
//! floor dir, the crontab and linger. Such a form goes to the classifier only when a
//! user message names it, efrd copies what it changes first, so `efr undo` brings it
//! back, and the classifier can read what it installs:
//!
//! - `systemctl --user enable`, `reenable`, `preset`, `mask`, `add-wants` and
//!   `add-requires` of units by name, with a few plain options, when efrd can copy the
//!   unit config dir ([`CoverAsk::UnitLinks`]) and the unit file of each unit existed
//!   in a unit dir before the turn: outside every write root, unchanged in this turn,
//!   and at most 8 KiB, so its text goes into the record;
//! - `ln -s <src>` into a `*.wants` or `*.requires` dir and `cp <src>` or `ln <src>`
//!   into a floor, when `<src>` holds as a unit file does;
//! - `crontab <file>` under the same rule for the file, `crontab -` fed a literal by
//!   `echo` or `printf`, and `crontab -r`, when efrd can copy the crontab
//!   ([`CoverAsk::CronTable`]);
//! - `loginctl enable-linger` and `disable-linger` with no operand or with one user,
//!   when efrd can copy the linger setting and the user is its own
//!   ([`CoverAsk::Linger`]).
//!
//! Every other form stays with the user: `systemctl --user link`, `edit`, `revert`,
//! `enable <path>`, a unit that no unit dir held, `crontab -e`, `crontab -` with any
//! other input, `at`, `batch`, `systemd-run --user`.

use std::path::{Path, PathBuf};

use efr_protocol::ActionFacts;

use super::programs::base_name;
use super::scan::{OPAQUE, scan};
use super::{Approver, Envelope, ExitNeed, Installed, cover};
use crate::{AutoSupport, CallFacts, CoverAsk, Source};

/// The verbs of `systemctl --user` that the classifier may judge when named.
const UNIT_VERBS: &[&str] = &["enable", "reenable", "preset", "mask", "add-wants", "add-requires"];

/// The options of `systemctl` that such a form may carry: none changes where the links
/// go or what they name.
const UNIT_OPTIONS: &[&str] = &[
    "--user",
    "--now",
    "--quiet",
    "-q",
    "--no-reload",
    "--no-block",
    "--no-ask-password",
    "--no-pager",
    "--force",
    "-f",
];

/// The suffixes of a unit name that efr knows; a name without one is a `.service`.
const UNIT_SUFFIXES: &[&str] = &[
    ".service",
    ".socket",
    ".timer",
    ".path",
    ".target",
    ".mount",
    ".automount",
    ".swap",
    ".slice",
];

/// The forms that name the crontab in a user message.
const CRON_NAMES: &[&str] = &["cron", "crontab"];

/// The forms that name the linger setting in a user message.
const LINGER_NAMES: &[&str] = &["linger", "lingering"];

/// True for the `loginctl` verbs that set the linger setting.
fn linger_verb(verb: &str) -> bool {
    verb == "enable-linger" || verb == "disable-linger"
}

/// The unit names that `systemctl` with `args` changes by a verb of [`UNIT_VERBS`], each
/// with its suffix, or `None` for any other form: another verb, an option outside
/// [`UNIT_OPTIONS`], a path, a template, or a name that efr cannot read.
pub(super) fn unit_operands(args: &[String]) -> Option<(&str, Vec<String>)> {
    let mut verb: Option<&str> = None;
    let mut operands: Vec<String> = Vec::new();
    for word in args {
        if word.starts_with('-') {
            if !UNIT_OPTIONS.contains(&word.as_str()) {
                return None;
            }
        } else if verb.is_none() {
            verb = Some(word);
        } else {
            operands.push(unit_name(word)?);
        }
    }
    let verb = verb.filter(|verb| UNIT_VERBS.contains(verb))?;
    if !args.iter().any(|word| word == "--user") || operands.is_empty() {
        return None;
    }
    // NOTE: `add-wants <target> <unit>...` adds links in the dir of the target.
    if matches!(verb, "add-wants" | "add-requires") && operands.len() < 2 {
        return None;
    }
    Some((verb, operands))
}

/// `word` as a unit name with its suffix, or `None` when it is no plain unit name.
fn unit_name(word: &str) -> Option<String> {
    let plain = !word.is_empty()
        && word.len() <= 255
        && !word.contains("..")
        && word.bytes().all(|byte| byte.is_ascii_alphanumeric() || b":-_.\\".contains(&byte));
    if !plain {
        return None;
    }
    if UNIT_SUFFIXES
        .iter()
        .any(|suffix| word.strip_suffix(suffix).is_some_and(|stem| !stem.is_empty()))
    {
        return Some(word.to_owned());
    }
    (!word.contains('.')).then(|| format!("{word}.service"))
}

/// The units whose file the classifier reads for `systemctl` with `args`: the units
/// that the form enables. A mask installs no file of its own.
fn installed_units(args: &[String]) -> Vec<String> {
    match unit_operands(args) {
        Some(("mask", _)) | None => Vec::new(),
        Some(("add-wants" | "add-requires", mut units)) => {
            units.remove(0);
            units
        }
        Some((_, units)) => units,
    }
}

/// The sources whose facts the engine reads for the command `words`, for
/// [`FactRequest::sources`](super::FactRequest::sources): the file of each unit that a
/// `systemctl --user` form names, the file of `crontab <file>`, and the sources of a `cp` or
/// `ln` into a floor. `at` resolves a word as the line does.
pub(super) fn sources(
    words: &[String],
    envelope: &Envelope<'_>,
    at: &dyn Fn(&str) -> Option<PathBuf>,
) -> Vec<Source> {
    let Some((first, args)) = words.split_first() else { return Vec::new() };
    match base_name(first) {
        "systemctl" => unit_operands(args)
            .map(|(_, units)| units.into_iter().map(Source::Unit).collect())
            .unwrap_or_default(),
        "crontab" => match args {
            [file] if !file.starts_with('-') => at(file).map(Source::File).into_iter().collect(),
            _ => Vec::new(),
        },
        _ => installs(words, at)
            .into_iter()
            .filter(|(dest, _)| envelope.is_floor(dest) || envelope.is_floor(&dest.join("x")))
            .flat_map(|(_, sources)| sources)
            .filter(|source| !envelope.is_secret(source) && !envelope.is_masked(source))
            .map(Source::File)
            .collect(),
    }
}

/// What the engine asks efrd to say it can copy before the command `words`, for
/// [`FactRequest::covers`](super::FactRequest::covers): the unit config dir for each
/// unit of a named `systemctl --user` form, the crontab for a `crontab` form that
/// replaces or removes it, the linger setting for `loginctl enable-linger` and
/// `disable-linger`.
pub(super) fn covers(words: &[String]) -> Vec<CoverAsk> {
    let Some((first, args)) = words.split_first() else { return Vec::new() };
    match (base_name(first), args) {
        ("systemctl", _) => unit_operands(args)
            .map(|(_, units)| units.into_iter().map(CoverAsk::UnitLinks).collect())
            .unwrap_or_default(),
        ("crontab", [only]) if only == "-r" || only == "-" || !only.starts_with('-') => {
            vec![CoverAsk::CronTable]
        }
        ("loginctl", [verb, user @ ..]) if linger_verb(verb) && user.len() <= 1 => {
            vec![CoverAsk::Linger(user.first().cloned())]
        }
        _ => Vec::new(),
    }
}

/// What a `cp` or `ln` of `words` puts where: the destination and the sources, each
/// resolved by `at`. Empty for any other command, for a form with `-t`, and when efr
/// cannot resolve a word.
pub(super) fn installs(
    words: &[String],
    at: &dyn Fn(&str) -> Option<PathBuf>,
) -> Vec<(PathBuf, Vec<PathBuf>)> {
    let Some((first, args)) = words.split_first() else { return Vec::new() };
    if !matches!(base_name(first), "cp" | "ln") {
        return Vec::new();
    }
    if args.iter().any(|word| word == "-t" || word.starts_with("--target-directory")) {
        return Vec::new();
    }
    let operands: Vec<&String> = args.iter().filter(|word| !word.starts_with('-')).collect();
    let Some((dest, sources)) = operands.split_last() else { return Vec::new() };
    if sources.is_empty() {
        return Vec::new();
    }
    let Some(dest) = at(dest) else { return Vec::new() };
    let sources: Option<Vec<PathBuf>> = sources.iter().map(|word| at(word)).collect();
    sources.map(|sources| vec![(dest, sources)]).unwrap_or_default()
}

/// What the classifier reads of each of `sources`, or `None` when one of them does not
/// hold the rule of PB19a: a regular file that the facts read, outside every write
/// root, cache and `$SCRATCH`, no mask and no secret, unchanged in this turn, and at
/// most [`ActionFacts::MAX_EDIT_BYTES`] of text.
pub(super) fn readable(
    envelope: &Envelope<'_>,
    facts: Option<&CallFacts>,
    sources: &[Source],
) -> Option<Vec<Installed>> {
    let facts = facts?;
    let outside = |path: &Path| {
        !envelope.in_root(path)
            && !envelope.is_masked(path)
            && !envelope.is_secret(path)
            && !envelope.locations().is_efr_own(path)
    };
    let mut read = Vec::new();
    for source in sources {
        let fact = facts.source(source)?;
        let path = fact.path.as_ref()?;
        let text = fact.text.as_ref().filter(|text| text.len() <= ActionFacts::MAX_EDIT_BYTES)?;
        let named = match source {
            Source::File(named) => outside(named),
            Source::Unit(_) => true,
        };
        if fact.changed || !named || !outside(path) {
            return None;
        }
        read.push(Installed { path: path.clone(), text: text.clone() });
    }
    Some(read)
}

/// Routes the persistence exit `need` of the command `words` of `line`, when it is a
/// form of the module doc: who approves it, what efrd copies first, and what the
/// classifier reads. Any other form keeps the user as its approver.
pub(super) fn route(
    need: &mut ExitNeed,
    words: &[String],
    line: &str,
    envelope: &Envelope<'_>,
    facts: Option<&CallFacts>,
    support: &AutoSupport,
    at: &dyn Fn(&str) -> Option<PathBuf>,
) {
    let Some((first, args)) = words.split_first() else { return };
    let (copies, installs, names): (Vec<CoverAsk>, Vec<Installed>, &[&str]) = match base_name(first)
    {
        "systemctl" => {
            let Some((_, units)) = unit_operands(args) else { return };
            let sources: Vec<Source> = units.iter().cloned().map(Source::Unit).collect();
            let Some(read) = readable(envelope, facts, &sources) else { return };
            let enabled = installed_units(args);
            let installs = read
                .into_iter()
                .zip(&units)
                .filter(|(_, unit)| enabled.contains(unit))
                .map(|(read, _)| read)
                .collect();
            (units.into_iter().map(CoverAsk::UnitLinks).collect(), installs, &[])
        }
        "crontab" => {
            let installs = match args {
                [flag] if flag == "-r" => Vec::new(),
                [flag] if flag == "-" && literal_pipe(line) => Vec::new(),
                [file] if !file.starts_with('-') => {
                    let Some(path) = at(file) else { return };
                    let Some(read) = readable(envelope, facts, &[Source::File(path)]) else {
                        return;
                    };
                    read
                }
                _ => return,
            };
            (vec![CoverAsk::CronTable], installs, CRON_NAMES)
        }
        "loginctl" => match args {
            [verb, user @ ..] if linger_verb(verb) && user.len() <= 1 => {
                (vec![CoverAsk::Linger(user.first().cloned())], Vec::new(), LINGER_NAMES)
            }
            _ => return,
        },
        _ => return,
    };
    named(need, copies, installs, facts, support);
    for name in names {
        if !need.names.iter().any(|known| known == name) {
            need.names.push((*name).to_owned());
        }
    }
}

/// Makes `need` an exit that the classifier judges when named, with `copies` and
/// `installs`, when efrd keeps the home store and the facts say that it can copy every
/// ask of `copies`. Else it stays with the user and copies nothing: an approval of the
/// user then runs it as before phase B.
fn named(
    need: &mut ExitNeed,
    copies: Vec<CoverAsk>,
    installs: Vec<Installed>,
    facts: Option<&CallFacts>,
    support: &AutoSupport,
) {
    if !support.floor_backup || cover::covered(&copies, facts) != Some(true) {
        return;
    }
    need.copies = copies;
    need.covered = Some(true);
    need.backup_before_write = true;
    need.installs = installs;
    need.approver = Approver::ClassifierWhenNamed;
}

/// True when `line` is exactly one `echo` or `printf` of literal words piped into
/// `crontab -`: the record's line then holds the whole crontab.
pub(super) fn literal_pipe(line: &str) -> bool {
    let segments = scan(line);
    let [feed, crontab] = segments.as_slice() else { return false };
    let literal = |words: &[String]| words.iter().all(|word| !word.contains([OPAQUE, '$', '`']));
    let feeds =
        feed.words.first().is_some_and(|program| matches!(base_name(program), "echo" | "printf"));
    feeds
        && literal(&feed.words)
        && feed.redirects.is_empty()
        && crontab.redirects.is_empty()
        && crontab.words.len() == 2
        && base_name(&crontab.words[0]) == "crontab"
        && crontab.words[1] == "-"
        && separators(line) == "|"
}

/// The characters of `line` outside quotes that split or join commands: `|`, `&`,
/// `;`, a new line, `(`, `)`, `<`, `>` and the backquote.
fn separators(line: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in line.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => {}
            (Some(_), '\\') | (None, '\\') => escaped = true,
            (Some(_), '`') => out.push('`'),
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '|' | '&' | ';' | '\n' | '(' | ')' | '<' | '>' | '`') => out.push(c),
            (None, _) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests;
