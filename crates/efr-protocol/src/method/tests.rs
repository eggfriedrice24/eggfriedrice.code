use std::str::FromStr;

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    AdminConfigReload, AdminLoginApiKey, AdminLoginOpenAi, AdminLogout, AdminProjectAdd,
    AdminProjectRemove, AdminSandboxCheck, AdminStatus, ApprovalDecision, ApprovalRespond,
    Base64Bytes, CallId, Capabilities, CommandId, ConversationCompact, ConversationContext,
    ConversationDiff, ConversationHistory, ConversationId, ConversationResume,
    ConversationSubscribe, ConversationsList, ConversationsRecent, Hello, InputRespond, JobId,
    JobsList, JobsStop, LeaseReport, Method, ModelsList, Origin, Presence, PresenceSet,
    ProjectsList, PromptSend, PromptWithdraw, PtyAttach, PtyId, PtyResize, PtyWrite, QuestionId,
    SandboxExplain, SandboxSurfaceRespond, ScopeName, SecretText, ShellContext, Size,
    TurnInterrupt, TurnSettings, TurnSteer, TurnUndo, WithdrawTarget,
};

const COMMAND: &str = "01928c4e-7a3b-7c1d-8e2f-00000000000c";
const CONVERSATION: &str = "01928c4e-7a3b-7c1d-8e2f-000000000002";
const CALL: &str = "01928c4e-7a3b-7c1d-8e2f-000000000004";
const PTY: &str = "01928c4e-7a3b-7c1d-8e2f-000000000003";

fn command() -> CommandId {
    CommandId::from_str(COMMAND).unwrap()
}

fn conversation() -> ConversationId {
    ConversationId::from_str(CONVERSATION).unwrap()
}

fn pty() -> PtyId {
    PtyId::from_str(PTY).unwrap()
}

