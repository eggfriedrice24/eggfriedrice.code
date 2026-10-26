//! The JSON Schema documents of the whole protocol, built only with the `schema` feature.
//!
//! [`document`] is the reference: the method table with each method's scope, for
//! `cargo xtask protocol-docs`. [`codegen_document`] is the same types in the form that
//! a code generator reads, which `cargo xtask protocol-schema` writes to
//! `docs/protocol.schema.json` for the phone app. The schemas come from the `JsonSchema`
//! derives on the wire types; this module adds the frames, whose serde impls are written
//! by hand, and the method table.

use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde_json::{Map, Value, json};

use crate::{
    AdminConfigReload, AdminConfigReloadResult, AdminLoginApiKey, AdminLoginApiKeyResult,
    AdminLoginOpenAi, AdminLoginOpenAiItem, AdminLogout, AdminLogoutResult, AdminProjectAdd,
    AdminProjectAddResult, AdminProjectRemove, AdminProjectRemoveResult, AdminSandboxCheck,
    AdminSandboxCheckResult, AdminStatus, AdminStatusResult, ApprovalRespond,
    ApprovalRespondResult, CallBackground, CallBackgroundResult, ConversationCompact,
    ConversationCompactResult, ConversationContext, ConversationContextResult, ConversationDelete,
    ConversationDeleteResult, ConversationDiff, ConversationDiffResult, ConversationHistory,
    ConversationHistoryResult, ConversationResume, ConversationResumeResult, ConversationSearch,
    ConversationSearchResult, ConversationSubscribe, ConversationSubscribeItem, ConversationsList,
    ConversationsListResult, ConversationsRecent, ConversationsRecentResult, ConversationsSearch,
    ConversationsSearchResult, DeviceEnroll, DeviceEnrollResult, DeviceList, DeviceListResult,
    DevicePairConfirm, DevicePairConfirmResult, DevicePairStart, DevicePairStartItem, DeviceProve,
    DeviceProveResult, DeviceRevoke, DeviceRevokeResult, ErrorCode, ErrorFrame, Event,
    EventEnvelope, Hello, HelloResult, InputRespond, InputRespondResult, JobsList, JobsListResult,
    JobsStop, JobsStopResult, LeaseReport, LeaseReportResult, Method, ModelsList, ModelsListResult,
    PROTOCOL_VERSION, PresenceSet, PresenceSetResult, ProjectsList, ProjectsListResult, PromptSend,
    PromptSendResult, PromptWithdraw, PromptWithdrawResult, PtyAttach, PtyAttachItem, PtyResize,
    PtyResizeResult, PtyWrite, PtyWriteResult, RequestId, SandboxExplain, SandboxExplainResult,
    SandboxSurfaceRespond, SandboxSurfaceRespondResult, ScopeName, ShellInterrupt,
    ShellInterruptResult, ShellStatus, ShellStatusResult, TurnInterrupt, TurnInterruptResult,
    TurnSteer, TurnSteerResult, TurnUndo, TurnUndoResult,
};

