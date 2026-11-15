//! `conversation.switch_preview`: what a switch of the model or the account of a
//! conversation costs, before the user picks it, for the model sheet of the phone.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ConversationId;

/// The params of `conversation.switch_preview`, a unary method with the scope that
/// `prompt.send` needs. It changes nothing: efrd says what the next turn of the
/// conversation would cost on the target, from the conversation's context now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationSwitchPreview {
    /// The conversation.
    pub conversation_id: ConversationId,
    /// The target model, as `settings.model` of a prompt names it, such as
    /// `gpt-6.1-sol`.
    pub model: String,
    /// The provider or account of `model`, as `TurnSettings::provider` names it, such
    /// as `openai-subscription.b` or `b`. Absent: the provider that a prompt with
    /// `model` alone would take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// The result of `conversation.switch_preview`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationSwitchPreviewResult {
    /// What the switch costs.
    pub cost: SwitchCost,
    /// The provider id that the target names, such as `openai-subscription.b`. Absent
    /// when efrd cannot tell it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// What a switch of the model or the account costs the next turn. Each tier includes
/// the cost of the tiers before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SwitchCost {
    /// Nothing: the target is the model and the account of the conversation now, such
    /// as a change of the effort only.
    None,
    /// The provider's cache of the conversation does not serve the target, so the next
    /// request pays for its whole context once: another model, or the same model on
    /// another account. A client says nothing about it.
    ColdCache,
    /// The context does not fit the target's window and auto compaction is on, so efrd
    /// compacts the conversation before the next turn, and the cache starts cold. A
    /// client asks before it switches. With auto compaction off, efrd answers
    /// `cold_cache`: the next turn then refuses the request as a full context.
    Compacts,
    /// efrd cannot tell the cost: the `provider` of the params names no account with a
    /// login, and the result then has no `provider`. A client also reads a cost that
    /// this build does not know as `unknown`.
    #[serde(other)]
    Unknown,
}

impl SwitchCost {
    /// Every cost, in the order of the enum.
    pub const ALL: [SwitchCost; 4] =
        [SwitchCost::None, SwitchCost::ColdCache, SwitchCost::Compacts, SwitchCost::Unknown];
}
