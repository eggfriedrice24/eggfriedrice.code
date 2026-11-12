//! `efr models` (TM-Commands frame 2): the daemon's model list, grouped by account the
//! way `,model` takes them. Each account has a head, `work · anthropic-api · from the
//! backend, fetched 5m ago`, and a row for each model: `*` before the default, the id,
//! the window (and the largest one that the config's list of models can set), and the
//! efforts with the model's default effort underlined. An account that logged out keeps
//! its head with `signed out`, and its rows are muted. A last muted line says what the
//! marks mean and how `,model` picks one. A turn on a model goes to the first account
//! with a login that lists it. With `--names`, only the ids, each once, one per line,
//! which the zsh plugin completes `,model` from.
//!
//! The daemon lists the models of the accounts with a login; `admin.status` names the
//! accounts that logged out, so the listing asks for it too. A daemon that cannot
//! answer it leaves those out.

use std::fmt::Write as _;

use jiff::Timestamp;
use unicode_width::UnicodeWidthStr as _;

use efr_protocol::{
    AdminStatus, AdminStatusResult, Method, ModelInfo, ModelSource, ModelsList, ModelsListResult,
    Origin,
};
use efr_render::{Attrs, RenderOptions, Role};

use crate::cli::ModelsArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format::context::count;
use crate::format::status::{account_of, origin, separator};
use crate::format::{self, Tone, paint};
use crate::live::effective_width;
use crate::output::Output;

/// The provider whose models `[anthropic] models` adds.
const ANTHROPIC: &str = "anthropic-api";

/// The narrowest column of the ids, so the blocks of the accounts line up.
const ID_COLUMNS: usize = 17;

/// The narrowest column of the windows, so the blocks of the accounts line up.
const WINDOW_COLUMNS: usize = 14;

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    args: &ModelsArgs,
) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let list: ModelsListResult = client.call(Method::ModelsList(ModelsList {})).await?;
    if args.names {
        return out.out(&names(&list));
    }
    let signed_out =
        match client.call::<AdminStatusResult>(Method::AdminStatus(AdminStatus {})).await {
            Ok(status) => status
                .providers
                .into_iter()
                .filter(|provider| !provider.logged_in && provider.provider.contains('.'))
                .map(|provider| provider.provider)
                .collect(),
            Err(error) => {
                tracing::debug!(error = %error, "the accounts that logged out could not be read");
                Vec::new()
            }
        };
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    out.out(&listing(&list, &signed_out, ctx.clock.now(), &options))
}

/// Each id once, on a line of its own, in the order of the list.
pub(crate) fn names(list: &ModelsListResult) -> String {
    let mut seen = std::collections::HashSet::new();
    list.models
        .iter()
        .filter(|model| seen.insert(model.id.as_str()))
        .map(|model| format!("{}\n", format::one_line(&model.id)))
        .collect()
}

/// One account of the listing: its provider id and its models.
struct Account<'a> {
    provider: Option<&'a str>,
    models: Vec<&'a ModelInfo>,
    signed_out: bool,
}

/// The listing: a block for each account at `now`, apart by a blank line, then the
/// muted line of the marks. `signed_out` names the accounts that logged out.
pub(crate) fn listing(
    list: &ModelsListResult,
    signed_out: &[String],
    now: Timestamp,
    options: &RenderOptions,
) -> String {
    let fallback = list.catalog.as_ref().and_then(|catalog| catalog.provider.as_deref());
    let mut accounts: Vec<Account<'_>> = Vec::new();
    for model in &list.models {
        let provider = model.provider.as_deref().or(fallback);
        match accounts.iter_mut().find(|account| account.provider == provider) {
            Some(account) => account.models.push(model),
            None => accounts.push(Account {
                provider,
                models: vec![model],
                signed_out: provider.is_some_and(|id| signed_out.iter().any(|out| out == id)),
            }),
        }
    }
    // An account whose list did not come yet has no model. It keeps its head, which says
    // where its list stands, so the command never prints nothing for a login.
    for catalog in list.catalogs.iter().chain(&list.catalog) {
        let provider = catalog.provider.as_deref();
        if !accounts.iter().any(|account| account.provider == provider) {
            accounts.push(Account {
                provider,
                models: Vec::new(),
                signed_out: provider.is_some_and(|id| signed_out.iter().any(|out| out == id)),
            });
        }
    }
    for id in signed_out {
        if !accounts.iter().any(|account| account.provider == Some(id.as_str())) {
            accounts.push(Account { provider: Some(id), models: Vec::new(), signed_out: true });
        }
    }
    let mut out = String::new();
    for account in &accounts {
        block(&mut out, list, account, now, options);
        out.push('\n');
    }
    if !list.models.is_empty() {
        let _ = writeln!(out, "{}", paint(&legend(&accounts, options), Tone::Dim, options));
    } else if out.ends_with("\n\n") {
        // No model, so no legend: the blank line that stands before it goes too.
        out.pop();
    }
    out
}

