use efr_protocol::{
    AdminLoginOpenAi, AdminStatus, ApprovalDecision, ApprovalRespond, Base64Bytes, CallId,
    Capabilities, CommandId, ConversationHistory, ConversationId, ConversationSubscribe,
    ConversationsList, Hello, InputRespond, LeaseReport, Method, Origin, PROTOCOL_VERSION,
    PageCursor, PromptSend, PtyAttach, PtyId, PtyResize, PtyWrite, ScopeName, SecretText, Seq,
    Size, TurnInterrupt, TurnSteer,
};
use pretty_assertions::assert_eq;

use crate::DaemonError;
use crate::methods::{authorize, cursor, granted, page_size, parse_cursor, scope};

fn id(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(n)
}

/// One request of every method.
fn every_method() -> Vec<Method> {
    let conversation_id = ConversationId::from_uuid(id(1));
    let command_id = CommandId::from_uuid(id(2));
    let pty_id = PtyId::from_uuid(id(3));
    vec![
        Method::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            origin: Origin::Cli,
            client: None,
            capabilities: Capabilities::default(),
            tty: None,
            pid: None,
            device_id: None,
        }),
        Method::ConversationsList(ConversationsList::default()),
        Method::ConversationSubscribe(ConversationSubscribe {
            conversation_id,
            after_seq: None,
            answers_input: false,
        }),
        Method::ConversationHistory(ConversationHistory {
            conversation_id,
            cursor: None,
            limit: None,
        }),
        Method::PromptSend(PromptSend {
            command_id,
            conversation_id: None,
            new_conversation: false,
            text: "hi".to_owned(),
            context: None,
            last_command: None,
            settings: efr_protocol::TurnSettings::default(),
        }),
        Method::TurnInterrupt(TurnInterrupt { command_id, conversation_id, turn_id: None }),
        Method::TurnSteer(TurnSteer {
            command_id,
            conversation_id,
            turn_id: None,
            text: "x".to_owned(),
        }),
        Method::ApprovalRespond(ApprovalRespond {
            command_id,
            conversation_id,
            call_id: CallId::from_uuid(id(4)),
            decision: ApprovalDecision::Allow,
        }),
        Method::PtyAttach(PtyAttach { pty_id, since_seq: None, scrollback_rows: None }),
        Method::PtyWrite(PtyWrite { pty_id, data: Base64Bytes::new(b"ls\r".to_vec()) }),
        Method::PtyResize(PtyResize { pty_id, size: Size { cols: 80, rows: 24 } }),
        Method::InputRespond(InputRespond {
            conversation_id,
            call_id: CallId::from_uuid(id(4)),
            text: SecretText::new("y"),
            hidden: false,
        }),
        Method::LeaseReport(LeaseReport::default()),
        Method::AdminStatus(AdminStatus::default()),
        Method::AdminLoginOpenAi(AdminLoginOpenAi::default()),
    ]
}

#[test]
fn every_method_needs_the_scope_the_protocol_names() {
    let methods = every_method();
    assert_eq!(methods.len(), 15, "one request per method");
    for method in &methods {
        assert_eq!(scope(method), ScopeName::for_method(method), "{}", method.name());
    }
}

#[test]
fn the_scope_table_is_the_designed_one() {
    let table: Vec<(&str, ScopeName)> =
        every_method().iter().map(|method| (method.name(), scope(method))).collect();

    assert_eq!(
        table,
        [
            ("hello", ScopeName::Read),
            ("conversations.list", ScopeName::Read),
            ("conversation.subscribe", ScopeName::Read),
            ("conversation.history", ScopeName::Read),
            ("prompt.send", ScopeName::Operate),
            ("turn.interrupt", ScopeName::Operate),
            ("turn.steer", ScopeName::Operate),
            ("approval.respond", ScopeName::Approve),
            ("pty.attach", ScopeName::Terminal),
            ("pty.write", ScopeName::Terminal),
            ("pty.resize", ScopeName::Terminal),
            ("input.respond", ScopeName::Terminal),
            ("lease.report", ScopeName::Read),
            ("admin.status", ScopeName::Admin),
            ("admin.login_openai", ScopeName::Admin),
        ]
    );
}

#[test]
fn local_surfaces_hold_every_scope() {
    for surface in [Origin::Shell, Origin::Cli, Origin::Proxy] {
        assert_eq!(granted(surface), ScopeName::ALL, "{surface:?}");
        for method in every_method() {
            assert!(authorize(surface, &method).is_ok(), "{surface:?} {}", method.name());
        }
    }
}

