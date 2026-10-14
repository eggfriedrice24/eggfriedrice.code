//! The stop thread's decision, the stop's order of signals and the job's environment,
//! without real processes or real waits.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::sync::Mutex;
use std::time::Duration;

use efr_sandbox::JobStop;
use pretty_assertions::assert_eq;
use rustix::process::{Pid, Signal};

use super::{Event, JobProcesses, is_bridge, monitor, parse_env, stop_job};
use crate::exit_child::{POLL, Processes, end_descendants};
use crate::os;

/// The stop thread's answer to `events`, with a deadline `after` from now; whether it
/// ran the stop, and what it decided before.
fn decide(events: &[Event], after: Duration) -> (Option<JobStop>, bool, Option<JobStop>) {
    let (send, receive) = std::sync::mpsc::channel();
    for event in events {
        send.send(*event).unwrap();
    }
    let decided = Mutex::new(None);
    let mut ran = false;
    let reason = monitor(&receive, os::now() + after, &mut || ran = true, &decided);
    let decided = *decided.lock().unwrap();
    (reason, ran, decided)
}

#[test]
fn job_ends_when_its_child_ends_without_a_stop() {
    assert_eq!(decide(&[Event::Ended], Duration::from_secs(60)), (None, false, None));
}

#[test]
fn a_signal_or_the_lifeline_stops_the_job_and_names_why() {
    let signal = decide(&[Event::Signal, Event::Lifeline], Duration::from_secs(60));
    assert_eq!(signal, (Some(JobStop::Signal), true, Some(JobStop::Signal)));
    let lifeline = decide(&[Event::Lifeline], Duration::from_secs(60));
    assert_eq!(lifeline, (Some(JobStop::Lifeline), true, Some(JobStop::Lifeline)));
}

#[test]
fn job_deadline_backstop_in_launcher() {
    // The deadline passed and nothing else came: the launcher ends the job itself.
    let deadline = decide(&[], Duration::ZERO);
    assert_eq!(deadline, (Some(JobStop::Deadline), true, Some(JobStop::Deadline)));
}

/// Processes that end on the signals the fake allows, with time that moves one poll
/// per pause.
struct Fake {
    procs: Vec<(i32, &'static str, Option<Signal>, bool)>,
    sent: Vec<(i32, Signal)>,
    elapsed: Duration,
}

impl Processes for Fake {
    fn descendants(&mut self) -> Vec<(Pid, String)> {
        self.procs
            .iter()
            .filter(|(_, _, _, alive)| *alive)
            .map(|(pid, name, _, _)| (Pid::from_raw(*pid).unwrap(), (*name).to_owned()))
            .collect()
    }

