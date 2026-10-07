//! The hidden shell's environment: the user's, minus what belongs to the daemon or to
//! another terminal, plus the few variables efr sets.

use std::collections::BTreeMap;
use std::path::Path;

use efr_protocol::ConversationId;

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
/// (to skip `exec tmux` or an instant prompt, for example). The efr zsh plugin reads
/// it and stays out of a hidden shell.
pub(crate) const HIDDEN_SHELL: &str = "EFR_HIDDEN_SHELL";

/// Pagers turned off. Nobody reads a pager on the hidden screen: `git log` or
/// `systemctl status` would open `less` there, and the run would wait until someone
/// quit it. `cat` is the value git, systemd, man, gh and bat all treat as "no pager";
/// the AWS CLI takes an empty value. The zsh integration sets the same values again
/// after the user's startup files, which often export `PAGER=less`; its tests keep the
/// two lists equal.
pub(crate) const PAGERS: &[(&str, &str)] = &[
    ("PAGER", "cat"),
    ("GIT_PAGER", "cat"),
    ("SYSTEMD_PAGER", "cat"),
    ("MANPAGER", "cat"),
    ("AWS_PAGER", ""),
    ("GH_PAGER", "cat"),
    ("BAT_PAGER", "cat"),
];

/// The variables that name an editor. Nobody can use one on the hidden screen either:
/// `git commit` without `-m` or `crontab -e` would open it there and wait for the
/// run's timeout. Each names the stub that the session writes next to the zsh
/// integration ([`crate::integration::EDITOR_FILE`]), which fails at once and says
/// why. The zsh integration sets them again after the user's startup files, as it does
/// the pagers; its tests keep the two lists equal.
pub(crate) const EDITORS: &[&str] =
    &["EDITOR", "VISUAL", "GIT_EDITOR", "GIT_SEQUENCE_EDITOR", "SUDO_EDITOR", "SYSTEMD_EDITOR"];

/// The programs of [`ShellConfig::trusted_programs`], separated by spaces, for a zsh
/// with the integration. The integration reads it once, right after the user's
/// `.zshenv`, and removes it from the environment.
pub(crate) const TRUSTED_PROGRAMS: &str = "_EFR_HS_TRUSTED_PROGRAMS";

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
        if !config.trusted_programs.is_empty() {
            env.insert(TRUSTED_PROGRAMS.to_owned(), config.trusted_programs.join(" "));
        }
    }
    env.insert("TERM".to_owned(), config.term.clone());
    env.insert("COLORTERM".to_owned(), config.colorterm.clone());
    // The holder passes exactly this environment, and a shell trusts PWD when it names
    // its working directory, which keeps the logical path the user gave.
    env.insert("PWD".to_owned(), cwd.to_string_lossy().into_owned());
    env.insert(HIDDEN_SHELL.to_owned(), "1".to_owned());
    for (name, value) in PAGERS {
        env.insert((*name).to_owned(), (*value).to_owned());
    }
    let editor = config.integration_dir.join(crate::integration::EDITOR_FILE);
    let editor = editor.to_string_lossy();
    for name in EDITORS {
        env.insert((*name).to_owned(), editor.clone().into_owned());
    }
    env
}

/// The conversation's sandbox dir, for a zsh with the integration. The integration
/// reads it once, right after the user's `.zshenv`, and removes it from the
/// environment, as it does [`TRUSTED_PROGRAMS`].
pub(crate) const SANDBOX_DIR: &str = "_EFR_HS_SBX_DIR";

/// The launcher of sandboxed calls, read the same way.
pub(crate) const SANDBOX_LAUNCHER: &str = "_EFR_HS_SBX_BIN";

/// The variables of the auto mode's wrapper for the shell of `conversation`: its
/// sandbox dir and the launcher, when the config names both and the shell is a zsh with
/// the integration; nothing otherwise, and the wrapper then refuses every call.
pub(crate) fn sandbox_env(
    config: &ShellConfig,
    conversation: ConversationId,
    integration: bool,
) -> Vec<(String, String)> {
    let (Some(root), Some(launcher)) = (&config.sandbox_dir, &config.sandbox_launcher) else {
        return Vec::new();
    };
    if !integration {
        return Vec::new();
    }
    let dir = crate::sandbox::conversation_dir(root, conversation);
    vec![
        (SANDBOX_DIR.to_owned(), dir.to_string_lossy().into_owned()),
        (SANDBOX_LAUNCHER.to_owned(), launcher.to_string_lossy().into_owned()),
    ]
}

fn is_scrubbed(name: &str) -> bool {
    efr_stdx::process::SCRUBBED_ENV.contains(&name)
        || SCRUBBED_NAMES.contains(&name)
        || SCRUBBED_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

#[cfg(test)]
mod tests;
