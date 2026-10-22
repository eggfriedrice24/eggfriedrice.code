//! A line that names a port of a job of its conversation that efrd does not forward
//! (the live test D2 7c of 2026-10-22): a program of the user holds the port, so the
//! job's port is `in_use`. A contained call in `auto` cannot reach the job there, and
//! the whole network would reach the user's program, not the job. So the call does not
//! run and nobody is asked; the model reads why. A port of no job still asks, and a
//! forwarded port still reaches its job.

use efr_protocol::{
    ApprovalDecision, ConversationId, Event, EventEnvelope, ForwardOutcome, Grant, Launch,
};
use efr_test_daemon::TTY;
use efr_test_support::Wait;
use pretty_assertions::assert_eq;

use super::{
    LOCALHOST, SETTLE, Setup, asked, forwarded, launches, line, release, settle_waits, turn,
    turn_on, until, with_job,
};
use crate::support::zsh_enabled;

/// What the model reads for the line of the live test.
const REFUSAL: &str = "efr: port 8767 of job j1 is not forwarded: 127.0.0.1:8767 is in use on \
    this machine. Your contained calls do not reach the job on this port, and full network \
    would reach another program on this machine, not the job. efr did not run this call. Stop \
    job j1 and start the server on another port.";

/// The line of the live test.
const CHECK: &str = "sleep 1; curl -sI http://127.0.0.1:8767/ | head -1";

/// The output and the error mark of each finished tool call of `events`.
fn results(events: &[EventEnvelope]) -> Vec<(String, bool)> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallCompleted { output, is_error, .. } => Some((output.clone(), *is_error)),
            _ => None,
        })
        .collect()
}

/// How many foreground calls of `conversation` the launcher started.
fn foreground(setup: &Setup, conversation: ConversationId) -> usize {
    let specs = setup.specs().into_iter();
    specs.filter(|spec| spec.conversation == conversation && spec.job.is_none()).count()
}

/// The outcome of each `job_port_forwarded` event of `events`, by port.
fn outcomes(events: &[EventEnvelope]) -> Vec<(u16, ForwardOutcome)> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::JobPortForwarded { port, outcome, .. } => Some((*port, *outcome)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn port_in_use_refuses_the_line_and_asks_nobody() {
    let setup = Setup::new();
    let answers = vec![line(CHECK), line("curl -s http://127.0.0.1:8768/"), None];
    let (daemon, conversation, mut forwarder, job_fifo) =
        with_job(&setup, None, answers, |builder| builder).await;
    daemon.host_ports().set_in_use(8767);
    forwarder.listen(LOCALHOST, 8767, None).await.unwrap();
    let events =
        until(&daemon, conversation, |event| matches!(event, Event::JobPortForwarded { .. })).await;
    assert_eq!(outcomes(&events), [(8767, ForwardOutcome::InUse)]);
    // NOTE: the job is no new job now, so the port of no job does not wait (DS44).
    daemon.clock().advance(SETTLE);
    let (_, events) = turn(&daemon, 2, "check the server").await;
    let results = results(&events);
    assert_eq!(results[0], (REFUSAL.to_owned(), true), "the model reads why");
    let [refused, other]: [Option<Launch>; 2] = launches(&events).try_into().unwrap();
    assert_eq!(refused, None, "the line of the job's port has no launch");
    let other = other.expect("the question names the launch of the port of no job");
    assert!(other.grants().contains(&Grant::OpenNetwork), "{other:?}");
    assert_eq!(foreground(&setup, conversation), 0, "neither call ran");
    let questions: Vec<&Event> = events
        .iter()
        .map(|envelope| &envelope.event)
        .filter(|event| matches!(event, Event::ApprovalRequested { .. }))
        .collect();
    assert_eq!(questions.len(), 1, "only the port of no job asks: {questions:?}");
    assert_eq!(results.len(), 2, "{results:?}");
    assert!(results[1].1, "the user said no to the port of no job");
    release(&job_fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn outcome_during_the_settle_wait_refuses_the_line() {
    // NOTE: the timing of the live test: the model checks the port before the job's
    // outcome came. The fact waits for it, and the outcome ends the wait at once.
    let setup = Setup::new();
    let answers = vec![line(CHECK), None];
    let (daemon, conversation, mut forwarder, job_fifo) =
        with_job(&setup, None, answers, |builder| builder).await;
    let before = settle_waits(&daemon);
    let check = turn_on(&daemon, TTY, 2, "check the server", ApprovalDecision::Allow);
    let side = async {
        Wait::new("the fact waits for the new job's outcome")
            .until(|| settle_waits(&daemon) > before)
            .await
            .unwrap();
        daemon.host_ports().set_in_use(8767);
        forwarder.listen(LOCALHOST, 8767, None).await.unwrap();
    };
    let ((_, events), ()) = tokio::join!(check, side);
    assert!(!asked(&events), "nobody is asked for the whole network");
    assert_eq!(launches(&events), [None], "the call did not run");
    assert_eq!(foreground(&setup, conversation), 0, "the launcher started no call");
    assert_eq!(results(&events), [(REFUSAL.to_owned(), true)]);
    release(&job_fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn forwarded_port_beside_a_port_in_use_still_reaches() {
    if !zsh_enabled("forwarded_port_beside_a_port_in_use_still_reaches") {
        return;
    }
    let setup = Setup::new();
    let answers = vec![line("curl -s http://localhost:8765/"), line(CHECK), None];
    let (daemon, conversation, mut forwarder, job_fifo) =
        with_job(&setup, Some(8765), answers, |builder| builder).await;
    daemon.host_ports().set_in_use(8767);
    forwarder.listen(LOCALHOST, 8767, None).await.unwrap();
    forwarded(&daemon, conversation, 2).await;
    let (_, events) = turn(&daemon, 2, "check the servers").await;
    assert!(!asked(&events), "{events:?}");
    assert_eq!(
        launches(&events),
        [Some(Launch::contained()), None],
        "only the line of the forwarded port ran"
    );
    assert_eq!(foreground(&setup, conversation), 1);
    let results = results(&events);
    assert_eq!(results[1], (REFUSAL.to_owned(), true));
    let spec = setup.call_spec(conversation, 0).await;
    assert_eq!(spec.reach.unwrap().ports, [8765], "the call reaches the forwarded port");
    release(&job_fifo).await;
    daemon.stop().await.unwrap();
}
