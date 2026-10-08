//! `settings`: the model reads efr's settings freely and changes `config.toml` only
//! through a checked change that the user approves.
//!
//! The tool lives here and not in `efr-tools`, because it needs `efr-config` and the
//! daemon's settings, and `efr-config` reaches `efr-permissions`, which `efr-tools`
//! must never reach. [`DaemonToolbox`](super::DaemonToolbox) offers it next to the
//! registry's tools.
//!
//! A change goes through four steps, each against the file as it is at that moment:
//!
//! 1. `requirements` plans it: the writer of `efr-config` applies it to the file (or to
//!    the example when there is none), the whole new file is checked as a load would
//!    check it, the default model must be in the model list and the default effort one
//!    that model takes, and a rule that names secrets is refused. A change that fails
//!    any of this goes back to the model, and nobody is asked. A valid one declares a
//!    [`SettingsChange`], which the engine always asks about, in every mode and
//!    whatever the rules say, and denies for a remote turn.
//! 2. `preview` plans it again and shows the unified diff of the file, the same diff a
//!    `write_file` approval shows. What it showed is kept for the call.
//! 3. The user answers.
//! 4. `invoke` plans it once more and writes only when the file and the new text are
//!    what the user saw. When the file changed meanwhile, nothing is written and the
//!    model is told to call the tool again, which plans against the new file and asks
//!    again. The writer compares the file's hash once more right before it writes.
//!    Then the daemon reloads, so the change applies from the next turn.
//!
//! The tool never declares the file as a path: config protection denies every tool's
//! write there, and the settings tool writes through `efr-config`'s writer after the
//! user approved the change itself.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use efr_config::{
    Applies, CONFIG_FILE, ConfigError, ConfigFile, Edit, FileState, Kind, SandboxSettings,
    Settings, Source, SudoCache, WriteProjects,
};
use efr_conversation::{ToolCall, ToolOutcome};
use efr_permissions::{Effect, Engine, Requirements, Rule, SettingsChange};
use efr_protocol::{AdminConfigReloadResult, CallId, ModelInfo};
use efr_provider::ToolDefinition;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::catalog::{self, Models};
use crate::reload::{Outcome, Reloads};

/// The tool's name.
pub(crate) const NAME: &str = "settings";

/// How many shown changes the tool keeps for calls that wait for an answer. A call the
/// user denied never comes back for its entry, so the oldest goes first.
const SHOWN: usize = 32;

/// What the model reads for a change whose file changed after the user saw it.
const CHANGED: &str = "config.toml changed after the user saw this change, so nothing was \
                       written. Call settings again with the same change: it is planned \
                       against the file as it is now, and the user is asked again.";

/// What the tool tells the model about itself.
const DESCRIPTION: &str = "Read or change efr's own settings: the defaults in config.toml \
    that apply to every terminal, such as the default model, reasoning effort and \
    permission mode, the permission rules, and the shell and conversation settings. Use it \
    when the user asks to set their default model, effort or mode, or to always allow or \
    deny something from now on. operation read lists every setting with its source, the \
    permission rules with their numbers, the models with their efforts, and whether the \
    last reload failed; it needs no approval. set and unset change one key, add_rule \
    appends a rule to [[permissions.rules]] and remove_rule removes one by its number from \
    read. Every change is checked first, then the user must approve it with the diff of \
    the file, also in auto mode; it applies from the next turn. To change the model, effort \
    or mode only for the current terminal, do not use this tool: tell the user to type \
    ,model <id>, ,effort <effort> or ,mode <mode>. A change never switches the running \
    turn or a terminal's own choice, which wins over the defaults, so never say that it \
    switched the current mode, model or effort. The tool never writes a rule about \
    secrets; the user writes those by hand. A turn from the phone can read the settings \
    but not change them.";

/// The input of `settings`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsInput {
    operation: Operation,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    value: Option<Value>,
    #[serde(default)]
    rule: Option<Value>,
    #[serde(default)]
    index: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Read,
    Set,
    Unset,
    AddRule,
    RemoveRule,
}