/// The JSON Schema (draft 2020-12) document of the protocol.
///
/// Top-level members: `protocol` (the version), `frames` (`client` and `server`),
/// `methods` (one entry per method with its `name`, `scope`, `stream` flag, `params`
/// schema and `result` or `item` schema), `event` and `event_envelope`, `error_codes`,
/// `scopes`, and `$defs`, which every `$ref` in the document points into.
pub fn document() -> Value {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let methods: Vec<Value> = methods(&mut generator).into_iter().map(Entry::into_value).collect();
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

/// One method of the table: its wire name, its scope, whether it streams, and the
/// schemas of its params and of its result or item.
struct Entry {
    name: &'static str,
    scope: ScopeName,
    stream: bool,
    params: Schema,
    answer: Schema,
}

impl Entry {
    /// The entry of `document`'s `methods` list.
    fn into_value(self) -> Value {
        let answer = if self.stream { "item" } else { "result" };
        let mut entry = json!({
            "name": self.name,
            "scope": self.scope,
            "stream": self.stream,
            "params": self.params.to_value(),
        });
        entry[answer] = self.answer.to_value();
        entry
    }
}

/// Every method, in the order of the `Method` enum.
fn methods(generator: &mut SchemaGenerator) -> Vec<Entry> {
    vec![
        unary::<Hello, HelloResult>(generator, "hello", ScopeName::Read),
        unary::<ConversationsList, ConversationsListResult>(
            generator,
            "conversations.list",
            ScopeName::Read,
        ),
        stream::<ConversationSubscribe, ConversationSubscribeItem>(
            generator,
            "conversation.subscribe",
            ScopeName::Read,
        ),
        unary::<ConversationHistory, ConversationHistoryResult>(
            generator,
            "conversation.history",
            ScopeName::Read,
        ),
        unary::<PromptSend, PromptSendResult>(generator, "prompt.send", ScopeName::Operate),
        unary::<TurnInterrupt, TurnInterruptResult>(
            generator,
            "turn.interrupt",
            ScopeName::Operate,
        ),
        unary::<TurnSteer, TurnSteerResult>(generator, "turn.steer", ScopeName::Operate),
        unary::<ApprovalRespond, ApprovalRespondResult>(
            generator,
            "approval.respond",
            ScopeName::Approve,
        ),
        stream::<PtyAttach, PtyAttachItem>(generator, "pty.attach", ScopeName::Terminal),
        unary::<PtyWrite, PtyWriteResult>(generator, "pty.write", ScopeName::Terminal),
        unary::<PtyResize, PtyResizeResult>(generator, "pty.resize", ScopeName::Terminal),
        unary::<InputRespond, InputRespondResult>(generator, "input.respond", ScopeName::Terminal),
        unary::<LeaseReport, LeaseReportResult>(generator, "lease.report", ScopeName::Read),
        unary::<ModelsList, ModelsListResult>(generator, "models.list", ScopeName::Read),
        unary::<ProjectsList, ProjectsListResult>(generator, "projects.list", ScopeName::Read),
        unary::<AdminProjectAdd, AdminProjectAddResult>(
            generator,
            "admin.project_add",
            ScopeName::Admin,
        ),
        unary::<AdminProjectRemove, AdminProjectRemoveResult>(
            generator,
            "admin.project_remove",
            ScopeName::Admin,
        ),
        unary::<AdminStatus, AdminStatusResult>(generator, "admin.status", ScopeName::Admin),
        unary::<AdminConfigReload, AdminConfigReloadResult>(
            generator,
            "admin.config_reload",
            ScopeName::Admin,
        ),
        stream::<AdminLoginOpenAi, AdminLoginOpenAiItem>(
            generator,
            "admin.login_openai",
            ScopeName::Admin,
        ),
        unary::<AdminLoginApiKey, AdminLoginApiKeyResult>(
            generator,
            "admin.login_api_key",
            ScopeName::Admin,
        ),
        unary::<AdminLogout, AdminLogoutResult>(generator, "admin.logout", ScopeName::Admin),
        unary::<SandboxExplain, SandboxExplainResult>(
            generator,
            "sandbox.explain",
            ScopeName::Read,
        ),
        unary::<SandboxSurfaceRespond, SandboxSurfaceRespondResult>(
            generator,
            "sandbox.surface_respond",
            ScopeName::Approve,
        ),
        unary::<AdminSandboxCheck, AdminSandboxCheckResult>(
            generator,
            "admin.sandbox_check",
            ScopeName::Admin,
        ),
        unary::<ConversationDiff, ConversationDiffResult>(
            generator,
            "conversation.diff",
            ScopeName::Read,
        ),
        unary::<PromptWithdraw, PromptWithdrawResult>(
            generator,
            "prompt.withdraw",
            ScopeName::Operate,
        ),
        unary::<ConversationCompact, ConversationCompactResult>(
            generator,
            "conversation.compact",
            ScopeName::Operate,
        ),
        unary::<PresenceSet, PresenceSetResult>(generator, "presence.set", ScopeName::Operate),
        unary::<TurnUndo, TurnUndoResult>(generator, "turn.undo", ScopeName::Operate),
        unary::<JobsList, JobsListResult>(generator, "jobs.list", ScopeName::Read),
        unary::<JobsStop, JobsStopResult>(generator, "jobs.stop", ScopeName::Operate),
        unary::<ConversationContext, ConversationContextResult>(
            generator,
            "conversation.context",
            ScopeName::Read,
        ),
        unary::<ConversationsRecent, ConversationsRecentResult>(
            generator,
            "conversations.recent",
            ScopeName::Read,
        ),
        unary::<ConversationResume, ConversationResumeResult>(
            generator,
            "conversation.resume",
            ScopeName::Operate,
        ),
        unary::<ShellInterrupt, ShellInterruptResult>(
            generator,
            "shell.interrupt",
            ScopeName::Terminal,
        ),
        unary::<ShellStatus, ShellStatusResult>(generator, "shell.status", ScopeName::Read),
        unary::<CallBackground, CallBackgroundResult>(
            generator,
            "call.background",
            ScopeName::Terminal,
        ),
        unary::<ConversationsSearch, ConversationsSearchResult>(
            generator,
            "conversations.search",
            ScopeName::Read,
        ),
        unary::<ConversationSearch, ConversationSearchResult>(
            generator,
            "conversation.search",
            ScopeName::Read,
        ),
        unary::<ConversationDelete, ConversationDeleteResult>(
            generator,
            "conversation.delete",
            ScopeName::Terminal,
        ),
        stream::<DevicePairStart, DevicePairStartItem>(
            generator,
            "device.pair_start",
            ScopeName::Admin,
        ),
        unary::<DevicePairConfirm, DevicePairConfirmResult>(
            generator,
            "device.pair_confirm",
            ScopeName::Admin,
        ),
        unary::<DeviceEnroll, DeviceEnrollResult>(generator, "device.enroll", ScopeName::Read),
        unary::<DeviceProve, DeviceProveResult>(generator, "device.prove", ScopeName::Read),
        unary::<DeviceList, DeviceListResult>(generator, "device.list", ScopeName::Admin),
        unary::<DeviceRevoke, DeviceRevokeResult>(generator, "device.revoke", ScopeName::Admin),
    ]
}

fn unary<P: JsonSchema, R: JsonSchema>(
    generator: &mut SchemaGenerator,
    name: &'static str,
    scope: ScopeName,
) -> Entry {
    Entry {
        name,
        scope,
        stream: false,
        params: generator.subschema_for::<P>(),
        answer: generator.subschema_for::<R>(),
    }
}

fn stream<P: JsonSchema, I: JsonSchema>(
    generator: &mut SchemaGenerator,
    name: &'static str,
    scope: ScopeName,
) -> Entry {
    Entry {
        name,
        scope,
        stream: true,
        params: generator.subschema_for::<P>(),
        answer: generator.subschema_for::<I>(),
    }
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

/// The JSON Schema (draft 2020-12) of the protocol in the form that a code generator
/// such as json-schema-to-typescript reads: a root object whose members name the top
/// types, and `$defs`, which holds every type and every `$ref` target.
///
/// The top types in `$defs`, next to the wire types:
///
/// - `ProtocolVersion`: the constant [`PROTOCOL_VERSION`].
/// - `ClientFrame`: `ClientRequest` (`id` and one `Method`) or `ClientCancel`.
/// - `ServerFrame`: `ServerItemFrame`, `ServerEndFrame`, `ErrorFrame` or
///   `ServerAckFrame`.
/// - `MethodName` and `StreamMethodName`: the wire names of every method and of the
///   streaming ones.
/// - `MethodParams` and `MethodResults`: one member per wire name, with the params
///   type and with the result type (the item type for a streaming method).
///
/// An event of a kind that the file does not list must be accepted and skipped:
/// `Event` lists only the kinds of this build.
pub fn codegen_document() -> Value {
    let (mut definitions, tops) = codegen_parts();
    definitions.extend(tops);
    let top = |name: &str| json!({ "$ref": format!("#/$defs/{name}") });
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "EfrProtocol",
        "description": "The efr protocol, generated from efr-protocol by `cargo xtask \
                        protocol-schema`; never edit it by hand. Frames are JSON objects: \
                        length-prefixed on the Unix socket, one text message each on the \
                        WebSocket. Every offset into text counts bytes of UTF-8. An event \
                        of a kind not listed here must be accepted and skipped.",
        "type": "object",
        "properties": {
            "protocol": top("ProtocolVersion"),
            "client_frame": top("ClientFrame"),
            "server_frame": top("ServerFrame"),
            "method_name": top("MethodName"),
            "stream_method_name": top("StreamMethodName"),
            "method_params": top("MethodParams"),
            "method_results": top("MethodResults"),
            "event": top("Event"),
            "event_envelope": top("EventEnvelope"),
            "error_code": top("ErrorCode"),
            "scope_name": top("ScopeName"),
        },
        "additionalProperties": false,
        "$defs": definitions,
    })
}

