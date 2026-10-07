//! `efr config`: show, check, edit and change `config.toml`, print its JSON schema, and
//! ask the daemon to read it again.
//!
//! - `show` prints every key of the file with its value and where it comes from, as
//!   TOML with the source in a comment, so it can be read, grepped and pasted; then
//!   what `efr` itself uses (the terminal, the roots), then the daemon's view from
//!   `admin.status`: the file it reads, the last reload's error and the keys that wait
//!   for a restart, with a warning when it reads another file than this shell would.
//! - `check` reads a file with the daemon's checks and the theme names of
//!   `efr-render`; an error names its line, its column and its key, and exits 1.
//! - `edit` opens the file in `$VISUAL`, else `$EDITOR`, else `vi`, after it creates a
//!   missing one from the commented example (never through a link to nothing); when
//!   `config.toml` is a symbolic link, the editor gets the file behind it, so the link
//!   stays a link; checks it when the editor exits and offers to edit again; then asks
//!   the daemon to reload.
//! - `set` and `unset` change one key through `efr-config`'s writer, which keeps
//!   comments and layout and writes the file behind a link, then ask for a reload.
//! - `schema` prints the JSON schema; `reload` asks the daemon to read the file now.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use efr_client::{ClientError, Discovered};
use efr_config::{AUTO_THEME, CONFIG_FILE, ConfigError, ConfigFile, Edit, Entry, FileState};
use efr_protocol::{AdminConfigReload, AdminConfigReloadResult, AdminStatus, AdminStatusResult};
use efr_protocol::{Method, Origin};
use efr_render::{ColourMode, Theme};
use efr_stdx::env::Var;

use crate::cli::ConfigCommand;
use crate::context::{Context, DEFAULT_LOG_FILTER};
use crate::error::CliError;
use crate::format;
use crate::output::Output;
use crate::settings::{self, Source};

/// How often `set` and `unset` plan their change again when the file changed while
/// they prepared it.
const ATTEMPTS: usize = 3;

/// The editor when neither `VISUAL` nor `EDITOR` names one.
const DEFAULT_EDITOR: &str = "vi";

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    command: &ConfigCommand,
) -> Result<(), CliError> {
    match command {
        ConfigCommand::Show => {
            let view = View::gather(ctx).await;
            out.out(&effective(ctx, &view))
        }
        ConfigCommand::Check { path } => check(ctx, out, path.as_deref()).await,
        ConfigCommand::Edit => edit(ctx, out).await,
        ConfigCommand::Schema => out.out(&efr_config::schema_text()),
        ConfigCommand::Reload => {
            let result = reload(ctx).await?;
            report_reload(out, &result)
        }
        ConfigCommand::Set { key, value } => change(ctx, out, key, Some(value)).await,
        ConfigCommand::Unset { key } => change(ctx, out, key, None).await,
    }
}

/// `config.toml` in the config root.
pub(crate) fn config_path(ctx: &Context) -> PathBuf {
    ctx.dirs.config().join(CONFIG_FILE)
}

/// What `show` prints, gathered first so the text is a pure function of it.
#[derive(Debug)]
pub(crate) struct View {
    pub(crate) discovered: Result<Discovered, ClientError>,
    /// The file's settings, or why they could not be read; then the defaults show.
    pub(crate) file: Result<efr_config::Settings, String>,
    pub(crate) file_state: FileState,
    /// The daemon's status, or why there is none.
    pub(crate) daemon: Result<AdminStatusResult, String>,
}

impl View {
    async fn gather(ctx: &Context) -> View {
        let path = config_path(ctx);
        let file = match read(&path).await {
            Ok(text) => efr_config::Settings::parse(&path, text.as_deref())
                .map_err(|error| settings::describe(&error)),
            Err(error) => Err(format!("it could not be read: {error}")),
        };
        let file_state = file_state(&path).await;
        let daemon = match status(ctx).await {
            Ok(status) => Ok(status),
            Err(CliError::Client(ClientError::DaemonNotRunning { .. })) => {
                Err("the daemon is not running".to_owned())
            }
            Err(error) => Err(format!("the daemon could not be asked: {error}")),
        };
        View { discovered: efr_client::discover(&ctx.dirs).await, file, file_state, daemon }
    }
}