/// The settings tool.
#[derive(Debug)]
pub(crate) struct SettingsTool {
    /// `config.toml` in the config root, which may be a symbolic link.
    path: PathBuf,
    /// The daemon's settings, for `read` and for the change of a file that has an error.
    settings: watch::Receiver<Arc<Settings>>,
    /// The engine, which knows the secret locations a rule must not name.
    engine: watch::Receiver<Arc<Engine>>,
    /// The reload task, asked after a write.
    reloads: Reloads,
    /// The model catalog, which the default model and effort are checked against.
    models: Arc<Models>,
    /// The changes the user saw, by call.
    shown: Mutex<VecDeque<(CallId, Shown)>>,
}

/// A change as the approval showed it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shown {
    before: Option<String>,
    after: String,
}

/// A change applied to the file in memory, checked, not yet written.
#[derive(Debug)]
struct Plan {
    file: ConfigFile,
    edit: Edit,
    shown: Shown,
    summary: String,
    loosens: bool,
    /// The key the change applies to, for when it applies.
    key: String,
}

impl SettingsTool {
    /// The tool for the config file in `config_dir`.
    pub(crate) fn new(
        config_dir: &Path,
        settings: watch::Receiver<Arc<Settings>>,
        engine: watch::Receiver<Arc<Engine>>,
        reloads: Reloads,
        models: Arc<Models>,
    ) -> Self {
        SettingsTool {
            path: config_dir.join(CONFIG_FILE),
            settings,
            engine,
            reloads,
            models,
            shown: Mutex::default(),
        }
    }

    /// The tool as the model sees it.
    pub(crate) fn definition() -> ToolDefinition {
        ToolDefinition::function(NAME, DESCRIPTION, input_schema())
    }

    /// What a call needs: nothing for `read`, a settings change for a valid change.
    /// An invalid change is the model's error, and nobody is asked.
    pub(crate) async fn requirements(&self, call: &ToolCall) -> Result<Requirements, String> {
        let input = parse(&call.input)?;
        if input.operation == Operation::Read {
            return Ok(Requirements::none());
        }
        let plan = self.plan(input).await?;
        Ok(Requirements::none()
            .with_settings_change(SettingsChange::new(plan.summary, plan.loosens)))
    }

    /// The unified diff of the file for the approval, kept for the call so that
    /// `invoke` writes only what the user saw.
    pub(crate) async fn preview(&self, call: &ToolCall) -> Option<String> {
        let input = parse(&call.input).ok()?;
        if input.operation == Operation::Read {
            return None;
        }
        let plan = self.plan(input).await.ok()?;
        let path = plan.file.target().to_path_buf();
        let Shown { before, after } = &plan.shown;
        // NOTE: a new file starts as the example, so the diff from the example shows the
        // change itself instead of 120 lines of comments.
        let mut text = match before {
            Some(before) => efr_tools::unified_diff(&path, Some(before), after),
            None => format!(
                "{} does not exist yet; it is created from the example with this change:\n{}",
                path.display(),
                efr_tools::unified_diff(&path, Some(efr_config::EXAMPLE), after)
            ),
        };
        if plan.loosens {
            text.insert_str(0, "This change loosens permissions.\n");
        }
        self.remember(call.context.call_id, plan.shown);
        Some(text)
    }

    /// Runs an approved call: `read`, or the change the user saw.
    pub(crate) async fn invoke(&self, call: ToolCall) -> ToolOutcome {
        let input = match parse(&call.input) {
            Ok(input) => input,
            Err(message) => return ToolOutcome::error(message),
        };
        if input.operation == Operation::Read {
            return ToolOutcome::ok(self.read().await);
        }
        let Some(shown) = self.take(call.context.call_id) else {
            return ToolOutcome::error(
                "The user was not shown this change, so nothing was written. Call settings \
                 again.",
            );
        };
        let written = match self.write(input, shown).await {
            Ok(written) => written,
            Err(message) => return ToolOutcome::error(message),
        };
        let reloaded = self.reloads.request("settings tool").await;
        ToolOutcome::ok(self.report(&written, reloaded))
    }

