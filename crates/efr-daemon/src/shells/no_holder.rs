//! The PTY holder of a build without one: every shell start fails and says why.

use std::io;

use async_trait::async_trait;
use efr_holder::{
    ChildStatus, HolderError, PtyHandle, PtyHolder, PtyId, PtyInfo, Signal, SignalTarget, Size,
    SpawnSpec,
};

/// Refuses every start; nothing else has a PTY to act on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NoHolder;

fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "this efrd was built without a PTY holder")
}

#[async_trait]
impl PtyHolder for NoHolder {
    async fn spawn(&self, spec: SpawnSpec) -> Result<PtyHandle, HolderError> {
        Err(HolderError::Spawn { program: spec.program, source: unsupported() })
    }

    async fn resize(&self, pty_id: PtyId, _size: Size) -> Result<(), HolderError> {
        Err(HolderError::NotFound { pty_id })
    }

    async fn signal(
        &self,
        pty_id: PtyId,
        _signal: Signal,
        _target: SignalTarget,
    ) -> Result<(), HolderError> {
        Err(HolderError::NotFound { pty_id })
    }

    async fn list(&self) -> Result<Vec<PtyInfo>, HolderError> {
        Ok(Vec::new())
    }

    async fn wait(&self, pty_id: PtyId) -> Result<ChildStatus, HolderError> {
        Err(HolderError::NotFound { pty_id })
    }

    async fn release(&self, pty_id: PtyId) -> Result<(), HolderError> {
        Err(HolderError::NotFound { pty_id })
    }
}
