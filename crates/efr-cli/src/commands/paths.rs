//! `efr paths`: where efr keeps its files, as this shell finds them, and as the daemon
//! does.
//!
//! For each root: the path, where it came from (its own variable, `EFR_HOME`, XDG or
//! `/run/user`) and whether it exists. Then the files that matter: `config.toml`
//! (absent, a file, or a link and its target), the database, the secrets directory and
//! the socket. When the daemon answers, its roots follow, and each root it keeps
//! elsewhere costs a warning with the fix: a daemon started with another `EFR_HOME` than
//! this shell reads another config and another database. `--json` prints the same.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use efr_client::ClientError;
use efr_config::FileState;
use efr_protocol::{DaemonRoots, RootDir, RootSource};
use efr_stdx::paths::{RootSource as LocalSource, RootSources};
use serde_json::{Value, json};

use crate::cli::PathsArgs;
use crate::commands::config::{config_path, file_state, status};
use crate::context::Context;
use crate::error::CliError;
use crate::format;
use crate::output::Output;

/// The fix for a daemon whose roots differ from this shell's.
const FIX: &str = "give efrd.service the same EFR_HOME with systemctl --user edit efrd";

/// One root as this shell finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Root {
    pub(crate) name: &'static str,
    pub(crate) path: PathBuf,
    pub(crate) source: String,
    pub(crate) exists: bool,
}

/// Everything `efr paths` shows, gathered first so the text is a pure function of it.
#[derive(Debug)]
pub(crate) struct Facts {
    pub(crate) roots: Vec<Root>,
    pub(crate) config: FileState,
    pub(crate) database: PathBuf,
    pub(crate) database_exists: bool,
    pub(crate) secrets: PathBuf,
    pub(crate) socket: PathBuf,
    pub(crate) socket_exists: bool,
    /// The daemon's roots, or why there are none.
    pub(crate) daemon: Result<DaemonRoots, String>,
}

pub(crate) async fn run(ctx: &Context, out: &mut Output, args: &PathsArgs) -> Result<(), CliError> {
    let facts = gather(ctx).await;
    let warnings = differences(&facts);
    if args.json {
        let mut text = serde_json::to_string_pretty(&json(&facts, &warnings))
            .map_err(|error| CliError::Output { source: error.into() })?;
        text.push('\n');
        out.out(&text)?;
    } else {
        out.out(&text(&facts))?;
    }
    for warning in warnings {
        out.err(&format!("efr: warning: {}\n", format::one_line(&warning)));
    }
    Ok(())
}

async fn gather(ctx: &Context) -> Facts {
    let dirs = ctx.dirs.clone();
    let sources = ctx.sources;
    let config = file_state(&config_path(ctx)).await;
    let local = tokio::task::spawn_blocking(move || {
        let roots = local_roots(&dirs, sources);
        let database = dirs.database_path();
        let socket = dirs.socket_path();
        (roots, database.exists(), socket.exists())
    })
    .await;
    let (roots, database_exists, socket_exists) =
        local.unwrap_or_else(|_| (local_roots(&ctx.dirs, sources), false, false));
    let daemon = match status(ctx).await {
        Ok(status) => status.roots.ok_or_else(|| "the daemon does not report its roots".to_owned()),
        Err(CliError::Client(ClientError::DaemonNotRunning { .. })) => {
            Err("not running".to_owned())
        }
        Err(error) => Err(format!("could not be asked: {error}")),
    };
    Facts {
        roots,
        config,
        database: ctx.dirs.database_path(),
        database_exists,
        secrets: ctx.dirs.secrets_dir(),
        socket: ctx.dirs.socket_path(),
        socket_exists,
        daemon,
    }
}

/// The four roots of `dirs` with `sources`. This looks at the file system.
fn local_roots(dirs: &efr_stdx::paths::Dirs, sources: RootSources) -> Vec<Root> {
    [
        ("config", dirs.config(), sources.config),
        ("data", dirs.data(), sources.data),
        ("state", dirs.state(), sources.state),
        ("runtime", dirs.runtime(), sources.runtime),
    ]
    .into_iter()
    .map(|(name, path, source)| Root {
        name,
        path: path.to_path_buf(),
        source: local_source(source),
        exists: path.is_dir(),
    })
    .collect()
}

