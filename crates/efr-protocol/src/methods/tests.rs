use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    AdminConfigReloadResult, AdminProjectAdd, AdminProjectRemove, AdminStatusResult, Base64Bytes,
    ConfigFileError, ConfigStatus, ConversationId, ConversationSubscribe,
    ConversationSubscribeItem, Draft, DraftPart, EffectiveSettings, InputRespond, LateSteer,
    LoginKind, Mode, ModelInfo, ModelSource, ModelsListResult, OverriddenSettings, PageCursor,
    ProjectInfo, PromptSend, PromptSendResult, PromptWithdraw, ProviderStatus, RootSource, Seq,
    ShellContext, TurnInterrupt, TurnInterruptResult, TurnSettings, TurnSteer, TurnSteerResult,
    WithdrawTarget,
};

const CONVERSATION: &str = "019a9b1c-3d00-7a10-8b20-000000000001";
const TURN: &str = "019a9b1c-3d00-7a10-8b20-000000000002";
const COMMAND: &str = "019a9b1c-3d00-7a10-8b20-000000000003";

#[test]
fn bytes_are_written_as_standard_base64_with_padding() {
    let bytes = Base64Bytes::new(b"ls\r".to_vec());
    assert_eq!(serde_json::to_value(&bytes).unwrap(), json!("bHMN"));
    assert_eq!(serde_json::to_value(Base64Bytes::new(b"a".to_vec())).unwrap(), json!("YQ=="));
}

#[test]
fn bytes_that_are_not_utf8_survive_a_round_trip() {
    let bytes = Base64Bytes::new(vec![0x1b, 0xff, 0x00, 0xe2, 0x82]);
    let back: Base64Bytes = serde_json::from_value(serde_json::to_value(&bytes).unwrap()).unwrap();
    assert_eq!(back.as_bytes(), [0x1b, 0xff, 0x00, 0xe2, 0x82]);
    assert_eq!(back.into_bytes(), vec![0x1b, 0xff, 0x00, 0xe2, 0x82]);
}

#[test]
fn empty_bytes_are_an_empty_string() {
    assert_eq!(serde_json::to_value(Base64Bytes::default()).unwrap(), json!(""));
}

#[test]
fn invalid_base64_is_rejected() {
    assert!(serde_json::from_value::<Base64Bytes>(json!("not base64!")).is_err());
    assert!(serde_json::from_value::<Base64Bytes>(json!([1, 2])).is_err());
}

#[test]
fn debug_shows_the_length_and_not_the_bytes() {
    let text = format!("{:?}", Base64Bytes::new(b"hunter2\r".to_vec()));
    assert_eq!(text, "Base64Bytes(<8 bytes>)");
}

#[test]
fn a_page_cursor_is_a_plain_string() {
    let cursor = PageCursor::new("c:41");
    assert_eq!(serde_json::to_value(&cursor).unwrap(), json!("c:41"));
    assert_eq!(cursor.as_str(), "c:41");
    let back: PageCursor = serde_json::from_value(json!("c:41")).unwrap();
    assert_eq!(back, cursor);
}

#[test]
fn a_subscribe_from_before_answers_input_still_parses_as_one_that_cannot_answer() {
    // The params of conversation.subscribe as clients sent them before answers_input.
    let old = r#"{"conversation_id":"019a9b1c-3d00-7a10-8b20-000000000001","after_seq":40}"#;
    let params: ConversationSubscribe = serde_json::from_str(old).unwrap();
    assert_eq!(params.conversation_id, CONVERSATION.parse::<ConversationId>().unwrap());
    assert_eq!(params.after_seq, Some(Seq::new(40)));
    assert!(!params.answers_input);
}

#[test]
fn answers_input_is_written_only_when_true() {
    let mut params = ConversationSubscribe {
        conversation_id: CONVERSATION.parse().unwrap(),
        after_seq: None,
        answers_input: false,
        drafts: false,
    };
    assert_eq!(serde_json::to_value(&params).unwrap(), json!({ "conversation_id": CONVERSATION }));
    params.answers_input = true;
    let value = serde_json::to_value(&params).unwrap();
    assert_eq!(value, json!({ "conversation_id": CONVERSATION, "answers_input": true }));
    let back: ConversationSubscribe = serde_json::from_value(value).unwrap();
    assert_eq!(back, params);
}

