//! The contract driven through `Arc<dyn PtyHolder>` with an in-memory holder, the way
//! `efr-shell` drives a real one. The fake keeps the rules that every holder shares:
//! validate first, refuse a duplicate id, answer `NotFound` after release.

use std::collections::BTreeMap;
use std::io;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use efr_protocol::{PtyId, Size};
use pretty_assertions::assert_eq;
use tokio::sync::watch;

use super::{PtyHandle, PtyHolder};
use crate::{ChildStatus, HolderError, PtyInfo, Signal, SignalTarget, SpawnSpec};

#[derive(Debug, Default)]
struct FakeHolder {
    ptys: Mutex<BTreeMap<PtyId, PtyInfo>>,
    /// One channel per held PTY that carries its status to `wait`; dropping it on
    /// release ends every wait with `NotFound`.
    reaped: Mutex<BTreeMap<PtyId, watch::Sender<ChildStatus>>>,
    next_pid: Mutex<u32>,
}

impl FakeHolder {
    /// Reaps the child of `pty_id`, which ended with `status`.
    fn reap(&self, pty_id: PtyId, status: ChildStatus) {
        self.ptys.lock().unwrap().get_mut(&pty_id).unwrap().status = status;
        self.reaped.lock().unwrap()[&pty_id].send_replace(status);
    }

    fn exit(&self, pty_id: PtyId, code: i32) {
        self.reap(pty_id, ChildStatus::Exited { code });
    }
}

#[async_trait]
impl PtyHolder for FakeHolder {
    async fn spawn(&self, spec: SpawnSpec) -> Result<PtyHandle, HolderError> {
        spec.validate()?;
        let mut ptys = self.ptys.lock().unwrap();
        if ptys.contains_key(&spec.pty_id) {
            return Err(HolderError::AlreadyExists { pty_id: spec.pty_id });
        }
        let child_pid = {
            let mut next = self.next_pid.lock().unwrap();
            *next += 1;
            1000 + *next
        };
        // A pipe stands in for the PTY master: the handle only needs an owned descriptor.
        let (reader, _writer) = io::pipe().unwrap();
        ptys.insert(
            spec.pty_id,
            PtyInfo {
                pty_id: spec.pty_id,
                child_pid,
                size: spec.size,
                status: ChildStatus::Running,
            },
        );
        self.reaped.lock().unwrap().insert(spec.pty_id, watch::Sender::new(ChildStatus::Running));
        Ok(PtyHandle { master: OwnedFd::from(reader), child_pid, pty_id: spec.pty_id })
    }

    async fn resize(&self, pty_id: PtyId, size: Size) -> Result<(), HolderError> {
        let mut ptys = self.ptys.lock().unwrap();
        let info = ptys.get_mut(&pty_id).ok_or(HolderError::NotFound { pty_id })?;
        info.size = size;
        Ok(())
    }

    async fn signal(
        &self,
        pty_id: PtyId,
        _signal: Signal,
        _target: SignalTarget,
    ) -> Result<(), HolderError> {
        let ptys = self.ptys.lock().unwrap();
        let info = ptys.get(&pty_id).ok_or(HolderError::NotFound { pty_id })?;
        if info.status.is_running() { Ok(()) } else { Err(HolderError::Exited { pty_id }) }
    }

    async fn list(&self) -> Result<Vec<PtyInfo>, HolderError> {
        Ok(self.ptys.lock().unwrap().values().copied().collect())
    }

    async fn foreground(&self, pty_id: PtyId) -> Result<Option<u32>, HolderError> {
        let ptys = self.ptys.lock().unwrap();
        let info = ptys.get(&pty_id).ok_or(HolderError::NotFound { pty_id })?;
        // The child is a session leader that nobody's job has displaced here.
        Ok(info.status.is_running().then_some(info.child_pid))
    }

    async fn wait(&self, pty_id: PtyId) -> Result<ChildStatus, HolderError> {
        let mut status = self
            .reaped
            .lock()
            .unwrap()
            .get(&pty_id)
            .map(watch::Sender::subscribe)
            .ok_or(HolderError::NotFound { pty_id })?;
        // `wait_for` looks at the current value first, so a child reaped before the call
        // answers at once. A dropped sender means the PTY was released.
        match status.wait_for(|status| !status.is_running()).await {
            Ok(status) => Ok(*status),
            Err(_) => Err(HolderError::NotFound { pty_id }),
        }
    }

    async fn release(&self, pty_id: PtyId) -> Result<(), HolderError> {
        self.reaped.lock().unwrap().remove(&pty_id);
        let mut ptys = self.ptys.lock().unwrap();
        ptys.remove(&pty_id).map(drop).ok_or(HolderError::NotFound { pty_id })
    }
}

fn pty(n: u8) -> PtyId {
    format!("01920000-0000-7000-8000-0000000000{n:02}").parse().unwrap()
}

fn spec(pty_id: PtyId) -> SpawnSpec {
    SpawnSpec::new(pty_id, "/usr/bin/zsh", "/home/user", Size { cols: 80, rows: 24 })
        .arg("-i")
        .var("TERM", "xterm-256color")
}

fn holder() -> Arc<dyn PtyHolder> {
    Arc::new(FakeHolder::default())
}

#[test]
fn the_trait_object_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<dyn PtyHolder>();
    assert_send_sync::<PtyHandle>();
}