    /// Plans `input` against the file as it is now, off the async workers.
    async fn plan(&self, input: SettingsInput) -> Result<Plan, String> {
        let path = self.path.clone();
        let engine = Arc::clone(&self.engine.borrow());
        let running = Arc::clone(&self.settings.borrow());
        let models = self.models.current();
        blocking(move || plan(&path, &input, &engine, &running, &models)).await
    }

    /// Plans `input` again and writes it when the user saw exactly this change.
    async fn write(&self, input: SettingsInput, shown: Shown) -> Result<Plan, String> {
        let path = self.path.clone();
        let engine = Arc::clone(&self.engine.borrow());
        let running = Arc::clone(&self.settings.borrow());
        let models = self.models.current();
        blocking(move || {
            let plan = plan(&path, &input, &engine, &running, &models)?;
            if plan.shown != shown {
                return Err(CHANGED.to_owned());
            }
            match plan.file.write(&plan.edit) {
                Ok(_) => Ok(plan),
                Err(ConfigError::Changed { .. }) => Err(CHANGED.to_owned()),
                Err(error) => Err(describe(&error)),
            }
        })
        .await
    }

    /// The result of a written change: what changed, when it applies, and whether the
    /// daemon must restart.
    fn report(
        &self,
        plan: &Plan,
        reloaded: Result<AdminConfigReloadResult, crate::DaemonError>,
    ) -> String {
        let mut text = format!("Changed {}: {}.", plan.file.target().display(), plan.summary);
        let applies = Applies::of(&plan.key);
        let when = match (applies, plan.key.as_str()) {
            (Applies::Restart, _) => {
                " It applies only after efrd restarts (systemctl --user restart efrd); until \
                 then the daemon keeps the running value. Tell the user."
            }
            (Applies::Client, _) => " efr reads it at its next run; the daemon does not use it.",
            (_, "permissions.rules" | "permissions.secret_paths") => {
                " It applies from the next tool call; no restart is needed."
            }
            _ => {
                " It applies from the next turn; this turn keeps its settings. No restart is needed."
            }
        };
        text.push_str(when);
        let source = self.settings.borrow().source(&plan.key);
        if let Source::Env(_) | Source::Flag(_) = source {
            let _ = write!(
                text,
                " Note: the daemon was started with {source} for {}, which wins over the file \
                 until it restarts without it.",
                plan.key
            );
        }
        match reloaded {
            Ok(result) if result.applied => {
                let others: Vec<&String> =
                    result.restart_needed.iter().filter(|key| **key != plan.key).collect();
                if !others.is_empty() {
                    let keys: Vec<&str> = others.iter().map(|key| key.as_str()).collect();
                    let _ = write!(
                        text,
                        " Earlier changes wait for a restart too: {}.",
                        keys.join(", ")
                    );
                }
            }
            Ok(result) => {
                let message = result.error.map(|error| error.message).unwrap_or_default();
                let _ = write!(
                    text,
                    " The daemon could not reload the file, so the old settings stay: {message}"
                );
            }
            Err(error) => {
                tracing::warn!(error = %error, "the settings tool could not ask for a reload");
                text.push_str(
                    " The daemon could not reload now; it reads the file when it next \
                     reloads or starts.",
                );
            }
        }
        text
    }

    /// What `read` returns: the file, the last reload, every setting with its source,
    /// the rules with their numbers and the models.
    async fn read(&self) -> String {
        let running = Arc::clone(&self.settings.borrow());
        let path = self.path.clone();
        let file = match tokio::task::spawn_blocking(move || FileState::of(&path)).await {
            Ok(file) => file,
            Err(_) => FileState::of(&self.path),
        };
        read_text(&file, &self.reloads.last(), &running, &self.models.effective(&running))
    }

    fn remember(&self, call_id: CallId, shown: Shown) {
        let mut kept = self.lock();
        kept.retain(|(known, _)| *known != call_id);
        if kept.len() == SHOWN {
            kept.pop_front();
        }
        kept.push_back((call_id, shown));
    }

