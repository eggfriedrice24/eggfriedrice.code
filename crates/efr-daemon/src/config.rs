//! The daemon's configuration: one TOML file at `$XDG_CONFIG_HOME/efr/config.toml`.
//!
//! Every value comes from the first of these that sets it, read from the last to the
//! first: the flags of `efrd`, the `EFR_*` variables, the file, the built-in defaults.
//! [`Config::effective`] prints every value with the place it came from. Unknown keys
//! in the file are an error, so a typo never silently does nothing.
//!
//! ```toml
//! log = "info"                     # EnvFilter directives; EFR_LOG, --log
//! screen = "auto"                  # auto, vt100 or ghostty; EFR_SCREEN, --screen
//!
//! [model]
//! provider = "openai-subscription" # or "openai-api"
//! name = "gpt-5.5"
//! system_prompt = "..."
//! max_output_tokens = 32000
//!
//! [openai]
//! originator = "efr"
//! models = ["gpt-5.5"]             # replaces the built-in model list
//! subscription_base_url = "https://chatgpt.com/backend-api/codex"
//! api_base_url = "https://api.openai.com/v1"
//! reasoning_effort = "medium"
//!
//! [shell]
//! program = "/usr/bin/zsh"
//! login = true
//! idle_minutes = 60                # 0 keeps idle shells forever
//!
//! [conversation]
//! max_queued = 16
//! approval_timeout_secs = 600      # absent: wait until answered
//! update_interval_ms = 200
//! tty_idle_hours = 12              # a terminal's conversation ends after; 0 never
//!
//! [permissions]
//! secret_paths = ["~/.config/rclone/rclone.conf"] # never read or written; ~/ or absolute
//!
//! [[permissions.rules]]            # after the built-in rules; the last match wins
//! action = "execute"
//! resource = { command = { program = "cargo", args = ["test"] } }
//! effect = "allow"
//!
//! [render]
//! theme = "ansi"                   # read by efr; the daemon only accepts the key
//! ```

use std::fmt::{self, Write as _};
use std::io;
use std::path::{Path, PathBuf};

use efr_permissions::{Policy, Resource, Rule};
use efr_stdx::env::{Env, Var};
use serde::Deserialize;

use crate::DaemonError;

/// The file name under the config root.
pub const CONFIG_FILE: &str = "config.toml";

/// The tracing filter when nothing sets one: lifecycle lines only, as a service.
pub const DEFAULT_LOG: &str = "info";

/// The provider of new conversations when nothing names one.
pub const DEFAULT_PROVIDER: &str = "openai-subscription";

/// The providers the daemon knows how to build.
const PROVIDERS: &[&str] = &["openai-subscription", "openai-api"];

/// The static rules every request carries as its system prompt. The live state of the
/// user's shell is not here; the conversation adds it to each prompt.
pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You are efr, a coding and system assistant that lives in the user's terminal. \
The user talks to you from their shell with lines that start with a comma. \
You run commands in a hidden zsh of your own, which starts in the user's working \
directory, and you read and write files with your tools. Every tool call is checked \
against the user's permission policy: writes outside $SCRATCH may need the user's \
approval, and secrets are never readable. Read-only commands such as ls, cat, rg, git \
status or systemctl status run at once when every argument is written out literally: \
no $VAR, no $(...), no redirection to a file, no pattern at the start of a word; any \
other command waits for the user's approval. Prefer small, reversible steps, say what \
you change, and keep answers short. Put throwaway files in $SCRATCH.";

/// Which screen backend the hidden shells get.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScreenChoice {
    /// ghostty when this build has it, otherwise vt100.
    #[default]
    Auto,
    /// The Zig-free vt100 backend.
    Vt100,
    /// The libghostty-vt backend.
    Ghostty,
}

impl ScreenChoice {
    /// The name in the file and in `EFR_SCREEN`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ScreenChoice::Auto => "auto",
            ScreenChoice::Vt100 => "vt100",
            ScreenChoice::Ghostty => "ghostty",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "auto" => Some(ScreenChoice::Auto),
            "vt100" => Some(ScreenChoice::Vt100),
            "ghostty" => Some(ScreenChoice::Ghostty),
            _ => None,
        }
    }
}

/// Where a value came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Source {
    /// The built-in default.
    #[default]
    Default,
    /// The config file.
    File,
    /// An environment variable.
    Env(Var),
    /// A flag of `efrd`.
    Flag(&'static str),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Default => f.write_str("default"),
            Source::File => f.write_str("file"),
            Source::Env(var) => write!(f, "env {var}"),
            Source::Flag(flag) => write!(f, "flag {flag}"),
        }
    }
}

