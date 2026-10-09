use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{ApprovalDecision, ErrorBody, ErrorCode, Origin, Usage};
use pretty_assertions::assert_eq;

use super::*;
use crate::testing::{self, TestClock, on_writer};
use crate::{Batch, WriterHandle, events};

async fn summary(writer: &WriterHandle, id: ConversationId) -> ConversationSummary {
    on_writer(writer, move |conn| get(conn, id)).await.unwrap().unwrap()
}

async fn status(writer: &WriterHandle, id: ConversationId) -> ConversationStatus {
    summary(writer, id).await.status
}

fn approval_requested(turn: u64, call: u64) -> Event {
    Event::ApprovalRequested {
        turn_id: testing::turn(turn),
        call_id: testing::call(call),
        summary: "write /etc/hosts".to_owned(),
        diff_preview: None,
        interactive: false,
        exit: None,
    }
}

fn approval_resolved(turn: u64, call: u64) -> Event {
    Event::ApprovalResolved {
        turn_id: testing::turn(turn),
        call_id: testing::call(call),
        decision: ApprovalDecision::Allow,
        origin: Origin::Shell,
    }
}

#[tokio::test]
async fn a_new_conversation_is_idle_with_its_tty_and_no_title() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);

    let committed =
        writer.append(Batch::new().event(id, testing::created(Some("pts-1")))).await.unwrap();

    let at = committed.events()[0].at;
    assert_eq!(
        summary(&writer, id).await,
        ConversationSummary {
            id,
            title: None,
            status: ConversationStatus::Idle,
            created_at: at,
            updated_at: at,
            last_seq: Seq::new(1),
            cwd: None,
            scope: None,
            tty: Some("pts-1".to_owned()),
        }
    );
}

#[tokio::test]
async fn the_first_prompt_titles_the_conversation_and_later_ones_do_not() {
    let clock = TestClock::new();
    let (writer, _thread) = testing::memory_writer(Arc::clone(&clock));
    let id = testing::conversation(1);
    writer
        .append(
            Batch::new()
                .event(id, testing::created(None))
                .event(id, testing::queued(1, "\n  first line \nsecond")),
        )
        .await
        .unwrap();
    clock.advance(Duration::from_secs(5));

    let later = writer.append(Batch::new().event(id, testing::queued(2, "other"))).await.unwrap();

    let summary = summary(&writer, id).await;
    assert_eq!(summary.title.as_deref(), Some("first line"));
    assert_eq!(summary.cwd, Some(testing::path("/etc/nixos")));
    assert_eq!(summary.updated_at, later.events()[0].at);
    assert_eq!(summary.last_seq, Seq::new(3));
    assert!(summary.created_at < summary.updated_at);
}

#[test]
fn titles_are_the_first_line_and_at_most_80_characters() {
    assert_eq!(title_from("  \n  hello world \nmore"), Some("hello world".to_owned()));
    assert_eq!(title_from(" \n\t"), None);
    // 77 characters of the line, then "...": exactly 80.
    let long = title_from(&"word ".repeat(30)).unwrap();
    assert_eq!(long, format!("{}wo...", "word ".repeat(15)));
    assert_eq!(long.chars().count(), 80);
    // A cut that ends in a space drops it, so the dots follow the word.
    let spaced = title_from(&format!("{} more", "x".repeat(76))).unwrap();
    assert_eq!(spaced, format!("{}...", "x".repeat(76)));
    let exact = "\u{e9}".repeat(80);
    assert_eq!(title_from(&exact), Some(exact.clone()));
}

#[tokio::test]
async fn the_status_follows_the_running_turn_and_its_approvals() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    let mut seen = Vec::new();
    for event in [
        testing::created(None),
        testing::queued(1, "fix the network"),
        testing::started(1, "/etc"),
        approval_requested(1, 1),
        approval_resolved(1, 1),
        Event::TurnCompleted {
            turn_id: testing::turn(1),
            usage: Some(Usage::default()),
            changes: None,
            context: None,
        },
    ] {
        writer.append(Batch::new().event(id, event)).await.unwrap();
        seen.push(status(&writer, id).await);
    }

    use ConversationStatus::{AwaitingApproval, Idle, Running};
    assert_eq!(seen, [Idle, Idle, Running, AwaitingApproval, Running, Idle]);
}

#[tokio::test]
async fn a_pending_approval_of_an_ended_turn_does_not_keep_the_conversation_waiting() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer
        .append(
            Batch::new()
                .event(id, testing::created(None))
                .event(id, testing::queued(1, "x"))
                .event(id, testing::started(1, "/"))
                .event(id, approval_requested(1, 1))
                .event(
                    id,
                    Event::TurnInterrupted {
                        turn_id: testing::turn(1),
                        usage: None,
                        context: None,
                    },
                ),
        )
        .await
        .unwrap();

    assert_eq!(status(&writer, id).await, ConversationStatus::Idle);
}

