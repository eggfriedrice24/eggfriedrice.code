//! The session's logic without IO: byte strings in, typed bytes and replies out.

use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use efr_protocol::{CallId, Seq};
use efr_test_support::TestClock;
use pretty_assertions::assert_eq;
use tokio::sync::{oneshot, watch};

use super::{Detached, RunEnd, RunOrder, SessionCore};
use crate::input::{Offer, Waiting};
use crate::modes::{InputModes, Job};
use crate::run::{Completion, Progress, RunMode};
use crate::testing::{Notices, conversation};
use crate::{Phase, ShellError, ShellNotice, ShellState};

type Answer = oneshot::Receiver<Result<RunEnd, ShellError>>;

fn core(integration: bool) -> (SessionCore, Arc<Notices>) {
    let pty_id = "01920000-0000-7000-8000-0000000000aa".parse().unwrap();
    let notices = Notices::new();
    let state = ShellState::new(pty_id, 7, PathBuf::from("/home/u"), integration);
    (SessionCore::new(conversation(1), state, Arc::clone(&notices) as _), notices)
}

fn order(id: u64, command: &str, mode: RunMode) -> (RunOrder, Answer, watch::Receiver<Progress>) {
    let (reply, answer) = oneshot::channel();
    let (progress, updates) = watch::channel(Progress::default());
    let order = RunOrder {
        id,
        command: command.to_owned(),
        mode,
        output_limit: 1024,
        call: Some(call_id(id)),
        forget_credentials: false,
        token: "0123456789abcdef".to_owned(),
        sandbox: None,
        reply,
        progress,
    };
    (order, answer, updates)
}

fn call_id(n: u64) -> CallId {
    format!("01920000-0000-7000-8000-0000000c{n:04}").parse().unwrap()
}

/// Feeds `bytes` as one chunk at the current end of the stream.
fn feed(core: &mut SessionCore, at: &mut u64, bytes: &[u8]) -> Vec<Vec<u8>> {
    let writes = core.chunk(Seq::new(*at), bytes, TestClock::START);
    *at += bytes.len() as u64;
    writes.into_iter().map(|bytes| bytes.to_vec()).collect()
}

fn ready(core: &mut SessionCore, at: &mut u64) {
    feed(core, at, b"\x1b]133;A\x07% \x1b]133;B\x07");
    assert_eq!(core.state().phase, Phase::Ready);
}

#[test]
fn a_run_at_a_ready_prompt_is_typed_at_once() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    let (order, _answer, _) = order(1, "ls", RunMode::Auto);
    let writes = core.submit(order);
    assert_eq!(writes, [b"\x1b[efr-clear~\x1b[200~ls\x1b[201~\r".to_vec()]);
}

#[test]
fn a_run_that_forgets_credentials_types_the_forget_key_at_the_next_prompt() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    let (mut first, mut answer, _) = order(1, "sudo true", RunMode::Auto);
    first.forget_credentials = true;
    core.submit(first);
    feed(&mut core, &mut at, b"\r\n\x1b]133;C\x07");

    let ended = feed(&mut core, &mut at, b"\x1b]133;D;0\x07\x1b]133;A\x07% ");
    assert!(answer.try_recv().unwrap().is_ok(), "the run is answered at its end");
    let (second, _second_answer, _) = order(2, "ls", RunMode::Auto);
    assert!(core.submit(second).is_empty(), "the next line waits for the prompt");
    let at_prompt = feed(&mut core, &mut at, b"\x1b]133;B\x07");

    assert!(ended.is_empty(), "nothing is typed before the line editor reads");
    assert_eq!(
        at_prompt,
        [b"\x1b[efr-forget~".to_vec(), b"\x1b[efr-clear~\x1b[200~ls\x1b[201~\r".to_vec()],
        "the forget key goes first"
    );
}