    fn take(&self, call_id: CallId) -> Option<Shown> {
        let mut kept = self.lock();
        let at = kept.iter().position(|(known, _)| *known == call_id)?;
        kept.remove(at).map(|(_, shown)| shown)
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<(CallId, Shown)>> {
        // Entries are pushed and removed whole, so a poisoned lock still holds them.
        self.shown.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The JSON schema of the input.
fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "operation": {
                "type": "string",
                "enum": ["read", "set", "unset", "add_rule", "remove_rule"],
                "description": "read: every setting with its source, the rules and the \
                    models. set and unset: one key. add_rule: append a rule. remove_rule: \
                    remove the rule with the number that read shows."
            },
            "key": {
                "type": "string",
                "description": "For set and unset: the dotted key, such as model.name, \
                    model.effort, permissions.mode, shell.sudo_cache, openai.models, \
                    conversation.approval_timeout_secs or render.theme."
            },
            "value": {
                "type": ["string", "integer", "boolean", "array"],
                "items": { "type": "string" },
                "description": "For set: the new value; a list of strings for a list key."
            },
            "rule": {
                "type": "object",
                "description": "For add_rule: the rule as in [[permissions.rules]], such \
                    as {\"action\": \"execute\", \"resource\": {\"command\": {\"program\": \
                    \"cargo\", \"args\": [\"test\"], \"under\": \"project\"}}, \"effect\": \
                    \"allow\"} or {\"action\": \"write\", \"resource\": {\"under\": \
                    \"~/.config/nvim\"}, \"effect\": \"allow\"}. action: any, read, write, \
                    execute or network. effect: allow, ask or deny."
            },
            "index": {
                "type": "integer",
                "minimum": 0,
                "description": "For remove_rule: the rule's number, as read shows it."
            }
        },
        "required": ["operation"],
        "additionalProperties": false
    })
}

fn parse(input: &Value) -> Result<SettingsInput, String> {
    SettingsInput::deserialize(input).map_err(|error| {
        format!("the input of the settings tool does not match its schema: {error}")
    })
}

/// Runs `job` on the blocking pool.
async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(job)
        .await
        .unwrap_or_else(|_| Err("efr could not read config.toml now; try again".to_owned()))
}

/// Applies `input` to the file at `path` in memory and checks the result. `engine`
/// knows the secret locations; `running` stands in for a file that has an error.
fn plan(
    path: &Path,
    input: &SettingsInput,
    engine: &Engine,
    running: &Settings,
    models: &efr_provider_openai::Catalog,
) -> Result<Plan, String> {
    let file = ConfigFile::open(path).map_err(|error| describe(&error))?;
    let mut edit = file.edit().map_err(|error| describe(&error))?;
    let before_settings = match Settings::parse(path, file.text()) {
        Ok(settings) => settings,
        // NOTE: a file with an error is judged against what runs, which is what its
        // last good version set.
        Err(_) => running.clone(),
    };
    let (key, summary) = apply(&mut edit, input, engine, &before_settings)?;
    let after = edit.text();
    let before = file.text().map(str::to_owned);
    if before.as_deref().unwrap_or(efr_config::EXAMPLE) == after {
        return Err(format!(
            "{} already says that; nothing changes, and the user was not asked.",
            file.target().display()
        ));
    }
    let next = Settings::parse(path, Some(&after)).map_err(|error| {
        format!(
            "The change was not made, because the new file would not be valid: {}",
            describe(&error)
        )
    })?;
    check_models(&next, models)?;
    let loosens = loosens(&before_settings, &next);
    Ok(Plan { file, edit, shown: Shown { before, after }, summary, loosens, key })
}

