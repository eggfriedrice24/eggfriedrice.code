//! The zsh integration: the two embedded scripts, where they are written, and how
//! zsh is started so it reads them.
//!
//! `assets/zsh/.zshenv` is a ZDOTDIR shim and `assets/zsh/efr-integration.zsh` emits
//! the OSC 133 and OSC 7 marks; their own comments say what each does. Both are
//! original scripts: ghostty's zsh integration emits the same marks, but it is GPLv3
//! and is never copied here.

use std::path::Path;

use efr_stdx::StdxError;

/// The ZDOTDIR shim, written as `.zshenv`.
pub(crate) const ZSHENV: &str = include_str!("../assets/zsh/.zshenv");

/// The integration script, written as `efr-integration.zsh` next to the shim.
pub(crate) const INTEGRATION: &str = include_str!("../assets/zsh/efr-integration.zsh");

/// The file names the shim and the script are written under.
pub(crate) const ZSHENV_FILE: &str = ".zshenv";
pub(crate) const INTEGRATION_FILE: &str = "efr-integration.zsh";

/// Writes both scripts into `dir`, creating it when needed, and replaces older
/// copies, so a daemon upgrade brings its own scripts. It blocks; async callers run it
/// in `spawn_blocking`.
pub(crate) fn install(dir: &Path) -> Result<(), StdxError> {
    std::fs::create_dir_all(dir)
        .map_err(|source| StdxError::CreateDir { path: dir.to_path_buf(), source })?;
    efr_stdx::fs::write_atomic(&dir.join(ZSHENV_FILE), ZSHENV.as_bytes())?;
    efr_stdx::fs::write_atomic(&dir.join(INTEGRATION_FILE), INTEGRATION.as_bytes())
}

/// True for a shell that can load the integration: a zsh.
pub(crate) fn supports(program: &Path) -> bool {
    program.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("zsh"))
}

/// The arguments of a new shell: interactive, and a login shell when asked.
pub(crate) fn args(login: bool) -> Vec<String> {
    let mut args = Vec::with_capacity(2);
    if login {
        args.push("-l".to_owned());
    }
    args.push("-i".to_owned());
    args
}

#[cfg(test)]
mod tests;