    fn signal(&mut self, pid: Pid, signal: Signal) {
        let pid = pid.as_raw_nonzero().get();
        self.sent.push((pid, signal));
        for (known, _, ends_on, alive) in &mut self.procs {
            if *known == pid && (*ends_on == Some(signal) || signal == Signal::KILL) {
                *alive = false;
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
fn job_stop_sends_term_before_kill() {
    // bwrap (10) and the namespace's init (11) get no SIGTERM: the init ignores it,
    // and bwrap's end would kill the job with no grace. The job's shell (12) traps
    // SIGTERM and ends; its server (13) ignores it and gets SIGKILL after the grace.
    let fake = Fake {
        procs: vec![
            (10, "bwrap", None, true),
            (11, "bwrap", None, true),
            (12, "zsh", Some(Signal::TERM), true),
            (13, "node", None, true),
        ],
        sent: Vec::new(),
        elapsed: Duration::ZERO,
    };
    let mut procs = JobProcesses::with(fake, BTreeSet::from([10, 11]));
    let ending = end_descendants(&mut procs);
    assert!(ending.survivors.is_empty(), "{ending:?}");
    let sent = &procs.procs.sent;
    let terms: Vec<i32> =
        sent.iter().filter(|(_, signal)| *signal == Signal::TERM).map(|(pid, _)| *pid).collect();
    assert_eq!(terms, [12, 13]);
    let first_kill = sent.iter().position(|(_, signal)| *signal == Signal::KILL).unwrap();
    let last_term = sent.iter().rposition(|(_, signal)| *signal == Signal::TERM).unwrap();
    assert!(last_term < first_kill, "{sent:?}");
    assert!(sent.contains(&(13, Signal::KILL)) && sent.contains(&(10, Signal::KILL)));
}

#[test]
fn the_env_file_gives_the_jobs_environment() {
    let env = parse_env(b"PATH=/usr/bin:/bin\0EMPTY=\0A=b=c\0=nameless\0no-equals\0\0");
    let pairs: Vec<(OsString, OsString)> = env.into_iter().collect();
    assert_eq!(
        pairs,
        [
            ("A".into(), "b=c".into()),
            ("EMPTY".into(), "".into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]
    );
}

/// The parents of the processes of [`bridge_fake`]: bwrap (10) runs the namespace's
/// init (11), which runs the job's shell (12) and the bridge (13); the shell runs a
/// child that named itself `efr-bridge` (14).
fn bridge_parents(pid: i32) -> Option<i32> {
    match pid {
        11 => Some(10),
        12 | 13 => Some(11),
        14 => Some(12),
        _ => None,
    }
}

fn bridge_fake() -> Fake {
    Fake {
        procs: vec![
            (10, "bwrap", None, true),
            (11, "bwrap", None, true),
            (12, "zsh", Some(Signal::TERM), true),
            (13, "efr-bridge", None, true),
            (14, "efr-bridge", Some(Signal::TERM), true),
        ],
        sent: Vec::new(),
        elapsed: Duration::ZERO,
    }
}

#[test]
fn job_stop_spares_bridge_by_name_and_parent() {
    let inits = BTreeSet::from([11]);
    assert!(is_bridge("efr-bridge", Some(11), &inits));
    assert!(!is_bridge("efr-bridge", Some(12), &inits), "a child of the job's shell");
    assert!(!is_bridge("efr-bridge", None, &inits), "a parent that /proc did not show");
    assert!(!is_bridge("node", Some(11), &inits), "another child of the init");
    assert!(!is_bridge("efr-bridge", Some(11), &BTreeSet::new()), "an exit child's job");
    let mut fake = bridge_fake();
    fake.procs.retain(|(pid, _, _, _)| *pid != 14);
    let mut procs = JobProcesses::with_inits(fake, BTreeSet::from([10, 11]), inits, bridge_parents);
    let ending = stop_job(&mut procs);
    // The bridge (13) gets no SIGTERM and no name in the list; it ends with the
    // namespace, which the fake's SIGKILL of bwrap stands for.
    assert_eq!(terms(&procs.procs.sent), [12]);
    assert_eq!(ending.stopped, ["zsh"], "{ending:?}");
    assert!(procs.procs.sent.contains(&(13, Signal::KILL)));
}

/// The pids that got SIGTERM, in order.
fn terms(sent: &[(i32, Signal)]) -> Vec<i32> {
    sent.iter().filter(|(_, signal)| *signal == Signal::TERM).map(|(pid, _)| *pid).collect()
}

#[test]
fn job_stop_does_not_spare_renamed_child_of_zsh() {
    // The child of the shell (14) that named itself efr-bridge gets SIGTERM and its
    // line, beside the real bridge (13), which gets neither.
    let inits = BTreeSet::from([11]);
    let mut procs =
        JobProcesses::with_inits(bridge_fake(), BTreeSet::from([10, 11]), inits, bridge_parents);
    let ending = stop_job(&mut procs);
    assert_eq!(terms(&procs.procs.sent), [12, 14]);
    assert_eq!(ending.stopped, ["zsh", "efr-bridge"], "{ending:?}");
}

#[test]
fn a_job_without_bridge_lists_every_process_as_before() {
    let mut procs = JobProcesses::with(bridge_fake(), BTreeSet::from([10, 11]));
    let ending = stop_job(&mut procs);
    // With no init known (an exit child's job), both names are listed.
    assert_eq!(ending.stopped, ["zsh", "efr-bridge"], "{ending:?}");
    assert_eq!(terms(&procs.procs.sent), [12, 13, 14]);
}

#[test]
fn the_parent_comes_from_proc_stat() {
    let own = rustix::process::getpid().as_raw_nonzero().get();
    let parent = rustix::process::getppid().map(|pid| pid.as_raw_nonzero().get());
    assert_eq!(super::parent_pid(own), parent);
    assert_eq!(super::parent_pid(i32::MAX), None);
}
