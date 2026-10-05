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

use super::reads::{Depth, Named, Reads, from_word, option_value};
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
///   so a link to a secret is judged as a read of the secret.
///
/// The value of an option that looks like a path is read, except the directory of
/// `-t` and `--target-directory`, which is written.
pub(super) fn writer(name: &str, args: &[Word]) -> Option<Reads> {
    let kind = match name {
        "cp" => Kind::Copy,
        "ln" => Kind::Link,
        _ if ALL_WRITTEN.contains(&name) => Kind::All,
        _ => return None,
    };
    let mut operands: Vec<&Word> = Vec::new();
    let mut reads = Reads::default();
    let mut target = false;
    let mut after_options = false;
    for word in args {
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
        if let Some(value) = option_value(word) {
            if names_target { reads.writes.push(value) } else { reads.named.push(value) }
        }
    }
    let written = |word: &&Word| from_word(word, Depth::One);
    match (kind, operands.split_last()) {
        (_, None) => {}
        (Kind::All, Some(_)) => reads.writes.extend(operands.iter().map(written)),
        // NOTE: which word `-t` takes is not certain here, so every operand may be the
        // directory that is written.
        (Kind::Copy | Kind::Link, Some(_)) if target => {
            reads.writes.extend(operands.iter().map(written));
        }
        (Kind::Link, Some((only, []))) => {
            reads.named.push(from_word(only, Depth::One));
            reads.writes.push(link_name(only));
        }
        (Kind::Copy | Kind::Link, Some((last, sources))) => {
            // NOTE: `cp -r ~ x` would copy the keys below `~` to where they are no
            // longer secret, so a source of `cp` is read with everything below it.
            let depth = if kind == Kind::Copy { Depth::Tree } else { Depth::One };
            reads.named.extend(sources.iter().map(|word| from_word(word, depth)));
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
