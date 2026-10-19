//! `efr sandbox allow` and `efr sandbox allow remove N`: the always-allow rules of
//! `config.toml` (`[[sandbox.allow]]`, phase E).
//!
//! `allow` reads the file, as the daemon reads it, and lists each rule with its number
//! and in words. A rule that efrd disabled, because it no longer passes a floor check,
//! gets `; disabled: <reason>` from the sandbox warnings of `admin.status`; without a
//! daemon, or when the daemon reads another file, a note says that efr cannot tell.
//!
//! `allow remove N` shows rule N in words and asks `remove sandbox.allow[N]: <rule>?
//! y/n` with one key, as `efr undo` does. Only `y` removes it, through `efr-config`'s
//! writer, which keeps comments and layout and writes the file behind a link. The writer
//! reads the file again first and removes the entry only when entry N still holds the
//! rule that the question showed. Then the daemon reloads, as after `efr config set`.
//! This and efrd's writer of an `a` answer are the only code that writes
//! `sandbox.allow`.

use std::io;
use std::path::{Path, PathBuf};

use efr_client::ClientError;
use efr_config::{AllowRule, ConfigError, ConfigFile, Settings};

use crate::commands::config::{self, config_path};
use crate::commands::undo;
use crate::context::Context;
use crate::error::CliError;
use crate::format::{self, Tone, always};
use crate::live::effective_width;
use crate::output::Output;
use crate::settings;

/// How often `remove` plans its change again when the file changed while it wrote.
const ATTEMPTS: usize = 3;

/// `efr sandbox allow`: every rule of the file with its number, and the ones that efrd
/// disabled.
pub(super) async fn list(ctx: &Context, out: &mut Output) -> Result<(), CliError> {
    let path = config_path(ctx);
    let rules = read_rules(&path, out).await?;
    if rules.is_empty() {
        return out.out(&format!("no always rules in {}\n", path.display()));
    }
    let disabled = disabled_rules(ctx, out, &path).await;
    let mut text = String::new();
    for (index, rule) in rules.iter().enumerate() {
        let reason = disabled.iter().find(|(at, _)| *at == index).map(|(_, why)| why.as_str());
        text.push_str(&always::list_row(index, &rule.describe(), reason));
        text.push('\n');
    }
    out.out(&text)
}

/// The rules that the daemon disabled, from its sandbox warnings; empty with a note on
/// stderr when the daemon cannot say for this file.
async fn disabled_rules(ctx: &Context, out: &mut Output, path: &Path) -> Vec<(usize, String)> {
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    let note = |out: &mut Output, text: &str| {
        let line = format::paint(text, Tone::Dim, &options);
        out.err(&format!("{line}\n"));
    };
    let status = match config::status(ctx).await {
        Ok(status) => status,
        Err(CliError::Client(ClientError::DaemonNotRunning { .. })) => {
            note(out, "the daemon is not running, so efr cannot say which rules are disabled");
            return Vec::new();
        }
        Err(error) => {
            let text = format!("efr cannot say which rules are disabled: {error}");
            note(out, &format::one_line(&text));
            return Vec::new();
        }
    };
    if let Some(daemon) = status.config.as_ref().filter(|daemon| daemon.path != path) {
        let text = format!(
            "the daemon reads {}, so efr cannot say which rules of this file are disabled",
            daemon.path.display()
        );
        note(out, &format::one_line(&text));
        return Vec::new();
    }
    if status.config.as_ref().is_some_and(|daemon| daemon.reload_error.is_some()) {
        note(out, "the daemon could not read the file at its last reload; efr status says why");
    }
    status.sandbox.map(|sandbox| always::disabled(&sandbox.warnings)).unwrap_or_default()
}

/// `efr sandbox allow remove N`: removes rule `index` after a question.
pub(super) async fn remove(ctx: &Context, out: &mut Output, index: usize) -> Result<(), CliError> {
    let path = config_path(ctx);
    let rules = read_rules(&path, out).await?;
    let Some(rule) = rules.get(index).cloned() else {
        return Err(CliError::NoAllowRule { index, count: rules.len() });
    };
    if !ctx.keys.available() {
        return Err(CliError::AllowRemoveNeedsTerminal);
    }
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    let question = always::remove_question(index, &rule.describe());
    out.out(&format!("{}\n", format::paint(&question, Tone::Attention, &options)))?;
    if !undo::answer(ctx).await? {
        out.err(&format!("{}\n", format::paint("nothing changed", Tone::Dim, &options)));
        return Ok(());
    }
    let mut attempt = 0;
    let target = loop {
        attempt += 1;
        let (owned_path, owned_rule) = (path.clone(), rule.clone());
        let removed =
            tokio::task::spawn_blocking(move || remove_rule(&owned_path, index, &owned_rule))
                .await
                .unwrap_or_else(|join| {
                    Err(Removal::Config(ConfigError::Read {
                        path: path.clone(),
                        source: io::Error::other(join),
                    }))
                });
        match removed {
            Ok(target) => break target,
            Err(Removal::Config(ConfigError::Changed { .. })) if attempt < ATTEMPTS => {}
            Err(Removal::Moved) => return Err(CliError::AllowRuleMoved { index }),
            Err(Removal::Config(error)) => {
                out.err(&format!("efr: {}: {}\n", path.display(), settings::describe(&error)));
                return Err(CliError::ConfigInvalid);
            }
        }
    };
    out.out(&format!(
        "{}: removed {}[{index}]: {}\n",
        target.display(),
        always::KEY,
        format::one_line(&rule.describe())
    ))?;
    config::reload_after_change(ctx, out).await
}

/// Why a removal did not happen.
#[derive(Debug)]
enum Removal {
    /// The file could not be read or written, or changed while it was written.
    Config(ConfigError),
    /// The entry at the index is another rule now.
    Moved,
}

impl From<ConfigError> for Removal {
    fn from(error: ConfigError) -> Self {
        Removal::Config(error)
    }
}

/// Removes entry `index` of `sandbox.allow` from the file at `path` when it still holds
/// `rule`, and returns the file written: the end of a link, or the file itself.
fn remove_rule(path: &Path, index: usize, rule: &AllowRule) -> Result<PathBuf, Removal> {
    let file = ConfigFile::open(path)?;
    let now = Settings::parse(file.path(), file.text())?;
    if now.sandbox.allow.get(index) != Some(rule) {
        return Err(Removal::Moved);
    }
    let mut edit = file.edit()?;
    edit.remove_table_entry(always::KEY, index)?;
    file.write(&edit)?;
    Ok(file.target().to_path_buf())
}

/// The rules of the file at `path`, as the daemon reads them; none when the file does
/// not exist. A file with an error is reported on stderr with its place.
async fn read_rules(path: &Path, out: &mut Output) -> Result<Vec<AllowRule>, CliError> {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(source) => {
            let error = ConfigError::Read { path: path.to_path_buf(), source };
            return Err(CliError::ConfigFile { source: Box::new(error) });
        }
    };
    match Settings::parse(path, text.as_deref()) {
        Ok(settings) => Ok(settings.sandbox.allow),
        Err(error) => {
            out.err(&format!("efr: {}: {}\n", path.display(), settings::describe(&error)));
            Err(CliError::ConfigInvalid)
        }
    }
}

#[cfg(test)]
mod tests;