#[tokio::test]
async fn the_cwd_and_scope_follow_the_latest_turn_and_scope_change() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer
        .append(
            Batch::new()
                .event(id, testing::created(None))
                .event(id, testing::queued(1, "x"))
                .event(id, testing::started(1, "/srv/app")),
        )
        .await
        .unwrap();
    assert_eq!(summary(&writer, id).await.scope, Some(Scope::Path(testing::path("/srv/app"))));

    writer
        .append(Batch::new().event(
            id,
            Event::ScopeChanged {
                turn_id: testing::turn(1),
                from: Scope::Path(testing::path("/srv/app")),
                to: Scope::Machine,
            },
        ))
        .await
        .unwrap();

    let summary = summary(&writer, id).await;
    assert_eq!(summary.scope, Some(Scope::Machine));
    assert_eq!(summary.cwd, Some(testing::path("/srv/app")));
}

#[tokio::test]
async fn a_new_conversation_from_a_tty_takes_the_tty_from_the_old_one() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let (old, new, other) =
        (testing::conversation(1), testing::conversation(2), testing::conversation(3));
    writer
        .append(
            Batch::new()
                .event(old, testing::created(Some("pts-1")))
                .event(other, testing::created(Some("pts-2")))
                .event(new, testing::created(Some("pts-1"))),
        )
        .await
        .unwrap();

    assert_eq!(summary(&writer, old).await.tty, None);
    assert_eq!(summary(&writer, new).await.tty.as_deref(), Some("pts-1"));
    assert_eq!(summary(&writer, other).await.tty.as_deref(), Some("pts-2"));
}

#[tokio::test]
async fn list_pages_through_conversations_most_recently_updated_first() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let ids: Vec<ConversationId> = (1..=3).map(testing::conversation).collect();
    for id in &ids {
        writer.append(Batch::new().event(*id, testing::created(None))).await.unwrap();
    }
    writer.append(Batch::new().event(ids[0], testing::queued(1, "bump"))).await.unwrap();

    let first = on_writer(&writer, |conn| list(conn, None, 2)).await.unwrap();
    let cursor = first.last().map(|summary| summary.last_seq);
    let second = on_writer(&writer, move |conn| list(conn, cursor, 2)).await.unwrap();

    let order = |page: &[ConversationSummary]| page.iter().map(|s| s.id).collect::<Vec<_>>();
    assert_eq!(order(&first), [ids[0], ids[2]]);
    assert_eq!(order(&second), [ids[1]]);
}

#[tokio::test]
async fn turns_keep_their_status_and_the_unfinished_ones_are_listed() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer
        .append(
            Batch::new()
                .event(id, testing::created(None))
                .event(id, testing::queued(1, "done"))
                .event(id, testing::started(1, "/"))
                .event(
                    id,
                    Event::TurnFailed {
                        turn_id: testing::turn(1),
                        error: ErrorBody::new(ErrorCode::Internal, "provider down"),
                        usage: None,
                        context: None,
                    },
                )
                .event(id, testing::queued(2, "waiting"))
                .event(id, testing::queued(3, "held"))
                .event(id, Event::PromptHeld { turn_id: testing::turn(3) })
                .event(id, testing::queued(4, "running"))
                .event(id, testing::started(4, "/")),
        )
        .await
        .unwrap();

    let all = on_writer(&writer, move |conn| turns(conn, id)).await.unwrap();
    let unfinished = on_writer(&writer, unfinished_turns).await.unwrap();

    let statuses: Vec<TurnStatus> = all.iter().map(|turn| turn.status).collect();
    use TurnStatus::{Failed, Held, Queued, Running};
    assert_eq!(statuses, [Failed, Queued, Held, Running]);
    assert!(all[0].started_at.is_some() && all[0].ended_at.is_some());
    assert_eq!(all[0].command_id, Some(testing::command(1)));
    assert_eq!(all[0].prompt, "done");
    assert_eq!(all[0].last_seq, Seq::new(4));
    let unfinished: Vec<TurnId> = unfinished.iter().map(|turn| turn.id).collect();
    assert_eq!(unfinished, [testing::turn(2), testing::turn(3), testing::turn(4)]);
    let one = on_writer(&writer, |conn| turn(conn, testing::turn(3))).await.unwrap().unwrap();
    assert_eq!((one.id, one.conversation_id, one.status), (testing::turn(3), id, Held));
    let unknown = on_writer(&writer, |conn| turn(conn, testing::turn(9))).await.unwrap();
    assert_eq!(unknown, None);
}

#[test]
fn turn_status_names_round_trip_and_finished_ones_are_marked() {
    for status in TurnStatus::ALL {
        assert_eq!(TurnStatus::from_column(status.as_str()).unwrap(), status);
    }
    assert!(TurnStatus::from_column("paused").is_err());
    let finished: Vec<TurnStatus> =
        TurnStatus::ALL.into_iter().filter(|status| status.is_finished()).collect();
    use TurnStatus::{Cancelled, Completed, Failed, Interrupted, Withdrawn};
    assert_eq!(finished, [Completed, Failed, Interrupted, Cancelled, Withdrawn]);
}

