use efr_protocol::{CommandId, JobId, JobsStop, Method, Origin};

use crate::DaemonError;
use crate::methods::authorize;
use crate::sandbox::peers::PeerSide;

/// A process of a job, or any other model-side peer, holds `read` only, so it can never
/// stop a job, or another conversation's job, through the socket. The user's own
/// shell can.
#[test]
fn jobs_stop_refused_for_model_side_peer() {
    let method = Method::JobsStop(JobsStop {
        command_id: CommandId::from_uuid(uuid::Uuid::from_u128(1)),
        conversation_id: None,
        job_id: JobId::nth(3),
    });
    let refused = authorize(Origin::Shell, PeerSide::ModelSide, &method);
    assert!(
        matches!(refused, Err(DaemonError::ModelSidePeer { method: "jobs.stop" })),
        "{refused:?}"
    );
    let gone = authorize(Origin::Shell, PeerSide::Unknown, &method);
    assert!(matches!(gone, Err(DaemonError::ModelSidePeer { .. })), "{gone:?}");
    assert!(authorize(Origin::Shell, PeerSide::User, &method).is_ok());
}
