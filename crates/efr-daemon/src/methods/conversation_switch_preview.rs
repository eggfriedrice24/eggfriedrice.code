//! `conversation.switch_preview`: what a switch of the model or the account of a
//! conversation would cost its next turn, for the model sheet of the phone.
//!
//! NOTE: a stub. It answers `none` for each target, until efrd compares the target with
//! the conversation's pick and its context with the target's window.

use efr_protocol::{ConversationSwitchPreview, ConversationSwitchPreviewResult, SwitchCost};
use efr_transport::Responder;

use crate::DaemonError;

const METHOD: &str = "conversation.switch_preview";

pub(crate) async fn handle(
    _params: ConversationSwitchPreview,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let result = ConversationSwitchPreviewResult { cost: SwitchCost::None, provider: None };
    let value = serde_json::to_value(&result)
        .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })?;
    responder.item(&value).await?;
    Ok(())
}
