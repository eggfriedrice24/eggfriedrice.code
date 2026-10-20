//! The recent commands that the zsh plugin hands over in `EFR_RECENT_COMMANDS`.
//!
//! The plugin writes a JSON array, oldest first, of objects with `command`, `status`,
//! `cwd`, `ended` (Unix seconds, which zsh has without a fork) and `duration_ms`. They
//! are context for the model only, so a value that does not parse costs the prompt its
//! recent commands and nothing else.

use std::path::PathBuf;

use efr_protocol::RecentCommand;
use jiff::Timestamp;
use serde::Deserialize;

/// The most entries read; the plugin keeps 10, and efrd shows at most that many.
const MAX_ENTRIES: usize = 10;

/// One entry as the plugin writes it.
#[derive(Deserialize)]
struct PluginEntry {
    command: String,
    #[serde(default)]
    status: Option<i32>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    ended: Option<i64>,
    #[serde(default)]
    duration_ms: Option<u64>,
}

/// The recent commands in `json`, oldest first; none when it does not parse.
pub(crate) fn parse(json: &str) -> Vec<RecentCommand> {
    let entries: Vec<PluginEntry> = match serde_json::from_str(json) {
        Ok(entries) => entries,
        Err(_) => {
            // NOTE: the error is not logged, because its text can quote a command.
            tracing::debug!("the recent commands from the plugin do not parse");
            return Vec::new();
        }
    };
    let skip = entries.len().saturating_sub(MAX_ENTRIES);
    entries
        .into_iter()
        .skip(skip)
        .filter(|entry| !entry.command.trim().is_empty())
        .map(|entry| RecentCommand {
            command: entry.command,
            status: entry.status,
            cwd: entry.cwd.filter(|cwd| cwd.is_absolute()),
            ended_at: entry.ended.and_then(|secs| Timestamp::from_second(secs).ok()),
            duration_ms: entry.duration_ms,
        })
        .collect()
}

#[cfg(test)]
mod tests;
