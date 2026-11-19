//! Attachments over a `TestDaemon`: the capability of `hello`, the uploads of
//! `attachment.put` and `attachment.discard`, and the images that the model reads with
//! `read_file`, as each provider sends them: an `input_image` in the output of a
//! function call for both OpenAI backends, an `image` block in a `tool_result` for
//! Anthropic. The blob outlives a restart, and a conversation delete removes it.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ApprovalRespondResult, AttachmentDiscard,
    AttachmentDiscardResult, AttachmentLimits, AttachmentOrigin, AttachmentPut,
    AttachmentPutResult, Base64Bytes, CHUNK_BYTES, ConversationDelete, ConversationDeleteResult,
    ConversationId, ErrorCode, Event, EventEnvelope, Method, PromptSendResult, UploadId,
};
use efr_test_daemon::{
    ClientError, MessagesAnswer, MessagesServer, ResponsesAnswer, ResponsesServer, TTY, TestDaemon,
    command_id, events_until,
};
use efr_test_support::attachments::PIXEL_PNG;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

mod uploads;

/// The code of the daemon's refusal of `result`.
fn refusal<T: std::fmt::Debug>(result: Result<T, ClientError>) -> ErrorCode {
    match result {
        Err(ClientError::Server { body }) => body.code,
        other => panic!("expected a refusal from the daemon, got {other:?}"),
    }
}

#[tokio::test]
async fn a_daemon_says_its_limits_and_with_attachments_off_refuses_an_upload() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let limits = AttachmentLimits {
        max_file_bytes: 20 * 1024 * 1024,
        max_per_message: 10,
        chunk_bytes: CHUNK_BYTES,
        max_edge_px: 2000,
    };
    assert_eq!(client.hello().capabilities.attachments, Some(limits));
    drop(client);
    daemon.stop().await.unwrap();

    let daemon = TestDaemon::builder()
        .config(|settings| settings.attachments.enabled = false)
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    assert_eq!(client.hello().capabilities.attachments, None);
    let upload_id = UploadId::from_uuid(uuid::Uuid::from_u128(1));
    let put = client
        .call::<AttachmentPutResult>(Method::AttachmentPut(AttachmentPut {
            upload_id,
            name: "shot.png".to_owned(),
            total_bytes: 4,
            offset: 0,
            data: Base64Bytes::new(*b"\x89PNG"),
            origin: Some(AttachmentOrigin::Clipboard),
        }))
        .await;
    assert_eq!(refusal(put), ErrorCode::Invalid);
    let discard = client
        .call::<AttachmentDiscardResult>(Method::AttachmentDiscard(AttachmentDiscard { upload_id }))
        .await;
    assert!(discard.is_ok(), "an unknown upload is no error: {discard:?}");

    drop(client);
    daemon.stop().await.unwrap();
}

/// The base64 text of [`PIXEL_PNG`].
fn pixel() -> String {
    STANDARD.encode(PIXEL_PNG)
}

/// The Claude model of the list, which reads images when `images`.
const CLAUDE: &str = "claude-opus-5-5";

fn claude(images: bool) -> Value {
    let mut model = MessagesServer::model(CLAUDE);
    model["capabilities"]["image_input"] = json!({"supported": images});
    model
}

/// Writes [`PIXEL_PNG`] as `shot.png` in the working directory of `daemon`.
fn plant(daemon: &TestDaemon) {
    std::fs::write(daemon.cwd().join("shot.png"), PIXEL_PNG).unwrap();
}

