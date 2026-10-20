//! The writer of always-allow rules (phase E, efr's auto spec 11.2, DE26): the
//! `AllowWriter` of the conversations.
//!
//! An `a` answer saves the rule that its question offered. The writer checks the rule
//! with [`Engine::check_allow`] on the running engine, the same check that a load of
//! `config.toml` runs, so it never writes a rule that the next load would disable. Then
//! it opens `config.toml` (through a link, as `efr config set` does), adds the rule to
//! `sandbox.allow` in the form that the file uses ([`Edit::add_table_entry`], which keeps
//! every comment), and writes the file; when the file changed under it, it plans the
//! change again, at most [`TRIES`] times. A rule that the list already holds is not
//! added again: the outcome names its index. Then it asks the reload task for a reload
//! and waits for it, at most [`RELOAD_WAIT`], so the next call sees the rule.
//!
//! It never fails with an error: each failure is a `NotSaved` with its reason, and the
//! turn runs the call as a "yes" all the same. A file without the owner's write bit
//! counts as read-only: the atomic write would replace it, but the user made it so.
//!
//! A rule that fails a floor check when the rules load stays in the list, disabled:
//! [`disabled_warnings`] gives its warning for `efr sandbox check` and
//! `efr sandbox allow`, and [`warn_disabled`] logs it once (DE44).
//!
//! The other path that writes `sandbox.allow` is `efr sandbox allow remove` in efr-cli.
//! Any program of the user's that writes `config.toml` can add a rule too; the reload
//! watcher reads it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use efr_config::{CONFIG_FILE, ConfigError, ConfigFile, Settings};
use efr_conversation::AllowWriter;
use efr_permissions::{AllowRule, Engine};
use efr_protocol::{AlwaysSaved, NotSavedReason};
use efr_stdx::time::Clock;
use tokio::sync::watch;

use crate::reload::Reloads;

/// How many times the writer plans the change when `config.toml` changed under it.
pub(crate) const TRIES: usize = 3;

/// How long the writer waits for the reload after it wrote the rule.
pub(crate) const RELOAD_WAIT: Duration = Duration::from_secs(5);

/// The key of the always rules in `config.toml`.
const KEY: &str = "sandbox.allow";

/// The warning of each disabled rule of `engine` in `SandboxStatus::warnings`, as
/// `efr sandbox check` shows it, such as `sandbox.allow[1] is disabled: it names a git
/// dir` (DE44).
pub(crate) fn disabled_warnings(engine: &Engine) -> Vec<String> {
    engine
        .allow_rules()
        .iter()
        .enumerate()
        .filter_map(|(index, (_, disabled))| {
            disabled.as_ref().map(|floor| format!("{KEY}[{index}] is disabled: {floor}"))
        })
        .collect()
}

/// Logs one warning for each rule that `engine` disables, unless `before` disabled the
/// same rule at the same place for the same reason: at the start (`before` is `None`)
/// and when a reload sends a new engine. A disabled rule never stops efrd (DE44).
/// Returns the text of each warning, which a reload passes to `efr config reload`
/// (DV44).
pub(crate) fn warn_disabled(engine: &Engine, before: Option<&Engine>) -> Vec<String> {
    let known = before.map(Engine::allow_rules).unwrap_or_default();
    let mut warnings = Vec::new();
    for (index, entry) in engine.allow_rules().iter().enumerate() {
        let (rule, Some(floor)) = entry else { continue };
        if known.get(index) == Some(entry) {
            continue;
        }
        let text = format!("{KEY}[{index}] is disabled: {floor}; it covers no call");
        tracing::warn!(index, rule = %rule.describe(), reason = %floor, "{text}");
        warnings.push(text);
    }
    warnings
}

/// Saves the rule of an `a` answer: see the module doc.
#[derive(Debug, Clone)]
pub(crate) struct RuleWriter {
    /// efr's config directory, which holds `config.toml`.
    config: PathBuf,
    /// The running engine, whose floor check a rule must pass first.
    engine: watch::Receiver<Arc<Engine>>,
    /// The reload task.
    reloads: Reloads,
    /// The clock of the reload wait.
    clock: Arc<dyn Clock>,
}

