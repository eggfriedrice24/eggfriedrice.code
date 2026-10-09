use std::path::Path;

use pretty_assertions::assert_eq;

use crate::{EXAMPLE, RESTART_KEYS, SCHEMA_URL, Settings, Source, keys};

const PATH: &str = "/home/u/.config/efr/config.toml";

/// A line of the example that sets a key once its `# ` is removed, with that key.
fn commented_key(line: &str) -> Option<&str> {
    let line = line.strip_prefix("# ")?;
    if line == "[[permissions.rules]]" {
        return Some("rules");
    }
    let (key, _) = line.split_once(" = ")?;
    key.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'_').then_some(key)
}

/// Every key the example names, as a dotted key, with the comment above it.
fn named_keys() -> Vec<(String, String)> {
    let mut table = None;
    let mut comment = String::new();
    let mut found = Vec::new();
    // The keys of the example rule belong to the rule, not to the table.
    let mut in_rule = false;
    for line in EXAMPLE.lines() {
        if in_rule && line.starts_with("# ") {
            continue;
        }
        in_rule = false;
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            table = Some(name.to_owned());
            comment.clear();
        } else if let Some(key) = commented_key(line) {
            let key = match (&table, key) {
                (_, "rules") => {
                    in_rule = true;
                    "permissions.rules".to_owned()
                }
                (Some(table), key) => format!("{table}.{key}"),
                (None, key) => key.to_owned(),
            };
            found.push((key, std::mem::take(&mut comment)));
        } else if let Some(text) = line.strip_prefix("# ") {
            comment.push_str(text);
            comment.push(' ');
        } else {
            comment.clear();
        }
    }
    found
}

/// The example with every commented key line made live.
fn uncommented() -> String {
    EXAMPLE
        .lines()
        .map(|line| match commented_key(line) {
            Some(_) => line.trim_start_matches("# "),
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_example_as_shipped_holds_only_the_defaults() {
    let settings = Settings::parse(Path::new(PATH), Some(EXAMPLE)).unwrap();

    assert_eq!(settings, Settings::parse(Path::new(PATH), None).unwrap());
}

#[test]
fn the_example_names_every_key_once_in_the_order_of_the_keys() {
    let named: Vec<String> = named_keys().into_iter().map(|(key, _)| key).collect();

    assert_eq!(named, keys());
}

#[test]
fn every_key_of_the_example_has_a_comment_and_says_when_it_needs_a_restart() {
    for (key, comment) in named_keys() {
        assert!(comment.len() > 20, "{key} has no comment");
        let restart = RESTART_KEYS.contains(&key.as_str());
        assert_eq!(comment.contains("(restart)"), restart, "{key}: {comment}");
    }
}

#[test]
fn the_example_with_every_line_uncommented_is_valid_and_sets_every_key() {
    let text = uncommented();

    let settings = Settings::parse(Path::new(PATH), Some(&text)).unwrap();

    for key in keys() {
        assert_eq!(settings.source(&key), Source::File, "{key}");
    }
    assert_eq!(settings.permissions.rules.rules().len(), 1);
}

#[test]
fn the_example_names_its_schema_first() {
    assert_eq!(EXAMPLE.lines().next(), Some(format!("#:schema {SCHEMA_URL}").as_str()));
}

#[test]
fn a_commented_key_shows_its_default_unless_it_is_a_sample() {
    let defaults = Settings::parse(Path::new(PATH), None).unwrap();
    let lines: Vec<&str> = EXAMPLE.lines().collect();
    let mut table: Option<&str> = None;
    let mut samples = Vec::new();
    // The keys of the example rule belong to the rule, not to the table.
    let mut in_rule = false;
    for (at, line) in lines.iter().enumerate() {
        if in_rule && line.starts_with("# ") {
            continue;
        }
        in_rule = false;
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            table = Some(name);
            continue;
        }
        let Some(key) = commented_key(line) else { continue };
        if key == "rules" {
            in_rule = true;
            continue;
        }
        let dotted = table.map_or_else(|| key.to_owned(), |table| format!("{table}.{key}"));
        let mut live = lines.clone();
        live[at] = line.trim_start_matches("# ");
        let mut settings = Settings::parse(Path::new(PATH), Some(&live.join("\n"))).unwrap();
        settings.set_source(&dotted, Source::Default);
        if settings != defaults {
            samples.push(dotted);
        }
    }
    // These keys have no default or an empty one, so they show a sample value; every
    // other key shows its default.
    let expected = [
        "model.name",
        "model.effort",
        "model.system_prompt",
        "model.max_output_tokens",
        "openai.models",
        "openai.subscription_base_url",
        "openai.api_base_url",
        "openai.organization",
        "openai.project",
        "anthropic.base_url",
        "anthropic.models",
        "anthropic.workspace_id",
        "permissions.secret_paths",
        "shell.program",
        "conversation.approval_timeout_secs",
        "sandbox.bwrap",
        "sandbox.write_roots",
        "sandbox.mask",
        "sandbox.protect",
        "sandbox.env_deny",
        "sandbox.env_keep",
        "sandbox.export_deny",
        "render.theme",
        "render.palette",
        "render.colors.text",
        "render.colors.muted",
        "render.colors.accent",
        "render.colors.heading",
        "render.colors.link",
        "render.colors.code",
        "render.colors.success",
        "render.colors.warning",
        "render.colors.error",
        "render.colors.quote",
        "render.colors.diff.add",
        "render.colors.diff.remove",
        "render.colors.diff.hunk",
    ];
    assert_eq!(samples, expected);
}