#[tokio::test]
async fn a_withdrawn_prompt_ends_its_turn_and_is_never_unfinished() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer
        .append(
            Batch::new()
                .event(id, testing::created(None))
                .event(id, testing::queued(1, "running"))
                .event(id, testing::started(1, "/"))
                .event(id, testing::queued(2, "taken back"))
                .event(
                    id,
                    Event::PromptWithdrawn { turn_id: testing::turn(2), origin: Origin::Shell },
                ),
        )
        .await
        .unwrap();

    let all = on_writer(&writer, move |conn| turns(conn, id)).await.unwrap();
    let unfinished = on_writer(&writer, unfinished_turns).await.unwrap();

    assert_eq!(all[1].status, TurnStatus::Withdrawn);
    assert!(all[1].started_at.is_none() && all[1].ended_at.is_some());
    assert_eq!(all[1].last_seq, Seq::new(5));
    let unfinished: Vec<TurnId> = unfinished.iter().map(|turn| turn.id).collect();
    assert_eq!(unfinished, [testing::turn(1)], "a restart never cancels a withdrawn prompt");
}

#[tokio::test]
async fn an_event_for_an_unknown_conversation_is_refused_and_writes_nothing() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let missing = testing::conversation(9);

    let error =
        writer.append(Batch::new().event(missing, testing::queued(1, "x"))).await.unwrap_err();

    assert!(
        matches!(error, StoreError::UnknownConversation { conversation_id } if conversation_id == missing),
        "{error:?}"
    );
    let log = on_writer(&writer, |conn| events::read_after(conn, Seq::ZERO, 10)).await.unwrap();
    assert_eq!(log, []);
}

#[tokio::test]
async fn a_conversation_cannot_be_created_twice() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer.append(Batch::new().event(id, testing::created(None))).await.unwrap();

    let error = writer.append(Batch::new().event(id, testing::created(None))).await.unwrap_err();

    assert!(matches!(error, StoreError::ConversationExists { .. }), "{error:?}");
}

#[tokio::test]
async fn an_event_of_a_conversation_needs_its_id() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());

    let error =
        writer.append(Batch::new().global_event(testing::queued(1, "x"))).await.unwrap_err();

    assert!(
        matches!(error, StoreError::MissingConversation { ref kind } if kind == "prompt_queued"),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_change_of_the_sandbox_probe_belongs_to_no_conversation() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());

    let reason = "unprivileged user namespaces are off".to_owned();
    let committed =
        writer.append(Batch::new().global_event(Event::SandboxUnavailable { reason })).await;

    assert!(committed.is_ok(), "{committed:?}");
}

#[tokio::test]
async fn a_logout_belongs_to_no_conversation() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());

    let logout = Event::LogoutCompleted { provider: "anthropic-api".to_owned() };
    let committed = writer.append(Batch::new().global_event(logout)).await;

    assert!(committed.is_ok(), "{committed:?}");
}

#[tokio::test]
async fn rebuilding_the_projections_reproduces_them_exactly() {
    let clock = TestClock::new();
    let (writer, _thread) = testing::memory_writer(Arc::clone(&clock));
    let (one, two) = (testing::conversation(1), testing::conversation(2));
    let steps = [
        Batch::new().event(one, testing::created(Some("pts-1"))),
        Batch::new().event(one, testing::queued(1, "update the system")),
        Batch::new().event(one, testing::started(1, "/etc")).event(
            one,
            Event::ShellStarted {
                pty_id: testing::pty(1),
                cwd: testing::path("/etc"),
                pid: Some(42),
            },
        ),
        Batch::new().event(one, approval_requested(1, 1)),
        Batch::new().event(one, approval_resolved(1, 1)).event(
            one,
            Event::CwdChanged { pty_id: testing::pty(1), cwd: testing::path("/var"), host: None },
        ),
        Batch::new().event(one, approval_requested(1, 2)),
        Batch::new()
            .event(
                one,
                Event::ApprovalExpired { turn_id: testing::turn(1), call_id: testing::call(2) },
            )
            .event(one, Event::TurnCancelled { turn_id: testing::turn(1) }),
        Batch::new()
            .event(two, testing::created(Some("pts-1")))
            .global_event(Event::LoginCompleted { provider: "openai".to_owned() }),
        Batch::new()
            .event(two, testing::queued(2, "held one"))
            .event(two, Event::PromptHeld { turn_id: testing::turn(2) })
            .event(one, Event::ShellExited { pty_id: testing::pty(1), exit_code: Some(0) }),
    ];
    for batch in steps {
        clock.advance(Duration::from_millis(1500));
        writer.append(batch).await.unwrap();
    }
    let snapshot = || {
        on_writer(&writer, |conn| {
            Ok(["conversations", "turns", "approvals", "shells"]
                .map(|table| testing::dump(conn, table)))
        })
    };

    let before = snapshot().await.unwrap();
    writer.rebuild_projections().await.unwrap();
    let after = snapshot().await.unwrap();

    assert!(before.iter().all(|rows| !rows.is_empty()));
    assert_eq!(before, after);
}