#[test]
fn a_run_that_keeps_credentials_types_nothing_after_its_end() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    let (first, _answer, _) = order(1, "sudo true", RunMode::Auto);
    core.submit(first);
    feed(&mut core, &mut at, b"\r\n\x1b]133;C\x07");

    assert!(feed(&mut core, &mut at, b"\x1b]133;D;0\x07\x1b]133;A\x07% \x1b]133;B\x07").is_empty());
}

#[test]
fn a_run_left_running_still_forgets_credentials_when_it_ends() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    let (mut first, _answer, _) = order(1, "sudo true", RunMode::Auto);
    first.forget_credentials = true;
    core.submit(first);
    feed(&mut core, &mut at, b"\r\n\x1b]133;C\x07[sudo] password: ");
    assert!(matches!(core.detach(1), Detached::Running { .. }));

    let writes = feed(&mut core, &mut at, b"\r\n\x1b]133;D;0\x07\x1b]133;A\x07% \x1b]133;B\x07");

    assert_eq!(writes, [b"\x1b[efr-forget~".to_vec()]);
}

#[test]
fn a_sentinel_run_that_forgets_credentials_types_the_forget_key_at_the_next_prompt() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    let (mut first, mut answer, _) = order(1, "sudo true", RunMode::Sentinel);
    first.forget_credentials = true;
    core.submit(first);

    let ended = feed(
        &mut core,
        &mut at,
        b"\r\n\x1b]133;C\x07__efr_0123456789abcdef_b\r\n\r\n__efr_0123456789abcdef_e:0:/home/u\r\n",
    );
    assert!(answer.try_recv().unwrap().is_ok(), "the run is answered at its end marker");
    let at_prompt = feed(&mut core, &mut at, b"\x1b]133;D;0\x07\x1b]133;A\x07% \x1b]133;B\x07");

    assert!(ended.is_empty(), "nothing is typed before the line editor reads");
    assert_eq!(at_prompt, [b"\x1b[efr-forget~".to_vec()]);
}

#[test]
fn a_sentinel_run_left_running_still_forgets_credentials_when_it_ends() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    let (mut first, _answer, _) = order(1, "sudo true", RunMode::Sentinel);
    first.forget_credentials = true;
    core.submit(first);
    feed(&mut core, &mut at, b"\r\n\x1b]133;C\x07__efr_0123456789abcdef_b\r\n[sudo] password: ");
    assert!(matches!(core.detach(1), Detached::Running { .. }));

    feed(&mut core, &mut at, b"\r\n__efr_0123456789abcdef_e:0:/home/u\r\n");
    let writes = feed(&mut core, &mut at, b"\x1b]133;D;0\x07\x1b]133;A\x07% \x1b]133;B\x07");

    assert_eq!(writes, [b"\x1b[efr-forget~".to_vec()]);
}

#[test]
fn a_sentinel_run_after_a_run_that_forgets_waits_for_the_forget_key() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    let (mut first, mut answer, _) = order(1, "sudo true", RunMode::Auto);
    first.forget_credentials = true;
    core.submit(first);
    feed(&mut core, &mut at, b"\r\n\x1b]133;C\x07");
    feed(&mut core, &mut at, b"\x1b]133;D;0\x07\x1b]133;A\x07% ");
    assert!(answer.try_recv().unwrap().is_ok(), "the run is answered at its end");

    let (second, _second_answer, _) = order(2, "id", RunMode::Sentinel);
    assert!(core.submit(second).is_empty(), "the sentinel line waits for the prompt");
    let at_prompt = feed(&mut core, &mut at, b"\x1b]133;B\x07");

    assert_eq!(at_prompt.len(), 2, "{at_prompt:?}");
    assert_eq!(at_prompt[0], b"\x1b[efr-forget~".to_vec(), "the forget key goes first");
    assert!(String::from_utf8_lossy(&at_prompt[1]).contains("eval 'id'"), "{at_prompt:?}");
}