/// One row per method: the request, its wire name, the scope it needs, whether it is a
/// write with a receipt, and whether it streams. This is the decision table the daemon
/// relies on.
fn table() -> Vec<(Method, &'static str, ScopeName, bool, bool)> {
    vec![
        (
            Method::Hello(Hello {
                protocol: 1,
                origin: Origin::Cli,
                client: None,
                capabilities: Capabilities::default(),
                tty: None,
                pid: None,
                device_id: None,
            }),
            "hello",
            ScopeName::Read,
            false,
            false,
        ),
        (
            Method::ConversationsList(ConversationsList::default()),
            "conversations.list",
            ScopeName::Read,
            false,
            false,
        ),
        (
            Method::ConversationSubscribe(ConversationSubscribe {
                conversation_id: conversation(),
                after_seq: None,
                answers_input: false,
                drafts: false,
            }),
            "conversation.subscribe",
            ScopeName::Read,
            false,
            true,
        ),
        (
            Method::ConversationHistory(ConversationHistory {
                conversation_id: conversation(),
                cursor: None,
                limit: None,
            }),
            "conversation.history",
            ScopeName::Read,
            false,
            false,
        ),
        (
            Method::PromptSend(PromptSend {
                command_id: command(),
                conversation_id: None,
                new_conversation: false,
                text: "why is the disk full".to_owned(),
                context: None,
                last_command: None,
                recent_commands: Vec::new(),
                settings: TurnSettings::default(),
            }),
            "prompt.send",
            ScopeName::Operate,
            true,
            false,
        ),
        (
            Method::TurnInterrupt(TurnInterrupt {
                command_id: command(),
                conversation_id: conversation(),
                turn_id: None,
                resend_steers: Vec::new(),
                resend_as: None,
                withdraw_steers: Vec::new(),
                withdraw: Vec::new(),
            }),
            "turn.interrupt",
            ScopeName::Operate,
            true,
            false,
        ),
        (
            Method::TurnSteer(TurnSteer {
                command_id: command(),
                conversation_id: conversation(),
                turn_id: None,
                text: "use journalctl".to_owned(),
                if_late: None,
            }),
            "turn.steer",
            ScopeName::Operate,
            true,
            false,
        ),
        (
            Method::ApprovalRespond(ApprovalRespond {
                command_id: command(),
                conversation_id: conversation(),
                call_id: CallId::from_str(CALL).unwrap(),
                decision: ApprovalDecision::Allow,
            }),
            "approval.respond",
            ScopeName::Approve,
            true,
            false,
        ),
        (
            Method::PtyAttach(PtyAttach { pty_id: pty(), since_seq: None, scrollback_rows: None }),
            "pty.attach",
            ScopeName::Terminal,
            false,
            true,
        ),
        (
            Method::PtyWrite(PtyWrite { pty_id: pty(), data: Base64Bytes::new(b"q".to_vec()) }),
            "pty.write",
            ScopeName::Terminal,
            false,
            false,
        ),
        (
            Method::PtyResize(PtyResize { pty_id: pty(), size: Size { cols: 80, rows: 24 } }),
            "pty.resize",
            ScopeName::Terminal,
            false,
            false,
        ),
        (
            Method::InputRespond(InputRespond {
                conversation_id: conversation(),
                call_id: CallId::from_str(CALL).unwrap(),
                text: SecretText::new("y"),
                hidden: false,
                manual: false,
            }),
            "input.respond",
            ScopeName::Terminal,
            false,
            false,
        ),
        (
            Method::LeaseReport(LeaseReport::default()),
            "lease.report",
            ScopeName::Read,
            false,
            false,
        ),
        (Method::ModelsList(ModelsList::default()), "models.list", ScopeName::Read, false, false),
        (Method::ProjectsList(ProjectsList {}), "projects.list", ScopeName::Read, false, false),
        (
            Method::AdminProjectAdd(AdminProjectAdd {
                path: "/p/app".into(),
                name: None,
                git_root: false,
            }),
            "admin.project_add",
            ScopeName::Admin,
            false,
            false,
        ),
        (
            Method::AdminProjectRemove(AdminProjectRemove { path: "/p/app".into() }),
            "admin.project_remove",
            ScopeName::Admin,
            false,
            false,
        ),
        (Method::AdminStatus(AdminStatus {}), "admin.status", ScopeName::Admin, false, false),
        (
            Method::AdminConfigReload(AdminConfigReload {}),
            "admin.config_reload",
            ScopeName::Admin,
            false,
            false,
        ),
        (
            Method::AdminLoginOpenAi(AdminLoginOpenAi {}),
            "admin.login_openai",
            ScopeName::Admin,
            false,
            true,
        ),
        (
            Method::AdminLoginApiKey(AdminLoginApiKey {
                provider: "openai-api".into(),
                key: SecretText::new("sk-proj-test"),
                check: true,
            }),
            "admin.login_api_key",
            ScopeName::Admin,
            false,
            false,
        ),
        (
            Method::AdminLogout(AdminLogout { provider: "openai-api".into() }),
            "admin.logout",
            ScopeName::Admin,
            false,
            false,
        ),
        (
            Method::SandboxExplain(SandboxExplain { path: "/home/u/.zshrc".into(), cwd: None }),
            "sandbox.explain",
            ScopeName::Read,
            false,
            false,
        ),
        (
            Method::SandboxSurfaceRespond(SandboxSurfaceRespond {
                command_id: command(),
                conversation_id: conversation(),
                question_id: QuestionId::from_str(CALL).unwrap(),
                keep: true,
            }),
            "sandbox.surface_respond",
            ScopeName::Approve,
            true,
            false,
        ),
        (
            Method::AdminSandboxCheck(AdminSandboxCheck {}),
            "admin.sandbox_check",
            ScopeName::Admin,
            false,
            false,
        ),
        (
            Method::ConversationDiff(ConversationDiff::default()),
            "conversation.diff",
            ScopeName::Read,
            false,
            false,
        ),
        (
            Method::PromptWithdraw(PromptWithdraw {
                command_id: command(),
                conversation_id: conversation(),
                target: WithdrawTarget::NewestFromTty { tty: "/dev/pts/3".to_owned() },
            }),
            "prompt.withdraw",
            ScopeName::Operate,
            true,
            false,
        ),
        (
            Method::ConversationCompact(ConversationCompact {
                command_id: command(),
                conversation_id: conversation(),
                focus: None,
            }),
            "conversation.compact",
            ScopeName::Operate,
            true,
            false,
        ),
        (
            Method::PresenceSet(PresenceSet {
                command_id: command(),
                conversation_id: None,
                presence: Presence::Away,
            }),
            "presence.set",
            ScopeName::Operate,
            true,
            false,
        ),
        (
            Method::TurnUndo(TurnUndo {
                command_id: command(),
                conversation_id: None,
                turn_id: None,
                dry_run: true,
                force: false,
                redo: false,
                list: false,
            }),
            "turn.undo",
            ScopeName::Operate,
            true,
            false,
        ),
        (
            Method::JobsList(JobsList { conversation_id: None }),
            "jobs.list",
            ScopeName::Read,
            false,
            false,
        ),
        (
            Method::JobsStop(JobsStop {
                command_id: command(),
                conversation_id: Some(conversation()),
                job_id: JobId::nth(3),
            }),
            "jobs.stop",
            ScopeName::Operate,
            true,
            false,
        ),
        (
            Method::ConversationContext(ConversationContext {
                conversation_id: conversation(),
                model: None,
            }),
            "conversation.context",
            ScopeName::Read,
            false,
            false,
        ),
        (
            Method::ConversationsRecent(ConversationsRecent::default()),
            "conversations.recent",
            ScopeName::Read,
            false,
            false,
        ),
        (
            Method::ConversationResume(ConversationResume {
                command_id: command(),
                conversation_id: conversation(),
                context: ShellContext::new("/home/user/p/app"),
                take: false,
            }),
            "conversation.resume",
            ScopeName::Operate,
            true,
            false,
        ),
    ]
}