/// Sends prompt `n` and follows its turn to the end, answering every approval with
/// "yes": the turn's own events, and its conversation.
async fn run_turn(
    daemon: &TestDaemon,
    n: u128,
    text: &str,
) -> (Vec<EventEnvelope>, ConversationId) {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent: PromptSendResult = client.call(daemon.prompt(n, text, TTY)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let mut seen = Vec::new();
    let mut answers = 100 * n;
    loop {
        let events = events_until(&mut follow, |event| {
            matches!(
                event,
                Event::ApprovalRequested { turn_id, .. }
                    | Event::TurnCompleted { turn_id, .. }
                    | Event::TurnFailed { turn_id, .. }
                    if *turn_id == sent.turn_id
            )
        })
        .await
        .unwrap();
        let last = events.last().unwrap().event.clone();
        seen.extend(
            events.into_iter().filter(|envelope| envelope.event.turn_id() == Some(sent.turn_id)),
        );
        let Event::ApprovalRequested { call_id, .. } = last else { break };
        answers += 1;
        let answer = Method::ApprovalRespond(ApprovalRespond {
            command_id: command_id(answers),
            conversation_id: sent.conversation_id,
            call_id,
            decision: ApprovalDecision::Allow,
        });
        let _: ApprovalRespondResult = client.call(answer).await.unwrap();
    }
    (seen, sent.conversation_id)
}

/// The output of the first function call result in `body`, a Responses request.
fn function_output(body: &Value) -> Value {
    let input = body["input"].as_array().unwrap();
    let output = input.iter().find(|item| item["type"] == "function_call_output");
    output.map(|item| item["output"].clone()).unwrap_or(Value::Null)
}

/// The image items of a function call result's `output`.
fn images_in(output: &Value) -> Vec<Value> {
    output
        .as_array()
        .map(|parts| parts.iter().filter(|part| part["type"] == "input_image").cloned().collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn an_image_that_read_file_reads_reaches_both_openai_backends_as_an_input_image() {
    for subscription in [true, false] {
        let server = ResponsesServer::start().await;
        server.push(ResponsesAnswer::tool_call(
            "call_1",
            "read_file",
            &json!({"path": "shot.png"}),
        ));
        server.push(ResponsesAnswer::text("A dot."));
        server.push(ResponsesAnswer::text("Still a dot."));
        let builder = TestDaemon::builder();
        let builder =
            if subscription { builder.subscription(&server) } else { builder.responses(&server) };
        let daemon = builder.start().await.unwrap();
        plant(&daemon);

        let (events, _) = run_turn(&daemon, 1, "what is in shot.png").await;
        assert_eq!(events.last().unwrap().event.kind(), "turn_completed", "{events:#?}");
        let recorded = events.iter().find_map(|envelope| match &envelope.event {
            Event::ToolCallCompleted { attachments, .. } => Some(attachments.clone()),
            _ => None,
        });
        let recorded = recorded.unwrap();
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0].sent.is_some(), "{recorded:?}");

        let requests = server.received();
        let images = images_in(&function_output(&requests[1].body));
        let url = format!("data:image/png;base64,{}", pixel());
        assert_eq!(images, [json!({"type": "input_image", "image_url": url, "detail": "high"})]);

        run_turn(&daemon, 2, "and now?").await;
        let requests = server.received();
        assert_eq!(
            function_output(&requests[2].body),
            function_output(&requests[1].body),
            "a later turn sends the same bytes"
        );
        daemon.stop().await.unwrap();
    }
}

/// The `content` of the `tool_result` of the third message of a Messages body.
fn tool_result_content(body: &Value) -> Value {
    body["messages"][2]["content"][0]["content"].clone()
}

#[tokio::test]
async fn an_image_reaches_claude_as_an_image_block_and_survives_a_restart() {
    let server = MessagesServer::start().await;
    server.set_models(vec![claude(true)]);
    server.push(MessagesAnswer::tool_use("toolu_1", "read_file", &json!({"path": "shot.png"})));
    server.push(MessagesAnswer::text("A dot."));
    server.push(MessagesAnswer::text("Still a dot."));
    let mut daemon = TestDaemon::builder().messages(&server).persistent().start().await.unwrap();
    plant(&daemon);

    run_turn(&daemon, 1, "what is in shot.png").await;
    let first = tool_result_content(&server.received()[1].body);
    let blocks = first.as_array().unwrap();
    assert_eq!(
        blocks[2],
        json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": pixel()}})
    );
    assert_eq!(blocks[3], json!({"type": "text", "text": "</attachment>"}));

    daemon.restart().await.unwrap();
    run_turn(&daemon, 2, "and now?").await;
    let after = tool_result_content(&server.received()[2].body);
    assert_eq!(after, first, "the history after a restart sends the same image");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_model_that_reads_no_image_gets_the_label_and_a_delete_removes_the_blob() {
    let server = MessagesServer::start().await;
    server.set_models(vec![claude(false)]);
    server.push(MessagesAnswer::tool_use("toolu_1", "read_file", &json!({"path": "shot.png"})));
    server.push(MessagesAnswer::text("I cannot see it."));
    let daemon = TestDaemon::builder().messages(&server).start().await.unwrap();
    plant(&daemon);

    let (_, conversation_id) = run_turn(&daemon, 1, "what is in shot.png").await;
    let content = tool_result_content(&server.received()[1].body);
    let texts: Vec<&str> =
        content.as_array().unwrap().iter().filter_map(|block| block["text"].as_str()).collect();
    assert_eq!(texts.len(), 2, "{content:#}");
    assert!(
        texts[1].ends_with("[efr: not sent as content. Open the file at the path.]\n</attachment>"),
        "{}",
        texts[1]
    );
    assert!(!content.to_string().contains("\"image\""));

    let blobs = daemon.dirs().dirs().data().join("attachments").join(conversation_id.to_string());
    assert!(blobs.exists(), "the blob waits for a model that reads images");
    let client = daemon.client().await.unwrap();
    let delete = Method::ConversationDelete(ConversationDelete {
        command_id: command_id(9000),
        conversation_id,
        force: true,
        stop: false,
    });
    let result: ConversationDeleteResult = client.call(delete).await.unwrap();
    assert!(matches!(result, ConversationDeleteResult::Deleted { .. }), "{result:?}");
    daemon.wait_deletes_ended().await;
    assert!(!blobs.exists());
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn with_attachments_off_read_file_refuses_an_image_as_a_binary_file() {
    let server = ResponsesServer::start().await;
    server.push(ResponsesAnswer::tool_call("call_1", "read_file", &json!({"path": "shot.png"})));
    server.push(ResponsesAnswer::text("No."));
    let daemon = TestDaemon::builder()
        .config(|settings| settings.attachments.enabled = false)
        .responses(&server)
        .start()
        .await
        .unwrap();
    plant(&daemon);

    run_turn(&daemon, 1, "what is in shot.png").await;
    let output = function_output(&server.received()[1].body);
    let text = output.as_str().unwrap();
    let path = daemon.cwd().join("shot.png");
    assert_eq!(text, format!("Error: {} is not a text file", path.display()));
    assert!(!daemon.dirs().dirs().data().join("attachments").exists());
    daemon.stop().await.unwrap();
}