/// The flags of `efrd` that override a config value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Flags {
    /// `--log`.
    pub log: Option<String>,
    /// `--screen`.
    pub screen: Option<String>,
}

impl Flags {
    /// Flags with the given `--log` and `--screen` values.
    pub fn new(log: Option<String>, screen: Option<String>) -> Self {
        Flags { log, screen }
    }
}

/// The OpenAI provider settings.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OpenAiSettings {
    /// The `originator` of the login URL and of every subscription request.
    pub originator: String,
    /// Model ids that replace the built-in list; `None` keeps it.
    pub models: Option<Vec<String>>,
    /// Replaces the subscription backend's base URL.
    pub subscription_base_url: Option<String>,
    /// Replaces the public API's base URL.
    pub api_base_url: Option<String>,
    /// The reasoning effort of reasoning models, such as `medium`.
    pub reasoning_effort: Option<String>,
}

/// How hidden shells start and when idle ones stop.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ShellSettings {
    /// The shell; `None` finds `zsh` on the `PATH`.
    pub program: Option<PathBuf>,
    /// Start it as a login shell.
    pub login: bool,
    /// Minutes without output or input, at a prompt and unwatched, before a hidden
    /// shell is closed; 0 keeps idle shells.
    pub idle_minutes: u64,
}

/// Conversation settings.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConversationSettings {
    /// The most prompts that may wait behind a running turn.
    pub max_queued: usize,
    /// How long an approval request waits; `None` waits until the user answers.
    pub approval_timeout_secs: Option<u64>,
    /// The shortest time between two streamed text updates, in milliseconds.
    pub update_interval_ms: u64,
    /// Hours without activity after which a terminal's next `,` line starts a new
    /// conversation instead of continuing the old one; 0 continues it forever.
    pub tty_idle_hours: u64,
}

/// Permission settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PermissionSettings {
    /// Files and directories that count as secrets on top of the built-in ones, so the
    /// model may never read or write them: absolute, or below the home directory as
    /// `~/...`.
    pub secret_paths: Vec<PathBuf>,
    /// The user's rules, `[[permissions.rules]]`. They come after
    /// [`Policy::defaults`] in the engine's policy, so they win where both match.
    pub rules: Policy,
}

impl PermissionSettings {
    /// The machine policy: the built-in rules, then the user's.
    pub fn policy(&self) -> Policy {
        Policy::defaults().then(self.rules.clone())
    }
}

/// The effective configuration of the daemon.
///
/// The fields are public so an in-process daemon (`efr-test-daemon`) can start from
/// [`Config::default`] and change what its test needs; [`Config::effective`] then
/// reports a changed field with the source it had.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Config {
    /// The config file, whether or not it exists.
    pub path: PathBuf,
    /// The tracing filter, in `EnvFilter` syntax.
    pub log: String,
    /// The screen backend.
    pub screen: ScreenChoice,
    /// The provider of new conversations: `openai-subscription` or `openai-api`.
    pub provider: String,
    /// The model id; `None` uses the provider's default.
    pub model: Option<String>,
    /// The system prompt.
    pub system_prompt: String,
    /// The most tokens one model call may produce; `None` leaves it to the provider.
    pub max_output_tokens: Option<u32>,
    /// The OpenAI provider settings.
    pub openai: OpenAiSettings,
    /// The hidden shell settings.
    pub shell: ShellSettings,
    /// The conversation settings.
    pub conversation: ConversationSettings,
    /// The permission settings.
    pub permissions: PermissionSettings,
    /// `render.theme`, which belongs to `efr`.
    pub render_theme: Option<String>,
    sources: Sources,
}

/// The source of each value that is not a default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Sources {
    entries: Vec<(&'static str, Source)>,
}

impl Sources {
    fn set(&mut self, key: &'static str, source: Source) {
        self.entries.retain(|(known, _)| *known != key);
        self.entries.push((key, source));
    }

