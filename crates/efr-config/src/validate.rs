//! The checks that the types alone do not make: sets of names, ranges and the form of
//! paths and URLs.

use std::ops::RangeInclusive;
use std::path::Path;

use crate::{PROVIDERS, Settings};

/// A value outside its allowed set or range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Invalid {
    pub(crate) key: &'static str,
    /// The value as TOML.
    pub(crate) value: String,
    pub(crate) expected: &'static str,
}

/// The first value of `settings` that is outside its allowed set or range, in the
/// order of the file.
pub(crate) fn check(settings: &Settings) -> Result<(), Invalid> {
    let Settings { log, model, openai, permissions, shell, conversation, render, .. } = settings;
    non_empty("log", log, "a tracing filter such as info")?;

    if !PROVIDERS.contains(&model.provider.as_str()) {
        return Err(invalid(
            "model.provider",
            text(&model.provider),
            "openai-subscription or openai-api",
        ));
    }
    if let Some(name) = &model.name {
        non_empty("model.name", name, "a model id such as gpt-5.5")?;
    }
    if let Some(effort) = &model.effort
        && !is_effort(effort)
    {
        return Err(invalid(
            "model.effort",
            text(effort),
            "a reasoning effort such as low, medium or high",
        ));
    }
    if let Some(tokens) = model.max_output_tokens {
        within(
            "model.max_output_tokens",
            u64::from(tokens),
            1..=1_000_000,
            "between 1 and 1000000",
        )?;
    }

    non_empty("openai.originator", &openai.originator, "a name such as efr")?;
    if let Some(models) = &openai.models
        && let Some(model) = models.iter().find(|model| model.trim().is_empty())
    {
        return Err(invalid("openai.models", text(model), "a list of model ids"));
    }
    if let Some(url) = &openai.subscription_base_url {
        http_url("openai.subscription_base_url", url)?;
    }
    if let Some(url) = &openai.api_base_url {
        http_url("openai.api_base_url", url)?;
    }

    if let Some(path) =
        permissions.secret_paths.iter().find(|path| !path.is_absolute() && !path.starts_with("~"))
    {
        return Err(invalid(
            "permissions.secret_paths",
            path_text(path),
            "absolute paths or paths that start with ~/",
        ));
    }

    if let Some(program) = &shell.program
        && !program.is_absolute()
    {
        return Err(invalid("shell.program", path_text(program), "an absolute path"));
    }
    within("shell.idle_minutes", shell.idle_minutes, 0..=525_600, "between 0 and 525600 (a year)")?;

    let queued = u64::try_from(conversation.max_queued).unwrap_or(u64::MAX);
    within("conversation.max_queued", queued, 1..=1024, "between 1 and 1024")?;
    if let Some(secs) = conversation.approval_timeout_secs {
        within(
            "conversation.approval_timeout_secs",
            secs,
            1..=604_800,
            "between 1 and 604800 (a week)",
        )?;
    }
    within(
        "conversation.update_interval_ms",
        conversation.update_interval_ms,
        0..=60_000,
        "between 0 and 60000",
    )?;
    within(
        "conversation.tty_idle_hours",
        conversation.tty_idle_hours,
        0..=8_760,
        "between 0 and 8760 (a year)",
    )?;

    if let Some(theme) = &render.theme {
        non_empty("render.theme", theme, "a theme name such as catppuccin-mocha")?;
    }
    Ok(())
}

/// A reasoning effort is one lowercase word, so a new effort of the backend needs no
/// change here; the daemon checks it against the efforts of the model.
fn is_effort(effort: &str) -> bool {
    !effort.is_empty()
        && effort.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}

fn non_empty(key: &'static str, value: &str, expected: &'static str) -> Result<(), Invalid> {
    if value.trim().is_empty() { Err(invalid(key, text(value), expected)) } else { Ok(()) }
}

fn http_url(key: &'static str, url: &str) -> Result<(), Invalid> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
    match rest {
        Some(rest) if !rest.is_empty() && !rest.contains(char::is_whitespace) => Ok(()),
        _ => Err(invalid(key, text(url), "an http or https URL")),
    }
}

fn within(
    key: &'static str,
    value: u64,
    range: RangeInclusive<u64>,
    expected: &'static str,
) -> Result<(), Invalid> {
    if range.contains(&value) { Ok(()) } else { Err(invalid(key, value.to_string(), expected)) }
}

fn invalid(key: &'static str, value: String, expected: &'static str) -> Invalid {
    Invalid { key, value, expected }
}

/// `value` as a TOML string.
fn text(value: &str) -> String {
    toml::Value::String(value.to_owned()).to_string()
}

fn path_text(path: &Path) -> String {
    text(&path.to_string_lossy())
}

#[cfg(test)]
mod tests;
