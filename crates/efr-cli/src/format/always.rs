//! The lines of always-allow rules (phase E): what an `a` answer saved, a call that a
//! rule covered, and the list of `efr sandbox allow`.
//!
//! Plain text only: the view and the commands choose the tone. The rule's words and its
//! place come from the daemon or from `config.toml`, so they pass through [`one_line`].

use efr_protocol::{AlwaysSaved, NotSavedReason};

use super::one_line;

/// The key of the always rules in `config.toml`.
pub(crate) const KEY: &str = "sandbox.allow";

/// The line after an `a` answer, from `always_rule_saved`: `saved as sandbox.allow[2]:
/// write under ~/notes`, or `rule not saved: config.toml is read-only; the command ran
/// once`.
pub(crate) fn saved(outcome: &AlwaysSaved) -> String {
    match outcome {
        AlwaysSaved::Saved { index, rule } => {
            format!("saved as {KEY}[{index}]: {}", one_line(rule))
        }
        AlwaysSaved::NotSaved { reason: NotSavedReason::ReloadTimeout } => {
            "the rule is in config.toml, but efrd did not read the file again in time; a later \
             command can ask once more"
                .to_owned()
        }
        AlwaysSaved::NotSaved { reason } => {
            format!("rule not saved: {}; the command ran once", not_saved(*reason))
        }
        _ => "efr cannot tell if the rule was saved; efr sandbox allow lists the rules".to_owned(),
    }
}

/// Why the rule of an `a` answer was not saved, in words.
fn not_saved(reason: NotSavedReason) -> &'static str {
    match reason {
        NotSavedReason::Changed => "config.toml changed each time efr tried to write it",
        NotSavedReason::ReadOnly => "config.toml is read-only",
        NotSavedReason::LoadCheck => "the rule did not pass the floor checks",
        NotSavedReason::Io => "efr could not read or write config.toml",
        _ => "an unknown reason",
    }
}

/// The dim line of a call that an always rule covered, from its `exit_judged` with the
/// judge `always`: `allowed by your rule sandbox.allow[2]: write under ~/notes`. The
/// event gives the rule's place as its category and the rule as its rationale.
pub(crate) fn covered(place: Option<&str>, rule: Option<&str>) -> String {
    let place = place.map(one_line).filter(|place| !place.trim().is_empty());
    let rule = rule.map(one_line).filter(|rule| !rule.trim().is_empty());
    match (place, rule) {
        (Some(place), Some(rule)) => format!("allowed by your rule {place}: {rule}"),
        (Some(place), None) => format!("allowed by your rule {place}"),
        (None, Some(rule)) => format!("allowed by your rule: {rule}"),
        (None, None) => "allowed by your rule".to_owned(),
    }
}

/// What efrd's warning of a disabled rule starts with after the rule's place, as in
/// `sandbox.allow[1] is disabled: it names a floor`.
const DISABLED: &str = "] is disabled: ";

/// The disabled rules among `warnings`, the sandbox warnings of `admin.status` (DE44):
/// each rule's index with the reason that efrd gave, in the order of the warnings.
pub(crate) fn disabled(warnings: &[String]) -> Vec<(usize, String)> {
    warnings
        .iter()
        .filter_map(|warning| {
            let rest = warning.strip_prefix(KEY)?.strip_prefix('[')?;
            let (index, reason) = rest.split_once(DISABLED)?;
            Some((index.parse().ok()?, reason.to_owned()))
        })
        .collect()
}

/// One row of `efr sandbox allow`: `sandbox.allow[0]  write under ~/notes`, with
/// `; disabled: <reason>` for a rule that efrd disabled.
pub(crate) fn list_row(index: usize, rule: &str, disabled: Option<&str>) -> String {
    let mut row = format!("{KEY}[{index}]  {}", one_line(rule));
    if let Some(reason) = disabled {
        row.push_str("; disabled: ");
        row.push_str(&one_line(reason));
    }
    row
}

/// The question of `efr sandbox allow remove N`: `remove sandbox.allow[0]: write under
/// ~/notes? y/n`.
pub(crate) fn remove_question(index: usize, rule: &str) -> String {
    format!("remove {KEY}[{index}]: {}? y/n", one_line(rule))
}

#[cfg(test)]
mod tests;
