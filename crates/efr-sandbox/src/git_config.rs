//! Config keys that make git or cargo run a program, and the small git config reader
//! that the plan uses for include paths and the hooks path.
//!
//! The surface guard does not trust this reader for its verdict: it lists a config
//! with git itself (`git config --file F --list --no-includes -z` in a scrubbed
//! environment) and passes the listing to [`code_keys`], so git's own parser decides
//! which keys exist.

/// `section.name` keys (no subsection) that run a program.
const CODE_KEYS: &[&str] = &[
    "core.fsmonitor",
    "core.hookspath",
    "core.sshcommand",
    "core.pager",
    "core.editor",
    "core.askpass",
    "core.gitproxy",
    "sequence.editor",
    "diff.external",
    "gpg.program",
    "include.path",
    "uploadpack.packobjectshook",
];

/// `(section, name)` of keys with any subsection that run a program, such as
/// `diff.<driver>.textconv`.
const CODE_KEYS_WITH_SUBSECTION: &[(&str, &str)] = &[
    ("diff", "textconv"),
    ("diff", "command"),
    ("merge", "driver"),
    ("gpg", "program"),
    ("includeif", "path"),
    ("remote", "uploadpack"),
    ("remote", "receivepack"),
];

/// Sections whose every key runs a program or hands out credentials.
const CODE_SECTIONS: &[&str] = &["filter", "credential"];

/// The values of `core.fsmonitor` that turn it off.
const OFF: &[&str] = &["false", "no", "off", "0", ""];

/// True when the git config key `key` with `value` makes git run a program.
pub fn is_code_key(key: &str, value: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    let Some((section, rest)) = lower.split_once('.') else { return false };
    let (sub, name) = match rest.rsplit_once('.') {
        Some((sub, name)) => (Some(sub), name),
        None => (None, rest),
    };
    if CODE_SECTIONS.contains(&section) {
        return true;
    }
    match sub {
        None if section == "core" && name == "fsmonitor" => !OFF.contains(&value.trim()),
        None if section == "alias" => value.trim_start().starts_with('!'),
        None => CODE_KEYS.contains(&format!("{section}.{name}").as_str()),
        Some(_) if section == "alias" => value.trim_start().starts_with('!'),
        Some(_) => CODE_KEYS_WITH_SUBSECTION.contains(&(section, name)),
    }
}

/// The code keys of a git config listing, each once, in the order of the listing.
///
/// `listing` is the output of `git config --list` (`key=value` lines) or, better, of
/// `git config --list -z` (`key\nvalue` entries separated by NUL), which a value with a
/// line break cannot confuse.
pub fn code_keys(listing: &str) -> Vec<String> {
    let entries: Vec<(&str, &str)> = if listing.contains('\0') {
        listing
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .map(|entry| entry.split_once('\n').unwrap_or((entry, "")))
            .collect()
    } else {
        listing.lines().map(|line| line.split_once('=').unwrap_or((line, ""))).collect()
    };
    let mut keys: Vec<String> = Vec::new();
    for (key, value) in entries {
        if is_code_key(key, value) && !keys.iter().any(|known| known == key) {
            keys.push(key.to_owned());
        }
    }
    keys
}

/// The keys and values of a git config text, with section and key names in lower
/// case and subsections as written: `includeif.gitdir:~/w/.path`. Enough for the
/// include paths and the hooks path of a pinned config; it is not the guard's parser.
pub fn parse_config(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut section = String::new();
    let mut lines = text.lines();
    while let Some(raw) = lines.next() {
        let line = raw.trim_start();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            section = header_name(header);
            continue;
        }
        let mut line = line.to_owned();
        while line.ends_with('\\') && !line.ends_with("\\\\") {
            line.pop();
            match lines.next() {
                Some(more) => line.push_str(more),
                None => break,
            }
        }
        let (name, value) = match line.split_once('=') {
            Some((name, value)) => (name.trim().to_ascii_lowercase(), value_text(value)),
            None => (line.trim().to_ascii_lowercase(), "true".to_owned()),
        };
        if !section.is_empty() && !name.is_empty() {
            out.push((format!("{section}.{name}"), value));
        }
    }
    out
}