/// The daemon's `admin.status`.
pub(crate) async fn status(ctx: &Context) -> Result<AdminStatusResult, CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    Ok(client.call(Method::AdminStatus(AdminStatus {})).await?)
}

/// The contents of `path`; `None` when it does not exist.
async fn read(path: &Path) -> io::Result<Option<String>> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// What is at `path`, looked at off the async workers.
pub(crate) async fn file_state(path: &Path) -> FileState {
    let owned = path.to_path_buf();
    match tokio::task::spawn_blocking(move || FileState::of(&owned)).await {
        Ok(state) => state,
        // NOTE: only a panic gets here; looking again on this worker costs one stat.
        Err(_) => FileState::of(path),
    }
}

/// The text of `efr config show`.
pub(crate) fn effective(ctx: &Context, view: &View) -> String {
    let mut lines = Lines::default();
    let path = config_path(ctx);
    lines.comment(&format!("{} ({})", path.display(), state_text(&view.file_state)));
    lines.comment("Every key of config.toml with where its value comes from. efrd applies");
    lines.comment("EFR_LOG, EFR_SCREEN and its flags on top; efrd --print-config shows them.");
    let defaults = efr_config::Settings::default();
    let settings = match &view.file {
        Ok(settings) => settings,
        Err(problem) => {
            lines.comment(&format!(
                "warning: config.toml has an error, so the defaults show: {problem}"
            ));
            &defaults
        }
    };
    for Entry { key, value, source, .. } in settings.entries() {
        lines.value(&key, &value, &source.to_string());
    }

    lines.table("efr");
    let theme_source = match &ctx.settings.theme_source {
        Source::Default => "default".to_owned(),
        Source::File(path) => path_text(path),
    };
    let theme_source = match (ctx.settings.auto_theme(), ctx.settings.background) {
        (true, Some(background)) => {
            format!("{theme_source}; auto, EFR_TERMINAL_BG is {}", background.name())
        }
        (true, None) => format!("{theme_source}; auto, the background is not known"),
        (false, _) => theme_source,
    };
    lines.value("theme", &string(ctx.settings.theme.name()), &theme_source);
    if let Some(path) = &ctx.settings.code_theme_path {
        lines.value("code_theme", &string(&path_text(path)), "the theme file; wins over theme");
    }
    let (colour, colour_source) = match ctx.term.colour() {
        ColourMode::None => ("none", "NO_COLOR is set"),
        ColourMode::TrueColor => ("truecolor", "COLORTERM"),
        _ => ("16", "default; COLORTERM does not say truecolor"),
    };
    lines.value("colour", &string(colour), colour_source);
    let (formatted, formatted_source) = match (ctx.term.stdout_tty, ctx.term.formats_stdout()) {
        (_, true) => ("true", "stdout is a terminal"),
        (true, false) => ("false", "TERM is dumb; raw markdown"),
        (false, false) => ("false", "stdout is not a terminal; raw markdown"),
    };
    lines.value("formatted", formatted, formatted_source);
    let (open, open_source) = match ctx.env.flag(Var::OpenBrowser) {
        Ok(true) => ("true", Var::OpenBrowser.name()),
        Ok(false) if ctx.env.var(Var::OpenBrowser).ok().flatten().is_some() => {
            ("false", Var::OpenBrowser.name())
        }
        Ok(false) => ("false", "default"),
        Err(_) => ("false", "EFR_OPEN_BROWSER is not a flag, so it is off"),
    };
    lines.value("open_browser", open, open_source);
    match ctx.env.var(Var::Log) {
        Ok(Some(filter)) => lines.value("log", &string(&filter), Var::Log.name()),
        _ => lines.value("log", &string(DEFAULT_LOG_FILTER), "default"),
    }

    lines.table("paths");
    let roots = [
        ("config", ctx.dirs.config(), ctx.sources.config),
        ("data", ctx.dirs.data(), ctx.sources.data),
        ("state", ctx.dirs.state(), ctx.sources.state),
        ("runtime", ctx.dirs.runtime(), ctx.sources.runtime),
    ];
    for (key, path, source) in roots {
        lines.value(key, &string(&path_text(path)), &source.to_string());
    }

    lines.table("daemon");
    match &view.discovered {
        Ok(Discovered { socket, info: Some(_) }) => {
            lines.value("socket", &string(&path_text(socket)), "daemon.json");
        }
        Ok(Discovered { socket, info: None }) => {
            lines.value("socket", &string(&path_text(socket)), "default; no daemon.json");
        }
        Err(error) => {
            let reason = format::one_line(&error.to_string());
            lines.comment(&format!("socket: unknown, because {reason}"));
        }
    }
    match &view.daemon {
        Ok(AdminStatusResult { config: Some(config), .. }) => {
            lines.value("config", &string(&path_text(&config.path)), "admin.status");
            if let Some(error) = &config.reload_error {
                let place = format::config_error_place(error);
                lines.value("reload_error", &string(&error.message), &place);
            }
            if !config.restart_needed.is_empty() {
                let keys: Vec<String> =
                    config.restart_needed.iter().map(String::as_str).map(string).collect();
                lines.value(
                    "restart_needed",
                    &format!("[{}]", keys.join(", ")),
                    "restart efrd to apply",
                );
            }
            if config.path != path {
                lines.comment(&format!(
                    "warning: the daemon reads {}, this shell reads {}; give efrd.service the same EFR_HOME or EFR_CONFIG_DIR with systemctl --user edit efrd",
                    config.path.display(),
                    path.display()
                ));
            }
        }
        Ok(_) => lines.comment("the daemon does not say which config file it reads"),
        Err(reason) => lines.comment(&format::one_line(reason)),
    }

    for warning in &ctx.settings.warnings {
        lines.comment(&format!("warning: {}", format::one_line(&warning.to_string())));
    }
    lines.out
}

