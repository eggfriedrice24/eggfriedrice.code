//! Deletes in a private cache layer that reach the user's real tool caches (efr's auto
//! spec, section 5.2).
//!
//! A contained call writes the tool caches (`~/.cache`, `~/.cargo`) in a private layer,
//! so a delete there frees nothing. The launcher reports the paths that the call deleted
//! in that layer (`SandboxSummary::cache_deletes`). After the call, [`Turn::cache_deletes`]
//! lets the engine decide them ([`Engine::decide_cache_delete`]): a path below a cache is
//! one `write` exit, and anything else stays private. The classifier reviews the exits
//! with no user message that names them, because a tool fills a cache again; when it
//! cannot decide, the user does. The question has a call id of its own, because the
//! call's own question, if it had one, was another. An allow hands the paths to
//! [`Toolbox::delete_cache_paths`](crate::Toolbox::delete_cache_paths), which checks
//! each one again, and records `cache_delete_applied`. A denial keeps the paths private
//! for the rest of the conversation, so the next call does not raise them again.

use std::path::PathBuf;
use std::sync::{Arc, PoisonError};

use efr_permissions::{Engine, Requirements};
use efr_protocol::{CallId, Event, ExitKind, Launch, SandboxSummary};
use efr_stdx::id::uuid_v7;

use super::{Authorization, Ruling, Turn};
use crate::exit;
use crate::{ConversationError, ToolCall};

/// What the model reads when the deletes of a call stay in its private layer.
const STAYED_PRIVATE: &str = "the deletes in the tool caches stayed private";

impl Turn {
    /// Raises the deletes that the contained call `call` made in its private cache
    /// layer, as `summary` reports them, and applies the allowed ones to the real
    /// caches. Returns the note that the model reads after the call's output, if any,
    /// and true when the user interrupted the turn during the question.
    pub(super) async fn cache_deletes(
        &mut self,
        call: &ToolCall,
        summary: &SandboxSummary,
    ) -> Result<(Option<String>, bool), ConversationError> {
        let denied =
            self.shared.denied_cache_deletes.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let paths: Vec<PathBuf> =
            summary.cache_deletes.iter().filter(|path| !denied.contains(*path)).cloned().collect();
        if paths.is_empty() {
            return Ok((None, false));
        }
        let engine = Arc::clone(&self.shared.deps.engine.borrow());
        let context = &call.context;
        let decision = engine.decide_cache_delete(&paths, &context.scope, &context.scratch);
        let kept: Vec<PathBuf> = decision
            .exits()
            .filter(|need| need.kind == ExitKind::Write)
            .filter_map(|need| need.target.clone())
            .collect();
        if kept.is_empty() {
            return Ok((
                Some(format!("[{STAYED_PRIVATE}: efr deletes none of these paths.]")),
                false,
            ));
        }
        let decision = if kept.len() == paths.len() {
            decision
        } else {
            engine.decide_cache_delete(&kept, &context.scope, &context.scratch)
        };
        let question_id =
            CallId::from_uuid(uuid_v7(&*self.shared.deps.clock, &*self.shared.deps.rng));
        let mut asked = context.clone();
        asked.call_id = question_id;
        let line = call.input.get("command").and_then(serde_json::Value::as_str);
        let requirements =
            line.map_or_else(Requirements::none, |line| Requirements::none().with_command(line));
        let approver = exit::route(&decision, &self.user_messages.naming, None);
        let ruling = Ruling {
            requirements,
            decision,
            launch: Launch::Direct,
            problem: None,
            engine: Arc::clone(&engine) as Arc<Engine>,
            approver,
            payload: None,
            summary: Some(question(&kept)),
        };
        let question = ToolCall::new(call.name.clone(), call.input.clone(), asked);
        let authorization = self.ask(&question, &ruling).await?;
        Ok(match authorization {
            Authorization::Allowed { .. } => {
                (Some(self.apply_cache_deletes(call, &kept).await?), false)
            }
            Authorization::Denied { .. } => {
                let mut denied =
                    self.shared.denied_cache_deletes.lock().unwrap_or_else(PoisonError::into_inner);
                denied.extend(kept);
                (Some(format!("[{STAYED_PRIVATE}: the deletes were refused.]")), false)
            }
            Authorization::Interrupted => (None, true),
            Authorization::Refused { .. } | Authorization::Expired { .. } => {
                (Some(format!("[{STAYED_PRIVATE}: nobody allowed them.]")), false)
            }
        })
    }

    /// Deletes `paths` in the real caches through the toolbox after an allow, and records
    /// what it deleted. Returns the note that the model reads.
    async fn apply_cache_deletes(
        &mut self,
        call: &ToolCall,
        paths: &[PathBuf],
    ) -> Result<String, ConversationError> {
        let toolbox = Arc::clone(&self.shared.deps.toolbox);
        match toolbox.delete_cache_paths(&call.context, paths).await {
            Ok(outcome) => {
                let count = outcome.deleted.len();
                let mut deleted = outcome.deleted;
                deleted.truncate(SandboxSummary::MAX_CACHE_DELETES);
                self.record(vec![Event::CacheDeleteApplied {
                    turn_id: self.turn_id(),
                    call_id: call.context.call_id,
                    paths: deleted,
                    freed_bytes: outcome.freed_bytes,
                }])
                .await?;
                let paths = if count == 1 { "path" } else { "paths" };
                Ok(format!(
                    "[efr deleted {count} {paths} in the real tool caches and freed {}.]",
                    size(outcome.freed_bytes)
                ))
            }
            Err(error) => Ok(format!("[{STAYED_PRIVATE}: {error}]")),
        }
    }
}

/// The summary of the question about deleting `paths` in the real caches.
fn question(paths: &[PathBuf]) -> String {
    let first = paths.first().map(|path| path.display().to_string()).unwrap_or_default();
    match paths.len() {
        1 => format!("delete {first} in the real tool caches"),
        n => format!("delete {n} paths in the real tool caches, such as {first}"),
    }
    .replace('\n', "\\n")
}

/// `bytes` in a short form for a person: `512 bytes`, `1.5 KiB`, `2.0 GiB`.
pub(crate) fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = UNITS[0];
    for next in &UNITS[1..] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next;
    }
    format!("{value:.1} {unit}")
}

#[cfg(test)]
mod tests;
