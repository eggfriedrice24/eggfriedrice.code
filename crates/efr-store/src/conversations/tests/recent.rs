//! The list of `,resume`: the conversations of one scope key first, then the others,
//! each part newest first, with their prompt turns.

use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{JobId, NoticeKind, Origin, ProjectId, RouteScope};
use pretty_assertions::assert_eq;

use super::super::*;
use crate::Batch;
use crate::testing::{self, TestClock, on_writer};

fn app() -> RouteScope {
    RouteScope::Project("00000009-0000-7000-8000-000000000001".parse::<ProjectId>().unwrap())
}

fn created_in(route: RouteScope) -> Event {
    Event::ConversationCreated { origin: Origin::Shell, tty: None, route: Some(route) }
}

fn ids(rows: &[RecentRow]) -> Vec<ConversationId> {
    rows.iter().map(|row| row.summary.id).collect()
}

#[tokio::test]
async fn recent_puts_the_scope_first_then_the_newest() {
    let clock = TestClock::new();
    let (writer, _thread) = testing::memory_writer(Arc::clone(&clock));
    let [old_app, home, new_app, other] = [1, 2, 3, 4].map(testing::conversation);
    let lib = RouteScope::Project("00000009-0000-7000-8000-000000000002".parse().unwrap());
    for (id, route) in
        [(old_app, app()), (home, RouteScope::Outside), (new_app, app()), (other, lib.clone())]
    {
        clock.advance(Duration::from_secs(60));
        writer.append(Batch::new().event(id, created_in(route))).await.unwrap();
    }

    let first = on_writer(&writer, |conn| recent(conn, Some(&app()), 10)).await.unwrap();
    let none = on_writer(&writer, |conn| recent(conn, None, 10)).await.unwrap();
    let three = on_writer(&writer, |conn| recent(conn, Some(&app()), 3)).await.unwrap();

    assert_eq!(ids(&first), [new_app, old_app, other, home]);
    assert_eq!(first[2].route, lib);
    assert_eq!(ids(&none), [other, new_app, home, old_app], "newest first only");
    assert_eq!(ids(&three), [new_app, old_app, other], "at most the limit");
}

#[tokio::test]
async fn recent_counts_prompt_turns_only() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    let wake = Event::JobNotice {
        conversation_id: id,
        turn_id: testing::turn(3),
        job_id: JobId::nth(1),
        notice_kind: NoticeKind::Exited,
        woke: true,
        source_seq: Seq::new(1),
        not_user_input: true,
        text: "<job id=\"j1\" state=\"exited\" exit=\"0\" command=\"make\">\n</job>".to_owned(),
    };
    writer
        .append(
            Batch::new()
                .event(id, created_in(app()))
                .event(id, testing::queued(1, "run the tests"))
                .event(id, testing::started(1, "/srv/app"))
                .event(id, testing::queued(2, "taken back"))
                .event(
                    id,
                    Event::PromptWithdrawn { turn_id: testing::turn(2), origin: Origin::Shell },
                )
                .event(id, wake)
                .event(id, testing::started(3, "/srv/app"))
                .event(id, testing::queued(4, "and then?")),
        )
        .await
        .unwrap();

    let rows = on_writer(&writer, |conn| recent(conn, None, 10)).await.unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].turns, 2, "the wake turn and the withdrawn prompt do not count");
    assert_eq!(rows[0].summary.title.as_deref(), Some("run the tests"));
    assert_eq!(rows[0].route, app());
}