#[test]
fn a_sentinel_run_goes_into_a_nested_shell_while_a_forget_key_waits_for_the_outer_prompt() {
    let (mut core, _) = core(true);
    let mut at = 0;
    feed(&mut core, &mut at, b"\x1b]133;C\x07");
    let (mut first, mut answer, _) = order(1, "sudo true", RunMode::Sentinel);
    first.forget_credentials = true;
    core.submit(first);
    feed(
        &mut core,
        &mut at,
        b"__efr_0123456789abcdef_b\r\n\r\n__efr_0123456789abcdef_e:0:/root\r\n",
    );
    assert!(answer.try_recv().unwrap().is_ok(), "the run is answered at its end marker");

    let (second, _second_answer, _) = order(2, "id", RunMode::Sentinel);
    let writes = core.submit(second);

    assert_eq!(writes.len(), 1, "{writes:?}");
    assert!(String::from_utf8_lossy(&writes[0]).contains("eval 'id'"), "{writes:?}");
}

#[test]
fn a_run_before_the_prompt_is_typed_when_it_comes() {
    let (mut core, _) = core(true);
    let mut at = 0;
    let (order, _answer, _) = order(1, "ls", RunMode::Auto);
    assert!(core.submit(order).is_empty());
    let writes = feed(&mut core, &mut at, b"\x1b]133;A\x07% \x1b]133;B\x07");
    assert_eq!(writes, [b"\x1b[efr-clear~\x1b[200~ls\x1b[201~\r".to_vec()]);
}

#[test]
fn a_command_end_split_across_chunks_is_cut_where_it_starts() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    let (order, mut answer, _) = order(1, "printf out", RunMode::Auto);
    core.submit(order);
    let output_start = at + b"\r\n\x1b]133;C\x07".len() as u64;
    feed(&mut core, &mut at, b"\r\n\x1b]133;C\x07out\x1b]13");
    let end_start = at - 4;
    assert!(answer.try_recv().is_err(), "the run has not ended yet");
    feed(&mut core, &mut at, b"3;D;0\x07");
    let end = answer.try_recv().unwrap().unwrap();
    assert_eq!(end.output.kept.clean().text, "out");
    assert_eq!(end.output.exit_code, Some(0));
    assert_eq!(end.output.range, Some(Seq::new(output_start)..Seq::new(end_start)));
    assert_eq!(end.cwd, PathBuf::from("/home/u"));
}

#[test]
fn a_running_shell_holds_an_auto_run_until_its_prompt() {
    let (mut core, _) = core(true);
    let mut at = 0;
    feed(&mut core, &mut at, b"\x1b]133;C\x07");
    let (order, mut answer, _) = order(1, "ls", RunMode::Auto);
    assert!(core.submit(order).is_empty());
    assert!(answer.try_recv().is_err(), "the run waits");
    assert!(feed(&mut core, &mut at, b"done\r\n\x1b]133;D;0\x07").is_empty());
    let writes = feed(&mut core, &mut at, b"\x1b]133;A\x07% \x1b]133;B\x07");
    assert_eq!(writes, [b"\x1b[efr-clear~\x1b[200~ls\x1b[201~\r".to_vec()]);
}

#[test]
fn an_unfinished_line_at_a_continuation_prompt_refuses_an_auto_run() {
    let (mut core, _) = core(true);
    let mut at = 0;
    feed(&mut core, &mut at, b"\x1b]133;P;k=s\x07> \x1b]133;B\x07");
    let (order, mut answer, _) = order(1, "ls", RunMode::Auto);
    assert!(core.submit(order).is_empty());
    assert!(matches!(answer.try_recv().unwrap(), Err(ShellError::Busy { .. })));
}

#[test]
fn a_sentinel_run_goes_in_even_while_the_shell_runs() {
    let (mut core, _) = core(true);
    let mut at = 0;
    feed(&mut core, &mut at, b"\x1b]133;C\x07");
    let (order, _answer, _) = order(1, "id", RunMode::Sentinel);
    let writes = core.submit(order);
    assert_eq!(writes.len(), 1);
    assert!(String::from_utf8_lossy(&writes[0]).contains("eval 'id'"));
}

