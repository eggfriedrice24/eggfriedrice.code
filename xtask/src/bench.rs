//! `cargo xtask bench-size` and `cargo xtask bench-compare`.
//!
//! `bench-size` reads the sizes of the release binaries in cargo's target dir and
//! compares each with its budget in `xtask/size-budget.json`. A binary over its budget
//! fails the command, so a size gain cannot go away unseen.
//!
//! `bench-compare` reads the JSON lines that two runs of `just bench-perf` wrote (one
//! directory each, through `EFR_BENCH_OUT`) and prints each line of the new run next
//! to the same line of the old run, with the change in percent. It never fails: a
//! busy machine makes numbers that are not valid, and a person reads the table.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use cargo_metadata::MetadataCommand;
use serde_json::Value;

use crate::output;

/// The budget file, relative to the repository root.
pub(crate) const SIZE_BUDGET: &str = "xtask/size-budget.json";

/// The binaries that ship, in the order of the table.
const BINARIES: [&str; 3] = ["efrd", "efr", "efr-sbx"];

pub(crate) fn size(root: &Path, dir: Option<PathBuf>) -> anyhow::Result<bool> {
    let dir = match dir {
        Some(dir) => dir,
        None => release_dir(root)?,
    };
    let text = fs::read_to_string(root.join(SIZE_BUDGET))
        .with_context(|| format!("could not read {SIZE_BUDGET}"))?;
    let budget = budgets(&text)?;
    let mut sizes = Vec::new();
    for name in BINARIES {
        let path = dir.join(name);
        let bytes = fs::metadata(&path)
            .with_context(|| {
                format!("could not read {}; run `just build-release` first", path.display())
            })?
            .len();
        sizes.push((name, bytes));
    }
    let (lines, fits) = size_table(&sizes, &budget);
    for line in lines {
        output::line(&line);
    }
    Ok(fits)
}

/// The release dir of cargo's target dir, from `cargo metadata`, so a
/// `CARGO_TARGET_DIR` outside the checkout works too.
fn release_dir(root: &Path) -> anyhow::Result<PathBuf> {
    let metadata = MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()
        .context("cargo metadata failed")?;
    Ok(metadata.target_directory.into_std_path_buf().join("release"))
}

/// The budget of each binary in bytes, from the budget file's `max_bytes` table.
pub(crate) fn budgets(text: &str) -> anyhow::Result<BTreeMap<String, u64>> {
    let value: Value = serde_json::from_str(text).context("the size budget is not JSON")?;
    let table = value
        .get("max_bytes")
        .and_then(Value::as_object)
        .context("the size budget has no max_bytes")?;
    table
        .iter()
        .map(|(name, bytes)| {
            let bytes =
                bytes.as_u64().with_context(|| format!("the budget of {name} is not a size"))?;
            Ok((name.clone(), bytes))
        })
        .collect()
}

/// The lines of the size table and whether every binary fits its budget. A binary with
/// no budget fails, so a new binary gets one.
pub(crate) fn size_table(
    sizes: &[(&str, u64)],
    budget: &BTreeMap<String, u64>,
) -> (Vec<String>, bool) {
    let mb = |bytes: u64| bytes as f64 / 1_000_000.0;
    let mut fits = true;
    let mut lines = Vec::new();
    for (name, bytes) in sizes {
        let line = match budget.get(*name) {
            Some(max) if bytes <= max => format!(
                "bench-size: {name:<8} {:>8.3} MB  budget {:>8.3} MB  room {:>7.3} MB",
                mb(*bytes),
                mb(*max),
                mb(max - bytes)
            ),
            Some(max) => {
                fits = false;
                format!(
                    "bench-size: {name:<8} {:>8.3} MB  budget {:>8.3} MB  OVER by {:.3} MB",
                    mb(*bytes),
                    mb(*max),
                    mb(bytes - max)
                )
            }
            None => {
                fits = false;
                format!(
                    "bench-size: {name:<8} {:>8.3} MB  has no budget in {SIZE_BUDGET}",
                    mb(*bytes)
                )
            }
        };
        lines.push(line);
    }
    (lines, fits)
}

pub(crate) fn compare(old: &Path, new: &Path) -> anyhow::Result<bool> {
    let old = runs(old)?;
    let new = runs(new)?;
    for line in compare_table(&old, &new) {
        output::line(&line);
    }
    Ok(true)
}

/// The measures of one run: `(bench, label)` to the `measure` object, from every
/// `*.jsonl` file of `dir`. A later line of the same key replaces an earlier one.
fn runs(dir: &Path) -> anyhow::Result<BTreeMap<(String, String), Value>> {
    let mut measures = BTreeMap::new();
    let entries = fs::read_dir(dir).with_context(|| format!("could not read {}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        measures.extend(parse_lines(&text));
    }
    Ok(measures)
}

/// The measures in the JSON lines `text`; a line that is not a measure is skipped.
pub(crate) fn parse_lines(text: &str) -> Vec<((String, String), Value)> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| {
            let bench = value.get("bench")?.as_str()?.to_owned();
            let label = value.get("label")?.as_str()?.to_owned();
            Some(((bench, label), value.get("measure")?.clone()))
        })
        .collect()
}

/// One line per measure of either run: the old value, the new value and the change.
pub(crate) fn compare_table(
    old: &BTreeMap<(String, String), Value>,
    new: &BTreeMap<(String, String), Value>,
) -> Vec<String> {
    let mut keys: Vec<&(String, String)> = old.keys().chain(new.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .map(|key| {
            let name = format!("{} {}", key.0, key.1);
            match (old.get(key), new.get(key)) {
                (Some(before), Some(after)) => format!("{name:<60} {}", change(before, after)),
                (Some(_), None) => format!("{name:<60} only in the old run"),
                (None, _) => format!("{name:<60} only in the new run"),
            }
        })
        .collect()
}

/// The change of one measure: p50 and p95 for a time, else the one value.
fn change(before: &Value, after: &Value) -> String {
    let number = |value: &Value, field: &str| value.get(field).and_then(Value::as_f64);
    let percent = |old: f64, new: f64| {
        if old == 0.0 {
            String::from("    n/a")
        } else {
            format!("{:>+6.1}%", (new - old) / old * 100.0)
        }
    };
    let kind = before.get("kind").and_then(Value::as_str).unwrap_or("");
    let fields: &[(&str, f64, &str)] = match kind {
        "time" => &[("p50_ns", 1e-6, "ms"), ("p95_ns", 1e-6, "ms")],
        "bytes" => &[("bytes", 1.0 / (1024.0 * 1024.0), "MiB")],
        "count" => &[("count", 1.0, "")],
        "slope" => &[("slope", 1.0, "")],
        _ => &[],
    };
    let parts: Vec<String> = fields
        .iter()
        .filter_map(|(field, scale, unit)| {
            let old = number(before, field)?;
            let new = number(after, field)?;
            let short = field.trim_end_matches("_ns");
            Some(format!(
                "{short} {:>10.3} -> {:>10.3}{unit} {}",
                old * scale,
                new * scale,
                percent(old, new)
            ))
        })
        .collect();
    if parts.is_empty() { String::from("not comparable") } else { parts.join("  ") }
}

#[cfg(test)]
mod tests;
