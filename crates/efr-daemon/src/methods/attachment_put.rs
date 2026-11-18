//! `attachment.put`: one chunk of the bytes of a file that a prompt will name.
//!
//! The client is not trusted for what it uploads: efrd checks the name, the sizes and
//! the origin here, and reads the type and the kind from the bytes after the last chunk
//! (`attachments/uploads.rs`). On the tailnet listener the owner is the device of the
//! proof and the origin is always `phone`; on the Unix socket the owner is the
//! connection and the origin is an `@path` or a paste. An `@path` of the engine's
//! secret class is refused: this stops an accident, it is no boundary, because the
//! client is the user's own `efr`.

use efr_permissions::PathClass;
use efr_protocol::{AttachmentOrigin, AttachmentPut, NAME_MAX_BYTES};
use efr_transport::{ConnectionContext, Responder};

use crate::DaemonError;
use crate::attachments::uploads::{Chunk, Owner};
use crate::state::{SCRATCH_DIR, State};

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: AttachmentPut,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let (limits, max_edge) = {
        let settings = state.settings.borrow();
        (crate::attachments::limits(&settings), settings.attachments.max_edge_px)
    };
    let Some(limits) = limits else {
        return Err(invalid("this efrd takes no attachments: attachments.enabled is false"));
    };
    if params.data.as_bytes().len() as u64 > limits.chunk_bytes {
        return Err(invalid("the chunk is larger than the chunk limit"));
    }
    if params.total_bytes > limits.max_file_bytes {
        return Err(invalid("the file is larger than attachments.max_file_mib allows"));
    }
    check_name(&params.name)?;
    let origin = origin(state, context, params.origin)?;
    let chunk = Chunk {
        upload_id: params.upload_id,
        name: params.name,
        total_bytes: params.total_bytes,
        offset: params.offset,
        data: params.data.into_bytes(),
        origin,
    };
    let owner = Owner::of(context);
    let result =
        state.uploads.put(owner, chunk, max_edge).await.map_err(|error| error.into_daemon())?;
    responder.item(&result).await?;
    Ok(())
}

/// Refuses a name that is no plain file name of at most [`NAME_MAX_BYTES`] bytes: the
/// copy of the file is `<id>-<name>` in `$SCRATCH/attachments`.
fn check_name(name: &str) -> Result<(), DaemonError> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(invalid("the name of the file is empty"));
    }
    if name.len() > NAME_MAX_BYTES {
        return Err(invalid("the name of the file is too long; cut it with fit_name"));
    }
    if name.chars().any(|c| c == '/' || c.is_control()) {
        return Err(invalid("the name of the file holds a / or a control character"));
    }
    Ok(())
}

/// The origin of an upload of `context` that names `origin`: efrd sets `phone` for a
/// device, and a local client names an `@path` or a paste.
fn origin(
    state: &State,
    context: &ConnectionContext,
    origin: Option<AttachmentOrigin>,
) -> Result<AttachmentOrigin, DaemonError> {
    if context.remote().is_some() {
        return match origin {
            None => Ok(AttachmentOrigin::Phone),
            Some(_) => Err(invalid("a phone names no origin: efrd sets it")),
        };
    }
    match origin {
        Some(AttachmentOrigin::Path { path }) if !path.is_absolute() => {
            Err(invalid("the path of an @path origin is not absolute"))
        }
        Some(AttachmentOrigin::Path { path }) => {
            let scratch = state.dirs.data().join(SCRATCH_DIR);
            let class = state.engine.borrow().locations().classify(&path, &scratch);
            if class == Some(PathClass::Secrets) {
                return Err(invalid("the file of the @path is a secret file"));
            }
            Ok(AttachmentOrigin::Path { path })
        }
        Some(AttachmentOrigin::Clipboard) => Ok(AttachmentOrigin::Clipboard),
        _ => Err(invalid("a local upload comes from an @path or a paste")),
    }
}

fn invalid(reason: &'static str) -> DaemonError {
    DaemonError::InvalidParams { reason }
}

#[cfg(test)]
mod tests;