/// Applies one operation to `edit`: the key it changes and a summary in plain words.
fn apply(
    edit: &mut Edit,
    input: &SettingsInput,
    engine: &Engine,
    before: &Settings,
) -> Result<(String, String), String> {
    let refused = |error: ConfigError| describe(&error);
    match input.operation {
        Operation::Set => {
            let key = needed(input.key.as_deref(), "key", "set")?;
            let value = input.value.as_ref().ok_or_else(|| missing("value", "set"))?;
            if key == "permissions.rules" {
                return Err(rules_key());
            }
            set(edit, key, value).map_err(refused)?;
            let shown = shown_value(edit, key);
            Ok((key.to_owned(), format!("set {key} = {shown}")))
        }
        Operation::Unset => {
            let key = needed(input.key.as_deref(), "key", "unset")?;
            if key == "permissions.rules" {
                return Err(rules_key());
            }
            if !edit.unset(key).map_err(refused)? {
                return Err(format!(
                    "{key} is not set in config.toml, so its default applies already; \
                     nothing changes."
                ));
            }
            Ok((key.to_owned(), format!("unset {key}, so its default applies")))
        }
        Operation::AddRule => {
            let value = input.rule.as_ref().ok_or_else(|| missing("rule", "add_rule"))?;
            let rule: Rule = serde_json::from_value(value.clone())
                .map_err(|error| format!("The rule is not a rule: {error}"))?;
            refuse_secrets(engine, &rule)?;
            let index = before.permissions.rules.rules().len();
            edit.add_rule(&rule).map_err(refused)?;
            Ok((
                "permissions.rules".to_owned(),
                format!("add rule permissions.rules[{index}] = {}", rule_text(&rule)),
            ))
        }
        Operation::RemoveRule => {
            let index = input.index.ok_or_else(|| missing("index", "remove_rule"))?;
            let rules = before.permissions.rules.rules();
            let rule = rules.get(index).ok_or_else(|| {
                format!(
                    "permissions.rules[{index}] does not exist; config.toml has {} rules. \
                     Call read to see them with their numbers.",
                    rules.len()
                )
            })?;
            refuse_secrets(engine, rule)?;
            edit.remove_rule(index).map_err(refused)?;
            Ok((
                "permissions.rules".to_owned(),
                format!("remove rule permissions.rules[{index}] = {}", rule_text(rule)),
            ))
        }
        Operation::Read => Err("read changes nothing".to_owned()),
    }
}

/// Sets `key` to the JSON `value`, read as the key's kind.
fn set(edit: &mut Edit, key: &str, value: &Value) -> Result<(), ConfigError> {
    let wrong = |expected: String| ConfigError::InvalidValue {
        key: key.to_owned(),
        value: value.to_string(),
        expected,
    };
    let kind =
        efr_config::kind(key).ok_or_else(|| ConfigError::UnknownKey { key: key.to_owned() })?;
    match value {
        Value::String(text) => edit.set_text(key, text),
        Value::Bool(flag) => edit.set(key, toml_edit::Value::from(*flag)),
        Value::Number(number) => match number.as_i64() {
            Some(number) => edit.set(key, toml_edit::Value::from(number)),
            None => Err(wrong(kind.to_string())),
        },
        Value::Array(items) if kind == Kind::List => {
            let words: Option<Vec<&str>> = items.iter().map(Value::as_str).collect();
            match words {
                Some(words) => edit.set(key, toml_edit::Value::Array(words.into_iter().collect())),
                None => Err(wrong(kind.to_string())),
            }
        }
        _ => Err(wrong(kind.to_string())),
    }
}

/// The value of `key` in the edited file, as one line of TOML.
fn shown_value(edit: &Edit, key: &str) -> String {
    let text = edit.text();
    let Ok(document) = text.parse::<toml_edit::DocumentMut>() else {
        return "(the new value)".to_owned();
    };
    let mut item = document.as_item();
    for part in key.split('.') {
        match item.get(part) {
            Some(inner) => item = inner,
            None => return "(the new value)".to_owned(),
        }
    }
    match item.as_value() {
        Some(value) => {
            let mut value = value.clone();
            value.decor_mut().clear();
            let line = value.to_string();
            if line.len() > 200 { format!("<{} bytes>", line.len()) } else { line }
        }
        None => "(the new value)".to_owned(),
    }
}

