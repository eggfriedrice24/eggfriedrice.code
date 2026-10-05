//! `efr project`: list, add and remove the projects of the registry, `projects.toml` in
//! the config root, through the daemon.
//!
//! A registered project is where the `auto` mode writes freely and runs the project's
//! build, test and git commands, and where `cautious` writes freely. The daemon owns the
//! change, because `efr-scope` owns the file and the CLI does not depend on it: it keeps
//! the file's comments and its link, and reloads, so the engine trusts the change from
//! the next tool call on.
//!
//! - `add [PATH]` registers PATH, with links resolved. Without PATH, it registers the
//!   root of the git work tree that holds the current directory, or the directory
//!   itself; the daemon then refuses the home directory and `/`, which only a named
//!   PATH registers.
//! - `list` prints one line per project: its name and its root.
//! - `remove PATH` takes the project with that root out.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use efr_protocol::{
    AdminConfigReloadResult, AdminProjectAdd, AdminProjectAddResult, AdminProjectRemove,
    AdminProjectRemoveResult, Method, Origin, ProjectInfo, ProjectsList, ProjectsListResult,
};
use unicode_width::UnicodeWidthStr as _;

use crate::cli::ProjectCommand;
use crate::context::Context;
use crate::error::CliError;
use crate::format;
use crate::output::Output;

/// What `list` shows for a project without a name.
const NO_NAME: &str = "-";

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    command: &ProjectCommand,
) -> Result<(), CliError> {
    match command {
        ProjectCommand::List => {
            let client = ctx.connect(Origin::Cli, None).await?;
            let list: ProjectsListResult =
                client.call(Method::ProjectsList(ProjectsList {})).await?;
            out.out(&listing(&list))
        }
        ProjectCommand::Add { path, name } => {
            let (path, git_root) = match path {
                Some(path) => (absolute(ctx, path)?, false),
                None => (ctx.cwd.clone().ok_or(CliError::NoWorkingDirectory)?, true),
            };
            let client = ctx.connect(Origin::Cli, None).await?;
            let params = AdminProjectAdd { path, name: name.clone(), git_root };
            let added: AdminProjectAddResult = client.call(Method::AdminProjectAdd(params)).await?;
            out.out(&format!("registered {}\n", project_text(&added.project)))?;
            reload_outcome(out, &added.reload);
            Ok(())
        }
        ProjectCommand::Remove { path } => {
            let path = absolute(ctx, path)?;
            let client = ctx.connect(Origin::Cli, None).await?;
            let removed: AdminProjectRemoveResult =
                client.call(Method::AdminProjectRemove(AdminProjectRemove { path })).await?;
            out.out(&format!("removed {}\n", project_text(&removed.project)))?;
            reload_outcome(out, &removed.reload);
            Ok(())
        }
    }
}

/// `path` against the current directory, without resolving anything: the daemon
/// resolves links, so a path through a link names what the user sees.
fn absolute(ctx: &Context, path: &Path) -> Result<PathBuf, CliError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = ctx.cwd.as_ref().ok_or(CliError::NoWorkingDirectory)?;
    Ok(cwd.join(path))
}

/// `the project app (/home/u/p/app)`, or `the project at /etc/nixos` without a name.
fn project_text(project: &ProjectInfo) -> String {
    let root = format::one_line(&project.root.display().to_string());
    match &project.name {
        Some(name) => format!("the project {} ({root})", format::one_line(name)),
        None => format!("the project at {root}"),
    }
}

/// Says on stderr when the reload after a change did not apply, so the engine still
/// has the old projects. The change itself is written, so it is no failure.
fn reload_outcome(out: &mut Output, reload: &AdminConfigReloadResult) {
    if reload.applied {
        return;
    }
    let reason = match &reload.error {
        Some(error) => format::config_error(error),
        None => "it was not applied".to_owned(),
    };
    out.err(&format!(
        "efr: config.toml has an error, so the permissions keep the old projects until a reload succeeds: {reason}\n"
    ));
}

/// One line per project: the name, `-` for none, then the root.
pub(crate) fn listing(list: &ProjectsListResult) -> String {
    if list.projects.is_empty() {
        return format!(
            "no project is registered in {}; efr project add registers the one you are in\n",
            format::one_line(&list.file.display().to_string())
        );
    }
    let names: Vec<String> = list
        .projects
        .iter()
        .map(|project| project.name.as_deref().map_or_else(|| NO_NAME.to_owned(), format::one_line))
        .collect();
    let width = names.iter().map(|name| name.width()).max().unwrap_or(0);
    let mut out = String::new();
    for (project, name) in list.projects.iter().zip(&names) {
        let pad = width - name.width();
        let root = format::one_line(&project.root.display().to_string());
        let _ = writeln!(out, "{name}{:pad$}  {root}", "");
    }
    out
}

#[cfg(test)]
mod tests;
