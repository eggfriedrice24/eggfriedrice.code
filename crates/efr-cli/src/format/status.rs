//! The listings outside a turn: `efr status`, the model catalog, the full mode row, a
//! held or failed reload, and `efr history` without a conversation. The commands slice
//! of the terminal look owns this file; `format.rs` re-exports its items, so callers
//! name them as `format::...`.

use std::fmt::Write as _;
use std::path::Path;

use efr_protocol::{
    AdminConfigReloadResult, AdminStatusResult, CatalogOrigin, CatalogStatus, ConfigFileError,
    ConversationStatus, ConversationsListResult, FullModeStatus, HeldConfig, LoginKind, Mode,
    ProviderStatus,
};
use efr_render::RenderOptions;
use jiff::Timestamp;
use jiff::tz::TimeZone;

use super::{Tone, ago, local_time, one_line, paint, sandbox, until};

/// A conversation's status in the words of the listing.
fn status_word(status: ConversationStatus) -> &'static str {
    match status {
        ConversationStatus::Idle => "idle",
        ConversationStatus::Running => "running",
        ConversationStatus::AwaitingApproval => "awaiting approval",
        _ => "unknown",
    }
}

/// `efr status`: the daemon's identity and health, one fact per line, with its times in
/// `zone`. `file_mode` is the default mode that `config.toml` sets, as `efr` reads the
/// file; the daemon does not report it, and a row shows it only when it is `full`.
pub(crate) fn status(
    status: &AdminStatusResult,
    socket: &Path,
    now: Timestamp,
    zone: &TimeZone,
    file_mode: Option<Mode>,
) -> String {
    let mut out = String::new();
    let mut row = |key: &str, value: &str| {
        let _ = writeln!(out, "{key:<14} {value}");
    };
    row(
        "daemon",
        &format!(
            "efrd {}, protocol {}, pid {}",
            one_line(&status.version),
            status.protocol,
            status.pid
        ),
    );
    row("daemon id", &status.daemon_id.to_string());
    // Whole seconds: the nanoseconds of a start time say nothing to a person.
    let started = local_time(status.started_at, zone);
    row("started", &format!("{started} ({})", ago(status.started_at, now)));
    row("socket", &one_line(&socket.display().to_string()));
    row("screen", &one_line(&status.screen_backend));
    row("conversations", &status.conversations.to_string());
    row("shells", &status.shells.to_string());
    if status.providers.is_empty() {
        row("providers", "none configured");
    }
    for provider in &status.providers {
        let mut state = match (provider.logged_in, provider.expires_at) {
            (false, _) => "not logged in".to_owned(),
            (true, None) => "logged in".to_owned(),
            (true, Some(expires)) => {
                let at = local_time(expires, zone);
                format!("logged in, token expires {at} ({})", until(expires, now))
            }
        };
        if let Some(hint) = &provider.key_hint {
            let _ = write!(state, ", key {}", one_line(hint));
        }
        let what = if provider.key_hint.is_some() { "" } else { ", key" };
        if let Some(refused) = provider.key_refused_at {
            let at = local_time(refused, zone);
            let _ = write!(state, "{what} refused at {at} ({})", ago(refused, now));
        } else if key_unchecked(provider) {
            let _ = write!(state, "{what} {KEY_UNCHECKED}");
        }
        if provider.active {
            state.push_str(" (active)");
        }
        row("provider", &format!("{}: {state}", one_line(&provider.provider)));
    }
    if let Some(models) = &status.catalog {
        row("models", &catalog(models, now));
    }
    if let Some(config) = &status.config {
        let file = match (&config.symlink_target, config.exists) {
            (Some(target), _) => format!("{} -> {}", config.path.display(), target.display()),
            (None, true) => config.path.display().to_string(),
            (None, false) => format!("{} (absent)", config.path.display()),
        };
        row("config", &one_line(&file));
        if let Some(error) = &config.reload_error {
            row("config error", &config_error(error));
        }
        if !config.restart_needed.is_empty() {
            row("restart needed", &one_line(&config.restart_needed.join(", ")));
        }
        if let Some(held) = &config.held {
            row("config held", &held_text(held));
        }
    }
    // NOTE: a daemon from before full mode reports none, so there is nothing to show.
    if let Some(full) = &status.full_mode {
        row("full mode", &full_mode_words(full));
    }
    if file_mode == Some(Mode::Full) {
        let refused = status.full_mode.as_ref().is_some_and(|full| full.refused.is_some());
        let words = if refused {
            "full; efrd cannot run it, so a turn that names no mode runs as auto"
        } else {
            "full"
        };
        row("default mode", words);
    }
    // NOTE: a daemon from before the sandbox reports none, so there is nothing to show.
    if let Some(sandbox) = &status.sandbox {
        row("sandbox", &sandbox::status_line(sandbox));
        if let Some(fix) = sandbox.fix.as_deref().filter(|_| !sandbox.available) {
            row("sandbox fix", &one_line(fix));
        }
        if sandbox.available {
            row("netview", &sandbox::netview_words(sandbox));
            row("bus filter", &sandbox::bus_filter_words(sandbox));
        }
    }
    out
}

/// What `efr status` says of a stored API key that efrd has not checked since it
/// started (DV32): the refusals of a key live in efrd's memory only.
pub(crate) const KEY_UNCHECKED: &str = "not checked since efrd started";

/// True when `provider` is logged in with an API key and efrd knows nothing of it yet:
/// no request with the key got an answer since efrd started, and no checked login.
fn key_unchecked(provider: &ProviderStatus) -> bool {
    provider.logged_in
        && provider.login == Some(LoginKind::ApiKey)
        && provider.key_refused_at.is_none()
        && provider.key_checked_at.is_none()
}