#[test]
fn the_startup_timeout_types_a_queued_run_with_sentinels() {
    let (mut core, _) = core(true);
    let (order, _answer, _) = order(1, "id", RunMode::Auto);
    assert!(core.submit(order).is_empty());
    let writes = core.startup_expired();
    assert!(
        String::from_utf8_lossy(&writes[0]).starts_with("printf '__efr_%s_b\\n' 0123456789abcdef")
    );
}

#[test]
fn detaching_a_queued_run_drops_it() {
    let (mut core, _) = core(true);
    let (order, _answer, _) = order(9, "id", RunMode::Auto);
    core.submit(order);
    assert!(matches!(core.detach(9), Detached::Unstarted));
    let mut at = 0;
    assert!(feed(&mut core, &mut at, b"\x1b]133;A\x07\x1b]133;B\x07").is_empty());
}

#[test]
fn detaching_a_typed_run_returns_the_output_so_far() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    let (order, _answer, mut updates) = order(3, "make", RunMode::Auto);
    core.submit(order);
    feed(&mut core, &mut at, b"\r\n\x1b]133;C\x07cc -c a.c\r\n");
    assert!(updates.has_changed().unwrap());
    assert_eq!(updates.borrow_and_update().bytes, 11);
    let Detached::Running { kept, last_output, .. } = core.detach(3) else {
        panic!("the run was typed");
    };
    assert_eq!(kept.clean().text, "cc -c a.c\n");
    assert_eq!(last_output, Some(TestClock::START));
    assert!(matches!(core.detach(3), Detached::Gone));
}

#[test]
fn a_run_whose_caller_left_while_the_shell_started_is_never_typed() {
    let (mut core, _) = core(true);
    let mut at = 0;
    let (first, answer, _) = order(1, "rm -rf build", RunMode::Auto);
    assert!(core.submit(first).is_empty());
    drop(answer);
    assert!(feed(&mut core, &mut at, b"\x1b]133;A\x07% \x1b]133;B\x07").is_empty());
    // The dropped run left nothing behind: the next one is typed at once.
    let (next, _next_answer, _) = order(2, "ls", RunMode::Auto);
    assert_eq!(core.submit(next), [Bytes::from_static(b"\x1b[efr-clear~\x1b[200~ls\x1b[201~\r")]);
}

#[test]
fn a_run_whose_caller_left_while_a_command_ran_is_never_typed() {
    let (mut core, _) = core(true);
    let mut at = 0;
    feed(&mut core, &mut at, b"\x1b]133;C\x07");
    let (order, answer, _) = order(1, "make install", RunMode::Auto);
    assert!(core.submit(order).is_empty());
    drop(answer);
    // Ctrl+C ends the earlier command and brings the prompt back.
    assert!(feed(&mut core, &mut at, b"^C\r\n\x1b]133;D;130\x07").is_empty());
    assert!(feed(&mut core, &mut at, b"\x1b]133;A\x07% \x1b]133;B\x07").is_empty());
}

#[test]
fn a_run_detached_before_its_output_holds_the_next_until_it_ends() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    let (first, _first_answer, _) = order(1, "sleep 9", RunMode::Auto);
    assert_eq!(core.submit(first).len(), 1);
    assert!(matches!(core.detach(1), Detached::Running { .. }));
    // The line is typed but its `C` has not come: the phase still says Ready.
    assert_eq!(core.state().phase, Phase::Ready);
    let (second, _second_answer, _) = order(2, "ls", RunMode::Auto);
    assert!(core.submit(second).is_empty(), "nothing is typed ahead into the first line");
    assert!(feed(&mut core, &mut at, b"\r\n\x1b]133;C\x07\x1b]133;D;0\x07").is_empty());
    let writes = feed(&mut core, &mut at, b"\x1b]133;A\x07% \x1b]133;B\x07");
    assert_eq!(writes, [b"\x1b[efr-clear~\x1b[200~ls\x1b[201~\r".to_vec()]);
}