#[test]
fn drafts_are_off_when_absent_and_written_only_when_on() {
    let old = json!({ "conversation_id": CONVERSATION, "answers_input": true });
    let params: ConversationSubscribe = serde_json::from_value(old).unwrap();
    assert!(!params.drafts, "an older client gets no drafts");

    let mut params = ConversationSubscribe {
        conversation_id: CONVERSATION.parse().unwrap(),
        after_seq: None,
        answers_input: false,
        drafts: false,
    };
    assert_eq!(serde_json::to_value(&params).unwrap(), json!({ "conversation_id": CONVERSATION }));
    params.drafts = true;
    let value = serde_json::to_value(&params).unwrap();
    assert_eq!(value, json!({ "conversation_id": CONVERSATION, "drafts": true }));
    let back: ConversationSubscribe = serde_json::from_value(value).unwrap();
    assert_eq!(back, params);
}

#[test]
fn a_draft_item_has_the_wire_form_of_the_spec() {
    let item = ConversationSubscribeItem::Draft(Draft {
        turn_id: TURN.parse().unwrap(),
        after_seq: Seq::new(1234),
        draft: DraftPart::Text { index: 0, offset: 12, delta: "takes 3.1 GiB".to_owned() },
    });
    let value = serde_json::to_value(&item).unwrap();
    assert_eq!(
        value,
        json!({
            "kind": "draft",
            "turn_id": TURN,
            "after_seq": 1234,
            "draft": { "kind": "text", "index": 0, "offset": 12, "delta": "takes 3.1 GiB" },
        })
    );
    let back: ConversationSubscribeItem = serde_json::from_value(value).unwrap();
    assert_eq!(back, item);
}

#[test]
fn a_reasoning_draft_without_a_title_leaves_it_out() {
    let part = DraftPart::Reasoning { offset: 0, delta: "Look".to_owned(), title: None };
    let value = serde_json::to_value(&part).unwrap();
    assert_eq!(value, json!({ "kind": "reasoning", "offset": 0, "delta": "Look" }));
    assert_eq!(serde_json::from_value::<DraftPart>(value).unwrap(), part);
}

#[test]
fn answers_input_must_be_a_boolean() {
    let params = json!({ "conversation_id": CONVERSATION, "answers_input": "yes" });
    assert!(serde_json::from_value::<ConversationSubscribe>(params).is_err());
}

#[test]
fn the_longest_answer_is_frozen_with_the_wire_contract() {
    // Clients cap the line they read at this length, so a change is a protocol change.
    assert_eq!(InputRespond::MAX_TEXT_BYTES, 1024);
}

#[test]
fn a_prompt_from_before_turn_settings_parses_as_one_that_asks_for_none() {
    let old = json!({ "command_id": COMMAND, "text": "why" });
    let params: PromptSend = serde_json::from_value(old.clone()).unwrap();
    assert!(params.settings.is_empty());
    assert_eq!(serde_json::to_value(&params).unwrap(), old, "no settings are written");
}

#[test]
fn a_prompt_carries_the_settings_it_asks_for() {
    let mut params: PromptSend =
        serde_json::from_value(json!({ "command_id": COMMAND, "text": "why" })).unwrap();
    params.settings = TurnSettings { mode: Some(Mode::Manual), ..TurnSettings::default() };
    let value = serde_json::to_value(&params).unwrap();
    assert_eq!(value["settings"], json!({ "mode": "manual" }));
    let back: PromptSend = serde_json::from_value(value).unwrap();
    assert_eq!(back, params);
    assert!(format!("{params:?}").contains("Manual"), "{params:?}");
}

#[test]
fn a_prompt_with_an_unknown_mode_is_rejected() {
    let params = json!({ "command_id": COMMAND, "text": "why", "settings": { "mode": "yolo" } });
    assert!(serde_json::from_value::<PromptSend>(params).is_err());
}

