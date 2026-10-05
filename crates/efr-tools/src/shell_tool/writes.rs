//! The writer programs: the programs whose operands are paths they create, change,
//! move or delete, so the shell tool declares them as writes and the permission
//! engine judges them as writes in every mode.
//!
//! Declared as reads, `rm ~/.ssh/known_hosts` would be judged like `cat`, and
//! `cp x ~/.config/efr/config.toml` would not meet the rule that keeps efr's config
//! out of reach of every tool. As writes, the path rules decide: in the `auto` mode a
//! write in the turn's project or `$SCRATCH` runs without a question, and a write
//! anywhere else asks or is refused.
//!
//! The reading errs towards writes: a word that may be an option's value counts as an
//! operand, and when the text cannot show which operand `cp` or `ln` writes, every
//! operand is a write.

use super::reads::{Depth, Named, Reads, from_word, named_value, option_value};
use super::words::Word;

/// The writer programs, by how their operands divide into writes and reads.
const ALL_WRITTEN: &[&str] = &["rm", "rmdir", "mkdir", "touch", "mv", "chmod", "truncate", "tee"];

/// What the program `name` with `args` writes and reads, when it is a writer program:
///
/// - `rm`, `rmdir`, `mkdir`, `touch`, `mv` (sources and target), `chmod`, `truncate`
///   and `tee` write every operand;
/// - `cp` writes its last operand, or the directory of `-t`, and reads the others
///   with everything below them, as a recursive copy does;
/// - `ln` writes the link: its last operand, the directory of `-t`, or, with one
///   operand, the name of that operand in the working directory. It reads the others,
///   so a link to a secret is judged as a read of the secret;
/// - a hard link (`ln` without `-s`, `cp -l`) writes its sources too, because the
///   new name writes the same file: `ln ~/.config/efr/config.toml x` is a write of the
///   config.
///
/// The value of an option that looks like a path is read, except the directory of
/// `-t` and `--target-directory`, which is written. `cp` and `ln` know which of their
/// options take the next word as a value (`-S x`, `--suffix x`), so that word is never
/// an operand. When an option follows an operand (GNU takes options anywhere), or an
/// option is not one of theirs, every operand of `cp` and `ln` is written. `cp`, `ln` and `mv` may create or
/// move a symbolic link, which the result says ([`Reads::links`]).
pub(super) fn writer(name: &str, args: &[Word]) -> Option<Reads> {
    let kind = match name {
        "cp" => Kind::Copy,
        "ln" => Kind::Link,
        _ if ALL_WRITTEN.contains(&name) => Kind::All,
        _ => return None,
    };
    let options = match kind {
        Kind::Copy => Some(&CP),
        Kind::Link => Some(&LN),
        Kind::All => None,
    };
    let mut operands: Vec<&Word> = Vec::new();
    let mut reads = Reads { links: matches!(name, "cp" | "ln" | "mv"), ..Reads::default() };
    let mut target = false;
    // NOTE: true when the words do not show for certain which operand `cp` or `ln`
    // writes, so that every operand counts as written.
    let mut unsure = false;
    // NOTE: `ln` makes a hard link unless the line shows `-s`; `cp` only with `-l`.
    let mut hard = kind == Kind::Link;
    let mut after_options = false;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let text = word.text.as_str();
        if !after_options && text == "--" {
            after_options = true;
            continue;
        }
        // NOTE: an expansion's value is unknown; the engine asks for a line that holds
        // one, so it cannot slip through as a write of nothing.
        if text == "-" || word.expansion {
            continue;
        }
        if after_options || !text.starts_with('-') {
            operands.push(word);
            continue;
        }
        let names_target = matches!(name, "cp" | "mv" | "ln") && names_target_directory(text);
        target |= names_target;
        match kind {
            // NOTE: an abbreviation counts for `--link` from one letter on, which errs
            // towards a hard link, and for `--symbolic` only from two.
            Kind::Link if has_flag(text, 's', "symbolic", 2) => hard = false,
            Kind::Copy if has_flag(text, 'l', "link", 1) => hard = true,
            _ => {}
        }
        if let Some(options) = options {
            // NOTE: GNU `cp` and `ln` also take options after the operands, so in
            // `cp a ~/.bashrc -S x` the last word is the suffix and `~/.bashrc` is
            // written. Once an option follows an operand, any operand may be written.
            let option = options.read(text);
            unsure |= !option.known || !operands.is_empty();
            match option.value {
                Some(OptionValue::Next(of)) => {
                    // NOTE: the next word is the value, never an operand.
                    if let Some(value) = words.next()
                        && of == ValueOf::Target
                        && !value.expansion
                    {
                        reads.writes.push(from_word(value, Depth::One));
                    }
                    continue;
                }
                Some(OptionValue::Attached(ValueOf::Target, value)) => {
                    reads.writes.push(named_value(value));
                    continue;
                }
                Some(OptionValue::Attached(ValueOf::Other, _)) | None => {}
            }
        }
        if let Some(value) = option_value(word) {
            if names_target { reads.writes.push(value) } else { reads.named.push(value) }
        }
    }
    let written = |word: &&Word| from_word(word, Depth::One);
    match (kind, operands.split_last()) {
        (_, None) => {}
        (Kind::All, Some(_)) => reads.writes.extend(operands.iter().map(written)),
        // NOTE: with `-t` every operand may be the directory that is written, when the
        // reading missed which word `-t` takes; with an option the reading is not sure
        // of, or one after an operand, any operand may be the destination.
        (Kind::Copy | Kind::Link, Some(_)) if target || unsure => {
            reads.writes.extend(operands.iter().map(written));
        }
        (Kind::Link, Some((only, []))) => {
            let source = from_word(only, Depth::One);
            if hard {
                reads.writes.push(source)
            } else {
                reads.named.push(source)
            }
            reads.writes.push(link_name(only));
        }
        (Kind::Copy | Kind::Link, Some((last, sources))) => {
            // NOTE: `cp -r ~ x` would copy the keys below `~` to where they are no
            // longer secret, so a source of `cp` is read with everything below it.
            let depth = if kind == Kind::Copy { Depth::Tree } else { Depth::One };
            let sources = sources.iter().map(|word| from_word(word, depth));
            if hard {
                reads.writes.extend(sources)
            } else {
                reads.named.extend(sources)
            }
            reads.writes.push(from_word(last, Depth::One));
        }
    }
    Some(reads)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Every operand is written.
    All,
    /// `cp`: the last operand is written.
    Copy,
    /// `ln`: the link is written.
    Link,
}

