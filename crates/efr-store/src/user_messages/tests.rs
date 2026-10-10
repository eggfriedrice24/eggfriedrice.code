use efr_protocol::{Origin, Seq};
use pretty_assertions::assert_eq;

use super::*;
use crate::Batch;
use crate::testing::{self, TestClock, on_writer};

#[tokio::test]
async fn every_user_message_of_one_conversation_reads_back_oldest_first() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    let other = testing::conversation(2);
    let steered = Event::TurnSteered { turn_id: testing::turn(1), text: "my zshrc".to_owned() };
    let withdrawn = Event::PromptWithdrawn { turn_id: testing::turn(2), origin: Origin::Shell };
    let taken_back = Event::SteeringWithdrawn {
        turn_id: testing::turn(1),
        steers: vec![Seq::new(4)],
        origin: Origin::Shell,
    };
    let committed = writer
        .append(
            Batch::new()
                .event(id, testing::created(None))
                .event(id, testing::queued(1, "fix the build"))
                .event(id, testing::started(1, "/home/u/p/app"))
                .event(id, steered)
                .event(id, testing::queued(2, "and then push"))
                .event(id, withdrawn)
                .event(id, taken_back)
                .event(other, testing::created(None))
                .event(other, testing::queued(3, "another conversation")),
        )
        .await
        .unwrap();

    let read = on_writer(&writer, move |conn| of_conversation(conn, id)).await.unwrap();

    let all = committed.events();
    let kept: Vec<EventEnvelope> =
        [1, 3, 4, 5, 6].into_iter().map(|index| all[index].clone()).collect();
    assert_eq!(read, kept);
}

#[tokio::test]
async fn a_conversation_without_messages_reads_nothing() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer.append(Batch::new().event(id, testing::created(None))).await.unwrap();

    let read = on_writer(&writer, move |conn| of_conversation(conn, id)).await.unwrap();

    assert!(read.is_empty());
}
