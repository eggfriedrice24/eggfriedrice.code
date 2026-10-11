//! The words that name an exit in a user message (spec 9.1 and 9.2), and the one check
//! that decides whether the user's messages name it.
//!
//! The engine never reads user text. It gives each exit its forms
//! ([`ExitNeed::names`](super::ExitNeed::names)), and the turn calls [`names_match`] with
//! the user's messages, for the route of a
//! [`Approver::ClassifierWhenNamed`](super::Approver::ClassifierWhenNamed) exit and for
//! `named_in_user_messages` in the record alike, so the two never disagree.

use std::path::{Component, Path};

use crate::FloorGroup;

/// The phrases that name the floor files of a group, besides their own path and word
/// forms: "my zsh config" names `~/.zshrc`, "my sway config" names
/// `~/.config/sway/config`, whose stem names nothing alone. The dir below `~/.config`
/// alone is no phrase, because words such as `git` or `fish` are too common. A unit
/// file is named also by "`<unit>` unit" and "`<unit>` service".
pub const NAME_PHRASES: &[(FloorGroup, &[&str])] = &[
    (FloorGroup::Zsh, &["zsh config", "zsh startup", "shell config"]),
    (FloorGroup::Bash, &["bash config", "shell config"]),
    (FloorGroup::Fish, &["fish config"]),
    (FloorGroup::Hypr, &["hypr", "hyprland"]),
    (FloorGroup::Sway, &["sway config"]),
    (FloorGroup::I3, &["i3 config"]),
    (FloorGroup::Git, &["git config", "gitconfig"]),
    (FloorGroup::Cargo, &["cargo config"]),
    (FloorGroup::Nvim, &["nvim config", "neovim config", "nvim init"]),
    (FloorGroup::Vim, &["vim config", "vimrc"]),
    (FloorGroup::Direnv, &["direnv config"]),
    (FloorGroup::Autostart, &["autostart"]),
];

/// The suffixes of a systemd unit file.
const UNIT_SUFFIXES: &[&str] =
    &[".service", ".timer", ".socket", ".path", ".target", ".mount", ".slice"];

/// File stems that name nothing alone: a file whose stem is one of them, such as
/// `~/.config/sway/config` or `~/.config/nvim/init.lua`, gets no word form, and only a
/// path form or a phrase names it. "fix the config" must not name every config file.
pub const GENERIC_STEMS: &[&str] = &[
    "config", "init", "settings", "env", "profile", "conf", "rc", "main", "default", "user",
    "local",
];

/// True when a message of `messages` names one of `names`.
///
/// A form with a `/` is a path form (`/home/u/.zshrc`, `~/.zshrc`, `u/.zshrc`) and
/// matches as text, but only as whole path components: `bin/git` names
/// `~/.local/bin/git`, and not `src/bin/github.rs` or `xbin/git`. Any other form is a
/// word or a phrase (`zshrc`, `sway config`, `github.com`) and matches whole words in
/// the same order, case-insensitive, with only white space between them; its first word
/// may carry a possessive `'s` ("sway's config"). A word ends at a character that is
/// not a letter, a digit or `_`, so `zshrc` does not name `zshrcx`.
pub fn names_match(names: &[String], messages: &[String]) -> bool {
    messages.iter().any(|message| {
        let lower = message.to_lowercase();
        names.iter().any(|form| {
            if form.contains('/') {
                path_in(form, message)
            } else {
                phrase_in(&form.to_lowercase(), &lower)
            }
        })
    })
}

