//! The route columns of the conversations projection: one row per terminal and scope
//! key, the pid of the shell that holds it and its newest routed event (DV1).

use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{AttachHow, Origin, ProjectId, RouteScope, ShellContext, TurnSettings};
use pretty_assertions::assert_eq;

use super::super::*;
use super::summary;
use crate::testing::{self, TestClock, on_writer};
use crate::{Batch, WriterHandle};

fn project(n: u64) -> ProjectId {
    format!("00000009-0000-7000-8000-{n:012x}").parse().unwrap()
}

fn app() -> RouteScope {
    RouteScope::Project(project(1))
}

fn created_in(tty: &str, route: RouteScope) -> Event {
    Event::ConversationCreated {
        origin: Origin::Shell,
        tty: Some(tty.to_owned()),
        route: Some(route),
    }
}

fn attached(tty: &str, route: RouteScope, shell_pid: Option<u32>) -> Event {
    Event::TerminalAttached { tty: tty.to_owned(), route, shell_pid, how: AttachHow::PickedUp }
}

/// A prompt for turn `turn` from the shell `pid` of `tty`.
fn queued_from(turn: u64, tty: Option<&str>, pid: Option<u32>) -> Event {
    let mut context = ShellContext::new("/srv/app");
    context.tty = tty.map(str::to_owned);
    context.shell_pid = pid;
    Event::PromptQueued {
        turn_id: testing::turn(turn),
        command_id: testing::command(turn),
        text: "fix it".to_owned(),
        origin: Origin::Shell,
        context: Some(context),
        settings: TurnSettings::default(),
        steers: Vec::new(),
    }
}

async fn slots_now(writer: &WriterHandle) -> Vec<SlotRow> {
    on_writer(writer, slots).await.unwrap()
}

/// The slots as `(tty, route, conversation n, pid)`, sorted.
async fn table(writer: &WriterHandle) -> Vec<(String, RouteScope, ConversationId, Option<u32>)> {
    let mut rows: Vec<_> = slots_now(writer)
        .await
        .into_iter()
        .map(|slot| (slot.tty, slot.route, slot.conversation_id, slot.shell_pid))
        .collect();
    rows.sort_by(|a, b| (&a.0, a.2).cmp(&(&b.0, b.2)));
    rows
}

#[tokio::test]
async fn create_clears_only_the_same_tty_and_scope() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let [one, two, three, four] = [1, 2, 3, 4].map(testing::conversation);
    writer
        .append(
            Batch::new()
                .event(one, created_in("pts-1", app()))
                .event(two, created_in("pts-1", RouteScope::Outside))
                .event(three, created_in("pts-2", app()))
                .event(four, created_in("pts-1", app())),
        )
        .await
        .unwrap();

    assert_eq!(
        table(&writer).await,
        [
            ("pts-1".to_owned(), RouteScope::Outside, two, None),
            ("pts-1".to_owned(), app(), four, None),
            ("pts-2".to_owned(), app(), three, None),
        ]
    );
    assert_eq!(summary(&writer, one).await.tty, None);
}

#[tokio::test]
async fn old_create_without_route_clears_every_row_of_the_tty() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let [one, two, three] = [1, 2, 3].map(testing::conversation);
    writer
        .append(
            Batch::new()
                .event(one, created_in("pts-1", app()))
                .event(two, created_in("pts-1", RouteScope::Outside))
                .event(three, testing::created(Some("pts-1"))),
        )
        .await
        .unwrap();

    assert_eq!(table(&writer).await, [("pts-1".to_owned(), RouteScope::Outside, three, None)]);
}

#[tokio::test]
async fn terminal_attached_moves_the_row() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let [held, old, other] = [1, 2, 3].map(testing::conversation);
    writer
        .append(
            Batch::new()
                .event(held, created_in("pts-1", app()))
                .event(held, queued_from(1, Some("pts-1"), Some(100)))
                .event(old, created_in("pts-2", app()))
                .event(other, created_in("pts-2", RouteScope::Outside)),
        )
        .await
        .unwrap();

    let committed = writer
        .append(
            Batch::new()
                .event(held, attached("pts-2", app(), Some(200)))
                .event(held, queued_from(2, Some("pts-2"), Some(300))),
        )
        .await
        .unwrap();

    assert_eq!(
        table(&writer).await,
        [
            ("pts-2".to_owned(), app(), held, Some(200)),
            ("pts-2".to_owned(), RouteScope::Outside, other, None),
        ]
    );
    assert_eq!(summary(&writer, old).await.tty, None, "the slot's old conversation is unheld");
    let current = on_writer(&writer, |conn| of_tty(conn, "pts-2")).await.unwrap();
    assert_eq!(current[0].id, held, "the attach makes the slot current");
    let slot = slots_now(&writer).await.into_iter().find(|slot| slot.conversation_id == held);
    assert_eq!(slot.unwrap().routed_seq, committed.events()[1].seq, "the prompt is the newest");
}