#[test]
fn an_exit_fails_the_runs_and_is_a_notice() {
    let (mut core, notices) = core(true);
    let (order, mut answer, _) = order(1, "id", RunMode::Auto);
    core.submit(order);
    core.exited(None);
    assert!(matches!(answer.try_recv().unwrap(), Err(ShellError::Exited { status: None, .. })));
    assert!(matches!(notices.all().as_slice(), [ShellNotice::Exited { status: None, .. }]));
}

#[test]
fn a_second_run_is_busy_while_the_first_is_queued() {
    let (mut core, _) = core(true);
    let (first, _first_answer, _) = order(1, "a", RunMode::Auto);
    let (second, mut second_answer, _) = order(2, "b", RunMode::Auto);
    core.submit(first);
    core.submit(second);
    assert!(matches!(second_answer.try_recv().unwrap(), Err(ShellError::Busy { .. })));
}

#[test]
fn a_bare_end_after_the_line_means_it_never_ran() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    let (order, mut answer, _) = order(1, ")", RunMode::Auto);
    core.submit(order);
    feed(&mut core, &mut at, b")\r\nzsh: parse error\r\n\x1b]133;D\x07");
    let end = answer.try_recv().unwrap().unwrap();
    assert_eq!(end.output.completion, Completion::NotStarted);
    assert_eq!(end.output.kept.clean().text, ")\nzsh: parse error\n");
}

#[test]
fn the_caller_hears_once_that_the_command_runs_even_before_any_output() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    let (order, _answer, mut updates) = order(1, "sudo true", RunMode::Auto);
    core.submit(order);
    feed(&mut core, &mut at, b"\r\n");
    assert!(!updates.borrow_and_update().started, "the echo is not the command");
    feed(&mut core, &mut at, b"\x1b]133;C\x07");
    assert!(updates.has_changed().unwrap());
    let progress = updates.borrow_and_update().clone();
    assert!(progress.started);
    assert_eq!(progress.bytes, 0, "the start alone is no output");
    feed(&mut core, &mut at, b"pw: ");
    let progress = updates.borrow_and_update().clone();
    assert!(progress.started);
    assert_eq!(progress.bytes, 4);
}

#[test]
fn a_probe_tells_whether_the_command_runs_and_when_it_last_printed() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    assert_eq!(core.probe(1, None), None);
    let (order, mut answer, _) = order(1, "sudo true", RunMode::Auto);
    core.submit(order);
    let before = core.probe(1, None).unwrap();
    assert!(!before.running);
    assert_eq!(before.job, None, "the actor reads the job");
    feed(&mut core, &mut at, b"\r\n\x1b]133;C\x07pw: ");
    let running = core.probe(1, None).unwrap();
    assert!(running.running);
    assert_eq!(running.last_output, Some(TestClock::START));
    assert_eq!(running.answers, 0);
    core.answered();
    assert_eq!(core.probe(1, None).unwrap().answers, 1);
    assert_eq!(core.probe(2, None), None, "another run");
    feed(&mut core, &mut at, b"\r\n\x1b]133;D;0\x07");
    assert!(answer.try_recv().is_ok());
    assert_eq!(core.probe(1, None), None, "the run ended");
}

#[test]
fn only_the_running_command_of_the_same_call_takes_an_answer() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    assert!(matches!(core.answerable(call_id(1)), Err(ShellError::NoCall { .. })));
    let (order, _answer, _) = order(1, "sudo true", RunMode::Auto);
    core.submit(order);
    // Typed, but its `C` has not come: the line editor may still be reading.
    assert!(matches!(core.answerable(call_id(1)), Err(ShellError::NotWaiting { .. })));
    feed(&mut core, &mut at, b"\r\n\x1b]133;C\x07pw: ");
    core.answerable(call_id(1)).unwrap();
    assert!(matches!(core.answerable(call_id(2)), Err(ShellError::NotWaiting { .. })));
    // Its `D` ends it: the shell is no longer the command's.
    feed(&mut core, &mut at, b"\r\n\x1b]133;D;0\x07");
    assert!(matches!(core.answerable(call_id(1)), Err(ShellError::NoCall { .. })));
}