impl RuleWriter {
    pub(crate) fn new(
        config: &Path,
        engine: watch::Receiver<Arc<Engine>>,
        reloads: Reloads,
        clock: Arc<dyn Clock>,
    ) -> Self {
        RuleWriter { config: config.to_path_buf(), engine, reloads, clock }
    }

    /// Waits for a reload of the file, so the next call sees the rule.
    async fn reload(&self) -> Result<(), NotSavedReason> {
        match self.clock.timeout(RELOAD_WAIT, self.reloads.request("always rule")).await {
            Ok(Ok(result)) if result.applied => Ok(()),
            Ok(Ok(result)) => {
                // NOTE: the writer checked the whole file as a load does, so only a change
                // that came after it can make the reload fail.
                let error = result.error.map(|error| error.message).unwrap_or_default();
                tracing::warn!(error = %error, "the reload after an always rule did not apply");
                Err(NotSavedReason::ReloadTimeout)
            }
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "the reload after an always rule did not run");
                Err(NotSavedReason::ReloadTimeout)
            }
            Err(_) => Err(NotSavedReason::ReloadTimeout),
        }
    }
}

#[async_trait]
impl AllowWriter for RuleWriter {
    async fn add(&self, rule: AllowRule) -> AlwaysSaved {
        let engine = Arc::clone(&self.engine.borrow());
        if let Err(floor) = engine.check_allow(&rule) {
            tracing::warn!(rule = %rule.describe(), reason = %floor, "an always rule fails its load check; it is not saved");
            return AlwaysSaved::NotSaved { reason: NotSavedReason::LoadCheck };
        }
        let path = self.config.join(CONFIG_FILE);
        let described = rule.describe();
        let written = tokio::task::spawn_blocking(move || write_rule(&path, &rule))
            .await
            .unwrap_or(Err(NotSavedReason::Io));
        let index = match written {
            Ok(index) => index,
            Err(reason) => return AlwaysSaved::NotSaved { reason },
        };
        if let Err(reason) = self.reload().await {
            return AlwaysSaved::NotSaved { reason };
        }
        AlwaysSaved::Saved { index: u32::try_from(index).unwrap_or(u32::MAX), rule: described }
    }
}

/// Adds `rule` to `sandbox.allow` of the config file at `path`, and returns its index:
/// the index of an equal rule that the list already holds, or of the new entry. It
/// blocks.
pub(crate) fn write_rule(path: &Path, rule: &AllowRule) -> Result<usize, NotSavedReason> {
    write_rule_with(path, rule, &mut || {})
}

/// [`write_rule`], with `before_write` run after each plan and before its write, so a
/// test can change the file in between.
fn write_rule_with(
    path: &Path,
    rule: &AllowRule,
    before_write: &mut dyn FnMut(),
) -> Result<usize, NotSavedReason> {
    for _ in 0..TRIES {
        let file = ConfigFile::open(path).map_err(|error| reason(&error))?;
        if read_only(file.target()) {
            return Err(NotSavedReason::ReadOnly);
        }
        let current = Settings::parse(file.path(), file.text()).map_err(|error| reason(&error))?;
        if let Some(index) = current.sandbox.allow.iter().position(|known| known == rule) {
            return Ok(index);
        }
        let mut edit = file.edit().map_err(|error| reason(&error))?;
        let index = edit.add_table_entry(KEY, rule).map_err(|error| reason(&error))?;
        before_write();
        match file.write(&edit) {
            Ok(_) => return Ok(index),
            Err(ConfigError::Changed { .. }) => {}
            Err(error) => return Err(reason(&error)),
        }
    }
    Err(NotSavedReason::Changed)
}

/// True when `path` exists and its owner may not write it.
fn read_only(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o200 == 0)
}

/// The reason of a failed write for `error`: a file or a directory that may not be
/// written is read-only, a file that changed under each try changed, the rest is I/O.
fn reason(error: &ConfigError) -> NotSavedReason {
    if matches!(error, ConfigError::Changed { .. }) {
        return NotSavedReason::Changed;
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = source {
        if let Some(io) = current.downcast_ref::<std::io::Error>()
            && matches!(
                io.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
            )
        {
            return NotSavedReason::ReadOnly;
        }
        source = current.source();
    }
    NotSavedReason::Io
}

#[cfg(test)]
mod tests;