/// `core`, `includeif.gitdir:~/w/` or `remote.origin` from the text after `[`.
fn header_name(header: &str) -> String {
    let header = header.split(']').next().unwrap_or_default();
    match header.split_once('"') {
        Some((name, sub)) => {
            let sub = sub.rsplit_once('"').map_or(sub, |(sub, _)| sub).replace("\\\"", "\"");
            format!("{}.{sub}", name.trim().to_ascii_lowercase())
        }
        None => {
            // `[section.sub]`: the old form, whose subsection is lower case.
            header.trim().to_ascii_lowercase()
        }
    }
}

/// A value without its comment and quotes, with the escapes that git knows.
fn value_text(raw: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    let mut chars = raw.trim().chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => quoted = !quoted,
            '#' | ';' if !quoted => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('b') => {
                    out.pop();
                }
                Some(other) => out.push(other),
                None => {}
            },
            other => out.push(other),
        }
    }
    if quoted { out } else { out.trim_end().to_owned() }
}

/// The keys of a project's `.cargo/config.toml` (or `.cargo/config`) that make the
/// user's next `cargo` run a program or take code from another source, such as
/// `build.rustc-wrapper`.
///
/// The reader is line-based and errs toward naming a key: the list is a report for the
/// user, not a gate.
pub fn cargo_code_keys(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut table: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = strip_toml_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            let inner = line.trim_start_matches('[').trim_end_matches(']');
            table = dotted(inner);
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let mut path = table.clone();
        path.extend(dotted(key));
        let value = value.trim();
        if value.starts_with('{') {
            let before = found.len();
            for (inner_key, _) in
                value.trim_matches(['{', '}']).split(',').filter_map(|pair| pair.split_once('='))
            {
                let mut inner = path.clone();
                inner.extend(dotted(inner_key));
                push_cargo_key(&mut found, &inner);
            }
            // NOTE: a nested inline table is not read key by key; a code key in it is
            // still named, with `*` for the parts in between.
            let top = path.first().map(String::as_str).unwrap_or_default();
            if found.len() == before && ["build", "target", "host", "source"].contains(&top) {
                for leaf in CARGO_LEAVES {
                    if inline_key(value, leaf) {
                        let key = format!("{}.*.{leaf}", path.join("."));
                        if !found.contains(&key) {
                            found.push(key);
                        }
                    }
                }
            }
        } else {
            push_cargo_key(&mut found, &path);
        }
    }
    found
}

/// The last parts of the cargo keys that run a program.
const CARGO_LEAVES: &[&str] =
    &["rustc-wrapper", "rustc-workspace-wrapper", "runner", "linker", "replace-with"];

/// True when `leaf` stands as a key in the inline table text `value`.
fn inline_key(value: &str, leaf: &str) -> bool {
    value.match_indices(leaf).any(|(at, _)| {
        let before = value[..at].trim_end().chars().last();
        let after = value[at + leaf.len()..].trim_start().chars().next();
        matches!(before, Some('{' | ',' | '.' | '"' | '\''))
            && matches!(after, Some('=' | '"' | '\''))
    })
}

fn push_cargo_key(found: &mut Vec<String>, path: &[String]) {
    let parts: Vec<&str> = path.iter().map(String::as_str).collect();
    let is_code = matches!(
        parts.as_slice(),
        ["build", "rustc-wrapper" | "rustc-workspace-wrapper"]
            | ["target", _, "runner" | "linker"]
            | ["host", "linker"]
            | ["host", _, "linker"]
            | ["source", _, "replace-with"]
    );
    let key = parts.join(".");
    if is_code && !found.contains(&key) {
        found.push(key);
    }
}

/// The parts of a dotted TOML key, without quotes.
fn dotted(key: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in key.trim().chars() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), c) if c == open => quote = None,
            (None, '.') => parts.push(std::mem::take(&mut current).trim().to_owned()),
            (_, c) => current.push(c),
        }
    }
    parts.push(current.trim().to_owned());
    parts
}

fn strip_toml_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (at, c) in line.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), c) if c == open => quote = None,
            (None, '#') => return &line[..at],
            _ => {}
        }
    }
    line
}

#[cfg(test)]
mod tests;
