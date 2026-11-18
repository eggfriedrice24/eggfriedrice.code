use efr_protocol::{
    Attachment, AttachmentId, AttachmentKind, AttachmentOrigin, ConversationId, Event, Origin,
    WithheldReason,
};
use pretty_assertions::assert_eq;

use super::*;
use crate::testing::{self, TestClock, on_writer};
use crate::{Batch, StoreWriter, WriterHandle};

/// The attachment `n`, an image with the bytes of `sha`.
fn image(n: u32, sha: &str) -> Attachment {
    Attachment {
        id: AttachmentId::nth(n),
        name: format!("shot-{n}.png"),
        media_type: "image/png".to_owned(),
        bytes: 1_000,
        sha256: sha.repeat(32),
        kind: AttachmentKind::Image { width: 20, height: 10 },
        origin: AttachmentOrigin::Tool { path: format!("/home/u/shot-{n}.png").into() },
        copy: None,
        sent: None,
    }
}

/// Turn `turn` queued with `attachments`.
fn queued(turn: u64, attachments: Vec<Attachment>) -> Event {
    match testing::queued(turn, "look at these") {
        Event::PromptQueued {
            turn_id,
            command_id,
            text,
            origin,
            context,
            settings,
            steers,
            ..
        } => Event::PromptQueued {
            turn_id,
            command_id,
            text,
            origin,
            context,
            settings,
            steers,
            attachments,
        },
        _ => unreachable!("testing::queued makes a prompt_queued"),
    }
}

/// The completion of call `call` of turn 1, with `attachments`.
fn completed(call: u64, attachments: Vec<Attachment>) -> Event {
    Event::ToolCallCompleted {
        turn_id: testing::turn(1),
        call_id: testing::call(call),
        output: "shot.png: PNG image, 1 KB".to_owned(),
        truncated: false,
        is_error: false,
        exit_code: None,
        sandbox: None,
        refusal: None,
        changes: None,
        diff: None,
        left_running: false,
        attachments,
    }
}

/// A writer with conversation 1, whose turn 1 queued `a1` and `a2` with the same bytes.
async fn with_prompt() -> (WriterHandle, StoreWriter, ConversationId) {
    let (writer, thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    let batch = Batch::new()
        .event(id, testing::created(None))
        .event(id, queued(1, vec![image(1, "ab"), image(2, "ab")]))
        .event(id, testing::started(1, "/"));
    writer.append(batch).await.unwrap();
    (writer, thread, id)
}

async fn summary_of(writer: &WriterHandle, id: ConversationId) -> Summary {
    on_writer(writer, move |conn| summary(conn, id)).await.unwrap()
}

#[tokio::test]
async fn the_same_bytes_twice_give_two_rows_with_two_ids() {
    let (writer, _thread, id) = with_prompt().await;
    let rows = on_writer(&writer, |conn| Ok(testing::dump(conn, "attachments"))).await.unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(summary_of(&writer, id).await, Summary { last: 2, withheld: Vec::new() });
}

#[tokio::test]
async fn a_tool_image_adds_a_row_and_a_withheld_event_marks_its_rows() {
    let (writer, _thread, id) = with_prompt().await;
    let batch = Batch::new().event(id, completed(1, vec![image(3, "cd")])).event(
        id,
        Event::AttachmentsWithheld {
            turn_id: Some(testing::turn(1)),
            attachments: vec![AttachmentId::nth(1), AttachmentId::nth(3)],
            reason: WithheldReason::Budget,
        },
    );
    writer.append(batch).await.unwrap();
    assert_eq!(
        summary_of(&writer, id).await,
        Summary { last: 3, withheld: vec![AttachmentId::nth(1), AttachmentId::nth(3)] }
    );

    let before = on_writer(&writer, |conn| Ok(testing::dump(conn, "attachments"))).await.unwrap();
    writer.rebuild_projections().await.unwrap();
    let after = on_writer(&writer, |conn| Ok(testing::dump(conn, "attachments"))).await.unwrap();
    assert_eq!(after, before, "a rebuild gives the same rows");
}

#[tokio::test]
async fn a_withdrawn_prompt_drops_only_its_own_rows() {
    let (writer, _thread, id) = with_prompt().await;
    let batch = Batch::new()
        .event(id, queued(2, vec![image(3, "ef")]))
        .event(id, Event::PromptWithdrawn { turn_id: testing::turn(2), origin: Origin::Shell });
    writer.append(batch).await.unwrap();
    assert_eq!(summary_of(&writer, id).await.last, 2);
}

#[tokio::test]
async fn a_delete_drops_the_rows() {
    let (writer, _thread, id) = with_prompt().await;
    writer
        .append(Batch::new().event(id, Event::ConversationDeleted { origin: Origin::Cli }))
        .await
        .unwrap();
    let rows = on_writer(&writer, |conn| Ok(testing::dump(conn, "attachments"))).await.unwrap();
    assert_eq!(rows, Vec::<String>::new());
}

#[tokio::test]
async fn a_conversation_with_no_attachment_has_an_empty_summary() {
    let (writer, _thread, _id) = with_prompt().await;
    assert_eq!(summary_of(&writer, testing::conversation(2)).await, Summary::default());
}