    fn get(&self, key: &str) -> Source {
        self.entries
            .iter()
            .find(|(known, _)| *known == key)
            .map(|(_, source)| source.clone())
            .unwrap_or_default()
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            path: PathBuf::from(CONFIG_FILE),
            log: DEFAULT_LOG.to_owned(),
            screen: ScreenChoice::Auto,
            provider: DEFAULT_PROVIDER.to_owned(),
            model: None,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_owned(),
            max_output_tokens: None,
            openai: OpenAiSettings {
                originator: efr_provider_openai::DEFAULT_ORIGINATOR.to_owned(),
                models: None,
                subscription_base_url: None,
                api_base_url: None,
                reasoning_effort: None,
            },
            shell: ShellSettings { program: None, login: true, idle_minutes: 60 },
            conversation: ConversationSettings {
                max_queued: 16,
                approval_timeout_secs: None,
                update_interval_ms: 200,
                tty_idle_hours: 12,
            },
            permissions: PermissionSettings::default(),
            render_theme: None,
            sources: Sources::default(),
        }
    }
}

impl Config {
    /// Reads `config.toml` from `config_dir` (a missing file is an empty one) and applies
    /// `env` and `flags` over it.
    pub fn load(config_dir: &Path, env: &Env, flags: &Flags) -> Result<Config, DaemonError> {
        let path = config_dir.join(CONFIG_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(source) if source.kind() == io::ErrorKind::NotFound => None,
            Err(source) => return Err(DaemonError::ReadConfig { path, source }),
        };
        Config::resolve(&path, text.as_deref(), env, flags)
    }

    /// The configuration from the file `path` with contents `text` (`None` when it does
    /// not exist), then `env`, then `flags`.
    pub fn resolve(
        path: &Path,
        text: Option<&str>,
        env: &Env,
        flags: &Flags,
    ) -> Result<Config, DaemonError> {
        let mut config = Config { path: path.to_path_buf(), ..Config::default() };
        if let Some(text) = text {
            let file: File = toml::from_str(text).map_err(|source| DaemonError::ParseConfig {
                path: path.to_path_buf(),
                source: Box::new(source),
            })?;
            config.apply_file(file)?;
        }
        let from_env = |var: Var| env.var(var).map_err(|source| DaemonError::Env { source });
        if let Some(log) = from_env(Var::Log)? {
            config.set_log(log, Source::Env(Var::Log));
        }
        if let Some(screen) = from_env(Var::Screen)? {
            config.set_screen(&screen, Source::Env(Var::Screen))?;
        }
        if let Some(log) = &flags.log {
            config.set_log(log.clone(), Source::Flag("--log"));
        }
        if let Some(screen) = &flags.screen {
            config.set_screen(screen, Source::Flag("--screen"))?;
        }
        Ok(config)
    }

    /// Where `key` (a dotted name, as [`Config::effective`] prints it) came from.
    pub fn source(&self, key: &str) -> Source {
        self.sources.get(key)
    }

    /// Every value with its source, one `key = value  # source` line each, in a fixed
    /// order, for `efrd --print-config`.
    pub fn effective(&self) -> String {
        let mut out = format!("# {}\n", self.path.display());
        for (key, value) in self.entries() {
            // Writing to a String cannot fail.
            let _ = writeln!(out, "{key} = {value}  # {}", self.source(key));
        }
        out
    }

    fn entries(&self) -> Vec<(&'static str, String)> {
        let text = |value: &str| toml::Value::String(value.to_owned()).to_string();
        let optional = |value: Option<&str>| value.map_or_else(|| "(unset)".to_owned(), text);
        let number =
            |value: Option<u64>| value.map_or_else(|| "(unset)".to_owned(), |n| n.to_string());
        let paths = |paths: &[PathBuf]| {
            let list: Vec<String> =
                paths.iter().map(|path| text(&path.to_string_lossy())).collect();
            format!("[{}]", list.join(", "))
        };
        let models = self.openai.models.as_ref().map(|models| {
            let list: Vec<String> = models.iter().map(|model| text(model)).collect();
            format!("[{}]", list.join(", "))
        });
        vec![
            ("log", text(&self.log)),
            ("screen", text(self.screen.as_str())),
            ("model.provider", text(&self.provider)),
            ("model.name", optional(self.model.as_deref())),
            ("model.system_prompt", format!("<{} bytes>", self.system_prompt.len())),
            ("model.max_output_tokens", number(self.max_output_tokens.map(u64::from))),
            ("openai.originator", text(&self.openai.originator)),
            ("openai.models", models.unwrap_or_else(|| "(built in)".to_owned())),
            (
                "openai.subscription_base_url",
                optional(self.openai.subscription_base_url.as_deref()),
            ),
            ("openai.api_base_url", optional(self.openai.api_base_url.as_deref())),
            ("openai.reasoning_effort", optional(self.openai.reasoning_effort.as_deref())),
            (
                "shell.program",
                optional(self.shell.program.as_ref().map(|path| path.to_string_lossy()).as_deref()),
            ),
            ("shell.login", self.shell.login.to_string()),
            ("shell.idle_minutes", self.shell.idle_minutes.to_string()),
            ("conversation.max_queued", self.conversation.max_queued.to_string()),
            ("conversation.approval_timeout_secs", number(self.conversation.approval_timeout_secs)),
            ("conversation.update_interval_ms", self.conversation.update_interval_ms.to_string()),
            ("conversation.tty_idle_hours", self.conversation.tty_idle_hours.to_string()),
            ("permissions.secret_paths", paths(&self.permissions.secret_paths)),
            ("permissions.rules", rules(&self.permissions.rules)),
            ("render.theme", optional(self.render_theme.as_deref())),
        ]
    }

