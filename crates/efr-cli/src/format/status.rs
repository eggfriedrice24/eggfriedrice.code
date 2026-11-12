//! The listings outside a turn: `efr status`, the model catalog, the full mode row, a
//! held or failed reload, and `efr history` without a conversation. The commands slice
//! of the terminal look owns this file; `format.rs` re-exports its items, so callers
//! name them as `format::...`.

use std::fmt::Write as _;
use std::path::Path;

use efr_protocol::{
    AdminConfigReloadResult, AdminStatusResult, CatalogOrigin, CatalogStatus, ConfigFileError,
    ConfigStatus, ConversationStatus, ConversationsListResult, FullModeStatus, HeldConfig,
    LoginKind, Mode, ProviderStatus, SandboxStatus,
};
use efr_render::RenderOptions;
use jiff::Timestamp;
use unicode_width::UnicodeWidthStr as _;

use super::sandbox::{self, tilde};
use super::{Tone, ago, one_line, paint, span};

/// A conversation's status in the words of the listing.
fn status_word(status: ConversationStatus) -> &'static str {
    match status {
        ConversationStatus::Idle => "idle",
        ConversationStatus::Running => "running",
        ConversationStatus::AwaitingApproval => "awaiting approval",
        _ => "unknown",
    }
}

/// What `efr status` shows: the daemon's answer and the facts around it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StatusView<'a> {
    pub(crate) status: &'a AdminStatusResult,
    /// Where the model list of each account with a login came from (`models.list`).
    /// Empty from a daemon that lists one provider: the status's own catalog shows.
    pub(crate) catalogs: &'a [CatalogStatus],
    /// The socket that `efr` reached the daemon on.
    pub(crate) socket: &'a Path,
    pub(crate) now: Timestamp,
    /// The default mode that `config.toml` sets, as `efr` reads the file; the daemon
    /// does not report it, and a row shows it only when it is `full`.
    pub(crate) file_mode: Option<Mode>,
    /// The home directory, which the path of the config file starts with as `~`.
    pub(crate) home: Option<&'a Path>,
}

/// The narrowest column of labels: the longest label of most listings, `bus filter`,
/// and two blanks. A rare longer label, such as `restart needed`, widens it.
const LABEL: usize = 12;

/// The pieces of a value, each with its tone.
type Pieces = Vec<(String, Tone)>;

/// Rows with a muted label in a left column and the value after it; `None` is a blank
/// row between groups.
struct Labelled<'o> {
    rows: Vec<Option<(&'static str, Pieces)>>,
    options: &'o RenderOptions,
}

impl Labelled<'_> {
    /// A row of `label` and `value`.
    fn row(&mut self, label: &'static str, value: Pieces) {
        self.rows.push(Some((label, value)));
    }

    /// A row of a label with one value in `text`.
    fn text(&mut self, label: &'static str, value: String) {
        self.row(label, vec![(value, Tone::Text)]);
    }

    fn blank(&mut self) {
        self.rows.push(None);
    }

    /// The rows: each label muted and padded to the column, then its value.
    fn finish(self) -> String {
        let longest = self.rows.iter().flatten().map(|(label, _)| label.width()).max();
        let column = longest.map_or(LABEL, |longest| (longest + 2).max(LABEL));
        let mut out = String::new();
        for row in &self.rows {
            if let Some((label, value)) = row {
                let pad = " ".repeat(column - label.width());
                let _ = write!(out, "{}{pad}", paint(label, Tone::Dim, self.options));
                for (text, tone) in value {
                    out.push_str(&paint(text, *tone, self.options));
                }
            }
            out.push('\n');
        }
        out
    }
}

