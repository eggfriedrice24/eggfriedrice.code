//! The bounded end of an exit child's descendants, over fake processes and fake time.

use std::time::Duration;

use pretty_assertions::assert_eq;
use rustix::process::{Pid, Signal};

use super::{Ending, GRACE, POLL, Processes, end_descendants};

/// A fake process: its pid, its name, and the signal that ends it, if any.
struct Fake {
    pid: i32,
    name: &'static str,
    ends_on: Option<Signal>,
    alive: bool,
}

/// Processes whose time moves one poll interval per pause.
struct FakeProcesses {
    procs: Vec<Fake>,
    elapsed: Duration,
    sent: Vec<(i32, Signal)>,
}

impl FakeProcesses {
    fn new(procs: Vec<(i32, &'static str, Option<Signal>)>) -> Self {
        let procs = procs
            .into_iter()
            .map(|(pid, name, ends_on)| Fake { pid, name, ends_on, alive: true })
            .collect();
        FakeProcesses { procs, elapsed: Duration::ZERO, sent: Vec::new() }
    }
}

impl Processes for FakeProcesses {
    fn descendants(&mut self) -> Vec<(Pid, String)> {
        self.procs
            .iter()
            .filter(|fake| fake.alive)
            .map(|fake| (Pid::from_raw(fake.pid).unwrap(), fake.name.to_owned()))
            .collect()
    }

    fn signal(&mut self, pid: Pid, signal: Signal) {
        self.sent.push((pid.as_raw_nonzero().get(), signal));
        for fake in &mut self.procs {
            if fake.pid == pid.as_raw_nonzero().get() && fake.ends_on == Some(signal) {
                fake.alive = false;
            }
        }
    }

    fn reap(&mut self) {}

    fn elapsed(&self) -> Duration {
        self.elapsed
    }

    fn pause(&mut self) {
        self.elapsed += POLL;
    }
}

#[test]
fn no_descendants_end_at_once() {
    let mut procs = FakeProcesses::new(Vec::new());
    assert_eq!(end_descendants(&mut procs), Ending::default());
    assert!(procs.sent.is_empty());
}

#[test]
fn sigterm_ends_a_background_job() {
    let mut procs = FakeProcesses::new(vec![(10, "vite", Some(Signal::TERM))]);
    let ending = end_descendants(&mut procs);
    assert_eq!(ending, Ending { stopped: vec!["vite".to_owned()], survivors: Vec::new() });
    assert_eq!(procs.sent, [(10, Signal::TERM)]);
    assert_eq!(procs.elapsed, Duration::ZERO);
}

#[test]
fn sigkill_ends_a_job_that_ignores_sigterm_after_the_grace() {
    let mut procs = FakeProcesses::new(vec![(10, "node", Some(Signal::KILL))]);
    let ending = end_descendants(&mut procs);
    assert_eq!(ending, Ending { stopped: vec!["node".to_owned()], survivors: Vec::new() });
    assert_eq!(procs.sent, [(10, Signal::TERM), (10, Signal::KILL)]);
    assert_eq!(procs.elapsed, GRACE + POLL);
}

#[test]
fn end_descendants_is_bounded_for_an_unkillable_survivor() {
    // A process that sudo left running as root: no signal of this user reaches it.
    let mut procs = FakeProcesses::new(vec![(10, "sudo", None), (11, "tee", Some(Signal::TERM))]);
    let ending = end_descendants(&mut procs);
    assert_eq!(
        ending,
        Ending {
            stopped: vec!["sudo".to_owned(), "tee".to_owned()],
            survivors: vec!["sudo".to_owned()],
        }
    );
    assert_eq!(procs.elapsed, GRACE * 2);
    assert!(procs.sent.contains(&(10, Signal::KILL)));
}
