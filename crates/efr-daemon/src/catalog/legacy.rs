//! The legacy mark of a model list: a model is legacy when the same list has a newer
//! model of its family.
//!
//! The family of an id is the id without its version, and the version is the first run
//! of `-` parts that hold only digits and dots, each part at most 4 bytes, so a date
//! such as `20251001` is no version: `claude-opus-5-5` is the family `claude-opus-*` at
//! version 5.5, `gpt-6.1-sol` the family `gpt-*-sol` at version 6.1. An id with no
//! version, such as `o3` or `gpt-4o`, is never legacy. The versions compare number by
//! number, so 6.1 is newer than 6, and 6 is newer than 5.6.
//!
//! [`KEEP`] lists the models that stay out of the rule: the user keeps them with the
//! current models although a newer one of their family exists.

use efr_protocol::ModelInfo;

/// The models that are never legacy.
const KEEP: &[&str] = &["claude-fable-5-1"];

/// The longest part of a version, in bytes. A longer run of digits is a date.
const VERSION_PART_MAX: usize = 4;

/// Sets [`ModelInfo::legacy`] for each model of `models`, by the ids of the same list.
pub(crate) fn mark(models: &mut [ModelInfo]) {
    let versions: Vec<Option<(String, Vec<u32>)>> =
        models.iter().map(|model| family(&model.id)).collect();
    for (at, model) in models.iter_mut().enumerate() {
        model.legacy = !KEEP.contains(&model.id.as_str())
            && versions[at].as_ref().is_some_and(|(own, version)| {
                versions.iter().flatten().any(|(other, newer)| other == own && newer > version)
            });
    }
}

/// The family and the version of `id`, or `None` when it has no version.
fn family(id: &str) -> Option<(String, Vec<u32>)> {
    let parts: Vec<&str> = id.split('-').collect();
    let start = parts.iter().position(|part| is_version(part))?;
    let end =
        parts[start..].iter().position(|part| !is_version(part)).map_or(parts.len(), |n| start + n);
    let mut version = Vec::new();
    for part in &parts[start..end] {
        for number in part.split('.') {
            version.push(number.parse().ok()?);
        }
    }
    let mut name: Vec<&str> = parts[..start].to_vec();
    name.push("*");
    name.extend(parts[end..].iter().filter(|part| !is_date(part)));
    Some((name.join("-"), version))
}

/// True for a part of a version, such as `5` or `6.1`.
fn is_version(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= VERSION_PART_MAX
        && part.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.')
        && !part.starts_with('.')
        && !part.ends_with('.')
        && !part.contains("..")
}

/// True for a date part of an id, such as `20251001`.
fn is_date(part: &str) -> bool {
    part.len() > VERSION_PART_MAX && part.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests;
