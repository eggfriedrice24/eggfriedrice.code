//! The zsh integration: the embedded scripts, where they are written, and how zsh is
//! started so it reads them.
//!
//! `assets/zsh/.zshenv` is a ZDOTDIR shim and `assets/zsh/efr-integration.zsh` emits
//! the OSC 133 and OSC 7 marks; their own comments say what each does. Both are
//! original scripts: ghostty's zsh integration emits the same marks, but it is GPLv3
//! and is never copied here. `assets/efr-editor` is the editor of every hidden shell,
//! a zsh or not: it fails at once and says why. `assets/zsh/efr-child.zsh` is the child
//! shell of each sandboxed call of the auto mode: `efr-sbx` runs it and reads its
//! records.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use efr_stdx::StdxError;

/// The ZDOTDIR shim, written as `.zshenv`.
pub(crate) const ZSHENV: &str = include_str!("../assets/zsh/.zshenv");

/// The integration script, written as `efr-integration.zsh` next to the shim.
pub(crate) const INTEGRATION: &str = include_str!("../assets/zsh/efr-integration.zsh");

/// The editor stub, written as `efr-editor` next to the shim.
pub(crate) const EDITOR: &str = include_str!("../assets/efr-editor");

/// The child shell's script of the auto mode's launcher, written as `efr-child.zsh`
/// next to the shim. `efr-sbx` runs it as `zsh -f efr-child.zsh SNAPSHOT STATE LINE`.
pub(crate) const CHILD: &str = include_str!("../assets/zsh/efr-child.zsh");

/// The file names the shim, the scripts and the editor are written under.
pub(crate) const ZSHENV_FILE: &str = ".zshenv";
pub(crate) const INTEGRATION_FILE: &str = "efr-integration.zsh";
pub(crate) const EDITOR_FILE: &str = "efr-editor";
pub(crate) const CHILD_FILE: &str = "efr-child.zsh";

/// The editor's mode: the user runs it, and so does root for a program that `sudo`
/// starts, which needs no bit of its own.
const EDITOR_MODE: u32 = 0o700;

/// Writes the four files into `dir`, creating it when needed, and replaces older
/// copies, so a daemon upgrade brings its own scripts. It blocks; async callers run it
/// in `spawn_blocking`.
pub(crate) fn install(dir: &Path) -> Result<(), StdxError> {
    std::fs::create_dir_all(dir)
        .map_err(|source| StdxError::CreateDir { path: dir.to_path_buf(), source })?;
    efr_stdx::fs::write_atomic(&dir.join(ZSHENV_FILE), ZSHENV.as_bytes())?;
    efr_stdx::fs::write_atomic(&dir.join(INTEGRATION_FILE), INTEGRATION.as_bytes())?;
    efr_stdx::fs::write_atomic(&dir.join(CHILD_FILE), CHILD.as_bytes())?;
    let editor = dir.join(EDITOR_FILE);
    efr_stdx::fs::write_atomic(&editor, EDITOR.as_bytes())?;
    // NOTE: write_atomic leaves the new file at 0600, which nobody can execute. Shells
    // start only once the install has returned, so none of them sees the file then.
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(EDITOR_MODE))
        .map_err(|source| StdxError::WriteFile { path: editor, source })
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