/// `rule` as one line of inline TOML, as the file would hold it.
fn rule_text(rule: &Rule) -> String {
    match toml_edit::ser::to_document(rule) {
        Ok(document) => {
            let mut inline = document.as_table().clone().into_inline_table();
            inline.fmt();
            inline.to_string()
        }
        Err(_) => format!("{rule:?}"),
    }
}

/// Refuses a rule that names secrets: the user writes those by hand.
fn refuse_secrets(engine: &Engine, rule: &Rule) -> Result<(), String> {
    if engine.names_secrets(&rule.resource) {
        return Err(format!(
            "The settings tool never adds or removes a rule that names secrets ({}); \
             nothing was asked or changed. Tell the user to edit config.toml by hand \
             (efr config edit) if they want it.",
            rule_text(rule)
        ));
    }
    Ok(())
}

/// The default model must be in the model list, and the default effort one it takes.
fn check_models(settings: &Settings, catalog: &efr_provider_openai::Catalog) -> Result<(), String> {
    let (models, _) = catalog::effective_models(settings, catalog);
    if models.is_empty() {
        return Ok(());
    }
    let model = catalog::default_model(settings, catalog);
    let Some(info) = models.iter().find(|info| info.id == model) else {
        return Err(format!(
            "The change was not made: {model} is not in the model list ({}). A new model id \
             goes into openai.models first.",
            ids(&models)
        ));
    };
    if let Some(effort) = &settings.model.effort
        && !info.takes_effort(effort)
    {
        return Err(format!(
            "The change was not made: {model} does not take the effort {effort}; it takes {}.",
            info.efforts.join(", ")
        ));
    }
    Ok(())
}

fn ids(models: &[ModelInfo]) -> String {
    models.iter().map(|info| info.id.as_str()).collect::<Vec<_>>().join(", ")
}

/// True when `next` lets more run without a question than `before`: a new allow rule,
/// a removed deny or ask rule, a mode toward `auto`, `per_call` to `keep`, a secret
/// path less, or a `[sandbox]` key that opens the sandbox (efr's auto spec, section
/// 13.1).
fn loosens(before: &Settings, next: &Settings) -> bool {
    let old = before.permissions.rules.rules();
    let new = next.permissions.rules.rules();
    let added = without(new, old);
    let removed = without(old, new);
    added.iter().any(|rule| rule.effect == Effect::Allow)
        || removed.iter().any(|rule| rule.effect != Effect::Allow)
        || next.permissions.mode > before.permissions.mode
        || (before.shell.sudo_cache == SudoCache::PerCall
            && next.shell.sudo_cache == SudoCache::Keep)
        || before
            .permissions
            .secret_paths
            .iter()
            .any(|path| !next.permissions.secret_paths.contains(path))
        || sandbox_loosens(&before.sandbox, &next.sandbox)
}

/// True when `next` opens the sandbox more than `before`: another bwrap, all projects
/// writable, a new write root, cache, kept or promoted variable or rebuildable dir, or
/// a mask glob or synced folder less.
fn sandbox_loosens(before: &SandboxSettings, next: &SandboxSettings) -> bool {
    fn adds<T: PartialEq>(before: &[T], next: &[T]) -> bool {
        next.iter().any(|item| !before.contains(item))
    }
    next.bwrap != before.bwrap
        || (next.write_projects == WriteProjects::All
            && before.write_projects != WriteProjects::All)
        || adds(&before.write_roots, &next.write_roots)
        || adds(&before.caches, &next.caches)
        || adds(&next.mask_globs, &before.mask_globs)
        || adds(&before.env_keep, &next.env_keep)
        || adds(&before.promote_env, &next.promote_env)
        || adds(&next.synced_dirs, &before.synced_dirs)
        || adds(&before.rebuildable, &next.rebuildable)
}

/// The rules of `rules` that `other` does not hold, each counted as often as it occurs.
fn without<'a>(rules: &'a [Rule], other: &[Rule]) -> Vec<&'a Rule> {
    let mut left: Vec<&Rule> = other.iter().collect();
    let mut extra = Vec::new();
    for rule in rules {
        match left.iter().position(|known| *known == rule) {
            Some(at) => {
                left.swap_remove(at);
            }
            None => extra.push(rule),
        }
    }
    extra
}