/// The block of one account: its head, then a row for each of its models.
fn block(
    out: &mut String,
    list: &ModelsListResult,
    account: &Account<'_>,
    now: Timestamp,
    options: &RenderOptions,
) {
    let dot = separator(options);
    let catalog = match account.provider {
        Some(id) => list
            .catalogs
            .iter()
            .chain(&list.catalog)
            .find(|catalog| catalog.provider.as_deref() == Some(id)),
        None => list.catalog.as_ref(),
    };
    match account.provider {
        Some(id) => {
            let (name, kind) = account_of(id);
            out.push_str(&paint(&name, Tone::TextBold, options));
            out.push(' ');
            let dot = dot.trim_start();
            if account.signed_out {
                out.push_str(&paint(
                    &format!("{dot}{kind} {}", dot.trim_end()),
                    Tone::Dim,
                    options,
                ));
                out.push(' ');
                out.push_str(&paint("signed out", Tone::Attention, options));
            } else {
                let mut rest = format!("{dot}{kind}");
                if let Some(catalog) = catalog {
                    let _ = write!(rest, " {dot}{}", origin(catalog, now));
                }
                out.push_str(&paint(&rest, Tone::Dim, options));
            }
            out.push('\n');
        }
        // NOTE: an older daemon names no provider: the head says where the list came from.
        None => {
            if let Some(catalog) = catalog {
                let _ = writeln!(out, "{}", paint(&origin(catalog, now), Tone::Dim, options));
            }
        }
    }
    let ids: Vec<String> = account.models.iter().map(|model| format::one_line(&model.id)).collect();
    let windows: Vec<String> = account.models.iter().map(|model| window(model)).collect();
    let id_width = ids.iter().map(|id| id.width()).max().unwrap_or(0).max(ID_COLUMNS);
    let window_width =
        windows.iter().map(|window| window.width()).max().unwrap_or(0).max(WINDOW_COLUMNS);
    for ((model, id), window) in account.models.iter().zip(&ids).zip(&windows) {
        let cells = format!(
            "{id}{}  {window}{}  ",
            " ".repeat(id_width - id.width()),
            " ".repeat(window_width - window.width())
        );
        let mut pieces: Vec<(String, Tone, bool)> = Vec::new();
        if model.default {
            pieces.push(("*".to_owned(), Tone::Accent, false));
            pieces.push((format!(" {cells}"), Tone::Text, false));
        } else {
            pieces.push((format!("  {cells}"), Tone::Text, false));
        }
        pieces.extend(efforts(model));
        match model.source {
            ModelSource::Builtin => {}
            ModelSource::Config => {
                let from = format!("{dot}from {}", config_table(account.provider));
                pieces.push((from, Tone::Dim, false));
            }
            _ => pieces.push((format!("{dot}from elsewhere"), Tone::Dim, false)),
        }
        if account.signed_out {
            // NOTE: the rows of an account that logged out are muted, with no marks.
            let text: String = pieces.iter().map(|(text, ..)| text.as_str()).collect();
            let _ = writeln!(out, "{}", paint(&text, Tone::Dim, options));
            continue;
        }
        for (text, tone, underline) in &pieces {
            if *underline {
                out.push_str(&options.paint_with(Role::Text, UNDERLINE, text));
            } else {
                out.push_str(&paint(text, *tone, options));
            }
        }
        out.push('\n');
    }
}

/// The underline of the default effort.
const UNDERLINE: Attrs = Attrs { underline: true, ..Attrs::NONE };

/// The efforts of `model`, each with whether it is underlined: `efforts` muted, then the
/// efforts in `text` with the model's default effort underlined; `no efforts` for a
/// model of the catalog that takes none, `efforts unknown` for one that efr does not
/// know.
fn efforts(model: &ModelInfo) -> Vec<(String, Tone, bool)> {
    if model.efforts.is_empty() {
        let words = match model.source {
            ModelSource::Builtin => "no efforts",
            _ => "efforts unknown",
        };
        return vec![(words.to_owned(), Tone::Dim, false)];
    }
    let mut pieces = vec![("efforts".to_owned(), Tone::Dim, false)];
    for effort in &model.efforts {
        let effort = format::one_line(effort);
        pieces.push((" ".to_owned(), Tone::Text, false));
        let underline = model.default_effort.as_deref() == Some(effort.as_str());
        pieces.push((effort, Tone::Text, underline));
    }
    pieces
}

/// The muted last line: what `*` and the underline mean, and how `,model` picks a model,
/// with the first model that is not the default as the example.
fn legend(accounts: &[Account<'_>], options: &RenderOptions) -> String {
    let dot = separator(options);
    let mut line = format!("* the default{dot}underlined: the default effort");
    let example = accounts.iter().filter(|account| !account.signed_out).find_map(|account| {
        let model = account.models.iter().find(|model| !model.default)?;
        let id = format::one_line(&model.id);
        Some(match account.provider {
            Some(provider) => format!("{}/{id}", account_of(provider).0),
            None => id,
        })
    });
    if let Some(example) = example {
        let _ = write!(line, "{dot},model {example} picks one");
    }
    line
}

/// The config's list of models of `provider` (or of its kind, for an account), which
/// adds the models that its catalog does not hold. An older daemon names no provider;
/// it runs only OpenAI.
fn config_table(provider: Option<&str>) -> &'static str {
    match provider.map(|id| id.split_once('.').map_or(id, |(kind, _)| kind)) {
        Some(ANTHROPIC) => "[anthropic] models",
        _ => "[openai] models",
    }
}

/// The window of `model`, such as `272k (up to 872k)`, or `window unknown`.
fn window(model: &ModelInfo) -> String {
    match (model.context_window, model.max_context_window) {
        (Some(window), Some(max)) if max > window => {
            format!("{} (up to {})", count(window), count(max))
        }
        (Some(window), _) => count(window),
        (None, _) => "window unknown".to_owned(),
    }
}

#[cfg(test)]
mod tests;
