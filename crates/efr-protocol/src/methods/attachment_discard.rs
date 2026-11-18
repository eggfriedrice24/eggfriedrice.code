//! `attachment.discard`: drop an upload that no prompt will name.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::UploadId;

/// The params of `attachment.discard`, a unary method with the scope `operate`. efrd
/// removes the bytes of the upload. An unknown upload is no error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentDiscard {
    /// The upload.
    pub upload_id: UploadId,
}

/// The result of `attachment.discard`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentDiscardResult {}
