//! The settings that run while the config hold keeps a change back, kept on disk so the
//! hold lasts over a restart of efrd (full mode, DF17).
//!
//! At start efrd reads `config.toml` as it is. Without this file, a change that a full
//! turn wrote and that the hold kept back would apply with no warning after a reboot or
//! `systemctl --user restart efrd`, which a full line may run. So while a change is
//! held, the running settings live in the state dir as one TOML file; at start, when the
//! file still loosens against them, efrd runs with the kept settings and the change
//! stays held until the user runs `efr config reload`.

use std::path::Path;

use efr_config::Settings;
use efr_protocol::HeldConfig;

use super::{full_lowered, held_text};

/// The file in the state dir that holds the running settings of a held change.
pub(crate) const KEPT_FILE: &str = "held-config.toml";

/// The first line of the file, for a person who opens it.
const HEADER: &str = "# The settings that efrd ran with while it held a change of config.toml.\n\
                      # efrd reads this file at start. Run efr config reload to load config.toml.\n";

/// Writes `settings`, the settings that run while a change is held, below `state`.
pub(crate) async fn write(state: &Path, settings: &Settings) {
    let text = match toml_edit::ser::to_document(settings) {
        Ok(document) => format!("{HEADER}{document}"),
        Err(error) => {
            tracing::warn!(error = %error, "the settings of the held change could not be kept");
            return;
        }
    };
    let state = state.to_path_buf();
    let written = tokio::task::spawn_blocking(
        move || -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            std::fs::create_dir_all(&state)?;
            efr_stdx::fs::write_atomic(&state.join(KEPT_FILE), text.as_bytes())?;
            Ok(())
        },
    )
    .await;
    match written {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "the settings of the held change could not be kept");
        }
        Err(_) => tracing::warn!("keeping the settings of the held change panicked"),
    }
}

/// Removes the kept settings below `state`, once no change is held.
pub(crate) async fn remove(state: &Path) {
    let path = state.join(KEPT_FILE);
    let removed = tokio::task::spawn_blocking(move || remove_file(&path)).await;
    if removed.is_err() {
        tracing::warn!("removing the settings of the held change panicked");
    }
}

/// The settings to start with, and the change that stays held: `file`, the settings of
/// `config.toml` with the variables and flags of this start, unless the settings kept
/// below `state` show that a change of the file is held. Then the kept settings run,
/// laid under the variables and flags of this start, with each key of `file` that
/// turns full mode off or down, and the keys that loosen stay held.
pub(crate) async fn restore(state: &Path, file: Settings) -> (Settings, Option<HeldConfig>) {
    let path = state.join(KEPT_FILE);
    let read = {
        let path = path.clone();
        tokio::task::spawn_blocking(move || std::fs::read_to_string(path)).await
    };
    let text = match read {
        Ok(Ok(text)) => text,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => return (file, None),
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "the settings of a held change could not be read");
            return (file, None);
        }
        Err(_) => return (file, None),
    };
    let Some((settings, held)) = restored(&file, &text) else {
        remove_file(&path);
        return (file, None);
    };
    tracing::warn!(loosens = ?held.loosens, "{}", held_text(&held));
    write(state, &settings).await;
    (settings, Some(held))
}

/// The kept settings of `text` over `file`, and the keys of `file` that loosen against
/// them; `None` when the text does not load or when `file` loosens nothing.
fn restored(file: &Settings, text: &str) -> Option<(Settings, HeldConfig)> {
    let kept = match Settings::parse(&file.path, Some(text)) {
        Ok(kept) => kept,
        Err(error) => {
            tracing::warn!(error = %error, "the settings of a held change do not load; config.toml applies");
            return None;
        }
    };
    // NOTE: the variables and flags of this start still win, and a key that needs a
    // restart takes the file's value now.
    let kept = match file.reloaded(kept) {
        Ok(reloaded) => reloaded.settings,
        Err(error) => {
            tracing::warn!(error = %error, "the settings of a held change do not load; config.toml applies");
            return None;
        }
    };
    let kept = full_lowered(&kept, file);
    let loosens = crate::loosens::loosening(&kept, file);
    if loosens.is_empty() {
        return None;
    }
    let held = HeldConfig { loosens: loosens.iter().map(|key| (*key).to_owned()).collect() };
    Some((kept, held))
}

fn remove_file(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(error = %error, "the settings of the held change could not be removed");
        }
    }
}