/// `absent`, `file`, or `link to <target>` and whether the target exists.
fn state_text(file: &FileState) -> String {
    match (&file.symlink_target, file.exists) {
        (Some(target), true) => format!("a link to {}", target.display()),
        (Some(target), false) => format!("a link to {}, which does not exist", target.display()),
        (None, true) => "a file".to_owned(),
        (None, false) => "absent; every value is its default".to_owned(),
    }
}

/// `efr config check`: the file's problem, or that it is fine.
async fn check(ctx: &Context, out: &mut Output, path: Option<&Path>) -> Result<(), CliError> {
    let named = path.is_some();
    let path = path.map_or_else(|| config_path(ctx), Path::to_path_buf);
    let text = match read(&path).await {
        Ok(None) if named => {
            out.out(&format!("{}: it does not exist\n", path.display()))?;
            return Err(CliError::ConfigInvalid);
        }
        Ok(text) => text,
        Err(error) => {
            out.out(&format!("{}: it could not be read: {error}\n", path.display()))?;
            return Err(CliError::ConfigInvalid);
        }
    };
    match settings::problem(&path, text.as_deref(), ctx.tilde_home()).await {
        None if text.is_none() => {
            out.out(&format!("{}: ok, absent; every value is its default\n", path.display()))
        }
        None => out.out(&format!("{}: ok\n", path.display())),
        Some(problem) => {
            out.out(&format!("{}: {problem}\n", path.display()))?;
            Err(CliError::ConfigInvalid)
        }
    }
}