/// The definitions of the wire types, and the top types that [`codegen_document`]
/// adds, apart, so a test can check that no top type hides a wire type.
pub(crate) fn codegen_parts() -> (Map<String, Value>, Map<String, Value>) {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let methods = methods(&mut generator);
    let method = generator.subschema_for::<Method>();
    // NOTE: only for the definitions that they add; the root names them by `$ref`.
    generator.subschema_for::<Event>();
    generator.subschema_for::<EventEnvelope>();
    generator.subschema_for::<ErrorCode>();
    generator.subschema_for::<ScopeName>();
    let id = generator.subschema_for::<RequestId>();
    let error = generator.subschema_for::<ErrorFrame>();
    let definitions = generator.take_definitions(true);

    let names: Vec<&str> = methods.iter().map(|entry| entry.name).collect();
    let streams: Vec<&str> =
        methods.iter().filter(|entry| entry.stream).map(|entry| entry.name).collect();
    let mut params = Map::new();
    let mut results = Map::new();
    for entry in methods {
        params.insert(entry.name.to_owned(), entry.params.to_value());
        results.insert(entry.name.to_owned(), entry.answer.to_value());
    }
    let tops = json!({
        "ProtocolVersion": {
            "description": "The protocol version that both sides send in `hello`.",
            "type": "integer",
            "const": PROTOCOL_VERSION,
        },
        "ClientRequest": {
            "description": "Start a request: `id` and a method with its params.",
            "allOf": [
                {
                    "type": "object",
                    "required": ["id"],
                    "properties": { "id": id },
                },
                method,
            ],
        },
        "ClientCancel": {
            "description": "Cancel a request in flight.",
            "type": "object",
            "required": ["cancel"],
            "properties": { "cancel": id },
            "additionalProperties": false,
        },
        "ClientFrame": {
            "description": "A frame from a client.",
            "oneOf": [
                { "$ref": "#/$defs/ClientRequest" },
                { "$ref": "#/$defs/ClientCancel" },
            ],
        },
        "ServerItemFrame": {
            "description": "A result, or one item of a stream: it follows the member of \
                            `MethodResults` for the method of the request.",
            "type": "object",
            "required": ["id", "item"],
            "properties": { "id": id, "item": {} },
            "additionalProperties": false,
        },
        "ServerEndFrame": {
            "description": "The request finished.",
            "type": "object",
            "required": ["id", "end"],
            "properties": { "id": id, "end": { "type": "boolean", "const": true } },
            "additionalProperties": false,
        },
        "ServerAckFrame": {
            "description": "The daemon accepted a streaming request.",
            "type": "object",
            "required": ["ack"],
            "properties": { "ack": id },
            "additionalProperties": false,
        },
        "ServerFrame": {
            "description": "Every request gets zero or more item frames and then one end or \
                            error frame.",
            "oneOf": [
                { "$ref": "#/$defs/ServerItemFrame" },
                { "$ref": "#/$defs/ServerEndFrame" },
                error,
                { "$ref": "#/$defs/ServerAckFrame" },
            ],
        },
        "MethodName": {
            "description": "The wire name of a method.",
            "type": "string",
            "enum": names,
        },
        "StreamMethodName": {
            "description": "The wire name of a method that streams: it may send many items.",
            "type": "string",
            "enum": streams,
        },
        "MethodParams": {
            "description": "The params type of each method, by wire name.",
            "type": "object",
            "properties": params,
            "required": names,
            "additionalProperties": false,
        },
        "MethodResults": {
            "description": "The result type of each method, by wire name; for a streaming \
                            method, the type of each item.",
            "type": "object",
            "properties": results,
            "required": names,
            "additionalProperties": false,
        },
    });
    let tops = match tops {
        Value::Object(tops) => tops,
        // NOTE: `json!` of an object literal is always an object.
        _ => Map::new(),
    };
    (definitions, tops)
}

#[cfg(test)]
mod tests;