#[test]
fn a_prompt_send_result_from_before_turn_settings_parses_without_settings() {
    let old = json!({ "conversation_id": CONVERSATION, "turn_id": TURN, "seq": 3, "queued": true });
    let result: PromptSendResult = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(result.settings, None);
    assert_eq!(serde_json::to_value(&result).unwrap(), old);
}

#[test]
fn a_prompt_send_result_reports_the_settings_of_its_turn() {
    let result = PromptSendResult {
        conversation_id: CONVERSATION.parse().unwrap(),
        turn_id: TURN.parse().unwrap(),
        seq: Seq::new(3),
        queued: false,
        settings: Some(EffectiveSettings {
            mode: Mode::Auto,
            model: "gpt-5.4".to_owned(),
            effort: None,
            overridden: OverriddenSettings { model: true, ..OverriddenSettings::default() },
            fallback: None,
        }),
    };
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(
        value["settings"],
        json!({ "mode": "auto", "model": "gpt-5.4", "overridden": { "model": true } })
    );
    let back: PromptSendResult = serde_json::from_value(value).unwrap();
    assert_eq!(back, result);
}

#[test]
fn a_model_without_efforts_or_default_is_its_id_and_source() {
    let model = ModelInfo {
        id: "gpt-5.5-preview".to_owned(),
        efforts: Vec::new(),
        default_effort: None,
        default: false,
        source: ModelSource::Config,
        context_window: None,
        max_context_window: None,
        prefer_websockets: false,
    };
    let value = serde_json::to_value(&model).unwrap();
    assert_eq!(value, json!({ "id": "gpt-5.5-preview", "source": "config" }));
    let back: ModelInfo = serde_json::from_value(value).unwrap();
    assert_eq!(back, model);
}

#[test]
fn the_default_model_and_its_efforts_are_written() {
    let result = ModelsListResult {
        models: vec![ModelInfo {
            id: "gpt-5.5".to_owned(),
            efforts: vec!["low".to_owned(), "high".to_owned()],
            default_effort: Some("low".to_owned()),
            default: true,
            source: ModelSource::Builtin,
            context_window: None,
            max_context_window: None,
            prefer_websockets: false,
        }],
        catalog: None,
    };
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({ "models": [{
            "id": "gpt-5.5",
            "efforts": ["low", "high"],
            "default_effort": "low",
            "default": true,
            "source": "builtin",
        }] })
    );
}

#[test]
fn a_model_needs_an_id_and_a_known_source() {
    assert!(serde_json::from_value::<ModelInfo>(json!({ "source": "builtin" })).is_err());
    assert!(serde_json::from_value::<ModelInfo>(json!({ "id": "x" })).is_err());
    let unknown = json!({ "id": "x", "source": "remote" });
    assert!(serde_json::from_value::<ModelInfo>(unknown).is_err());
}

#[test]
fn a_reload_that_applied_is_one_flag() {
    let result = AdminConfigReloadResult { applied: true, error: None, restart_needed: Vec::new() };
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(value, json!({ "applied": true }));
    let back: AdminConfigReloadResult = serde_json::from_value(value).unwrap();
    assert_eq!(back, result);
}

#[test]
fn a_reload_names_the_keys_that_wait_for_a_restart() {
    let result = AdminConfigReloadResult {
        applied: true,
        error: None,
        restart_needed: vec!["screen".to_owned(), "model.provider".to_owned()],
    };
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({ "applied": true, "restart_needed": ["screen", "model.provider"] })
    );
}

#[test]
fn a_config_error_without_a_place_is_only_its_message() {
    let error = ConfigFileError {
        message: "config.toml is not readable".to_owned(),
        line: None,
        column: None,
        key: None,
    };
    let value = serde_json::to_value(&error).unwrap();
    assert_eq!(value, json!({ "message": "config.toml is not readable" }));
    let back: ConfigFileError = serde_json::from_value(value).unwrap();
    assert_eq!(back, error);
}

#[test]
fn a_reload_result_needs_its_applied_flag() {
    assert!(serde_json::from_value::<AdminConfigReloadResult>(json!({})).is_err());
}