/// `efr status` (TM-Commands frame 1): a muted label in a left column, the value in
/// `text`, and a colour only where something is wrong or needs care. Three groups,
/// apart by a blank line: the daemon and its config; the accounts and where their
/// model lists came from; the sandbox and full mode.
pub(crate) fn status(view: &StatusView<'_>, options: &RenderOptions) -> String {
    let StatusView { status, now, .. } = *view;
    let dot = separator(options);
    let mut rows = Labelled { rows: Vec::new(), options };
    let up = u64::try_from(now.duration_since(status.started_at).as_secs()).unwrap_or(0);
    let up = short_span(up);
    rows.text(
        "efrd",
        format!(
            "{}{dot}protocol {}{dot}up {up}{dot}{}",
            one_line(&status.version),
            status.protocol,
            one_line(&view.socket.display().to_string())
        ),
    );
    if let Some(config) = &status.config {
        config_rows(&mut rows, config, view.home);
    }
    rows.blank();
    account_rows(&mut rows, view);
    let own: Vec<CatalogStatus> = status.catalog.iter().cloned().collect();
    let catalogs = if view.catalogs.is_empty() { &own[..] } else { view.catalogs };
    if !catalogs.is_empty() {
        let words: Vec<String> = catalogs
            .iter()
            .map(|catalog| match &catalog.provider {
                Some(provider) => format!("{}: {}", account_of(provider).0, origin(catalog, now)),
                None => origin(catalog, now),
            })
            .collect();
        rows.text("models", words.join(&dot));
    }
    rows.blank();
    // NOTE: a daemon from before the sandbox reports none, so there is nothing to show.
    if let Some(sandbox) = &status.sandbox {
        sandbox_rows(&mut rows, sandbox, &dot);
    }
    // NOTE: a daemon from before full mode reports none, so there is nothing to show.
    if let Some(full) = &status.full_mode {
        let tone =
            if full.allowed && full.refused.is_some() { Tone::Attention } else { Tone::Text };
        rows.row("full mode", vec![(full_mode_words(full), tone)]);
    }
    if view.file_mode == Some(Mode::Full) {
        let refused = status.full_mode.as_ref().is_some_and(|full| full.refused.is_some());
        let mut value = vec![("full".to_owned(), Tone::Failure)];
        if refused {
            let why = "efrd cannot run it, so a turn that names no mode runs as auto";
            value.push((format!("{dot}{why}"), Tone::Text));
        }
        rows.row("default mode", value);
    }
    rows.finish()
}

/// The rows of the config file: its path (and where its link points), then an error of
/// its last reload, the keys that need a restart and a change that efrd holds.
fn config_rows(rows: &mut Labelled<'_>, config: &ConfigStatus, home: Option<&Path>) {
    let arrow = rows.options.glyphs().arrow;
    let mut value = vec![(tilde(&config.path, home), Tone::Text)];
    match (&config.symlink_target, config.exists) {
        (Some(target), _) => value.push((format!(" {arrow} {}", tilde(target, home)), Tone::Text)),
        (None, true) => {}
        (None, false) => value.push((" (absent)".to_owned(), Tone::Dim)),
    }
    rows.row("config", value);
    if let Some(error) = &config.reload_error {
        rows.row("config error", vec![(config_error(error), Tone::Failure)]);
    }
    if !config.restart_needed.is_empty() {
        let keys = one_line(&config.restart_needed.join(", "));
        let words = format!("{keys} (systemctl --user restart efrd)");
        rows.row("restart needed", vec![(words, Tone::Attention)]);
    }
    if let Some(held) = &config.held {
        rows.row("config held", vec![(held_text(held), Tone::Attention)]);
    }
}

/// The name that `,model` takes for the provider `id`, and its kind:
/// `anthropic-api.work` is `work` of `anthropic-api`. A provider with no account is
/// named by its short name, `anthropic` or `openai`, else by its id.
pub(crate) fn account_of(id: &str) -> (String, String) {
    match id.split_once('.') {
        Some((kind, account)) => (one_line(account), one_line(kind)),
        None => {
            let name = match id {
                "anthropic-api" => "anthropic",
                "openai-subscription" => "openai",
                other => other,
            };
            (one_line(name), one_line(id))
        }
    }
}