/// The text of `read`.
fn read_text(
    file: &FileState,
    last: &Outcome,
    settings: &Settings,
    models: &[ModelInfo],
) -> String {
    let mut text = String::new();
    let place = match (&file.symlink_target, file.exists) {
        (Some(target), true) => format!("a link to {}", target.display()),
        (Some(target), false) => format!("a link to {}, which does not exist", target.display()),
        (None, true) => "a file".to_owned(),
        (None, false) => "missing, so every value is its default".to_owned(),
    };
    let _ = writeln!(text, "config.toml: {} ({place})", file.path.display());
    match &last.error {
        Some(error) => {
            let _ = writeln!(
                text,
                "The last reload failed, so the old settings below still run: {}{}",
                error.message,
                error.key.as_ref().map(|key| format!(" ({key})")).unwrap_or_default()
            );
        }
        None => text.push_str("The last reload succeeded.\n"),
    }
    if !last.restart_needed.is_empty() {
        let _ = writeln!(
            text,
            "These keys changed in the file and wait for a restart of efrd: {}",
            last.restart_needed.join(", ")
        );
    }
    text.push_str(
        "\nSettings of the daemon, each with its source and when a change applies \
         (live: the next turn; restart: after efrd restarts; client: efr reads it):\n",
    );
    for entry in settings.entries() {
        if entry.key == "permissions.rules" {
            continue;
        }
        let _ = writeln!(
            text,
            "{} = {}  # {}, {}",
            entry.key,
            entry.value,
            entry.source,
            Applies::of(&entry.key).as_str()
        );
    }
    let rules = settings.permissions.rules.rules();
    if rules.is_empty() {
        text.push_str("\nPermission rules: none in config.toml.\n");
    } else {
        text.push_str("\nPermission rules of config.toml, after the built-in rules of the mode:\n");
        for (index, rule) in rules.iter().enumerate() {
            let _ = writeln!(text, "permissions.rules[{index}] = {}", rule_text(rule));
        }
    }
    text.push_str("\nModels (model.name must be one of them; model.effort one of its efforts):\n");
    for model in models {
        let mut facts = Vec::new();
        if !model.efforts.is_empty() {
            let mut efforts = format!("efforts {}", model.efforts.join(", "));
            if let Some(effort) = &model.default_effort {
                let _ = write!(efforts, ", default effort {effort}");
            }
            facts.push(efforts);
        }
        if let Some(window) = model.context_window {
            let mut fact = format!("window {window} tokens");
            if let Some(max) = model.max_context_window.filter(|max| *max > window) {
                let _ = write!(fact, ", openai.models can raise it up to {max}");
            }
            facts.push(fact);
        }
        let default = if model.default { " (the default)" } else { "" };
        let facts =
            if facts.is_empty() { String::new() } else { format!(": {}", facts.join("; ")) };
        let _ = writeln!(text, "{}{default}{facts}", model.id);
    }
    text.push_str(
        "\nThe user changes the model, effort or mode for one terminal with ,model, ,effort \
         and ,mode; this tool changes the defaults of every terminal.\n",
    );
    text
}

fn needed<'a>(value: Option<&'a str>, field: &str, operation: &str) -> Result<&'a str, String> {
    value.filter(|value| !value.trim().is_empty()).ok_or_else(|| missing(field, operation))
}

fn missing(field: &str, operation: &str) -> String {
    format!("{operation} needs {field}")
}

fn rules_key() -> String {
    "permissions.rules holds tables; change it with add_rule and remove_rule".to_owned()
}

/// A config error with its causes, for the model.
fn describe(error: &ConfigError) -> String {
    let wire = error.file_error();
    match (wire.line, wire.column) {
        (Some(line), Some(column)) => format!("{} (line {line}, column {column})", wire.message),
        _ => wire.message,
    }
}

#[cfg(test)]
mod tests;
