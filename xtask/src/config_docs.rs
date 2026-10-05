//! `cargo xtask config-docs`: writes `docs/config.md` and `docs/config.schema.json`
//! from `efr-config`, or with `--check` fails when either is not current, which CI
//! runs. The text comes from the typed tables, so a new key reaches both files without
//! an edit here.

use std::path::Path;

use anyhow::Context as _;

use crate::output;

/// The generated files, relative to the repository root.
pub(crate) const REFERENCE: &str = "docs/config.md";
pub(crate) const SCHEMA: &str = "docs/config.schema.json";

/// The files and their current text.
pub(crate) fn generated() -> anyhow::Result<[(&'static str, String); 2]> {
    let reference = efr_config::reference().context("the config reference could not be built")?;
    Ok([(REFERENCE, reference), (SCHEMA, efr_config::schema_text())])
}

pub(crate) fn run(root: &Path, check: bool) -> anyhow::Result<bool> {
    let mut current = true;
    for (path, text) in generated()? {
        let file = root.join(path);
        let on_disk = std::fs::read_to_string(&file).unwrap_or_default();
        if on_disk == text {
            output::line(&format!("config-docs: {path} is current"));
            continue;
        }
        if check {
            output::line(&format!(
                "config-docs: {path} is not current; run `cargo xtask config-docs`"
            ));
            current = false;
        } else {
            std::fs::write(&file, text).with_context(|| format!("could not write {path}"))?;
            output::line(&format!("config-docs: wrote {path}"));
        }
    }
    Ok(current)
}

#[cfg(test)]
mod tests;