#[test]
fn a_status_from_before_roots_and_config_parses_without_them() {
    let old = json!({
        "daemon_id": "019a9b1c-3d00-7a10-8b20-000000000007",
        "version": "0.1.0",
        "protocol": 1,
        "pid": 1234,
        "started_at": "2026-10-03T08:00:00Z",
        "screen_backend": "vt100",
        "conversations": 2,
        "shells": 1,
        "providers": [],
    });
    let status: AdminStatusResult = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(status.roots, None);
    assert_eq!(status.config, None);
    assert_eq!(serde_json::to_value(&status).unwrap(), old, "nothing new is written");
}

#[test]
fn a_provider_from_before_logins_by_key_is_inactive_with_no_login_kind() {
    let old = json!({ "provider": "openai", "logged_in": true });
    let status: ProviderStatus = serde_json::from_value(old.clone()).unwrap();
    assert!(!status.active);
    assert_eq!(status.login, None);
    assert_eq!(status.key_hint, None);
    assert_eq!(serde_json::to_value(&status).unwrap(), old, "nothing new is written");
}

#[test]
fn login_kinds_are_snake_case_names_on_the_wire() {
    let names: Vec<_> = [LoginKind::Subscription, LoginKind::ApiKey]
        .iter()
        .map(|kind| serde_json::to_value(kind).unwrap())
        .collect();
    assert_eq!(names, [json!("subscription"), json!("api_key")]);
}

#[test]
fn root_sources_are_snake_case_names_on_the_wire() {
    let names: Vec<_> =
        [RootSource::DirVariable, RootSource::EfrHome, RootSource::Xdg, RootSource::RunUser]
            .iter()
            .map(|source| serde_json::to_value(source).unwrap())
            .collect();
    assert_eq!(names, [json!("dir_variable"), json!("efr_home"), json!("xdg"), json!("run_user")]);
}

#[test]
fn a_missing_plain_config_file_is_its_path_and_a_false_exists() {
    let config = ConfigStatus {
        path: "/home/me/.config/efr/config.toml".into(),
        exists: false,
        symlink_target: None,
        reload_error: None,
        restart_needed: Vec::new(),
    };
    let value = serde_json::to_value(&config).unwrap();
    assert_eq!(value, json!({ "path": "/home/me/.config/efr/config.toml", "exists": false }));
    let back: ConfigStatus = serde_json::from_value(value).unwrap();
    assert_eq!(back, config);
}

#[test]
fn a_config_status_needs_its_path_and_exists_flag() {
    assert!(serde_json::from_value::<ConfigStatus>(json!({ "exists": true })).is_err());
    assert!(serde_json::from_value::<ConfigStatus>(json!({ "path": "/c/config.toml" })).is_err());
}

#[test]
fn a_project_add_without_a_name_registers_the_path_itself() {
    let params: AdminProjectAdd = serde_json::from_value(json!({ "path": "/p/app" })).unwrap();
    assert_eq!(params, AdminProjectAdd { path: "/p/app".into(), name: None, git_root: false });
    assert_eq!(serde_json::to_value(&params).unwrap(), json!({ "path": "/p/app" }));
}

#[test]
fn a_project_add_and_remove_need_a_path() {
    assert!(serde_json::from_value::<AdminProjectAdd>(json!({ "git_root": true })).is_err());
    assert!(serde_json::from_value::<AdminProjectRemove>(json!({})).is_err());
}

#[test]
fn a_project_without_a_name_is_its_id_and_root() {
    let project = ProjectInfo {
        id: "019a9b1c-3d00-7a10-8b20-000000000008".parse().unwrap(),
        root: "/etc/nixos".into(),
        name: None,
    };
    let value = serde_json::to_value(&project).unwrap();
    assert_eq!(
        value,
        json!({ "id": "019a9b1c-3d00-7a10-8b20-000000000008", "root": "/etc/nixos" })
    );
    assert_eq!(serde_json::from_value::<ProjectInfo>(value).unwrap(), project);
}

#[test]
fn a_steer_from_before_if_late_parses_as_one_that_is_refused_when_late() {
    let old = json!({ "command_id": COMMAND, "conversation_id": CONVERSATION, "text": "also" });
    let steer: TurnSteer = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(steer.if_late, None);
    assert_eq!(serde_json::to_value(&steer).unwrap(), old, "no if_late is written");
}