#[test]
fn jobs_stop_needs_operate_scope() {
    let stop = Method::JobsStop(JobsStop {
        command_id: command(),
        conversation_id: None,
        job_id: JobId::nth(1),
    });
    assert_eq!(ScopeName::for_method(&stop), ScopeName::Operate);
    assert_eq!(stop.command_id(), Some(command()));
    let wire = serde_json::to_value(&stop).unwrap();
    assert_eq!(
        wire,
        json!({ "method": "jobs.stop", "params": { "command_id": COMMAND, "job_id": "j1" } })
    );
    let bad = json!({ "method": "jobs.stop", "params": { "command_id": COMMAND, "job_id": "3" } });
    assert!(serde_json::from_value::<Method>(bad).is_err(), "a job id is j and a number");
}

#[test]
fn jobs_list_needs_read_scope() {
    let list = Method::JobsList(JobsList { conversation_id: None });
    assert_eq!(ScopeName::for_method(&list), ScopeName::Read);
    assert_eq!(list.command_id(), None);
    assert!(!list.is_stream());
    let wire = serde_json::to_value(&list).unwrap();
    assert_eq!(wire, json!({ "method": "jobs.list", "params": {} }));
}

#[test]
fn conversation_context_needs_read_scope() {
    let context = Method::ConversationContext(ConversationContext {
        conversation_id: conversation(),
        model: None,
    });
    assert_eq!(ScopeName::for_method(&context), ScopeName::Read);
    assert_eq!(context.command_id(), None);
    assert!(!context.is_stream());
    let wire = serde_json::to_value(&context).unwrap();
    assert_eq!(
        wire,
        json!({ "method": "conversation.context", "params": { "conversation_id": CONVERSATION } })
    );
}

#[test]
fn every_method_needs_the_scope_in_the_table() {
    for (method, name, scope, _, _) in table() {
        assert_eq!(ScopeName::for_method(&method), scope, "{name}");
    }
}

#[test]
fn only_admin_methods_need_the_admin_scope() {
    for (method, name, _, _, _) in table() {
        let is_admin = ScopeName::for_method(&method) == ScopeName::Admin;
        assert_eq!(is_admin, name.starts_with("admin."), "{name}");
    }
}

#[test]
fn every_name_is_the_wire_tag() {
    for (method, name, _, _, _) in table() {
        assert_eq!(method.name(), name);
        assert_eq!(serde_json::to_value(&method).unwrap()["method"], json!(name));
    }
}

#[test]
fn writes_carry_their_command_id() {
    for (method, name, _, is_write, _) in table() {
        let expected = is_write.then(command);
        assert_eq!(method.command_id(), expected, "{name}");
    }
}

#[test]
fn streaming_methods_are_marked() {
    for (method, name, _, _, streams) in table() {
        assert_eq!(method.is_stream(), streams, "{name}");
    }
}

#[test]
fn every_method_reads_back_as_itself() {
    for (method, name, _, _, _) in table() {
        let back: Method = serde_json::from_value(serde_json::to_value(&method).unwrap()).unwrap();
        assert_eq!(back, method, "{name}");
    }
}

#[test]
fn params_are_an_object_even_when_empty() {
    assert_eq!(
        serde_json::to_value(Method::AdminStatus(AdminStatus {})).unwrap(),
        json!({ "method": "admin.status", "params": {} })
    );
}

#[test]
fn other_members_of_the_request_frame_are_ignored() {
    let method: Method = serde_json::from_value(
        json!({ "id": 9, "method": "admin.status", "params": {}, "trace": "x" }),
    )
    .unwrap();
    assert_eq!(method, Method::AdminStatus(AdminStatus {}));
}

#[test]
fn an_unknown_method_is_rejected_by_name() {
    let err = serde_json::from_value::<Method>(json!({ "method": "shell.exec", "params": {} }))
        .unwrap_err();
    assert!(err.to_string().contains("shell.exec"), "{err}");
}

#[test]
fn params_with_the_wrong_shape_are_rejected() {
    let err = serde_json::from_value::<Method>(
        json!({ "method": "pty.resize", "params": { "pty_id": PTY } }),
    )
    .unwrap_err();
    assert!(err.to_string().contains("size"), "{err}");
}

#[test]
fn an_api_key_login_keeps_the_key_out_of_debug_and_checks_unless_told_not_to() {
    let params = json!({
        "method": "admin.login_api_key",
        "params": { "provider": "anthropic-api", "key": "sk-ant-api03-secret-a1b2" },
    });
    let method: Method = serde_json::from_value(params).unwrap();
    let Method::AdminLoginApiKey(login) = &method else { panic!("{}", method.name()) };
    assert!(login.check, "a login without check checks the key");
    assert_eq!(login.key.expose_secret(), "sk-ant-api03-secret-a1b2");
    let debug = format!("{method:?}");
    assert!(!debug.contains("secret"), "{debug}");
    assert!(debug.contains("anthropic-api"), "{debug}");
}
