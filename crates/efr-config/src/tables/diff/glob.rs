//! The globs of `[diff.groups]`: a small matcher over the paths that efr shows, and
//! the checks of a glob and of a group's name at load.
//!
//! - `*` matches any characters inside one part of a path, `?` one character, and
//!   `**`, as a whole part, any number of parts.
//! - A glob without `/` matches the name of a file in any directory: `*.snap`.
//! - A glob that ends in `/` matches every file below a directory that the rest
//!   matches: `evals/` below any `evals` directory, `crates/evals/` only below that
//!   path.
//! - Any other glob matches the whole path from its start: `evals/**/*.toml`.

/// The longest name of a group: it stands in the line at the end of a turn.
const MAX_NAME: usize = 24;

/// True when `glob` matches `path`.
pub(crate) fn matches(glob: &str, path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    if let Some(dir) = glob.strip_suffix('/') {
        let patterns: Vec<&str> = dir.split('/').collect();
        let dirs = &parts[..parts.len().saturating_sub(1)];
        return match patterns.as_slice() {
            [one] => dirs.iter().any(|part| part_matches(one, part)),
            _ => (1..=dirs.len()).any(|end| parts_match(&patterns, &dirs[..end])),
        };
    }
    let patterns: Vec<&str> = glob.split('/').collect();
    match patterns.as_slice() {
        [one] => parts.last().is_some_and(|name| part_matches(one, name)),
        _ => parts_match(&patterns, &parts),
    }
}

/// True when the parts of a glob match the parts of a path, all of them.
fn parts_match(patterns: &[&str], parts: &[&str]) -> bool {
    match patterns.split_first() {
        None => parts.is_empty(),
        Some((&"**", rest)) => (0..=parts.len()).any(|skip| parts_match(rest, &parts[skip..])),
        Some((pattern, rest)) => parts
            .split_first()
            .is_some_and(|(part, after)| part_matches(pattern, part) && parts_match(rest, after)),
    }
}

/// True when one part of a glob, with `*` and `?`, matches one part of a path.
fn part_matches(pattern: &str, part: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let part: Vec<char> = part.chars().collect();
    // NOTE: the classic two-pointer match with one backtrack point: linear in practice,
    // and never exponential, whatever the glob.
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < part.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, t));
                p += 1;
            }
            Some('?') => {
                p += 1;
                t += 1;
            }
            Some(c) if *c == part[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some((at, from)) => {
                    p = at + 1;
                    t = from + 1;
                    star = Some((at, from + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

/// `None` when `glob` is a glob of `[diff.groups]`, else what is wrong.
pub(crate) fn check_glob(glob: &str) -> Option<&'static str> {
    if glob.is_empty() || glob.chars().any(char::is_control) {
        return Some("globs such as evals/ or *.eval.toml, not empty");
    }
    let body = glob.strip_suffix('/').unwrap_or(glob);
    let parts: Vec<&str> = body.split('/').collect();
    for (at, part) in parts.iter().enumerate() {
        // NOTE: an absolute path starts with an empty part: efr shows a file outside
        // the roots by its absolute path.
        if part.is_empty() && !(at == 0 && parts.len() > 1) {
            return Some("globs without an empty part, such as a//b");
        }
        if *part == "." || *part == ".." {
            return Some("globs without . or .. parts");
        }
        if part.contains("**") && *part != "**" {
            return Some("globs where ** stands alone between slashes, such as evals/**/*.toml");
        }
    }
    None
}

/// The names that a group cannot have: `total` names the last row of the table of
/// `efr diff --stat`.
const RESERVED_NAMES: &[&str] = &["total"];

/// `None` when `name` is a name of a group, else what is wrong.
pub(crate) fn check_name(name: &str) -> Option<&'static str> {
    let good = !name.is_empty()
        && name.chars().count() <= MAX_NAME
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !RESERVED_NAMES.contains(&name);
    (!good).then_some("group names of 1 to 24 letters, digits, - or _, other than total")
}

#[cfg(test)]
mod tests;
