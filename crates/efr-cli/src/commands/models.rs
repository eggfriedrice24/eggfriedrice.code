//! `efr models`: the daemon's model list, the default marked with `*`, each model's
//! context window, and where the list came from. With `--names`, only the ids, one per
//! line, which the zsh plugin completes `,model` from.

use std::fmt::Write as _;

use jiff::Timestamp;
use unicode_width::UnicodeWidthStr as _;

use efr_protocol::{Method, ModelInfo, ModelSource, ModelsList, ModelsListResult, Origin};

use crate::cli::ModelsArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format;
use crate::format::context::count;
use crate::output::Output;

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    args: &ModelsArgs,
) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let list: ModelsListResult = client.call(Method::ModelsList(ModelsList {})).await?;
    out.out(&if args.names { names(&list) } else { listing(&list, ctx.clock.now()) })
}

/// Each id on a line of its own.
pub(crate) fn names(list: &ModelsListResult) -> String {
    list.models.iter().map(|model| format!("{}\n", format::one_line(&model.id))).collect()
}

/// A line per model: `*` before the default, the id, the window (and the largest one
/// that `[openai] models` can set), the efforts with the model's default effort, and
/// where a model that is not in the catalog comes from. A last line says where the
/// catalog came from, at `now`.
pub(crate) fn listing(list: &ModelsListResult, now: Timestamp) -> String {
    let ids: Vec<String> = list.models.iter().map(|model| format::one_line(&model.id)).collect();
    let windows: Vec<String> = list.models.iter().map(window).collect();
    let id_width = ids.iter().map(|id| id.width()).max().unwrap_or(0);
    let window_width = windows.iter().map(|window| window.width()).max().unwrap_or(0);
    let mut out = String::new();
    for ((model, id), window) in list.models.iter().zip(&ids).zip(&windows) {
        let mark = if model.default { '*' } else { ' ' };
        let id_pad = id_width - id.width();
        let window_pad = window_width - window.width();
        let mut line = format!("{mark} {id}{:id_pad$}  {window}{:window_pad$}  ", "", "");
        if model.efforts.is_empty() {
            line.push_str("efforts unknown");
        } else {
            let _ = write!(line, "efforts: {}", format::one_line(&model.efforts.join(", ")));
        }
        if let Some(effort) = &model.default_effort {
            let _ = write!(line, "; default {}", format::one_line(effort));
        }
        match model.source {
            ModelSource::Builtin => {}
            ModelSource::Config => line.push_str("; from [openai] models"),
            _ => line.push_str("; from elsewhere"),
        }
        let _ = writeln!(out, "{}", line.trim_end());
    }
    if let Some(catalog) = &list.catalog {
        let _ = writeln!(out, "models: {}", format::catalog(catalog, now));
    }
    out
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
