use efr_protocol::{ConversationId, Event, InputWait, Origin, UserAnswer};
use pretty_assertions::assert_eq;

use crate::testing::{
    self, TestClock, input_changed as input, on_writer, question, surface_question, user_question,
};
use crate::{Batch, StoreWriter, WriterHandle};

async fn rows(writer: &WriterHandle) -> Vec<String> {
    on_writer(writer, |conn| Ok(testing::dump(conn, "waits"))).await.unwrap()
}

fn completed(call: u64) -> Event {
    Event::ToolCallCompleted {
        turn_id: testing::turn(1),
        call_id: testing::call(call),
        output: String::new(),
        truncated: false,
        is_error: false,
        exit_code: Some(0),
        sandbox: None,
        refusal: None,
        changes: None,
        diff: None,
        left_running: false,
        attachments: Vec::new(),
    }
}

/// A conversation with turn 1 running.
async fn running() -> (WriterHandle, StoreWriter, ConversationId) {
    let (writer, thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    let batch = Batch::new()
        .event(id, testing::created(None))
        .event(id, testing::queued(1, "x"))
        .event(id, testing::started(1, "/"));
    writer.append(batch).await.unwrap();
    (writer, thread, id)
}

#[tokio::test]
async fn an_input_wait_stays_until_its_call_stops_waiting_or_completes() {
    let (writer, _thread, id) = running().await;

    writer.append(Batch::new().event(id, input(1, InputWait::Visible))).await.unwrap();
    writer.append(Batch::new().event(id, input(1, InputWait::Hidden))).await.unwrap();
    writer.append(Batch::new().event(id, input(2, InputWait::Hidden))).await.unwrap();
    assert_eq!(rows(&writer).await.len(), 2);

    writer.append(Batch::new().event(id, input(1, InputWait::None))).await.unwrap();
    writer.append(Batch::new().event(id, completed(2))).await.unwrap();
    assert_eq!(rows(&writer).await, Vec::<String>::new());
}

#[tokio::test]
async fn a_question_stays_until_its_answer() {
    let (writer, _thread, id) = running().await;

    let asked = Batch::new().event(id, user_question(1)).event(id, surface_question(2));
    writer.append(asked).await.unwrap();
    let kinds = on_writer(&writer, |conn| {
        let mut stmt = conn.prepare("SELECT kind FROM waits ORDER BY id")?;
        let kinds = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(kinds.collect::<Result<Vec<_>, _>>()?)
    })
    .await
    .unwrap();
    assert_eq!(kinds, [super::QUESTION, super::SURFACE]);

    let answered = Batch::new()
        .event(
            id,
            Event::UserQuestionAnswered {
                turn_id: testing::turn(1),
                question_id: question(1),
                answer: UserAnswer::Interrupted,
                origin: None,
            },
        )
        .event(
            id,
            Event::SurfaceQuestionAnswered {
                turn_id: testing::turn(1),
                question_id: question(2),
                keep: false,
                origin: Some(Origin::Shell),
            },
        );
    writer.append(answered).await.unwrap();
    assert_eq!(rows(&writer).await, Vec::<String>::new());
}

#[tokio::test]
async fn no_wait_outlives_its_turn() {
    for end in testing::turn_ends(1) {
        let (writer, _thread, id) = running().await;
        let waiting = Batch::new()
            .event(id, input(1, InputWait::Visible))
            .event(id, user_question(1))
            .event(id, surface_question(2));
        writer.append(waiting).await.unwrap();
        assert_eq!(rows(&writer).await.len(), 3);

        writer.append(Batch::new().event(id, end.clone())).await.unwrap();

        assert_eq!(rows(&writer).await, Vec::<String>::new(), "{}", end.kind());
    }
}

#[tokio::test]
async fn a_command_left_in_the_hidden_shell_adds_no_wait() {
    let (writer, _thread, id) = running().await;
    writer.append(Batch::new().event(id, completed(1))).await.unwrap();

    let detached = Event::ShellCommandInputChanged {
        call_id: testing::call(1),
        input: InputWait::Visible,
        contained: false,
        looks_secret: false,
        prompt: "[Y/n]".to_owned(),
    };
    writer.append(Batch::new().event(id, detached)).await.unwrap();

    assert_eq!(rows(&writer).await, Vec::<String>::new());
}

#[tokio::test]
async fn a_conversation_delete_removes_its_waits() {
    let (writer, _thread, id) = running().await;
    let waiting = Batch::new().event(id, input(1, InputWait::Visible)).event(id, user_question(1));
    writer.append(waiting).await.unwrap();

    writer
        .append(Batch::new().event(id, Event::ConversationDeleted { origin: Origin::Cli }))
        .await
        .unwrap();

    assert_eq!(rows(&writer).await, Vec::<String>::new());
}