/// True when the path form `form` stands in `text` as whole path components: no
/// character of a file name right before it, and after it the end, a `/`, or a
/// character that no file name of a path form holds. A `.` after it ends it only when no
/// letter or digit follows, so `~/.zshrc.` at the end of a sentence names `~/.zshrc`,
/// and `~/.zshrc.bak` does not.
fn path_in(form: &str, text: &str) -> bool {
    let in_name = |c: char| is_word_char(c) || matches!(c, '.' | '-' | '~' | '+' | '@');
    let mut from = 0;
    while let Some(at) = text[from..].find(form) {
        let start = from + at;
        let end = start + form.len();
        let before = text[..start].chars().next_back();
        let mut after = text[end..].chars();
        let ends = match after.next() {
            None | Some('/') => true,
            Some('.') => !after.next().is_some_and(is_word_char),
            Some(c) => !in_name(c),
        };
        if !before.is_some_and(in_name) && ends {
            return true;
        }
        from = start + text[start..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// True when the words of `phrase`, in lower case, stand in `text`, in lower case, as
/// [`names_match`] describes.
fn phrase_in(phrase: &str, text: &str) -> bool {
    let words: Vec<&str> = phrase.split_whitespace().collect();
    let Some(first) = words.first() else {
        return false;
    };
    let mut from = 0;
    while let Some(at) = text[from..].find(first) {
        let start = from + at;
        if starts_word(text, start) && rest_follows(text, start + first.len(), &words[1..], true) {
            return true;
        }
        from = start + text[start..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// True when `rest` follows a word of a phrase that ends at `end` in `text`.
fn rest_follows(text: &str, end: usize, rest: &[&str], first: bool) -> bool {
    let Some((next, after)) = rest.split_first() else {
        return ends_word(text, end);
    };
    let mut starts = vec![end];
    if first {
        for possessive in ["'s", "\u{2019}s"] {
            if text[end..].starts_with(possessive) {
                starts.push(end + possessive.len());
            }
        }
    }
    starts.into_iter().any(|at| {
        let gap = text[at..].len() - text[at..].trim_start().len();
        let word = at + gap;
        gap > 0
            && text[word..].starts_with(next)
            && rest_follows(text, word + next.len(), after, false)
    })
}

/// True for a letter, a digit or `_`: a character inside a word.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// True when no word character stands right before `at`.
fn starts_word(text: &str, at: usize) -> bool {
    !text[..at].chars().next_back().is_some_and(is_word_char)
}

/// True when no word character stands right at `at`.
fn ends_word(text: &str, at: usize) -> bool {
    !text[at..].chars().next().is_some_and(is_word_char)
}

/// The path forms of `path`, in normal form: its absolute form, its `~` form when it
/// lies below `home`, and its last two components.
pub(crate) fn path_forms(path: &Path, home: &Path) -> Vec<String> {
    let mut forms = vec![path.display().to_string()];
    if let Ok(below) = path.strip_prefix(home)
        && !below.as_os_str().is_empty()
    {
        forms.push(format!("~/{}", below.display()));
    }
    let names: Vec<&str> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    if let [.., parent, name] = names.as_slice() {
        forms.push(format!("{parent}/{name}"));
    }
    dedup(forms)
}

/// The word forms of a file: its name without a leading dot (`zshrc`, `hyprland.conf`)
/// and its stem (`hyprland`), or none when the stem is one of [`GENERIC_STEMS`].
pub(crate) fn word_forms(path: &Path) -> Vec<String> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let base = name.trim_start_matches('.');
    let stem = base.split('.').next().unwrap_or_default();
    if stem.is_empty() || GENERIC_STEMS.contains(&stem.to_lowercase().as_str()) {
        return Vec::new();
    }
    dedup(vec![base.to_owned(), stem.to_owned()])
}

/// The groups whose file names are common words: a program on the `PATH` (`git`), a
/// unit (`backup.service`), a desktop entry (`firefox.desktop`) and an agent or editor
/// config (`.vscode/tasks.json`). Their bare stem names nothing alone, so "commit this
/// with git" does not name `~/.local/bin/git`; the name with its suffix still does.
const STEMLESS_GROUPS: &[FloorGroup] =
    &[FloorGroup::Programs, FloorGroup::Systemd, FloorGroup::Autostart, FloorGroup::Agent];

/// The forms that name the floor file `path` of `group`: its path forms, its word
/// forms (for a group of [`STEMLESS_GROUPS`] only the name with a suffix), and the
/// phrases of its group.
pub(crate) fn floor_names(path: &Path, home: &Path, group: Option<FloorGroup>) -> Vec<String> {
    let mut forms = path_forms(path, home);
    let stemless = group.is_some_and(|group| STEMLESS_GROUPS.contains(&group));
    forms.extend(word_forms(path).into_iter().filter(|form| !stemless || form.contains('.')));
    if let Some(group) = group {
        let phrases = NAME_PHRASES.iter().filter(|(known, _)| *known == group);
        forms.extend(
            phrases.flat_map(|(_, phrases)| phrases.iter().map(|phrase| (*phrase).to_owned())),
        );
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        let unit = UNIT_SUFFIXES.iter().find_map(|suffix| name.strip_suffix(suffix));
        if let (FloorGroup::Systemd, Some(unit)) = (group, unit.filter(|unit| !unit.is_empty())) {
            forms.push(format!("{unit} unit"));
            forms.push(format!("{unit} service"));
        }
    }
    dedup(forms)
}

/// `forms` without repeats, in their first order.
pub(crate) fn dedup(forms: Vec<String>) -> Vec<String> {
    let mut kept: Vec<String> = Vec::with_capacity(forms.len());
    for form in forms {
        if !form.is_empty() && !kept.contains(&form) {
            kept.push(form);
        }
    }
    kept
}

#[cfg(test)]
mod tests;
