//! The listings outside a turn: `efr status`, the model catalog, the full mode row, a
//! held or failed reload, and `efr history` without a conversation. The commands slice
//! of the terminal look owns this file; `format.rs` re-exports its items, so callers
//! name them as `format::...`.

use std::fmt::Write as _;
use std::path::Path;

use efr_protocol::{
    AccountLimits, AdminConfigReloadResult, AdminStatusResult, Attention, CatalogOrigin,
    CatalogStatus, ConfigFileError, ConfigStatus, ConversationStatus, ConversationsListResult,
    FullModeStatus, HeldConfig, LoginKind, Mode, ProviderStatus, SandboxStatus,
};
use efr_render::RenderOptions;
use efr_stdx::text::span;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use unicode_width::UnicodeWidthStr as _;

use super::sandbox::{self, tilde};
use super::{Tone, accounts, ago, one_line, paint};
use crate::error::login_command;

/// Who can act on a conversation, from its `status` and its `attention`: the one rule of
/// the lists, which the phone's rows follow too (spec 6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowState {
    /// An approval waits that the phone may give too.
    Approval,
    /// A question of the model waits for an answer.
    Answer,
    /// Only the user's own machine can answer what waits.
    Terminal,
    /// A turn runs.
    Running,
    /// No turn runs, and the last turn that ended failed.
    Failed,
    /// Nothing of the above.
    Idle,
}

impl RowState {
    /// The words of the state and their tone (TM-Commands 0.0.12): the three waiting
    /// states in `warning`, `running` muted, `failed` in `error`, and none for idle.
    pub(crate) fn words(self) -> (&'static str, Tone) {
        match self {
            RowState::Approval => ("needs approval", Tone::Attention),
            RowState::Answer => ("needs an answer", Tone::Attention),
            RowState::Terminal => ("answer in the terminal", Tone::Attention),
            RowState::Running => ("running", Tone::Dim),
            RowState::Failed => ("failed", Tone::Failure),
            RowState::Idle => ("", Tone::Plain),
        }
    }
}

/// The state of a row, in this order: an approval that the phone may give, a question
/// of the model, a wait that only the terminal can answer, a running turn, a failed
/// last turn, else idle. An `attention` that this build does not know counts as none,
/// and so does a `status` that it does not know.
pub(crate) fn row_state(status: ConversationStatus, attention: Option<Attention>) -> RowState {
    match (status, attention) {
        (ConversationStatus::AwaitingApproval, attention)
            if attention != Some(Attention::Terminal) =>
        {
            RowState::Approval
        }
        (_, Some(Attention::Answer)) => RowState::Answer,
        (_, Some(Attention::Terminal)) => RowState::Terminal,
        (ConversationStatus::Running, _) => RowState::Running,
        (_, Some(Attention::Failed)) => RowState::Failed,
        _ => RowState::Idle,
    }
}

/// The width of the column of the state words: `answer in the terminal`, the longest.
const STATE_COLUMN: usize = 22;

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
    /// The time zone of the user, for the reset of a limit.
    pub(crate) zone: &'a TimeZone,
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
                Some(provider) => {
                    format!("{}: {}", accounts::account_of(provider), origin(catalog, now))
                }
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

/// The width below which `efr status` leaves out the key, the token, the login command
/// and `active`, and gives the age of the limits its own row (TM-Accounts 9).
const NARROW: usize = 80;

/// True when the screen is narrower than [`NARROW`].
fn narrow(options: &RenderOptions) -> bool {
    options.is_terminal() && usize::from(options.width()) < NARROW
}

/// The rows of the accounts (TM-Accounts 8, 9): one for each provider and account that
/// efrd knows, in the order of `models.list` (the account of new conversations, the
/// others with a login, then the signed-out ones), then the rest, grouped by kind: the
/// account of new conversations first in its kind, then the default account of the
/// kind. The name, the kind in words and the state stand in columns. The default
/// account of a kind has no name. Under each account with a login, its limits, 2
/// columns in.
fn account_rows(rows: &mut Labelled<'_>, view: &StatusView<'_>) {
    let providers = &view.status.providers;
    if providers.is_empty() {
        rows.row("accounts", vec![("none configured".to_owned(), Tone::Dim)]);
        return;
    }
    // NOTE: efrd sends the accounts in this order (`efr_protocol::order_by_list`), so
    // `efr status` keeps it.
    let named: Vec<(String, String)> = providers
        .iter()
        .map(|provider| {
            let id = &provider.provider;
            (accounts::account_name(id).unwrap_or_default(), accounts::kind_words(id))
        })
        .collect();
    let name_w = named.iter().map(|(name, _)| name.width()).max().unwrap_or(0);
    let kind_w = named.iter().map(|(_, kind)| kind.width()).max().unwrap_or(0);
    let narrow = narrow(rows.options);
    for (at, (provider, (name, kind))) in providers.iter().zip(&named).enumerate() {
        let label = if at == 0 { "accounts" } else { "" };
        // NOTE: with only default accounts, no account has a name and the column goes.
        let name = if name_w == 0 {
            String::new()
        } else {
            format!("{name}{}  ", " ".repeat(name_w - name.width()))
        };
        let cells = format!("{name}{kind}{}  ", " ".repeat(kind_w - kind.width()));
        let mut value = vec![(cells, Tone::Text)];
        let mut state = account_state(provider, view.now, rows.options);
        if narrow {
            state = narrow_state(state, &separator(rows.options));
        }
        value.extend(state);
        rows.row(label, value);
        // NOTE: a signed-out account shows no limits: they would be old, and it cannot
        // run anyway.
        if let Some(limits) = provider.limits.as_ref().filter(|_| provider.logged_in) {
            limit_lines(rows, limits, view, narrow);
        }
    }
}

