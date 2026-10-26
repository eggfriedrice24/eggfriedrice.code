//! `cargo xtask protocol-schema`: writes `docs/protocol.schema.json` from efr-protocol,
//! or with `--check` fails when it is not current, which CI runs. The phone app makes
//! its TypeScript types from this file with json-schema-to-typescript, so the file is
//! the form that such a generator reads (`efr_protocol::schema::codegen_document`).
//!
//! The command also refuses a schema that holds a tuple (`prefixItems`, or `items` as a
//! list): json-schema-to-typescript cannot read one, so no wire type may be a Rust
//! tuple.

use std::path::Path;

use anyhow::Context as _;
use serde_json::Value;

use crate::output;

/// The generated file, relative to the repository root.
pub(crate) const SCHEMA: &str = "docs/protocol.schema.json";

/// The text of the file: pretty JSON with a final newline.
pub(crate) fn generated() -> anyhow::Result<String> {
    let document = efr_protocol::schema::codegen_document();
    let mut text =
        serde_json::to_string_pretty(&document).context("the protocol schema did not encode")?;
    text.push('\n');
    Ok(text)
}

/// The JSON paths of every tuple in `value`, such as `/$defs/Pair/prefixItems`.
pub(crate) fn tuples(value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    walk(value, "", &mut found);
    found.sort();
    found
}

fn walk(value: &Value, path: &str, found: &mut Vec<String>) {
    match value {
        Value::Object(members) => {
            for (key, member) in members {
                let here = format!("{path}/{key}");
                if key == "prefixItems" || (key == "items" && member.is_array()) {
                    found.push(here.clone());
                }
                walk(member, &here, found);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                walk(item, &format!("{path}/{index}"), found);
            }
        }
        _ => {}
    }
}

pub(crate) fn run(root: &Path, check: bool) -> anyhow::Result<bool> {
    let tuples = tuples(&efr_protocol::schema::codegen_document());
    if !tuples.is_empty() {
        for path in &tuples {
            output::line(&format!(
                "protocol-schema: {path} is a tuple, which a code generator cannot read; \
                 make the wire type a struct"
            ));
        }
        return Ok(false);
    }
    let text = generated()?;
    let file = root.join(SCHEMA);
    let on_disk = std::fs::read_to_string(&file).unwrap_or_default();
    if on_disk == text {
        output::line(&format!("protocol-schema: {SCHEMA} is current"));
        return Ok(true);
    }
    if check {
        output::line(&format!(
            "protocol-schema: {SCHEMA} is not current; run `cargo xtask protocol-schema`"
        ));
        return Ok(false);
    }
    std::fs::write(&file, text).with_context(|| format!("could not write {SCHEMA}"))?;
    output::line(&format!("protocol-schema: wrote {SCHEMA}"));
    Ok(true)
}

#[cfg(test)]
mod tests;
