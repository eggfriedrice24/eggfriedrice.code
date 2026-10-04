//! The hidden shell's environment: the user's, minus what belongs to the daemon or to
//! another terminal, plus the few variables efr sets.

use std::collections::BTreeMap;
use std::path::Path;

use crate::ShellConfig;

/// Variables removed by name, on top of `efr_stdx::process::SCRUBBED_ENV` (the
/// daemon's systemd unit).
pub(crate) const SCRUBBED_NAMES: &[&str] = &[
    // Facts about the terminal that started the daemon (`just run`), not about the
    // hidden screen.
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERMINFO",
    "COLORFGBG",
    "WINDOWID",
    "VTE_VERSION",
    "TMUX",
    "TMUX_PANE",
    "STY",
    // A stale size makes programs ignore the PTY's own.
    "COLUMNS",
    "LINES",
    // The hidden shell is a first-level shell of its own.
    "SHLVL",
    "OLDPWD",
    "_",
];

/// Variables removed by prefix: efr's own configuration, and the session variables of
/// terminal emulators and multiplexers.
pub(crate) const SCRUBBED_PREFIXES: &[&str] =
    &["EFR_", "_EFR_", "GHOSTTY_", "KITTY_", "WEZTERM_", "ITERM_", "KONSOLE_", "ZELLIJ"];

/// Where the shim finds the user's own `ZDOTDIR`.
pub(crate) const USER_ZDOTDIR: &str = "_EFR_USER_ZDOTDIR";

/// Set in every hidden shell, so the user's startup files can tell it from a terminal
/// (to skip `exec tmux` or an instant prompt, for example). Nothing in efr reads it.
pub(crate) const HIDDEN_SHELL: &str = "EFR_HIDDEN_SHELL";

/// The whole environment of a new hidden shell started in `cwd`. `integration` is
/// true for a zsh that gets the ZDOTDIR shim.
pub(crate) fn shell_env(
    config: &ShellConfig,
    cwd: &Path,
    integration: bool,
) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = config
        .base_env
        .iter()
        .filter(|(name, _)| !is_scrubbed(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    if integration {
        if let Some(user_zdotdir) = env.remove("ZDOTDIR") {
            env.insert(USER_ZDOTDIR.to_owned(), user_zdotdir);
        }
        env.insert("ZDOTDIR".to_owned(), config.integration_dir.to_string_lossy().into_owned());
    }
    env.insert("TERM".to_owned(), config.term.clone());
    env.insert("COLORTERM".to_owned(), config.colorterm.clone());
    // The holder passes exactly this environment, and a shell trusts PWD when it names
    // its working directory, which keeps the logical path the user gave.
    env.insert("PWD".to_owned(), cwd.to_string_lossy().into_owned());
    env.insert(HIDDEN_SHELL.to_owned(), "1".to_owned());
    env
}

fn is_scrubbed(name: &str) -> bool {
    efr_stdx::process::SCRUBBED_ENV.contains(&name)
        || SCRUBBED_NAMES.contains(&name)
        || SCRUBBED_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

#[cfg(test)]
mod tests;
