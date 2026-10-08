//! The scrollback line of a compaction of the model's context
//! (`conversation_compacted`), which `efr history` and `efr compact` print. The README
//! of efr-conversation, section "Context", "Display", is the contract.

use efr_protocol::{Compaction, CompactionTrigger};

use super::tokens;

/// The one line of `compaction`:
///
/// - when it left the context at or above its limit (a miss of the breaker),
///   `context full: compaction did not free enough room (still 240k); run ,compact or
///   efr new`;
/// - after a request that the model refused, `context full: the request was 281k of
///   272k tokens; compacted and retried`;
/// - else `context compacted (auto): 231k -> 24.0k tokens, kept 3 turns, summary 3.2k`,
///   with `(efr compact)` for a manual compaction and `pruned 12 outputs` in place of
///   the summary when only pruning ran.
pub(crate) fn compacted(compaction: &Compaction) -> String {
    if compaction.tokens_after >= compaction.limit {
        return format!(
            "context full: compaction did not free enough room (still {}); run ,compact or \
             efr new",
            tokens(compaction.tokens_after)
        );
    }
    if compaction.trigger == CompactionTrigger::Overflow {
        return format!(
            "context full: the request was {} of {} tokens; compacted and retried",
            tokens(compaction.tokens_before),
            tokens(compaction.window)
        );
    }
    let trigger = match compaction.trigger {
        CompactionTrigger::Manual => "efr compact",
        _ => "auto",
    };
    let mut line = format!(
        "context compacted ({trigger}): {} -> {} tokens",
        tokens(compaction.tokens_before),
        tokens(compaction.tokens_after)
    );
    match compaction.kept_turns {
        0 => {}
        1 => line.push_str(", kept 1 turn"),
        kept => line.push_str(&format!(", kept {kept} turns")),
    }
    match &compaction.summary {
        Some(summary) => {
            // NOTE: the model's own count of the summary when it gave one, else the
            // estimate of 4 bytes a token that efrd counts with.
            let size = compaction
                .usage
                .map(|usage| usage.output_tokens)
                .filter(|output| *output > 0)
                .unwrap_or_else(|| (summary.len() as u64).div_ceil(4));
            line.push_str(&format!(", summary {}", tokens(size)));
        }
        None => match compaction.pruned_outputs {
            1 => line.push_str(", pruned 1 output"),
            outputs => line.push_str(&format!(", pruned {outputs} outputs")),
        },
    }
    line
}

#[cfg(test)]
mod tests;
