//! `efr config show`: the effective client settings, each with where it comes from.
//!
//! The output is TOML with the source of each value in a comment, so it can be read,
//! grepped and pasted into `config.toml`. It shows what `efr` itself uses; the
//! daemon's own settings stay with the daemon, which has no protocol method for them
//! yet.

use std::fmt::Write as _;
use std::path::Path;

use efr_client::{ClientError, Discovered};
use efr_render::ColourMode;
use efr_stdx::env::Var;

use crate::cli::ConfigCommand;
use crate::context::{Context, DEFAULT_LOG_FILTER};
use crate::error::CliError;
use crate::format;
use crate::output::Output;
use crate::settings::Source;

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    command: &ConfigCommand,
) -> Result<(), CliError> {
    match command {
        ConfigCommand::Show => {
            let discovered = efr_client::discover(&ctx.dirs).await;
            out.out(&effective(ctx, &discovered))
        }
    }
}

/// The settings as TOML lines with their sources.
pub(crate) fn effective(ctx: &Context, discovered: &Result<Discovered, ClientError>) -> String {
    let mut lines = Lines::default();
    lines.comment("The settings efr uses and where each one comes from.");
    lines.comment("The daemon's own settings are not shown; efrd reads them itself.");

    lines.table("render");
    let theme_source = match &ctx.settings.theme_source {
        Source::Default => "default".to_owned(),
        Source::File(path) => path_text(path),
    };
    lines.value("theme", &string(ctx.settings.theme.name()), &theme_source);
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

    lines.table("paths");
    let roots = [
        ("config", ctx.dirs.config(), Var::ConfigDir),
        ("data", ctx.dirs.data(), Var::DataDir),
        ("state", ctx.dirs.state(), Var::StateDir),
        ("runtime", ctx.dirs.runtime(), Var::RuntimeDir),
    ];
    for (key, path, var) in roots {
        let source = match ctx.env.path(var) {
            Ok(Some(_)) => var.name(),
            _ => "XDG base directory",
        };
        lines.value(key, &string(&path_text(path)), source);
    }

    lines.table("daemon");
    match discovered {
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

    lines.table("login");
    let (open, open_source) = match ctx.env.flag(Var::OpenBrowser) {
        Ok(true) => ("true", Var::OpenBrowser.name()),
        Ok(false) if ctx.env.var(Var::OpenBrowser).ok().flatten().is_some() => {
            ("false", Var::OpenBrowser.name())
        }
        Ok(false) => ("false", "default"),
        Err(_) => ("false", "EFR_OPEN_BROWSER is not a flag, so it is off"),
    };
    lines.value("open_browser", open, open_source);

    lines.table("log");
    match ctx.env.var(Var::Log) {
        Ok(Some(filter)) => lines.value("filter", &string(&filter), Var::Log.name()),
        _ => lines.value("filter", &string(DEFAULT_LOG_FILTER), "default"),
    }

    for warning in &ctx.settings.warnings {
        lines.comment(&format!("warning: {}", format::one_line(&warning.to_string())));
    }
    lines.out
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