#[tokio::test]
async fn prompt_from_the_tty_marks_the_row_current() {
    let clock = TestClock::new();
    let (writer, _thread) = testing::memory_writer(Arc::clone(&clock));
    let [here, outside] = [1, 2].map(testing::conversation);
    writer
        .append(
            Batch::new()
                .event(here, created_in("pts-1", app()))
                .event(here, queued_from(1, Some("pts-1"), Some(100))),
        )
        .await
        .unwrap();
    writer
        .append(
            Batch::new()
                .event(outside, created_in("pts-1", RouteScope::Outside))
                .event(outside, queued_from(2, Some("pts-1"), Some(100))),
        )
        .await
        .unwrap();
    let first = on_writer(&writer, |conn| of_tty(conn, "pts-1")).await.unwrap();
    assert_eq!(first.iter().map(|s| s.id).collect::<Vec<_>>(), [outside, here]);

    // A prompt from no terminal, or from another one, changes nothing.
    writer.append(Batch::new().event(here, queued_from(3, None, None))).await.unwrap();
    writer.append(Batch::new().event(here, queued_from(4, Some("pts-9"), None))).await.unwrap();
    let same = on_writer(&writer, |conn| of_tty(conn, "pts-1")).await.unwrap();
    assert_eq!(same.iter().map(|s| s.id).collect::<Vec<_>>(), [outside, here]);

    writer.append(Batch::new().event(here, queued_from(5, Some("pts-1"), None))).await.unwrap();
    let back = on_writer(&writer, |conn| of_tty(conn, "pts-1")).await.unwrap();
    assert_eq!(back.iter().map(|s| s.id).collect::<Vec<_>>(), [here, outside]);
}

#[tokio::test]
async fn nested_shell_prompt_keeps_the_first_pid() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer
        .append(
            Batch::new()
                .event(id, created_in("pts-1", app()))
                .event(id, queued_from(1, Some("pts-1"), None)),
        )
        .await
        .unwrap();
    assert_eq!(table(&writer).await[0].3, None, "a prompt that names no shell");
    writer.append(Batch::new().event(id, queued_from(2, Some("pts-1"), Some(100)))).await.unwrap();
    writer.append(Batch::new().event(id, queued_from(3, Some("pts-1"), Some(200)))).await.unwrap();

    assert_eq!(table(&writer).await[0].3, Some(100), "the outer shell keeps the slot");
}

#[tokio::test]
async fn turn_started_fills_a_missing_route() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let [old, project_old, routed] = [1, 2, 3].map(testing::conversation);
    let in_project = |turn| Event::TurnStarted {
        turn_id: testing::turn(turn),
        cwd: "/srv/app".into(),
        scope: Scope::Project(project(1)),
        settings: None,
    };
    writer
        .append(
            Batch::new()
                .event(old, testing::created(None))
                .event(old, testing::queued(1, "first"))
                .event(old, testing::started(1, "/etc"))
                .event(project_old, testing::created(None))
                .event(project_old, testing::queued(2, "second"))
                .event(project_old, in_project(2))
                .event(routed, created_in("pts-1", RouteScope::Outside))
                .event(routed, testing::queued(3, "third"))
                .event(routed, in_project(3)),
        )
        .await
        .unwrap();

    let route_of = |id| on_writer(&writer, move |conn| route(conn, id));
    assert_eq!(route_of(old).await.unwrap(), Some(RouteScope::Outside));
    assert_eq!(route_of(project_old).await.unwrap(), Some(app()));
    assert_eq!(route_of(routed).await.unwrap(), Some(RouteScope::Outside), "the first key stays");
    // NOTE: no terminal held `project_old`, so it is no candidate of a pick-up (DV55).
    let newest = on_writer(&writer, |conn| newest_of_route(conn, &app())).await.unwrap();
    assert_eq!(newest.map(|summary| summary.id), None);
}