/// The options of GNU `cp` or `ln`, enough to tell the words that are values from the
/// operands. The short options `-S` (the backup suffix) and `-t` (the target
/// directory) take a value, attached or in the next word.
struct Options {
    /// The short options without a value.
    flags: &'static str,
    /// The long options whose value is the next word unless `=` gives it.
    long_values: &'static [&'static str],
    /// The other long options: without a value, or with one that only `=` gives.
    long_flags: &'static [&'static str],
}

/// GNU coreutils `cp`.
const CP: Options = Options {
    flags: "abdfHilLnPpRrsTuvxZ",
    long_values: &["no-preserve", "sparse", "suffix", "target-directory"],
    long_flags: &[
        "archive",
        "attributes-only",
        "backup",
        "context",
        "copy-contents",
        "debug",
        "dereference",
        "force",
        "help",
        "interactive",
        "keep-directory-symlink",
        "link",
        "no-clobber",
        "no-dereference",
        "no-target-directory",
        "one-file-system",
        "parents",
        "preserve",
        "recursive",
        "reflink",
        "remove-destination",
        "strip-trailing-slashes",
        "symbolic-link",
        "update",
        "verbose",
        "version",
    ],
};

/// GNU coreutils `ln`.
const LN: Options = Options {
    flags: "bdFfiLnPrsTv",
    long_values: &["suffix", "target-directory"],
    long_flags: &[
        "backup",
        "directory",
        "force",
        "help",
        "interactive",
        "logical",
        "no-dereference",
        "no-target-directory",
        "physical",
        "relative",
        "symbolic",
        "verbose",
        "version",
    ],
};