/// `efr config edit`.
async fn edit(ctx: &Context, out: &mut Output) -> Result<(), CliError> {
    let path = config_path(ctx);
    let Prepared { target, created } = create_from_example(&path).await?;
    if created {
        out.err(&format!("efr: created {} from the example\n", target.display()));
    }
    loop {
        // NOTE: the editor gets the file behind a link, never the link: an editor that
        // saves by writing a new file and renaming it over the old one would otherwise
        // replace a link into a dotfiles repository with a plain file.
        run_editor(ctx, &target).await?;
        let text = read(&path).await.map_err(|source| CliError::ConfigFile {
            source: Box::new(ConfigError::Read { path: path.clone(), source }),
        })?;
        let Some(problem) = settings::problem(&path, text.as_deref(), ctx.tilde_home()).await
        else {
            break;
        };
        out.err(&format!("efr: {}: {problem}\n", path.display()));
        if !edit_again(ctx, out).await? {
            out.err("efr: the daemon keeps its old settings until the file is fixed\n");
            return Err(CliError::ConfigInvalid);
        }
    }
    reload_after_change(ctx, out).await
}

/// The file that `edit` opens, and whether it was just created.
#[derive(Debug)]
struct Prepared {
    /// The end of the link when `config.toml` is a symbolic link, else the file.
    target: PathBuf,
    created: bool,
}

/// Creates the missing config file at `path` from the example, and returns the file to
/// edit. A link to nothing is refused.
async fn create_from_example(path: &Path) -> Result<Prepared, CliError> {
    let path = path.to_path_buf();
    let created = tokio::task::spawn_blocking(move || -> Result<Prepared, ConfigError> {
        let file = ConfigFile::open(&path)?;
        let target = file.target().to_path_buf();
        if file.text().is_some() {
            return Ok(Prepared { target, created: false });
        }
        file.write(&file.edit()?)?;
        Ok(Prepared { target, created: true })
    })
    .await;
    let created = created.unwrap_or_else(|join| {
        Err(ConfigError::Read { path: PathBuf::from(CONFIG_FILE), source: io::Error::other(join) })
    });
    created.map_err(|source| CliError::ConfigFile { source: Box::new(source) })
}

/// Runs the editor on `path` with the terminal, and waits for it.
async fn run_editor(ctx: &Context, path: &Path) -> Result<(), CliError> {
    let editor = ctx.editor.clone().unwrap_or_else(|| DEFAULT_EDITOR.to_owned());
    let dir = ctx.cwd.clone().unwrap_or_else(|| PathBuf::from("/"));
    // NOTE: through sh, as git runs its editor, so `code --wait` and other values with
    // arguments work; the path goes in as "$1", never into the script.
    let mut command = efr_stdx::process::command("sh", &dir);
    command.arg("-c").arg(format!("{editor} \"$@\"")).arg(&editor).arg(path);
    let status = command
        .status()
        .await
        .map_err(|source| CliError::Editor { editor: editor.clone(), source })?;
    if status.success() { Ok(()) } else { Err(CliError::EditorFailed { editor, status }) }
}

/// Asks whether to edit again; no terminal to ask on means no.
async fn edit_again(ctx: &Context, out: &mut Output) -> Result<bool, CliError> {
    if !ctx.keys.available() {
        return Ok(false);
    }
    out.err("efr: edit it again? [Y/n] ");
    let mut keys = ctx.keys.start()?;
    let key = keys.next().await;
    keys.stop().await;
    out.err("\n");
    Ok(matches!(key, Some(b'\r' | b'\n' | b'y' | b'Y')))
}

