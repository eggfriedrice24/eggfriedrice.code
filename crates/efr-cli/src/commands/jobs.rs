//! `efr jobs`: the background jobs of a conversation (`jobs.list`), and the stop of one
//! (`jobs.stop`).
//!
//! `efr jobs` and `efr jobs list` list the jobs of this terminal's conversation (the
//! newest whose `tty` is the terminal on stdin), else of the newest conversation, with
//! a muted line on stderr that says which; `--conversation` names another, and `--all`
//! lists the jobs of every conversation with its title. One row per job, newest first:
//! its id, its state, when it ends or ended, the size of its output and its command,
//! with a mark when processes of the job may still run.
//!
//! `efr jobs stop <id>` stops a job of the same conversation. efrd answers when the job
//! ended, so the command then shows how it ended and the last lines of its output. The
//! facts that only the job's `job_ended` holds (its tail, its survivors, a lost
//! launcher) come from the newest page of the conversation's log. An unknown job is the
//! daemon's `not_found` (exit 1).
//!
//! A command and a tail come from a program: they go through `format::jobs`, which
//! makes control and format characters visible.

use std::collections::{HashMap, HashSet};

use efr_protocol::{
    ConversationHistory, ConversationHistoryResult, ConversationId, ConversationsList,
    ConversationsListResult, Event, JobId, JobsList, JobsListResult, JobsStop, JobsStopResult,
    Method, Origin,
};
use efr_transport::client::Client;

use crate::cli::{JobsArgs, JobsCommand, JobsListArgs, JobsStopArgs};
use crate::commands::history::{Picked, pick, resolve};
use crate::context::Context;
use crate::error::CliError;
use crate::format::jobs::{self as job_lines, Ended, ListRow};
use crate::format::{self, Tone};
use crate::live::effective_width;
use crate::output::Output;

/// The events of the conversation's log that `efr jobs` reads for the facts of the
/// jobs' ends.
const LOG_PAGE: u32 = 500;

/// The conversations whose titles `efr jobs list --all` reads.
const TITLES: u32 = 200;

pub(crate) async fn run(ctx: &Context, out: &mut Output, args: &JobsArgs) -> Result<(), CliError> {
    match &args.command {
        None => list(ctx, out, &args.list).await,
        Some(JobsCommand::List(list_args)) => list(ctx, out, list_args).await,
        Some(JobsCommand::Stop(stop_args)) => stop(ctx, out, stop_args).await,
    }
}

/// The conversation that `query` names, else this terminal's, else the newest; with
/// how it was picked, for the muted line. `None` when there is no conversation.
async fn conversation(
    ctx: &Context,
    client: &Client,
    query: Option<&str>,
) -> Result<Option<(ConversationId, Option<Picked>)>, CliError> {
    match query {
        Some(query) => Ok(Some((resolve(client, query).await?, None))),
        None => Ok(pick(client, ctx.tty.as_deref()).await?.map(|(id, picked)| (id, Some(picked)))),
    }
}

/// Writes the muted line on stderr that says which conversation the command picked.
fn picked_line(ctx: &Context, out: &mut Output, picked: Option<Picked>, another: &str) {
    if let Some(picked) = picked {
        let options = ctx.render_options(effective_width(ctx.screen.size()));
        let line = format!("{}; {another}", picked.words());
        out.err(&format!("{}\n", format::paint(&line, Tone::Dim, &options)));
    }
}

/// The `job_ended` events of the newest page of `conversation`'s log, newest last.
async fn job_ends(client: &Client, conversation: ConversationId) -> Vec<Event> {
    let method = Method::ConversationHistory(ConversationHistory {
        conversation_id: conversation,
        cursor: None,
        limit: Some(LOG_PAGE),
    });
    match client.call::<ConversationHistoryResult>(method).await {
        Ok(page) => page
            .events
            .into_iter()
            .map(|envelope| envelope.event)
            .filter(|event| matches!(event, Event::JobEnded { .. }))
            .collect(),
        Err(error) => {
            tracing::debug!(error = %error, "the ends of the jobs could not be read");
            Vec::new()
        }
    }
}