/// What one option word of `cp` or `ln` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OptionWord<'a> {
    /// False for an option the table does not hold, or an abbreviation of more than one.
    known: bool,
    /// The value it takes, if it takes one.
    value: Option<OptionValue<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptionValue<'a> {
    /// The next word is the value.
    Next(ValueOf),
    /// The rest of the word is the value.
    Attached(ValueOf, &'a str),
}

/// Which option a value belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueOf {
    /// `-t` or `--target-directory`: the directory that is written.
    Target,
    /// Any other option: a value that is not a path the program writes.
    Other,
}

impl Options {
    /// Reads the option word `text`, which starts with `-` and is not `-` or `--`.
    fn read<'a>(&self, text: &'a str) -> OptionWord<'a> {
        let unknown = OptionWord { known: false, value: None };
        if let Some(long) = text.strip_prefix("--") {
            let (name, attached) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            let all = || self.long_values.iter().chain(self.long_flags).copied();
            // NOTE: getopt takes an exact name before an abbreviation, and refuses an
            // abbreviation of more than one option.
            let option = match all().find(|option| *option == name) {
                Some(option) => option,
                None => {
                    let mut matches = all().filter(|option| option.starts_with(name));
                    match (matches.next(), matches.next()) {
                        (Some(option), None) if !name.is_empty() => option,
                        _ => return unknown,
                    }
                }
            };
            if !self.long_values.contains(&option) {
                return OptionWord { known: true, value: None };
            }
            let of = if option == "target-directory" { ValueOf::Target } else { ValueOf::Other };
            let value = match attached {
                Some(value) => OptionValue::Attached(of, value),
                None => OptionValue::Next(of),
            };
            return OptionWord { known: true, value: Some(value) };
        }
        for (at, letter) in text.char_indices().skip(1) {
            let of = match letter {
                't' => ValueOf::Target,
                'S' => ValueOf::Other,
                _ if self.flags.contains(letter) => continue,
                _ => return unknown,
            };
            let rest = &text[at + letter.len_utf8()..];
            let value = if rest.is_empty() {
                OptionValue::Next(of)
            } else {
                OptionValue::Attached(of, rest)
            };
            return OptionWord { known: true, value: Some(value) };
        }
        OptionWord { known: true, value: None }
    }
}

/// True when the option word `text` may name the directory that `cp`, `mv` or `ln`
/// writes into: `--target-directory` or an abbreviation of it, or a cluster of short
/// options that holds `t`.
fn names_target_directory(text: &str) -> bool {
    match text.strip_prefix("--") {
        Some(long) => {
            let name = long.split_once('=').map_or(long, |(name, _)| name);
            !name.is_empty() && "target-directory".starts_with(name)
        }
        None => text[1..].contains('t'),
    }
}

/// True when the option word `text` sets the flag `short`, or the long option `long`
/// or an abbreviation of it of at least `least` letters. A short value option of `cp`
/// and `ln` (`-S`, `-t`) ends the cluster: what follows is its value.
fn has_flag(text: &str, short: char, long: &str, least: usize) -> bool {
    match text.strip_prefix("--") {
        Some(name) => {
            let name = name.split_once('=').map_or(name, |(name, _)| name);
            name.len() >= least && long.starts_with(name)
        }
        None => text[1..]
            .chars()
            .take_while(|letter| !matches!(letter, 'S' | 't'))
            .any(|letter| letter == short),
    }
}

/// The link that `ln TARGET` creates: the last part of `TARGET` in the working
/// directory, or the working directory itself when that part is not a plain name.
fn link_name(target: &Word) -> Named {
    let base = target.text.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    let text = match base {
        "" | "." | ".." | "~" => ".",
        name => name,
    };
    Named {
        text: text.to_owned(),
        tilde: false,
        pattern: text.find(['*', '?', '[']),
        depth: Depth::One,
    }
}

#[cfg(test)]
mod tests;