    fn set_log(&mut self, log: String, source: Source) {
        self.log = log;
        self.sources.set("log", source);
    }

    fn set_screen(&mut self, text: &str, source: Source) -> Result<(), DaemonError> {
        self.screen = ScreenChoice::parse(text).ok_or_else(|| DaemonError::InvalidConfig {
            key: "screen",
            value: text.to_owned(),
            expected: "auto, vt100 or ghostty",
        })?;
        self.sources.set("screen", source);
        Ok(())
    }

    fn apply_file(&mut self, file: File) -> Result<(), DaemonError> {
        let File { log, screen, model, openai, shell, conversation, permissions, render } = file;
        if let Some(log) = log {
            self.set_log(log, Source::File);
        }
        if let Some(screen) = screen {
            self.set_screen(&screen, Source::File)?;
        }
        let mut set = |key: &'static str| self.sources.set(key, Source::File);
        let ModelTable { provider, name, system_prompt, max_output_tokens } = model;
        if let Some(provider) = provider {
            if !PROVIDERS.contains(&provider.as_str()) {
                return Err(DaemonError::InvalidConfig {
                    key: "model.provider",
                    value: provider,
                    expected: "openai-subscription or openai-api",
                });
            }
            set("model.provider");
            self.provider = provider;
        }
        if let Some(name) = name {
            set("model.name");
            self.model = Some(name);
        }
        if let Some(system_prompt) = system_prompt {
            set("model.system_prompt");
            self.system_prompt = system_prompt;
        }
        if let Some(tokens) = max_output_tokens {
            set("model.max_output_tokens");
            self.max_output_tokens = Some(tokens);
        }
        let OpenAiTable {
            originator,
            models,
            subscription_base_url,
            api_base_url,
            reasoning_effort,
        } = openai;
        if let Some(originator) = originator {
            set("openai.originator");
            self.openai.originator = originator;
        }
        if let Some(models) = models {
            set("openai.models");
            self.openai.models = Some(models);
        }
        if let Some(url) = subscription_base_url {
            set("openai.subscription_base_url");
            self.openai.subscription_base_url = Some(url);
        }
        if let Some(url) = api_base_url {
            set("openai.api_base_url");
            self.openai.api_base_url = Some(url);
        }
        if let Some(effort) = reasoning_effort {
            set("openai.reasoning_effort");
            self.openai.reasoning_effort = Some(effort);
        }
        let ShellTable { program, login, idle_minutes } = shell;
        if let Some(program) = program {
            if !program.is_absolute() {
                return Err(DaemonError::InvalidConfig {
                    key: "shell.program",
                    value: program.to_string_lossy().into_owned(),
                    expected: "an absolute path",
                });
            }
            set("shell.program");
            self.shell.program = Some(program);
        }
        if let Some(login) = login {
            set("shell.login");
            self.shell.login = login;
        }
        if let Some(minutes) = idle_minutes {
            set("shell.idle_minutes");
            self.shell.idle_minutes = minutes;
        }
        let ConversationTable {
            max_queued,
            approval_timeout_secs,
            update_interval_ms,
            tty_idle_hours,
        } = conversation;
        if let Some(max_queued) = max_queued {
            set("conversation.max_queued");
            self.conversation.max_queued = max_queued;
        }
        if let Some(secs) = approval_timeout_secs {
            set("conversation.approval_timeout_secs");
            self.conversation.approval_timeout_secs = Some(secs);
        }
        if let Some(ms) = update_interval_ms {
            set("conversation.update_interval_ms");
            self.conversation.update_interval_ms = ms;
        }
        if let Some(hours) = tty_idle_hours {
            set("conversation.tty_idle_hours");
            self.conversation.tty_idle_hours = hours;
        }
        if let Some(rules) = permissions.rules {
            let mut policy = Policy::empty();
            for (index, value) in rules.into_iter().enumerate() {
                let rule: Rule = value.try_into().map_err(|source| DaemonError::ParseRule {
                    path: self.path.clone(),
                    index,
                    source: Box::new(source),
                })?;
                // NOTE: each rule is pushed onto a policy of the ones before it, so the
                // index in the error is the rule's place in the file.
                policy.push(rule).map_err(|source| DaemonError::InvalidRule {
                    path: self.path.clone(),
                    index,
                    source,
                })?;
            }
            set("permissions.rules");
            self.permissions.rules = policy;
        }
        if let Some(secret_paths) = permissions.secret_paths {
            if let Some(path) =
                secret_paths.iter().find(|path| !path.is_absolute() && !path.starts_with("~"))
            {
                return Err(DaemonError::InvalidConfig {
                    key: "permissions.secret_paths",
                    value: path.to_string_lossy().into_owned(),
                    expected: "absolute paths or paths that start with ~/",
                });
            }
            set("permissions.secret_paths");
            self.permissions.secret_paths = secret_paths;
        }
        if let Some(theme) = render.theme {
            set("render.theme");
            self.render_theme = Some(theme);
        }
        Ok(())
    }
}