async fn list(ctx: &Context, out: &mut Output, args: &JobsListArgs) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let conversation_id = if args.all {
        None
    } else {
        let Some((id, picked)) = conversation(ctx, &client, args.conversation.as_deref()).await?
        else {
            return out.out("no conversations yet\n");
        };
        picked_line(ctx, out, picked, "efr jobs list --conversation <id> lists another");
        Some(id)
    };
    let method = Method::JobsList(JobsList { conversation_id });
    let listed: JobsListResult = client.call(method).await?;
    if listed.jobs.is_empty() {
        let none = if args.all { "no jobs\n" } else { "no jobs in this conversation\n" };
        return out.out(none);
    }
    let titles = if args.all { titles(&client).await? } else { HashMap::new() };
    // NOTE: only the job's end names its survivors and a lost launcher.
    let mut leftover = HashSet::new();
    if let Some(conversation) = conversation_id {
        for event in job_ends(&client, conversation).await {
            if let Some((job_id, end)) = Ended::of(&event)
                && end.leaves_processes()
            {
                leftover.insert(job_id.clone());
            }
        }
    }
    let rows: Vec<ListRow<'_>> = listed
        .jobs
        .iter()
        .map(|job| ListRow {
            job,
            conversation: args.all.then(|| {
                titles
                    .get(&job.conversation_id)
                    .cloned()
                    .unwrap_or_else(|| job.conversation_id.to_string())
            }),
            leftover: leftover.contains(&job.id),
        })
        .collect();
    out.out(&job_lines::list(&rows, ctx.clock.now()))
}

/// The title of each listed conversation, `(untitled)` for one with none.
async fn titles(client: &Client) -> Result<HashMap<ConversationId, String>, CliError> {
    let method = Method::ConversationsList(ConversationsList {
        cursor: None,
        limit: Some(TITLES),
        tty: None,
    });
    let list: ConversationsListResult = client.call(method).await?;
    Ok(list
        .conversations
        .into_iter()
        .map(|summary| (summary.id, summary.title.unwrap_or_else(|| "(untitled)".to_owned())))
        .collect())
}

async fn stop(ctx: &Context, out: &mut Output, args: &JobsStopArgs) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let Some((conversation_id, picked)) =
        conversation(ctx, &client, args.conversation.as_deref()).await?
    else {
        return Err(CliError::NoConversationForJobs);
    };
    picked_line(ctx, out, picked, "efr jobs stop --conversation <id> stops a job of another");
    let method = Method::JobsStop(JobsStop {
        command_id: ctx.command_id(),
        conversation_id: Some(conversation_id),
        job_id: args.job.clone(),
    });
    let stopped: JobsStopResult = client.call(method).await?;
    let ends = job_ends(&client, conversation_id).await;
    let ended = ends.iter().rev().find(|event| ended_job(event) == Some(&stopped.job.id));
    out.out(&stop_report(&stopped, ended))
}

/// The job that a `job_ended` names.
fn ended_job(event: &Event) -> Option<&JobId> {
    match event {
        Event::JobEnded { job_id, .. } => Some(job_id),
        _ => None,
    }
}

/// What `efr jobs stop` prints: the line of the job's end, then the last lines of its
/// output, each after `  │ `, from its `job_ended` when the log holds it.
pub(crate) fn stop_report(stopped: &JobsStopResult, ended: Option<&Event>) -> String {
    let job_id = &stopped.job.id;
    let (line, tail) = match ended.and_then(Ended::of) {
        Some((_, end)) => (job_lines::ended(job_id, &end), ended_tail(ended)),
        None => (job_lines::ended(job_id, &Ended::from_info(&stopped.job)), Vec::new()),
    };
    let mut out = format!("{line}\n");
    for row in tail {
        out.push_str("  \u{2502} ");
        out.push_str(&row);
        out.push('\n');
    }
    out
}

/// The last lines of the tail of a `job_ended`, safe to print.
fn ended_tail(ended: Option<&Event>) -> Vec<String> {
    match ended {
        Some(Event::JobEnded { tail, .. }) => {
            job_lines::tail_lines(tail, job_lines::STOP_TAIL_LINES)
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests;
