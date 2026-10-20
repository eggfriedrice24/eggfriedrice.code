//! The credential helpers of the git config: what each one is, by its value, and where
//! it applies (v0.0.4, DV15 and DV17).
//!
//! A helper hands git a token. In the sandbox, git runs the same helper, so a helper
//! that reads its token from a place that the sandbox does not hide gives the token to
//! the call. efr knows where some helpers keep their tokens:
//!
//! - `gh auth git-credential` reads `hosts.yml` in gh's config dir, a `GH_TOKEN`-like
//!   variable, or the keyring. efrd checks each of them for the call.
//! - libsecret uses the keyring, `store` with no `--file` its default files (secrets),
//!   `cache` a socket that only a grant reaches, and Git Credential Manager the keyring
//!   or `~/.gcm` (a secret). None of them gives a token to the call.
//! - A shell form, a `store` with its own file, and every other program can read a
//!   token from anywhere, so the call may not reach their hosts.

use crate::network::host_and_port;

/// What a credential helper is, by its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HelperKind {
    /// `!gh auth git-credential` or `!<absolute path>/gh auth git-credential`, with an
    /// optional quoted path and exactly these two arguments.
    Gh,
    /// git's libsecret helper: the keyring on the session bus.
    Libsecret,
    /// git's `store` helper with its default files, which are secrets.
    StoreDefault,
    /// git's `store` helper with `--file`: a file anywhere.
    StoreFile,
    /// git's `cache` helper: the socket of its daemon.
    Cache,
    /// Git Credential Manager (`manager`, `manager-core`).
    Manager,
    /// Any other shell form (`!...`).
    Shell,
    /// Any other program, also one at an absolute path.
    Unknown,
}

impl HelperKind {
    /// True for a helper whose token the sandbox may not hide: the call may not reach
    /// its hosts. `gh_safe` says whether efrd found that the sandbox hides gh's token
    /// for the call.
    pub const fn denies(self, gh_safe: bool) -> bool {
        match self {
            HelperKind::Gh => !gh_safe,
            HelperKind::Shell | HelperKind::StoreFile | HelperKind::Unknown => true,
            HelperKind::Libsecret
            | HelperKind::StoreDefault
            | HelperKind::Cache
            | HelperKind::Manager => false,
        }
    }
}

/// Where a credential helper applies.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HelperScope {
    /// `credential.<url>.helper` with a URL whose host efr can read: git runs it only
    /// for that host.
    Host(String),
    /// `credential.helper`, or a scope whose host efr cannot read, such as a wildcard or
    /// a scope with no scheme: every remote.
    Every,
}

impl HelperScope {
    /// The scope of `credential.<url>.helper` (`None` for `credential.helper`). A path
    /// scope, such as `https://github.com/org`, applies to its host.
    pub fn of(url: Option<&str>) -> HelperScope {
        let Some(url) = url else { return HelperScope::Every };
        if !url.contains("://") {
            return HelperScope::Every;
        }
        match host_and_port(url) {
            Some((host, _)) => HelperScope::Host(host),
            None => HelperScope::Every,
        }
    }
}

/// A credential helper that can give the call a token: [`HelperKind::denies`] is true
/// for some call.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Helper {
    /// What it is.
    pub kind: HelperKind,
    /// Where it applies.
    pub scope: HelperScope,
    /// The program's name for a note, such as `pass-git-helper`, for
    /// [`HelperKind::Unknown`]; empty for the others. Never the text of a shell form,
    /// which can hold a token.
    pub program: String,
}

/// The most characters of a program's name that [`Helper::program`] keeps.
const PROGRAM_CHARS: usize = 64;

/// What the helper `value` is. Git runs a value that starts with `!` as a shell
/// command, an absolute path as it is, and any other value as `git credential-<value>`.
pub fn helper_kind(value: &str) -> HelperKind {
    let value = value.trim();
    if let Some(command) = value.strip_prefix('!') {
        return if is_gh_command(command) { HelperKind::Gh } else { HelperKind::Shell };
    }
    let mut words = value.split_whitespace();
    let Some(first) = words.next() else { return HelperKind::Unknown };
    let Some(name) = helper_name(first) else { return HelperKind::Unknown };
    match name {
        "libsecret" => HelperKind::Libsecret,
        "store" if words.any(|word| word.starts_with("--file")) => HelperKind::StoreFile,
        "store" => HelperKind::StoreDefault,
        "cache" => HelperKind::Cache,
        "manager" | "manager-core" => HelperKind::Manager,
        _ => HelperKind::Unknown,
    }
}

/// The helper of the config entry `value` in the scope `scope`, when it is one that can
/// give the call a token; `None` for an empty value (which resets the list) and for a
/// helper that never does.
pub fn helper(scope: Option<&str>, value: &str) -> Option<Helper> {
    if value.trim().is_empty() {
        return None;
    }
    let kind = helper_kind(value);
    if !kind.denies(false) {
        return None;
    }
    let program = match kind {
        HelperKind::Unknown => program_name(value),
        _ => String::new(),
    };
    Some(Helper { kind, scope: HelperScope::of(scope), program })
}

/// The helper's name in git's terms: the word itself, or the part after
/// `git-credential-` of an absolute path. `None` for another absolute path or a
/// relative one.
fn helper_name(first: &str) -> Option<&str> {
    if first.starts_with('/') {
        let base = first.rsplit('/').next().unwrap_or_default();
        return base.strip_prefix("git-credential-");
    }
    (!first.contains('/')).then_some(first)
}

/// The program's name of a helper value, for a note: the last part of its first word,
/// cut to [`PROGRAM_CHARS`], with only the characters of a file name.
fn program_name(value: &str) -> String {
    let first = value.split_whitespace().next().unwrap_or_default();
    let base = first.rsplit('/').next().unwrap_or_default();
    base.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
        .take(PROGRAM_CHARS)
        .collect()
}

/// True when the shell command `command` (a helper value without its `!`) is `gh auth
/// git-credential`: the program `gh`, or an absolute path that ends in `/gh`, in single
/// or double quotes or bare, then exactly the two words. A newline, a variable, a
/// command substitution or another word makes it a plain shell form.
fn is_gh_command(command: &str) -> bool {
    let command = command.trim_matches([' ', '\t']);
    if command.contains(['\n', '\r']) {
        return false;
    }
    let (program, rest) = match command.chars().next() {
        Some(quote @ ('\'' | '"')) => {
            let inner = &command[1..];
            let Some(end) = inner.find(quote) else { return false };
            let program = &inner[..end];
            if quote == '"' && program.contains(['$', '`', '\\']) {
                return false;
            }
            (program, &inner[end + 1..])
        }
        _ => {
            let end = command.find([' ', '\t']).unwrap_or(command.len());
            let program = &command[..end];
            let plain =
                |c: char| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+');
            if !program.chars().all(plain) {
                return false;
            }
            (program, &command[end..])
        }
    };
    let gh = program == "gh" || (program.starts_with('/') && program.ends_with("/gh"));
    if !gh || !rest.starts_with([' ', '\t']) {
        return false;
    }
    rest.split([' ', '\t']).filter(|word| !word.is_empty()).eq(["auth", "git-credential"])
}
