//! The path tables that the engine and the `auto` sandbox share, so the two cannot
//! disagree about a secret, a mask or a floor.
//!
//! The daemon reads these and passes plain paths to `efr-sandbox`, which never reads
//! them itself. The engine secrets live in `path_class.rs` with the classes; the
//! tables here are the ones that only the `auto` mode adds.

use std::path::{Path, PathBuf};

/// Paths, relative to the home directory, that a contained call reads as empty besides
/// the engine secrets: more credential stores, browser and messenger profiles, and
/// shell histories.
///
/// Only the `auto` mode reads this table. In `auto`, `read_file` of such a path asks
/// (a `masked_read` exit, user only), and a write to it is a `write` exit that runs in
/// the exit child, because a mask never opens for writing. `$XAUTHORITY` and
/// `$HISTFILE` depend on the environment, so the daemon adds them.
pub const SANDBOX_MASKS: &[&str] = &[
    // More credential stores.
    ".aws",
    ".kube",
    ".docker",
    ".codex",
    ".claude/.credentials.json",
    ".local/share/opencode",
    ".config/op",
    ".config/rclone",
    ".config/hub",
    ".config/github-copilot",
    ".config/containers/auth.json",
    ".local/share/kwalletd",
    ".pki",
    ".Xauthority",
    ".gradle/gradle.properties",
    ".m2/settings.xml",
    ".config/Bitwarden",
    ".config/1Password",
    ".config/KeePassXC",
    // Browser and messenger profiles.
    ".mozilla",
    ".librewolf",
    ".zen",
    ".thunderbird",
    ".var/app",
    ".config/chromium",
    ".config/google-chrome",
    ".config/BraveSoftware",
    ".config/vivaldi",
    ".config/Signal",
    ".config/discord",
    ".config/Slack",
    ".local/share/TelegramDesktop",
    ".cache/mozilla",
    ".cache/chromium",
    ".cache/google-chrome",
    // Shell histories.
    ".zsh_history",
    ".bash_history",
    ".local/share/fish/fish_history",
    ".python_history",
    ".node_repl_history",
];

/// The masks of [`SANDBOX_MASKS`] under `home`.
pub fn sandbox_masks(home: &Path) -> Vec<PathBuf> {
    SANDBOX_MASKS.iter().map(|relative| home.join(relative)).collect()
}

/// Names that stay read-only inside every write root of a contained call: agent and
/// editor configs that a later tool runs or obeys. A name that ends in `/`
/// is a directory.
///
/// In `auto`, a write to one of them inside a write root is a `persistence` exit, for
/// the shell tool and for `write_file` alike. A file edit of one may go to the
/// classifier as a floor file edit (spec 9.1).
pub const PROTECTED_NAMES: &[&str] = &[
    ".mcp.json",
    ".claude/",
    ".codex/",
    ".agents/",
    ".cursor/",
    ".opencode/",
    "opencode.json",
    "opencode.jsonc",
    ".vscode/",
    ".envrc",
    ".direnv/",
    ".efr/",
];

/// [`PROTECTED_NAMES`], for callers that take a function.
pub fn protected_names() -> &'static [&'static str] {
    PROTECTED_NAMES
}

/// Paths, relative to the home directory, that run code later outside the sandbox:
/// shell startup files, autostart and services, and tool config that names programs.
/// They stay read-only in a contained call, and in `auto` a write to one is a
/// `persistence` exit. Only the user approves it, except a file edit that a user
/// message names while efrd keeps a backup, which the classifier judges (spec 9.1);
/// [`PERSISTENCE_FLOOR_GROUPS`] gives the group of each entry.
///
/// The daemon adds what depends on the machine: `$ZDOTDIR`, the `PATH` dirs inside a
/// write root, the targets of dotfile links and `sandbox.protect`
/// ([`Locations::with_floor_root`](crate::Locations::with_floor_root)).
pub const PERSISTENCE_FLOORS: &[&str] = &[
    // Shell startup.
    ".zshenv",
    ".zprofile",
    ".zshrc",
    ".zlogin",
    ".zlogout",
    ".bashrc",
    ".bash_profile",
    ".bash_login",
    ".bash_logout",
    ".profile",
    ".config/fish",
    ".config/environment.d",
    ".pam_environment",
    ".xprofile",
    ".xinitrc",
    // Autostart and services.
    ".config/systemd",
    ".config/autostart",
    ".local/share/systemd",
    ".local/share/applications",
    ".config/hypr",
    ".config/sway",
    ".config/i3",
    // Tool config that runs code.
    ".gitconfig",
    ".config/git",
    ".cargo/config.toml",
    ".cargo/config",
    ".cargo/bin",
    ".local/bin",
    ".config/nvim",
    ".vimrc",
    ".config/direnv",
];