#[test]
fn an_answer_learns_the_job_whose_wait_its_run_reported_last() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    let (order, _answer, _) = order(1, "sudo true", RunMode::Auto);
    core.submit(order);
    feed(&mut core, &mut at, b"\r\n\x1b]133;C\x07pw: ");
    let waiting = |core: &SessionCore| core.answerable(call_id(1)).unwrap().waiting;
    assert_eq!(waiting(&core), None, "no wait was reported yet");
    let hidden = Waiting { group: 4242, hidden: true };
    core.waiting(1, Some(hidden));
    assert_eq!(waiting(&core), Some(hidden));
    core.waiting(2, Some(Waiting { group: 4343, hidden: false }));
    assert_eq!(waiting(&core), Some(hidden), "another run's report");
    core.waiting(1, None);
    assert_eq!(waiting(&core), None, "the wait ended");
}

#[test]
fn a_run_keeps_the_job_that_its_last_look_saw_while_the_command_ran() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    let (order, _answer, _) = order(1, "./deploy", RunMode::Auto);
    core.submit(order);
    let job = |group| Some(Job { group, modes: InputModes::new(true, true) });
    // A look before the command's `C` sees the line editor's terminal, not the command.
    core.probe(1, job(4343));
    feed(&mut core, &mut at, b"\r\n\x1b]133;C\x07");
    let looked = |core: &SessionCore| core.answerable(call_id(1)).unwrap().looked;
    assert_eq!(looked(&core), None, "no look while the command ran");
    core.probe(1, job(4242));
    assert_eq!(looked(&core), Some(4242));
    core.probe(2, job(4343));
    assert_eq!(looked(&core), Some(4242), "a look at another run");
    core.probe(1, None);
    assert_eq!(looked(&core), None, "the shell itself held the terminal");
}

#[test]
fn a_run_into_a_shell_that_reads_command_lines_reports_hidden_waits_only() {
    for (command, mode, offer) in [
        ("./deploy", RunMode::Auto, Offer::All),
        ("bash", RunMode::Auto, Offer::Hidden),
        ("sleep 60", RunMode::Sentinel, Offer::Hidden),
    ] {
        let (mut core, _) = core(false);
        let mut at = 0;
        let (order, _answer, _) = order(1, command, mode);
        core.submit(order);
        feed(&mut core, &mut at, b"__efr_0123456789abcdef_b\r\n");
        assert_eq!(core.answerable(call_id(1)).unwrap().offer, offer, "{command} {mode:?}");
    }
}

#[test]
fn a_detached_run_takes_no_answer() {
    let (mut core, _) = core(true);
    let mut at = 0;
    ready(&mut core, &mut at);
    let (order, _answer, _) = order(1, "sudo true", RunMode::Auto);
    core.submit(order);
    feed(&mut core, &mut at, b"\r\n\x1b]133;C\x07pw: ");
    assert!(matches!(core.detach(1), Detached::Running { .. }));
    assert!(matches!(core.answerable(call_id(1)), Err(ShellError::NoCall { .. })));
}

#[test]
fn a_sentinel_run_takes_an_answer_between_its_markers() {
    let (mut core, _) = core(false);
    let mut at = 0;
    let (order, _answer, mut updates) = order(1, "sudo true", RunMode::Auto);
    core.submit(order);
    assert!(matches!(core.answerable(call_id(1)), Err(ShellError::NotWaiting { .. })));
    feed(&mut core, &mut at, b"__efr_0123456789abcdef_b\r\npw: ");
    core.answerable(call_id(1)).unwrap();
    assert!(updates.borrow_and_update().started);
}
