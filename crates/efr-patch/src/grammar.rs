//! The grammar of the patch text.

/// The Lark grammar of the text that [`parse`](crate::parse) reads: the `apply_patch`
/// format of Codex (`codex-rs/core/assets/tools/apply_patch.lark`), written for efr.
/// It describes the same language, so a model trained on that format writes text that
/// it accepts. A provider sends it as the format of the freeform `apply_patch` tool.
///
/// The grammar is the format the model must write. [`parse`](crate::parse) is more
/// lenient where models are known to slip, so not every patch that parses matches the
/// grammar. A patch that the grammar accepts parses, except one that changes nothing
/// in a place: an update with no hunk and no move, or a hunk with `@@` lines only.
pub const GRAMMAR: &str = include_str!("apply_patch.lark");

#[cfg(test)]
mod tests;