/// Which provider's model catalog the daemon has and where it came from, at `now`:
/// `anthropic-api, from the backend, fetched 5m ago`, `from the cache, fetched 2h 5m
/// ago` (the backend did not answer yet), `built into efr`, or `no list yet; efrd
/// fetches it in the background`. An older daemon names no provider.
pub(crate) fn catalog(catalog: &CatalogStatus, now: Timestamp) -> String {
    let fetched = |place: &str| match catalog.fetched_at {
        Some(at) => format!("from {place}, fetched {}", ago(at, now)),
        None => format!("from {place}"),
    };
    let origin = match catalog.origin {
        CatalogOrigin::Backend => fetched("the backend"),
        CatalogOrigin::Cache => fetched("the cache"),
        CatalogOrigin::Builtin => "built into efr".to_owned(),
        CatalogOrigin::Missing => "no list yet; efrd fetches it in the background".to_owned(),
        _ => "from elsewhere".to_owned(),
    };
    match &catalog.provider {
        Some(provider) => format!("{}, {origin}", one_line(provider)),
        None => origin,
    }
}

/// The `full mode` row of `efr status`: `allowed`, `off (permissions.allow_full is
/// false)`, or `refused (<why>)` when the config allows it but efrd cannot run it.
pub(crate) fn full_mode_words(full: &FullModeStatus) -> String {
    match (full.allowed, full.refused) {
        (false, _) => "off (permissions.allow_full is false)".to_owned(),
        (true, Some(refusal)) => format!("refused ({})", refusal.describe()),
        (true, None) => "allowed".to_owned(),
    }
}

/// A change of `config.toml` that efrd holds because it loosens permissions and came
/// while full mode ran, with the keys that loosen and what to do.
pub(crate) fn held_text(held: &HeldConfig) -> String {
    let keys: Vec<String> = held.loosens.iter().map(|key| one_line(key)).collect();
    format!(
        "config.toml changed while a full turn ran, and the change loosens {}. If you did not \
         make this change, take it out of the file. Then run efr config reload.",
        keys.join(", ")
    )
}

/// A config file's error as one line: the message, then its key and place.
pub(crate) fn config_error(error: &ConfigFileError) -> String {
    let place = config_error_place(error);
    if place.is_empty() {
        one_line(&error.message)
    } else {
        one_line(&format!("{} ({place})", error.message))
    }
}

/// The key and the place of a config file's error, such as `shell.login, line 3,
/// column 1`, or nothing when it has neither.
pub(crate) fn config_error_place(error: &ConfigFileError) -> String {
    let mut parts = Vec::new();
    if let Some(key) = &error.key {
        parts.push(key.clone());
    }
    match (error.line, error.column) {
        (Some(line), Some(column)) => parts.push(format!("line {line}, column {column}")),
        (Some(line), None) => parts.push(format!("line {line}")),
        _ => {}
    }
    parts.join(", ")
}

/// The outcome of a reload, for `efr config reload` and the commands that change the
/// file. `held` is true when efrd did not apply a valid file because it loosens
/// permissions while or after full mode ran: the error then says why, and the file has
/// no error. Each warning of the reload follows as a dim line, such as `warning:
/// sandbox.allow[0] is disabled: ...` (DV44).
pub(crate) fn reloaded(
    result: &AdminConfigReloadResult,
    held: bool,
    options: &RenderOptions,
) -> String {
    let restart = one_line(&result.restart_needed.join(", "));
    let mut out = match (&result.error, result.applied) {
        // NOTE: the exception goes in the same line: a first line that says new turns
        // use the file reads as done, and the change of provider looked lost.
        (_, true) if !result.restart_needed.is_empty() => format!(
            "config.toml reloaded; new turns use it, except a change of {restart}, which \
             needs a restart: systemctl --user restart efrd\n"
        ),
        (_, true) => "config.toml reloaded; new turns use it\n".to_owned(),
        (Some(error), false) if held => {
            format!("config.toml is held; the old settings stay: {}\n", one_line(&error.message))
        }
        (Some(error), false) => {
            format!("config.toml has an error; the old settings stay: {}\n", config_error(error))
        }
        (None, false) => "config.toml was not applied; the old settings stay\n".to_owned(),
    };
    if !result.restart_needed.is_empty() && !result.applied {
        let _ = writeln!(out, "restart efrd to apply: {restart} (systemctl --user restart efrd)");
    }
    for warning in &result.warnings {
        let line = format!("warning: {}", one_line(warning));
        let _ = writeln!(out, "{}", paint(&line, Tone::Dim, options));
    }
    out
}

/// `efr history` without a conversation: one line per conversation, newest first.
pub(crate) fn conversations(list: &ConversationsListResult, now: Timestamp) -> String {
    if list.conversations.is_empty() {
        return "no conversations yet\n".to_owned();
    }
    let mut out = String::new();
    for summary in &list.conversations {
        let title = summary.title.as_deref().map_or_else(|| "(untitled)".to_owned(), one_line);
        // The age is at most 11 characters wide (`59m 59s ago`, `23h 59m ago`), so the
        // titles start in one column.
        let _ = writeln!(
            out,
            "{}  {:<17}  {:>11}  {title}",
            summary.id,
            status_word(summary.status),
            ago(summary.updated_at, now)
        );
    }
    if let Some(cursor) = &list.next_cursor {
        let _ = writeln!(out, "more: efr history --cursor {}", one_line(cursor.as_str()));
    }
    out
}
