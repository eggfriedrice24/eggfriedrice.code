//! `<runtime root>/prompt.tsv`: what the zsh plugin's prompt corner needs from efrd.
//!
//! The corner shows the scope and the permission mode of the next prompt at the right
//! edge of the user's zsh prompt (`efr_corner`), and the plugin may start no program
//! for it. The scope comes from `projects.tsv` (`projects_file.rs`). This file gives the
//! rest, in the same form: one line for each key, `<key>\t<value>`, mode 0600, by an
//! atomic rename, written only when its text changed.
//!
//! - `mode`: the mode of a prompt that names none: `permissions.mode` of the settings
//!   that efrd runs, or `auto` when that is `full` and efrd refuses full mode
//!   (`reload::full_refused`), as such a turn runs.
//! - `color.text`, `color.muted`, `color.accent`, `color.warning`, `color.error`: the
//!   colour of each role that the plugin paints, when `[render.colors]` or the theme
//!   file of `render.palette` sets it, as `#rrggbb` or a slot from 0 to 15. A role
//!   that neither sets has no line: the plugin then uses the role's default (dim,
//!   slot 3, slot 1). The plugin uses a colour only in truecolor, as efr does.
//!
//! efrd writes it at start, after each reload that installs new settings, and at a
//! prompt, next to the projects file. A theme file that cannot be read gives no
//! colours; efr itself says why.

use std::path::{Path, PathBuf};

use efr_config::{RenderColors, RoleColor, Settings, ThemeFile};

use crate::state::State;

/// The file's name in the runtime root.
pub(crate) const FILE: &str = "prompt.tsv";

/// The roles that the plugin paints, in the order of the file.
const ROLES: [&str; 5] = ["text", "muted", "accent", "warning", "error"];

/// Where the file of the runtime root `runtime` is.
pub(crate) fn path(runtime: &Path) -> PathBuf {
    runtime.join(FILE)
}

/// The file's text for `settings` in an efrd that refuses full mode when
/// `full_refused`, with the colours of `colors` (`[render.colors]` over the theme
/// file).
pub(crate) fn text(settings: &Settings, full_refused: bool, colors: &RenderColors) -> Vec<u8> {
    let mode = match settings.permissions.mode {
        efr_protocol::Mode::Full if full_refused => efr_protocol::Mode::Auto,
        mode => mode,
    };
    let mut text = format!("mode\t{}\n", mode.as_str());
    let set = colors.colors();
    for role in ROLES {
        let Some((_, colour)) = set.iter().find(|(name, _)| *name == role) else { continue };
        let value = match colour {
            RoleColor::Slot(slot) => slot.to_string(),
            RoleColor::Rgb(red, green, blue) => format!("#{red:02x}{green:02x}{blue:02x}"),
            _ => continue,
        };
        text.push_str(&format!("color.{role}\t{value}\n"));
    }
    text.into_bytes()
}

/// The colours of `settings`: `[render.colors]` over the theme file that
/// `render.palette` names, with a leading `~` as `home`. A theme file that cannot be
/// read adds nothing. It blocks: async callers run it on the blocking pool.
pub(crate) fn colors(settings: &Settings, home: &Path) -> RenderColors {
    let own = settings.render.colors.clone();
    let Some(file) = settings.render.palette_path(home) else {
        return own;
    };
    match ThemeFile::load(&file) {
        Ok(theme) => own.over(&theme.colors),
        Err(error) => {
            tracing::debug!(error = %error, "the theme file gives the prompt corner no colours");
            own
        }
    }
}

/// Writes the file of `state`'s runtime root for the settings that run now. A failed
/// write is logged: the corner then shows what it read before.
pub(crate) async fn refresh(state: &State) {
    let settings = std::sync::Arc::clone(&state.settings.borrow());
    let refused = crate::reload::full_refused(&settings, state.runs_as_root);
    let home = state.sandbox.home().path().to_path_buf();
    let file = path(state.dirs.runtime());
    let written = tokio::task::spawn_blocking(move || {
        let colors = colors(&settings, &home);
        crate::projects_file::sync(&file, &text(&settings, refused, &colors))
    })
    .await;
    log(written.ok());
}

/// Logs the outcome of a write; `None` when nothing was tried.
pub(crate) fn log(written: Option<Result<bool, efr_stdx::StdxError>>) {
    match written {
        Some(Ok(true)) => tracing::debug!("wrote the default mode for the zsh plugin"),
        Some(Err(error)) => {
            tracing::warn!(error = %error, "the default mode could not be written for the zsh plugin");
        }
        Some(Ok(false)) | None => {}
    }
}

#[cfg(test)]
mod tests;