fn local_source(source: LocalSource) -> String {
    source.to_string()
}

/// Where a daemon's root came from, named as for this shell's roots.
fn daemon_source(name: &str, source: RootSource) -> String {
    match source {
        RootSource::DirVariable => format!("EFR_{}_DIR", name.to_ascii_uppercase()),
        RootSource::EfrHome => "EFR_HOME".to_owned(),
        RootSource::Xdg => "XDG".to_owned(),
        RootSource::RunUser => "/run/user".to_owned(),
        _ => "unknown".to_owned(),
    }
}

/// The daemon's roots in the order of this shell's.
fn daemon_roots(roots: &DaemonRoots) -> [(&'static str, &RootDir); 4] {
    [
        ("config", &roots.config),
        ("data", &roots.data),
        ("state", &roots.state),
        ("runtime", &roots.runtime),
    ]
}

/// A warning for each root the daemon keeps elsewhere.
pub(crate) fn differences(facts: &Facts) -> Vec<String> {
    let Ok(daemon) = &facts.daemon else {
        return Vec::new();
    };
    daemon_roots(daemon)
        .into_iter()
        .zip(&facts.roots)
        .filter(|((_, theirs), ours)| theirs.path != ours.path)
        .map(|((name, theirs), ours)| {
            format!(
                "the daemon uses {} for the {name} root, this shell uses {}; {FIX}",
                theirs.path.display(),
                ours.path.display()
            )
        })
        .collect()
}

/// The text of `efr paths`.
pub(crate) fn text(facts: &Facts) -> String {
    let mut out = String::new();
    let mut row = |key: &str, value: &str| {
        let _ = writeln!(out, "{key:<14} {}", format::one_line(value));
    };
    for root in &facts.roots {
        let state = if root.exists { "exists" } else { "missing" };
        row(root.name, &format!("{}  ({}, {state})", root.path.display(), root.source));
    }
    let config = match (&facts.config.symlink_target, facts.config.exists) {
        (Some(target), true) => format!("a link to {}", target.display()),
        (Some(target), false) => format!("a link to {}, which does not exist", target.display()),
        (None, true) => "a file".to_owned(),
        (None, false) => "absent".to_owned(),
    };
    row("config.toml", &format!("{}  ({config})", facts.config.path.display()));
    row("efr.sqlite", &with_state(&facts.database, facts.database_exists));
    row("secrets/", &facts.secrets.display().to_string());
    row("daemon.sock", &with_state(&facts.socket, facts.socket_exists));
    match &facts.daemon {
        Ok(roots) => {
            for (name, root) in daemon_roots(roots) {
                let source = daemon_source(name, root.source);
                row(&format!("daemon {name}"), &format!("{}  ({source})", root.path.display()));
            }
        }
        Err(reason) => row("daemon", reason),
    }
    out
}

fn with_state(path: &Path, exists: bool) -> String {
    format!("{}  ({})", path.display(), if exists { "exists" } else { "absent" })
}

/// The JSON of `efr paths --json`.
fn json(facts: &Facts, warnings: &[String]) -> Value {
    let roots: serde_json::Map<String, Value> = facts
        .roots
        .iter()
        .map(|root| {
            let value = json!({ "path": root.path, "source": root.source, "exists": root.exists });
            (root.name.to_owned(), value)
        })
        .collect();
    let daemon = match &facts.daemon {
        Ok(roots) => {
            let roots: serde_json::Map<String, Value> = daemon_roots(roots)
                .into_iter()
                .map(|(name, root)| {
                    let source = daemon_source(name, root.source);
                    (name.to_owned(), json!({ "path": root.path, "source": source }))
                })
                .collect();
            json!({ "roots": roots })
        }
        Err(reason) => json!({ "error": reason }),
    };
    json!({
        "roots": roots,
        "files": {
            "config": {
                "path": facts.config.path,
                "exists": facts.config.exists,
                "symlink_target": facts.config.symlink_target,
            },
            "database": { "path": facts.database, "exists": facts.database_exists },
            "secrets": { "path": facts.secrets },
            "socket": { "path": facts.socket, "exists": facts.socket_exists },
        },
        "daemon": daemon,
        "warnings": warnings,
    })
}

#[cfg(test)]
mod tests;
