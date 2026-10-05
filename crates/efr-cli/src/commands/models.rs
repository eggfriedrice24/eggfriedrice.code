//! `efr models`: the daemon's model list, the default marked with `*`. With `--names`,
//! only the ids, one per line, which the zsh plugin completes `,model` from.

use std::fmt::Write as _;

use unicode_width::UnicodeWidthStr as _;

use efr_protocol::{Method, ModelSource, ModelsList, ModelsListResult, Origin};

use crate::cli::ModelsArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::format;
use crate::output::Output;

pub(crate) async fn run(
    ctx: &Context,
    out: &mut Output,
    args: &ModelsArgs,
) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let list: ModelsListResult = client.call(Method::ModelsList(ModelsList {})).await?;
    out.out(&if args.names { names(&list) } else { listing(&list) })
}

/// Each id on a line of its own.
pub(crate) fn names(list: &ModelsListResult) -> String {
    list.models.iter().map(|model| format!("{}\n", format::one_line(&model.id))).collect()
}

/// A line per model: `*` before the default, the id, the efforts with the model's
/// default effort, and where a model that is not built in comes from.
pub(crate) fn listing(list: &ModelsListResult) -> String {
    let ids: Vec<String> = list.models.iter().map(|model| format::one_line(&model.id)).collect();
    let width = ids.iter().map(|id| id.width()).max();
    let mut out = String::new();
    for (model, id) in list.models.iter().zip(&ids) {
        let mark = if model.default { '*' } else { ' ' };
        let pad = width.unwrap_or(0) - id.width();
        let mut line = format!("{mark} {id}{:pad$}  ", "");
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
    out
}

#[cfg(test)]
mod tests;