/// The state of a narrow row: its first word and a refusal of the key, which needs
/// care; the token, the key, the login command and `active` go.
fn narrow_state(state: Pieces, dot: &str) -> Pieces {
    let mut kept: Pieces = Vec::new();
    for (at, (text, tone)) in state.into_iter().enumerate() {
        if at == 0 {
            kept.push((text, tone));
        } else if tone == Tone::Attention {
            kept.push((dot.to_owned(), Tone::Text));
            kept.push((text, tone));
        }
    }
    kept
}

/// The limit rows of an account, 2 columns in (TM-Accounts 8, 9): the window muted, the
/// percent in the tone of its level, `reached` at 100 %, then `resets 14:20` muted; a
/// limit with no percent, such as `requests`, says `reached` in `error`. Limits older
/// than 5 minutes say their age once, on the first row; on a narrow screen the age has
/// a row of its own.
fn limit_lines(
    rows: &mut Labelled<'_>,
    limits: &AccountLimits,
    view: &StatusView<'_>,
    narrow: bool,
) {
    let dot = separator(rows.options);
    let lines = accounts::limit_rows(limits, view.now, view.zone);
    let name_w = lines.iter().map(|line| line.name.width()).max().unwrap_or(0).max(LIMIT_NAME);
    let old = view.now.duration_since(limits.observed_at).as_secs() > accounts::STALE_AFTER;
    let age = old.then(|| limit_age(limits.observed_at, view.now));
    for (at, line) in lines.iter().enumerate() {
        let name = format!("{}{}", line.name, " ".repeat(name_w + 2 - line.name.width()));
        let mut value: Pieces = vec![(LIMIT_INDENT.to_owned(), Tone::Plain), (name, Tone::Dim)];
        match line.percent {
            Some(percent) => {
                value
                    .push((format!("{:>4}", format!("{percent}%")), accounts::level_tone(percent)));
                if line.reached {
                    value.push(("  reached".to_owned(), Tone::Text));
                }
            }
            None => value.push(("reached".to_owned(), Tone::Failure)),
        }
        if let Some(resets) = &line.resets {
            let gap = if line.reached { dot.clone() } else { "  ".to_owned() };
            value.push((gap, Tone::Text));
            value.push((format!("resets {resets}"), Tone::Dim));
        }
        if at == 0
            && !narrow
            && let Some(age) = &age
        {
            value.push((" ".to_owned(), Tone::Plain));
            value.push((format!("{}{age}", dot.trim_start()), Tone::Dim));
        }
        rows.row("", value);
        if at == 0
            && narrow
            && let Some(age) = &age
        {
            rows.row("", vec![(LIMIT_INDENT.to_owned(), Tone::Plain), (age.clone(), Tone::Dim)]);
        }
    }
}

/// How far the limit rows stand in from the name of their account.
const LIMIT_INDENT: &str = "  ";

/// The narrowest column of the names of the limit rows: `requests`.
const LIMIT_NAME: usize = 8;

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
            Some(_) => {
                pieces.push(("signed out".to_owned(), Tone::Attention));
                pieces.push((dot.clone(), Tone::Text));
                pieces.push((login_command(&provider.provider), Tone::Code));
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

/// The age of the limits of an account (TM-Accounts 8, design prompt 6 item 14): whole
/// minutes under one hour, such as `12m ago`, then [`short_ago`], such as `2h 5m ago`.
pub(crate) fn limit_age(observed: Timestamp, now: Timestamp) -> String {
    let seconds = now.duration_since(observed).as_secs().unsigned_abs();
    let whole = if seconds < 3_600 { seconds - seconds % 60 } else { seconds };
    if whole < 60 {
        return short_ago(observed, now);
    }
    format!("{} ago", short_span(whole))
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
/// `no list yet; efrd fetches it in the background` (`after a login` for a signed-out
/// account).
pub(crate) fn origin(catalog: &CatalogStatus, now: Timestamp) -> String {
    let fetched = |place: &str| match catalog.fetched_at {
        Some(at) => format!("from {place}, fetched {}", short_ago(at, now)),
        None => format!("from {place}"),
    };
    match catalog.origin {
        CatalogOrigin::Backend => fetched("the backend"),
        CatalogOrigin::Cache => fetched("the cache"),
        CatalogOrigin::Builtin => "built into efr".to_owned(),
        // NOTE: efrd fetches the list of an account only with its login.
        CatalogOrigin::Missing if catalog.signed_out => {
            "no list yet; efrd fetches it after a login".to_owned()
        }
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

/// `efr history --list`: one line per conversation, newest first, with the words of its
/// [`RowState`] in their colour (TM-Commands 0.0.12).
pub(crate) fn conversations(
    list: &ConversationsListResult,
    now: Timestamp,
    options: &RenderOptions,
) -> String {
    if list.conversations.is_empty() {
        return "no conversations yet\n".to_owned();
    }
    let mut out = String::new();
    for summary in &list.conversations {
        let title = summary.title.as_deref().map_or_else(|| "(untitled)".to_owned(), one_line);
        let (words, tone) = row_state(summary.status, summary.attention).words();
        let pad = " ".repeat(STATE_COLUMN.saturating_sub(words.len()));
        // The age is at most 11 characters wide (`59m 59s ago`, `23h 59m ago`), so the
        // titles start in one column.
        let _ = writeln!(
            out,
            "{}  {}{pad}  {:>11}  {title}",
            summary.id,
            paint(words, tone, options),
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