#[tokio::test]
async fn spawn_returns_a_handle_for_the_spec_id() {
    let holder = holder();
    let handle = holder.spawn(spec(pty(1))).await.unwrap();
    assert_eq!(handle.pty_id, pty(1));
    assert_eq!(handle.child_pid, 1001);

    let listed = holder.list().await.unwrap();
    assert_eq!(
        listed,
        [PtyInfo {
            pty_id: pty(1),
            child_pid: 1001,
            size: Size { cols: 80, rows: 24 },
            status: ChildStatus::Running,
        }]
    );
}

#[tokio::test]
async fn an_invalid_spec_is_refused_before_anything_is_held() {
    let holder = holder();
    let bad = SpawnSpec::new(pty(1), "zsh", "/home/user", Size { cols: 80, rows: 24 });
    assert!(matches!(holder.spawn(bad).await, Err(HolderError::ProgramNotAbsolute { .. })));
    assert_eq!(holder.list().await.unwrap(), []);
}

#[tokio::test]
async fn a_retried_spawn_does_not_start_a_second_child() {
    let holder = holder();
    let _handle = holder.spawn(spec(pty(1))).await.unwrap();
    let retry = holder.spawn(spec(pty(1))).await;
    assert!(matches!(retry, Err(HolderError::AlreadyExists { pty_id }) if pty_id == pty(1)));
    assert_eq!(holder.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_exited_child_stays_listed_until_release() {
    let fake = Arc::new(FakeHolder::default());
    let holder: Arc<dyn PtyHolder> = fake.clone();
    let _handle = holder.spawn(spec(pty(1))).await.unwrap();
    fake.exit(pty(1), 0);

    let listed = holder.list().await.unwrap();
    assert_eq!(listed[0].status.exit_code(), Some(0));
    let signalled = holder.signal(pty(1), Signal::Interrupt, SignalTarget::ForegroundGroup).await;
    assert!(matches!(signalled, Err(HolderError::Exited { .. })));
    holder.resize(pty(1), Size { cols: 100, rows: 30 }).await.unwrap();

    holder.release(pty(1)).await.unwrap();
    assert_eq!(holder.list().await.unwrap(), []);
}

#[tokio::test]
async fn foreground_names_the_child_until_it_exits() {
    let fake = Arc::new(FakeHolder::default());
    let holder: Arc<dyn PtyHolder> = fake.clone();
    let handle = holder.spawn(spec(pty(1))).await.unwrap();
    assert_eq!(holder.foreground(pty(1)).await.unwrap(), Some(handle.child_pid));
    fake.exit(pty(1), 0);
    assert_eq!(holder.foreground(pty(1)).await.unwrap(), None);
}

#[tokio::test]
async fn wait_answers_at_once_for_a_child_that_was_reaped() {
    let fake = Arc::new(FakeHolder::default());
    let holder: Arc<dyn PtyHolder> = fake.clone();
    let _one = holder.spawn(spec(pty(1))).await.unwrap();
    let _two = holder.spawn(spec(pty(2))).await.unwrap();
    fake.exit(pty(1), 3);
    fake.reap(pty(2), ChildStatus::Signaled { signal: 9 });

    assert_eq!(holder.wait(pty(1)).await.unwrap(), ChildStatus::Exited { code: 3 });
    assert_eq!(holder.wait(pty(2)).await.unwrap(), ChildStatus::Signaled { signal: 9 });
}

#[tokio::test]
async fn wait_ends_when_the_child_is_reaped_and_never_reports_running() {
    let fake = Arc::new(FakeHolder::default());
    let holder: Arc<dyn PtyHolder> = fake.clone();
    let _handle = holder.spawn(spec(pty(1))).await.unwrap();

    let waiting = tokio::spawn({
        let holder = Arc::clone(&holder);
        async move { holder.wait(pty(1)).await }
    });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished(), "the child still runs");
    fake.exit(pty(1), 0);

    assert_eq!(waiting.await.unwrap().unwrap(), ChildStatus::Exited { code: 0 });
}

#[tokio::test]
async fn a_release_ends_a_wait_with_not_found() {
    let holder = holder();
    let _handle = holder.spawn(spec(pty(1))).await.unwrap();

    let waiting = tokio::spawn({
        let holder = Arc::clone(&holder);
        async move { holder.wait(pty(1)).await }
    });
    tokio::task::yield_now().await;
    holder.release(pty(1)).await.unwrap();

    let ended = waiting.await.unwrap();
    assert!(matches!(ended, Err(HolderError::NotFound { pty_id }) if pty_id == pty(1)));
}

#[tokio::test]
async fn every_method_answers_not_found_after_release() {
    let holder = holder();
    let _handle = holder.spawn(spec(pty(1))).await.unwrap();
    holder.release(pty(1)).await.unwrap();

    let size = Size { cols: 80, rows: 24 };
    let results = [
        holder.resize(pty(1), size).await,
        holder.signal(pty(1), Signal::Hangup, SignalTarget::Child).await,
        holder.foreground(pty(1)).await.map(drop),
        holder.wait(pty(1)).await.map(drop),
        holder.release(pty(1)).await,
    ];
    for result in results {
        assert!(matches!(result, Err(HolderError::NotFound { pty_id }) if pty_id == pty(1)));
    }
}