#[test]
fn a_late_steer_that_queues_carries_what_a_prompt_carries() {
    let value = json!({
        "command_id": COMMAND,
        "conversation_id": CONVERSATION,
        "text": "also",
        "if_late": { "kind": "queue", "context": { "pwd": "/tmp" }, "settings": { "mode": "auto" } },
    });
    let steer: TurnSteer = serde_json::from_value(value.clone()).unwrap();
    let Some(LateSteer::Queue { context, last_command, settings }) = &steer.if_late else {
        panic!("{steer:?}");
    };
    assert_eq!(context.as_ref().map(|context| context.pwd.as_path()), Some("/tmp".as_ref()));
    assert_eq!(*last_command, None);
    assert_eq!(settings.mode, Some(Mode::Auto));
    assert_eq!(serde_json::to_value(&steer).unwrap(), value);
    let bare: LateSteer = serde_json::from_value(json!({ "kind": "queue" })).unwrap();
    assert_eq!(
        bare,
        LateSteer::Queue { context: None, last_command: None, settings: TurnSettings::default() }
    );
}

#[test]
fn a_late_steer_hides_the_last_command_from_debug() {
    let late = LateSteer::Queue {
        context: Some(ShellContext::new("/tmp")),
        last_command: Some("export TOKEN=hunter2".to_owned()),
        settings: TurnSettings::default(),
    };
    let text = format!("{late:?}");
    assert!(!text.contains("hunter2"), "{text}");
    assert!(text.contains("/tmp"), "{text}");
}

#[test]
fn a_steer_result_says_queued_only_when_the_steer_became_a_prompt() {
    let old = json!({ "turn_id": TURN, "seq": 45 });
    let result: TurnSteerResult = serde_json::from_value(old.clone()).unwrap();
    assert!(!result.queued);
    assert_eq!(serde_json::to_value(&result).unwrap(), old, "no queued flag is written");
    let queued = TurnSteerResult { queued: true, ..result };
    assert_eq!(serde_json::to_value(&queued).unwrap()["queued"], json!(true));
}

#[test]
fn an_interrupt_from_before_esc_resends_and_withdraws_nothing() {
    let old = json!({ "command_id": COMMAND, "conversation_id": CONVERSATION, "turn_id": TURN });
    let interrupt: TurnInterrupt = serde_json::from_value(old.clone()).unwrap();
    assert!(interrupt.resend_steers.is_empty());
    assert_eq!(interrupt.resend_as, None);
    assert!(interrupt.withdraw_steers.is_empty());
    assert!(interrupt.withdraw.is_empty());
    assert_eq!(serde_json::to_value(&interrupt).unwrap(), old, "no empty lists are written");
    let old_result = json!({ "turn_id": TURN, "seq": 44 });
    let result: TurnInterruptResult = serde_json::from_value(old_result.clone()).unwrap();
    assert_eq!(result.resent, None);
    assert!(result.withdrawn.is_empty());
    assert!(result.withdrawn_steers.is_empty());
    assert_eq!(serde_json::to_value(&result).unwrap(), old_result);
}

#[test]
fn a_withdraw_names_its_target_by_kind() {
    let by_tty = json!({
        "command_id": COMMAND,
        "conversation_id": CONVERSATION,
        "target": { "kind": "newest_from_tty", "tty": "/dev/pts/3" },
    });
    let withdraw: PromptWithdraw = serde_json::from_value(by_tty.clone()).unwrap();
    assert_eq!(withdraw.target, WithdrawTarget::NewestFromTty { tty: "/dev/pts/3".to_owned() });
    assert_eq!(serde_json::to_value(&withdraw).unwrap(), by_tty);
    let no_target = json!({ "command_id": COMMAND, "conversation_id": CONVERSATION });
    assert!(serde_json::from_value::<PromptWithdraw>(no_target).is_err());
    let unknown = json!({
        "command_id": COMMAND,
        "conversation_id": CONVERSATION,
        "target": { "kind": "oldest" },
    });
    assert!(serde_json::from_value::<PromptWithdraw>(unknown).is_err());
}
