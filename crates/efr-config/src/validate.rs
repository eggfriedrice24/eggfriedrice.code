//! The checks that the types alone do not make: sets of names, ranges and the form of
//! paths and URLs.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use crate::{PROVIDERS, SandboxSettings, Settings};

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
    let Settings { log, model, openai, permissions, shell, conversation, sandbox, render, .. } =
        settings;
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
        && !efr_protocol::is_effort_word(effort)
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
    within(
        "shell.interactive_timeout_minutes",
        shell.interactive_timeout_minutes,
        1..=1_440,
        "between 1 and 1440 (a day)",
    )?;

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

    check_sandbox(sandbox)?;

    if let Some(theme) = &render.theme {
        non_empty("render.theme", theme, "a theme name such as catppuccin-mocha")?;
    }
    Ok(())
}

/// The checks of `[sandbox]`, in the order of the table.
fn check_sandbox(sandbox: &SandboxSettings) -> Result<(), Invalid> {
    const HOME_OR_ABSOLUTE: &str = "absolute paths or paths that start with ~/";
    if let Some(bwrap) = &sandbox.bwrap
        && !bwrap.is_absolute()
    {
        return Err(invalid("sandbox.bwrap", path_text(bwrap), "an absolute path"));
    }
    home_or_absolute("sandbox.write_roots", &sandbox.write_roots, HOME_OR_ABSOLUTE)?;
    if let Some(root) = sandbox
        .write_roots
        .iter()
        .find(|root| root.as_path() == Path::new("~") || root.parent().is_none())
    {
        return Err(invalid(
            "sandbox.write_roots",
            path_text(root),
            "directories below ~/ or absolute paths, never ~ or / itself",
        ));
    }
    home_or_absolute("sandbox.caches", &sandbox.caches, HOME_OR_ABSOLUTE)?;
    within("sandbox.cache_days", u64::from(sandbox.cache_days), 1..=3_650, "between 1 and 3650")?;
    within(
        "sandbox.cache_max_gib",
        u64::from(sandbox.cache_max_gib),
        1..=10_000,
        "between 1 and 10000",
    )?;
    home_or_absolute("sandbox.mask", &sandbox.mask, HOME_OR_ABSOLUTE)?;
    patterns("sandbox.mask_globs", &sandbox.mask_globs)?;
    home_or_absolute("sandbox.protect", &sandbox.protect, HOME_OR_ABSOLUTE)?;
    variables("sandbox.env_deny", &sandbox.env_deny)?;
    variables("sandbox.env_keep", &sandbox.env_keep)?;
    variables("sandbox.promote_env", &sandbox.promote_env)?;
    variables("sandbox.export_deny", &sandbox.export_deny)?;
    home_or_absolute("sandbox.synced_dirs", &sandbox.synced_dirs, HOME_OR_ABSOLUTE)?;
    patterns("sandbox.surface_files", &sandbox.surface_files)?;
    if let Some(name) = sandbox
        .rebuildable
        .iter()
        .find(|name| name.is_empty() || name.contains(['/', '\0']) || *name == "." || *name == "..")
    {
        return Err(invalid("sandbox.rebuildable", text(name), "directory names such as target"));
    }
    Ok(())
}

/// Every path is absolute or starts with the `~` component.
fn home_or_absolute(
    key: &'static str,
    paths: &[PathBuf],
    expected: &'static str,
) -> Result<(), Invalid> {
    match paths.iter().find(|path| !path.is_absolute() && !path.starts_with("~")) {
        Some(path) => Err(invalid(key, path_text(path), expected)),
        None => Ok(()),
    }
}

/// Every entry is a variable name, or a pattern of one with `*`.
fn variables(key: &'static str, names: &[String]) -> Result<(), Invalid> {
    let bad = |name: &String| {
        let mut bytes = name.bytes();
        let first_ok =
            bytes.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_' || b == b'*');
        !first_ok || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'*')
    };
    match names.iter().find(|name| bad(name)) {
        Some(name) => Err(invalid(key, text(name), "variable names, or patterns of them with *")),
        None => Ok(()),
    }
}

/// Every entry is a name or a relative path, with `*`, optionally behind a `!`.
fn patterns(key: &'static str, list: &[String]) -> Result<(), Invalid> {
    let bad = |pattern: &String| {
        let body = pattern.strip_prefix('!').unwrap_or(pattern);
        body.is_empty()
            || body.starts_with('/')
            || body.contains('\0')
            || body.split('/').any(|part| part.is_empty() || part == "..")
    };
    match list.iter().find(|pattern| bad(pattern)) {
        Some(pattern) => {
            Err(invalid(key, text(pattern), "names or relative paths, with * and an optional !"))
        }
        None => Ok(()),
    }
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