/// What a floor is for: the program or the place that runs its files later. In `auto`,
/// a file edit of a floor in a group may go to the classifier when a user message names
/// it and efrd keeps a backup (spec 9.1); a floor in no group, such as
/// `sandbox.protect`, stays with the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FloorGroup {
    /// The startup files of zsh, also those in `$ZDOTDIR`.
    Zsh,
    /// The startup files of bash and the login shell.
    Bash,
    /// The config of fish.
    Fish,
    /// The environment and start files of the desktop session.
    Session,
    /// The units of the user's systemd.
    Systemd,
    /// Autostart entries and desktop entries.
    Autostart,
    /// The config of Hyprland.
    Hypr,
    /// The config of sway.
    Sway,
    /// The config of i3.
    I3,
    /// The config of git.
    Git,
    /// The config of cargo.
    Cargo,
    /// Program files on the `PATH`.
    Programs,
    /// The config of Neovim.
    Nvim,
    /// The config of Vim.
    Vim,
    /// The config of direnv.
    Direnv,
    /// Agent and editor configs in a write root ([`PROTECTED_NAMES`]).
    Agent,
}

/// The group of each entry of [`PERSISTENCE_FLOORS`], in the same order.
pub const PERSISTENCE_FLOOR_GROUPS: &[(&str, FloorGroup)] = &[
    (".zshenv", FloorGroup::Zsh),
    (".zprofile", FloorGroup::Zsh),
    (".zshrc", FloorGroup::Zsh),
    (".zlogin", FloorGroup::Zsh),
    (".zlogout", FloorGroup::Zsh),
    (".bashrc", FloorGroup::Bash),
    (".bash_profile", FloorGroup::Bash),
    (".bash_login", FloorGroup::Bash),
    (".bash_logout", FloorGroup::Bash),
    (".profile", FloorGroup::Bash),
    (".config/fish", FloorGroup::Fish),
    (".config/environment.d", FloorGroup::Session),
    (".pam_environment", FloorGroup::Session),
    (".xprofile", FloorGroup::Session),
    (".xinitrc", FloorGroup::Session),
    (".config/systemd", FloorGroup::Systemd),
    (".config/autostart", FloorGroup::Autostart),
    (".local/share/systemd", FloorGroup::Systemd),
    (".local/share/applications", FloorGroup::Autostart),
    (".config/hypr", FloorGroup::Hypr),
    (".config/sway", FloorGroup::Sway),
    (".config/i3", FloorGroup::I3),
    (".gitconfig", FloorGroup::Git),
    (".config/git", FloorGroup::Git),
    (".cargo/config.toml", FloorGroup::Cargo),
    (".cargo/config", FloorGroup::Cargo),
    (".cargo/bin", FloorGroup::Programs),
    (".local/bin", FloorGroup::Programs),
    (".config/nvim", FloorGroup::Nvim),
    (".vimrc", FloorGroup::Vim),
    (".config/direnv", FloorGroup::Direnv),
];

/// The group of the built-in floor of [`PERSISTENCE_FLOORS`] that `path`, a path in
/// normal form, lies at or below in the home directory `home`.
pub(crate) fn floor_group(home: &Path, path: &Path) -> Option<FloorGroup> {
    PERSISTENCE_FLOOR_GROUPS
        .iter()
        .find(|(relative, _)| path.starts_with(home.join(relative)))
        .map(|(_, group)| *group)
}

/// The zsh startup files that `$ZDOTDIR` holds. The daemon makes each a floor in the
/// hidden shell's `$ZDOTDIR`, as [`PERSISTENCE_FLOORS`] does in the home directory.
pub const ZSH_STARTUP_FILES: &[&str] = &[".zshenv", ".zprofile", ".zshrc", ".zlogin", ".zlogout"];

/// The floors of [`PERSISTENCE_FLOORS`] under `home`.
pub fn persistence_floors(home: &Path) -> Vec<PathBuf> {
    PERSISTENCE_FLOORS.iter().map(|relative| home.join(relative)).collect()
}

/// Files of a repository's git dir that name programs git runs: they stay read-only
/// in a contained call, so a declared write to one is a `persistence` exit.
pub(crate) const GIT_SURFACE: &[&str] = &["config", "config.worktree", "hooks"];

/// True for a project `.env` file that the default `sandbox.mask_globs` mask: `.env`
/// and `.env.*`, except the examples that hold no values.
pub(crate) fn is_env_file(name: &str) -> bool {
    match name.strip_prefix(".env") {
        Some("") => true,
        Some(rest) => {
            rest.starts_with('.') && !matches!(rest, ".example" | ".sample" | ".template")
        }
        None => false,
    }
}

#[cfg(test)]
mod tests;