/// The command that logs in to an account of `kind` again: `efr login anthropic-api
/// --account personal`.
pub(crate) fn login_command(kind: &str, account: &str) -> String {
    let kind = match kind {
        "openai-subscription" => "openai".to_owned(),
        other => one_line(other),
    };
    format!("efr login {kind} --account {}", one_line(account))
}

/// The rows of the accounts: one for each provider and account that efrd knows, the
/// account of new conversations first, then the named accounts; the name, the kind and
/// the state in columns.
fn account_rows(rows: &mut Labelled<'_>, view: &StatusView<'_>) {
    let providers = &view.status.providers;
    if providers.is_empty() {
        rows.row("accounts", vec![("none configured".to_owned(), Tone::Dim)]);
        return;
    }
    // NOTE: the account of new conversations, then the named accounts, then the
    // providers with no account, each group in efrd's order.
    let rank = |provider: &ProviderStatus| match provider.active {
        true => 0,
        false if provider.provider.contains('.') => 1,
        false => 2,
    };
    let mut ordered: Vec<&ProviderStatus> = providers.iter().collect();
    ordered.sort_by_key(|provider| rank(provider));
    let named: Vec<(String, String)> =
        ordered.iter().map(|provider| account_of(&provider.provider)).collect();
    let name_w = named.iter().map(|(name, _)| name.width()).max().unwrap_or(0);
    let kind_w = named.iter().map(|(_, kind)| kind.width()).max().unwrap_or(0);
    for (at, (provider, (name, kind))) in ordered.iter().zip(&named).enumerate() {
        let label = if at == 0 { "accounts" } else { "" };
        let cells = format!(
            "{name}{}  {kind}{}  ",
            " ".repeat(name_w - name.width()),
            " ".repeat(kind_w - kind.width())
        );
        let mut value = vec![(cells, Tone::Text)];
        value.extend(account_state(provider, view.now, rows.options));
        rows.row(label, value);
    }
}

/// The state of one account: `logged in` with its token or its key, `signed out` with
/// the command that logs in again, or `not logged in`; then `active` for the account of
/// new conversations.
fn account_state(provider: &ProviderStatus, now: Timestamp, options: &RenderOptions) -> Pieces {
    let dot = separator(options);
    let mut pieces: Pieces = Vec::new();
    if provider.logged_in {
        pieces.push(("logged in".to_owned(), Tone::Success));
        if let Some(expires) = provider.expires_at {
            let when = if expires < now {
                format!("token expired {}", short_ago(expires, now))
            } else {
                format!("token expires {}", short_until(expires, now))
            };
            pieces.push((format!("{dot}{when}"), Tone::Text));
        }
        // NOTE: efrd writes the cut of a key as `...`; efr shows the mark of the locale.
        let hint = provider
            .key_hint
            .as_deref()
            .map(|hint| one_line(hint).replace("...", options.glyphs().more));
        if let Some(hint) = &hint {
            pieces.push((format!("{dot}key {hint}"), Tone::Text));
        }
        let key = if hint.is_some() { "" } else { "key " };
        if let Some(refused) = provider.key_refused_at {
            pieces.push((dot.clone(), Tone::Text));
            pieces.push((format!("{key}refused {}", short_ago(refused, now)), Tone::Attention));
        } else if key_unchecked(provider) {
            pieces.push((dot.clone(), Tone::Text));
            pieces.push((format!("{key}{KEY_UNCHECKED}"), Tone::Dim));
        }
    } else {
        match provider.provider.split_once('.') {
            // NOTE: efrd lists an account only while it runs, so an account with no
            // login logged out.
            Some((kind, account)) => {
                pieces.push(("signed out".to_owned(), Tone::Attention));
                pieces.push((dot.clone(), Tone::Text));
                pieces.push((login_command(kind, account), Tone::Code));
            }
            None => pieces.push(("not logged in".to_owned(), Tone::Dim)),
        }
    }
    if provider.active {
        pieces.push((dot, Tone::Text));
        pieces.push(("active".to_owned(), Tone::TextBold));
    }
    pieces
}