#[tokio::test]
async fn newest_of_route_skips_a_conversation_that_no_terminal_held() {
    let clock = TestClock::new();
    let (writer, _thread) = testing::memory_writer(Arc::clone(&clock));
    let [held, script] = [1, 2].map(testing::conversation);
    let in_project = |turn| Event::TurnStarted {
        turn_id: testing::turn(turn),
        cwd: "/srv/app".into(),
        scope: Scope::Project(project(1)),
        settings: None,
    };
    writer
        .append(
            Batch::new()
                .event(held, created_in("pts-1", app()))
                .event(held, queued_from(1, Some("pts-1"), Some(100)))
                .event(held, in_project(1)),
        )
        .await
        .unwrap();
    clock.advance(Duration::from_secs(60));
    writer
        .append(
            Batch::new()
                .event(script, testing::created(None))
                .event(script, testing::queued(2, "from cron"))
                .event(script, in_project(2)),
        )
        .await
        .unwrap();

    let route_of = |id| on_writer(&writer, move |conn| route(conn, id));
    assert_eq!(route_of(script).await.unwrap(), Some(app()), "it is listed with its project");
    let newest = on_writer(&writer, |conn| newest_of_route(conn, &app())).await.unwrap();
    assert_eq!(newest.map(|summary| summary.id), Some(held));
}

#[tokio::test]
async fn list_filtered_by_tty_puts_the_current_first() {
    let clock = TestClock::new();
    let (writer, _thread) = testing::memory_writer(Arc::clone(&clock));
    let [a, b, c, elsewhere] = [1, 2, 3, 4].map(testing::conversation);
    for (n, (id, route)) in
        [(a, app()), (b, RouteScope::Outside), (c, RouteScope::Project(project(2)))]
            .into_iter()
            .enumerate()
    {
        clock.advance(Duration::from_secs(1));
        writer
            .append(
                Batch::new()
                    .event(id, created_in("pts-1", route))
                    .event(id, queued_from(n as u64 + 1, Some("pts-1"), Some(100))),
            )
            .await
            .unwrap();
    }
    writer.append(Batch::new().event(elsewhere, created_in("pts-2", app()))).await.unwrap();
    // A job event moves `b` to the top of the list, but not to the top of the terminal.
    writer.append(Batch::new().event(b, testing::queued(9, "from the phone"))).await.unwrap();
    writer.append(Batch::new().event(a, queued_from(10, Some("pts-1"), Some(100)))).await.unwrap();

    let rows = on_writer(&writer, |conn| of_tty(conn, "pts-1")).await.unwrap();
    assert_eq!(rows.iter().map(|s| s.id).collect::<Vec<_>>(), [a, c, b]);
}

#[tokio::test]
async fn rebuild_equals_incremental_with_routes() {
    let clock = TestClock::new();
    let (writer, _thread) = testing::memory_writer(Arc::clone(&clock));
    let [one, two, three] = [1, 2, 3].map(testing::conversation);
    let steps = [
        Batch::new().event(one, testing::created(Some("pts-1"))),
        Batch::new().event(one, queued_from(1, Some("pts-1"), Some(100))),
        Batch::new().event(one, testing::started(1, "/etc")),
        Batch::new()
            .event(two, created_in("pts-1", app()))
            .event(two, queued_from(2, Some("pts-1"), Some(100))),
        Batch::new()
            .event(three, created_in("pts-2", RouteScope::Outside))
            .event(three, queued_from(3, Some("pts-2"), Some(200))),
        Batch::new()
            .event(two, attached("pts-2", app(), Some(200)))
            .event(two, queued_from(4, Some("pts-2"), Some(200))),
        Batch::new().event(one, queued_from(5, Some("pts-1"), Some(150))),
    ];
    for batch in steps {
        clock.advance(Duration::from_millis(1500));
        writer.append(batch).await.unwrap();
    }
    let snapshot = || on_writer(&writer, |conn| Ok(testing::dump(conn, "conversations")));

    let before = snapshot().await.unwrap();
    writer.rebuild_projections().await.unwrap();
    let after = snapshot().await.unwrap();

    assert_eq!(before, after);
    assert_eq!(
        table(&writer).await,
        [
            ("pts-1".to_owned(), RouteScope::Outside, one, Some(100)),
            ("pts-2".to_owned(), app(), two, Some(200)),
            ("pts-2".to_owned(), RouteScope::Outside, three, Some(200)),
        ]
    );
}
