//! The JSON Schema document of the whole protocol, built only with the `schema` feature.
//!
//! `cargo xtask protocol-docs` renders [`document`] into `docs/protocol.md`. The schemas
//! come from the `JsonSchema` derives on the wire types; this module adds the frames,
//! whose serde impls are written by hand, and the method table.

use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde_json::{Value, json};

use crate::{
    AdminConfigReload, AdminConfigReloadResult, AdminLoginOpenAi, AdminLoginOpenAiItem,
    AdminStatus, AdminStatusResult, ApprovalRespond, ApprovalRespondResult, ConversationHistory,
    ConversationHistoryResult, ConversationSubscribe, ConversationSubscribeItem, ConversationsList,
    ConversationsListResult, ErrorCode, ErrorFrame, Event, EventEnvelope, Hello, HelloResult,
    InputRespond, InputRespondResult, LeaseReport, LeaseReportResult, ModelsList, ModelsListResult,
    PROTOCOL_VERSION, PromptSend, PromptSendResult, PtyAttach, PtyAttachItem, PtyResize,
    PtyResizeResult, PtyWrite, PtyWriteResult, RequestId, ScopeName, TurnInterrupt,
    TurnInterruptResult, TurnSteer, TurnSteerResult,
};

/// The JSON Schema (draft 2020-12) document of the protocol.
///
/// Top-level members: `protocol` (the version), `frames` (`client` and `server`),
/// `methods` (one entry per method with its `name`, `scope`, `stream` flag, `params`
/// schema and `result` or `item` schema), `event` and `event_envelope`, `error_codes`,
/// `scopes`, and `$defs`, which every `$ref` in the document points into.
pub fn document() -> Value {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let methods = vec![
        unary::<Hello, HelloResult>(&mut generator, "hello", ScopeName::Read),
        unary::<ConversationsList, ConversationsListResult>(
            &mut generator,
            "conversations.list",
            ScopeName::Read,
        ),
        stream::<ConversationSubscribe, ConversationSubscribeItem>(
            &mut generator,
            "conversation.subscribe",
            ScopeName::Read,
        ),
        unary::<ConversationHistory, ConversationHistoryResult>(
            &mut generator,
            "conversation.history",
            ScopeName::Read,
        ),
        unary::<PromptSend, PromptSendResult>(&mut generator, "prompt.send", ScopeName::Operate),
        unary::<TurnInterrupt, TurnInterruptResult>(
            &mut generator,
            "turn.interrupt",
            ScopeName::Operate,
        ),
        unary::<TurnSteer, TurnSteerResult>(&mut generator, "turn.steer", ScopeName::Operate),
        unary::<ApprovalRespond, ApprovalRespondResult>(
            &mut generator,
            "approval.respond",
            ScopeName::Approve,
        ),
        stream::<PtyAttach, PtyAttachItem>(&mut generator, "pty.attach", ScopeName::Terminal),
        unary::<PtyWrite, PtyWriteResult>(&mut generator, "pty.write", ScopeName::Terminal),
        unary::<PtyResize, PtyResizeResult>(&mut generator, "pty.resize", ScopeName::Terminal),
        unary::<InputRespond, InputRespondResult>(
            &mut generator,
            "input.respond",
            ScopeName::Terminal,
        ),
        unary::<LeaseReport, LeaseReportResult>(&mut generator, "lease.report", ScopeName::Read),
        unary::<ModelsList, ModelsListResult>(&mut generator, "models.list", ScopeName::Read),
        unary::<AdminStatus, AdminStatusResult>(&mut generator, "admin.status", ScopeName::Admin),
        unary::<AdminConfigReload, AdminConfigReloadResult>(
            &mut generator,
            "admin.config_reload",
            ScopeName::Admin,
        ),
        stream::<AdminLoginOpenAi, AdminLoginOpenAiItem>(
            &mut generator,
            "admin.login_openai",
            ScopeName::Admin,
        ),
    ];
    let frames = json!({
        "client": client_frame(&mut generator),
        "server": server_frame(&mut generator),
    });
    let event = generator.subschema_for::<Event>().to_value();
    let event_envelope = generator.subschema_for::<EventEnvelope>().to_value();
    let definitions = generator.take_definitions(true);
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "efr protocol",
        "description": "Frames are length-prefixed JSON objects on the Unix socket. An event of \
                        a kind not listed here must be accepted and skipped.",
        "protocol": PROTOCOL_VERSION,
        "frames": frames,
        "methods": methods,
        "event": event,
        "event_envelope": event_envelope,
        "error_codes": ErrorCode::ALL,
        "scopes": ScopeName::ALL,
        "$defs": definitions,
    })
}

fn unary<P: JsonSchema, R: JsonSchema>(
    generator: &mut SchemaGenerator,
    name: &str,
    scope: ScopeName,
) -> Value {
    json!({
        "name": name,
        "scope": scope,
        "stream": false,
        "params": generator.subschema_for::<P>().to_value(),
        "result": generator.subschema_for::<R>().to_value(),
    })
}

fn stream<P: JsonSchema, I: JsonSchema>(
    generator: &mut SchemaGenerator,
    name: &str,
    scope: ScopeName,
) -> Value {
    json!({
        "name": name,
        "scope": scope,
        "stream": true,
        "params": generator.subschema_for::<P>().to_value(),
        "item": generator.subschema_for::<I>().to_value(),
    })
}

fn client_frame(generator: &mut SchemaGenerator) -> Schema {
    let id = generator.subschema_for::<RequestId>();
    json_schema!({
        "oneOf": [
            {
                "description": "Start a request. `method` names an entry of `methods` and \
                                `params` follows its `params` schema.",
                "type": "object",
                "required": ["id", "method", "params"],
                "properties": {
                    "id": id,
                    "method": { "type": "string" },
                    "params": { "type": "object" },
                },
            },
            {
                "description": "Cancel a request in flight.",
                "type": "object",
                "required": ["cancel"],
                "properties": { "cancel": id },
            },
        ],
    })
}

fn server_frame(generator: &mut SchemaGenerator) -> Schema {
    let id = generator.subschema_for::<RequestId>();
    let error = generator.subschema_for::<ErrorFrame>();
    json_schema!({
        "description": "Every request gets zero or more item frames and then one end or \
                        error frame.",
        "oneOf": [
            {
                "description": "A result, or one item of a stream; it follows the method's \
                                `result` or `item` schema.",
                "type": "object",
                "required": ["id", "item"],
                "properties": { "id": id, "item": true },
            },
            {
                "description": "The request finished.",
                "type": "object",
                "required": ["id", "end"],
                "properties": { "id": id, "end": { "const": true } },
            },
            error,
            {
                "description": "The daemon accepted a streaming request.",
                "type": "object",
                "required": ["ack"],
                "properties": { "ack": id },
            },
        ],
    })
}

#[cfg(test)]
mod tests;