/// The file as written. Every table and key is optional; unknown ones are errors.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    log: Option<String>,
    screen: Option<String>,
    #[serde(default)]
    model: ModelTable,
    #[serde(default)]
    openai: OpenAiTable,
    #[serde(default)]
    shell: ShellTable,
    #[serde(default)]
    conversation: ConversationTable,
    #[serde(default)]
    permissions: PermissionsTable,
    #[serde(default)]
    render: RenderTable,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelTable {
    provider: Option<String>,
    name: Option<String>,
    system_prompt: Option<String>,
    max_output_tokens: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenAiTable {
    originator: Option<String>,
    models: Option<Vec<String>>,
    subscription_base_url: Option<String>,
    api_base_url: Option<String>,
    reasoning_effort: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShellTable {
    program: Option<PathBuf>,
    login: Option<bool>,
    idle_minutes: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConversationTable {
    max_queued: Option<usize>,
    approval_timeout_secs: Option<u64>,
    update_interval_ms: Option<u64>,
    tty_idle_hours: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PermissionsTable {
    secret_paths: Option<Vec<PathBuf>>,
    /// Read one by one, so an error names the rule's place in the file.
    rules: Option<Vec<toml::Value>>,
}

/// The rules as one inline TOML array, for the effective dump, each with its keys in
/// the order the docs write them.
fn rules(policy: &Policy) -> String {
    let rules: Vec<String> = policy
        .rules()
        .iter()
        .map(|rule| {
            format!(
                "{{ action = {}, resource = {}, effect = {} }}",
                inline(&rule.action),
                resource(&rule.resource),
                inline(&rule.effect)
            )
        })
        .collect();
    format!("[{}]", rules.join(", "))
}

fn resource(resource: &Resource) -> String {
    let Resource::Command(pattern) = resource else {
        return inline(resource);
    };
    let mut fields = vec![format!("program = {}", inline(&pattern.program))];
    if !pattern.args.is_empty() {
        fields.push(format!("args = {}", inline(&pattern.args)));
    }
    if !pattern.forbid.is_empty() {
        fields.push(format!("forbid = {}", inline(&pattern.forbid)));
    }
    if let Some(max) = pattern.max_operands {
        fields.push(format!("max_operands = {max}"));
    }
    if let Some(under) = &pattern.under {
        fields.push(format!("under = {}", inline(&under.to_string_lossy())));
    }
    format!("{{ command = {{ {} }} }}", fields.join(", "))
}

/// One value in inline TOML.
fn inline<T: serde::Serialize + ?Sized>(value: &T) -> String {
    toml::Value::try_from(value)
        .map_or_else(|_| "(not printable)".to_owned(), |value| value.to_string())
}

/// `efr`'s table: the daemon only accepts its key, so the shared file stays valid.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderTable {
    theme: Option<String>,
}

#[cfg(test)]
mod tests;