/// The rows of the sandbox: `ready` with what it runs on, or `unavailable` with why and
/// its fix; then the network view and the bus filter of a ready one.
fn sandbox_rows(rows: &mut Labelled<'_>, sandbox: &SandboxStatus, dot: &str) {
    if sandbox.available {
        let mut parts = Vec::new();
        if let Some(abi) = sandbox.landlock_abi {
            parts.push(format!("Landlock ABI {abi}"));
        }
        if let Some(version) = &sandbox.bwrap_version {
            parts.push(format!("bubblewrap {}", one_line(version)));
        }
        parts.push(format!("network {}", sandbox::network_words(sandbox)));
        let rest: String = parts.iter().map(|part| format!("{dot}{part}")).collect();
        rows.row("sandbox", vec![("ready".to_owned(), Tone::Success), (rest, Tone::Text)]);
        rows.text("netview", sandbox::netview_words(sandbox));
        rows.text("bus filter", sandbox::bus_filter_words(sandbox));
        return;
    }
    let reason = sandbox.reason.as_deref().unwrap_or("the probe gave no reason");
    let rest = format!("{dot}{}{dot}auto runs as cautious", one_line(reason));
    rows.row("sandbox", vec![("unavailable".to_owned(), Tone::Attention), (rest, Tone::Text)]);
    if let Some(fix) = sandbox.fix.as_deref() {
        rows.text("sandbox fix", one_line(fix));
    }
}

/// A duration in its two largest units, with no second unit of zero, as the listings
/// outside a turn say it: `45s`, `5m`, `5m 3s`, `1h`, `2h 5m`, `3d 4h`.
pub(crate) fn short_span(seconds: u64) -> String {
    let span = span(seconds);
    match [" 0s", " 0m", " 0h"].iter().find_map(|zero| span.strip_suffix(zero)) {
        Some(short) => short.to_owned(),
        None => span,
    }
}

/// How long ago `then` was at `now`, in [`short_span`]: `5m ago`; `just now` under a
/// second.
pub(crate) fn short_ago(then: Timestamp, now: Timestamp) -> String {
    let seconds = now.duration_since(then).as_secs();
    if seconds < 1 {
        return "just now".to_owned();
    }
    format!("{} ago", short_span(seconds.unsigned_abs()))
}

/// How long until `then` from `now`, in [`short_span`]: `in 52m 3s`, or how long ago it
/// passed.
pub(crate) fn short_until(then: Timestamp, now: Timestamp) -> String {
    let seconds = then.duration_since(now).as_secs();
    if seconds < 0 {
        short_ago(then, now)
    } else {
        format!("in {}", short_span(seconds.unsigned_abs()))
    }
}

/// ` · ` between the parts of a value, with the separator of the locale.
pub(crate) fn separator(options: &RenderOptions) -> String {
    format!(" {} ", options.glyphs().separator)
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

/// Where a model list came from, at `now`: `from the backend, fetched 5m ago`, `from
/// the cache, fetched 2h 5m ago` (the backend did not answer yet), `built into efr`, or
/// `no list yet; efrd fetches it in the background`.
pub(crate) fn origin(catalog: &CatalogStatus, now: Timestamp) -> String {
    let fetched = |place: &str| match catalog.fetched_at {
        Some(at) => format!("from {place}, fetched {}", short_ago(at, now)),
        None => format!("from {place}"),
    };
    match catalog.origin {
        CatalogOrigin::Backend => fetched("the backend"),
        CatalogOrigin::Cache => fetched("the cache"),
        CatalogOrigin::Builtin => "built into efr".to_owned(),
        CatalogOrigin::Missing => "no list yet; efrd fetches it in the background".to_owned(),
        _ => "from elsewhere".to_owned(),
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

#[cfg(test)]
mod tests;