#[test]
fn a_phone_may_read_operate_and_approve_but_never_administer_or_type() {
    assert_eq!(granted(Origin::Phone), [ScopeName::Read, ScopeName::Operate, ScopeName::Approve]);
    for method in every_method() {
        let allowed = authorize(Origin::Phone, &method);
        match scope(&method) {
            ScopeName::Admin | ScopeName::Terminal => assert!(
                matches!(&allowed, Err(DaemonError::Forbidden { method: name, scope: needed })
                    if *name == method.name() && *needed == scope(&method)),
                "{}: {allowed:?}",
                method.name()
            ),
            _ => assert!(allowed.is_ok(), "{}", method.name()),
        }
    }
}

#[test]
fn a_cursor_round_trips_and_a_foreign_one_is_refused() {
    assert_eq!(parse_cursor(&cursor(Seq::new(42))).unwrap(), Seq::new(42));
    for foreign in ["42", "s:", "s:-1", "c:42", "s:4x"] {
        let result = parse_cursor(&PageCursor::new(foreign));
        assert!(matches!(result, Err(DaemonError::InvalidCursor { .. })), "{foreign}: {result:?}");
    }
}

#[test]
fn page_sizes_have_a_default_a_floor_and_a_ceiling() {
    assert_eq!(page_size(None, 50, 200), 50);
    assert_eq!(page_size(Some(0), 50, 200), 1);
    assert_eq!(page_size(Some(10), 50, 200), 10);
    assert_eq!(page_size(Some(9999), 50, 200), 200);
}

#[test]
fn a_resize_without_rows_or_columns_is_refused_and_a_huge_one_clamped() {
    use crate::methods::pty_resize::{MAX_SIZE, clamp};

    for size in [Size { cols: 0, rows: 24 }, Size { cols: 80, rows: 0 }] {
        assert!(matches!(clamp(size), Err(DaemonError::InvalidParams { .. })), "{size:?}");
    }
    assert_eq!(clamp(Size { cols: 80, rows: 24 }).unwrap(), Size { cols: 80, rows: 24 });
    assert_eq!(clamp(Size { cols: u16::MAX, rows: u16::MAX }).unwrap(), MAX_SIZE);
}

mod subscribe {
    use efr_protocol::{
        ConversationStatus, ConversationSummary, Event, EventEnvelope, Seq, TurnId,
    };
    use jiff::Timestamp;
    use pretty_assertions::assert_eq;

    use crate::methods::conversation_subscribe::{
        MAX_BYTES, MAX_EVENTS, fits, newest_that_fit, snapshot,
    };
    use crate::methods::{cursor, tests::id};

    fn at() -> Timestamp {
        Timestamp::from_second(1_800_000_000).unwrap()
    }

    fn steered(seq: u64, text_len: usize) -> EventEnvelope {
        EventEnvelope {
            seq: Seq::new(seq),
            conversation_id: None,
            at: at(),
            event: Event::TurnSteered {
                turn_id: TurnId::from_uuid(id(1)),
                text: "x".repeat(text_len),
            },
        }
    }

    fn summary() -> ConversationSummary {
        ConversationSummary {
            id: efr_protocol::ConversationId::from_uuid(id(5)),
            title: None,
            status: ConversationStatus::Idle,
            created_at: at(),
            updated_at: at(),
            last_seq: Seq::new(9),
            cwd: None,
            scope: None,
            tty: None,
        }
    }

    #[test]
    fn a_gap_replays_up_to_128_events_and_1_mib() {
        let events: Vec<_> = (1..=MAX_EVENTS as u64).map(|seq| steered(seq, 10)).collect();
        assert!(fits(&events));

        let too_many: Vec<_> = (1..=MAX_EVENTS as u64 + 1).map(|seq| steered(seq, 10)).collect();
        assert!(!fits(&too_many));

        let too_big = vec![steered(1, MAX_BYTES)];
        assert!(!fits(&too_big));
    }

    #[test]
    fn a_snapshot_keeps_the_newest_events_that_fit_and_a_cursor_for_the_rest() {
        let events: Vec<_> = (1..=MAX_EVENTS as u64 + 2).map(|seq| steered(seq, 10)).collect();

        let (kept, older) = newest_that_fit(events);
        let snapshot = snapshot(summary(), kept, older, Seq::new(200));

        assert!(older);
        assert_eq!(snapshot.events.len(), MAX_EVENTS);
        assert_eq!(snapshot.events[0].seq, Seq::new(3));
        assert_eq!(snapshot.history_cursor, Some(cursor(Seq::new(3))));
        assert_eq!(snapshot.hwm, Seq::new(200));
    }

    #[test]
    fn a_snapshot_of_a_short_conversation_has_no_history_cursor() {
        let (kept, older) = newest_that_fit(vec![steered(1, 10), steered(2, 10)]);

        let snapshot = snapshot(summary(), kept, older, Seq::new(2));

        assert!(!older);
        assert_eq!(snapshot.history_cursor, None);
        assert_eq!(snapshot.events.len(), 2);
    }
}