/// `efr config set` and `efr config unset`: `value` is the new value, `None` removes
/// the key.
async fn change(
    ctx: &Context,
    out: &mut Output,
    key: &str,
    value: Option<&str>,
) -> Result<(), CliError> {
    let auto = key == "render.theme" && value == Some(AUTO_THEME);
    if matches!(key, "render.theme" | "render.theme_dark" | "render.theme_light")
        && let Some(name) = value.filter(|_| !auto)
        && Theme::from_name(name).is_err()
    {
        out.err(&format!("efr: {key}: the theme {name:?} does not exist\n"));
        return Err(CliError::ConfigInvalid);
    }
    let path = config_path(ctx);
    let mut attempt = 0;
    let (target, removed) = loop {
        attempt += 1;
        let (owned_path, owned_key, owned_value) =
            (path.clone(), key.to_owned(), value.map(str::to_owned));
        let written = tokio::task::spawn_blocking(move || {
            write_change(&owned_path, &owned_key, owned_value.as_deref())
        })
        .await
        .unwrap_or_else(|join| {
            Err(ConfigError::Read { path: path.clone(), source: io::Error::other(join) })
        });
        match written {
            Ok(done) => break done,
            Err(ConfigError::Changed { .. }) if attempt < ATTEMPTS => {}
            Err(error) => {
                out.err(&format!("efr: {}: {}\n", path.display(), settings::describe(&error)));
                return Err(CliError::ConfigInvalid);
            }
        }
    };
    let line = match (value, removed) {
        (Some(value), _) => {
            format!("{}: set {key} = {}\n", target.display(), format::one_line(value))
        }
        (None, true) => format!("{}: removed {key}; its default applies\n", target.display()),
        (None, false) => format!("{}: {key} was not set\n", target.display()),
    };
    out.out(&line)?;
    reload_after_change(ctx, out).await
}

/// Applies one change to the file at `path`: the file written and whether a key was
/// removed.
fn write_change(
    path: &Path,
    key: &str,
    value: Option<&str>,
) -> Result<(PathBuf, bool), ConfigError> {
    let file = ConfigFile::open(path)?;
    let mut edit: Edit = file.edit()?;
    let removed = match value {
        Some(value) => {
            edit.set_text(key, value)?;
            false
        }
        None => {
            if !edit.unset(key)? {
                return Ok((file.target().to_path_buf(), false));
            }
            true
        }
    };
    file.write(&edit)?;
    Ok((file.target().to_path_buf(), removed))
}

/// Asks the daemon to reload.
async fn reload(ctx: &Context) -> Result<AdminConfigReloadResult, CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    Ok(client.call(Method::AdminConfigReload(AdminConfigReload {})).await?)
}

/// Asks the daemon to reload after a change, and says how that went. A daemon that is
/// not running reads the file when it starts.
async fn reload_after_change(ctx: &Context, out: &mut Output) -> Result<(), CliError> {
    match reload(ctx).await {
        Ok(result) => report_reload(out, &result),
        Err(CliError::Client(ClientError::DaemonNotRunning { .. })) => {
            out.out("the daemon is not running; it reads the file when it starts\n")
        }
        Err(error) => Err(error),
    }
}

/// The outcome of a reload; a refused file exits 1.
fn report_reload(out: &mut Output, result: &AdminConfigReloadResult) -> Result<(), CliError> {
    out.out(&format::reloaded(result))?;
    if result.applied { Ok(()) } else { Err(CliError::ConfigInvalid) }
}

/// TOML lines with a source comment after each value.
#[derive(Debug, Default)]
struct Lines {
    out: String,
}

impl Lines {
    fn comment(&mut self, text: &str) {
        let _ = writeln!(self.out, "# {text}");
    }

    fn table(&mut self, name: &str) {
        let _ = writeln!(self.out, "\n[{name}]");
    }

    fn value(&mut self, key: &str, value: &str, source: &str) {
        let _ = writeln!(self.out, "{key} = {value}  # {}", format::one_line(source));
    }
}

/// `text` as a TOML string.
fn string(text: &str) -> String {
    toml::Value::String(text.to_owned()).to_string()
}

fn path_text(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests;
