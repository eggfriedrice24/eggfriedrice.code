use efr_protocol::{
    AdminConfigReload, AdminLoginOpenAi, AdminProjectAdd, AdminProjectRemove, AdminSandboxCheck,
    AdminStatus, ApprovalDecision, ApprovalRespond, Base64Bytes, CallId, Capabilities, CommandId,
    ConversationCompact, ConversationContext, ConversationHistory, ConversationId,
    ConversationSearch, ConversationSubscribe, ConversationsList, ConversationsSearch,
    ConversationsWatch, ErrorCode, Hello, InputRespond, LeaseReport, Method, ModelsList, Origin,
    PROTOCOL_VERSION, PageCursor, Presence, PresenceSet, ProjectsList, PromptSend, PromptWithdraw,
    PtyAttach, PtyId, PtyResize, PtyWrite, QuestionId, SandboxExplain, SandboxSurfaceRespond,
    ScopeName, SecretText, Seq, Size, TurnInterrupt, TurnSteer, TurnUndo, WithdrawTarget,
};
use pretty_assertions::assert_eq;

use crate::DaemonError;
use crate::methods::approval_respond::remote_always;
use crate::methods::{
    authorize, authorize_remote, before_proof, cursor, granted, page_size, parse_cursor,
    remote_only, scope, user_side_only,
};
use crate::sandbox::peers::PeerSide;

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
            drafts: false,
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
            recent_commands: Vec::new(),
            settings: efr_protocol::TurnSettings::default(),
        }),
        Method::TurnInterrupt(TurnInterrupt {
            command_id,
            conversation_id,
            turn_id: None,
            resend_steers: Vec::new(),
            resend_as: None,
            withdraw_steers: Vec::new(),
            withdraw: Vec::new(),
        }),
        Method::TurnSteer(TurnSteer {
            command_id,
            conversation_id,
            turn_id: None,
            text: "x".to_owned(),
            if_late: None,
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
            manual: false,
        }),
        Method::LeaseReport(LeaseReport::default()),
        Method::ModelsList(ModelsList::default()),
        Method::ProjectsList(ProjectsList::default()),
        Method::AdminProjectAdd(AdminProjectAdd {
            path: "/p/app".into(),
            name: None,
            git_root: false,
        }),
        Method::AdminProjectRemove(AdminProjectRemove { path: "/p/app".into() }),
        Method::AdminStatus(AdminStatus::default()),
        Method::AdminConfigReload(AdminConfigReload::default()),
        Method::AdminLoginOpenAi(AdminLoginOpenAi::default()),
        Method::SandboxExplain(SandboxExplain { path: "/home/u/.zshrc".into(), cwd: None }),
        Method::SandboxSurfaceRespond(SandboxSurfaceRespond {
            command_id,
            conversation_id,
            question_id: QuestionId::from_uuid(id(5)),
            keep: false,
        }),
        Method::AdminSandboxCheck(AdminSandboxCheck::default()),
        Method::PromptWithdraw(PromptWithdraw {
            command_id,
            conversation_id,
            target: WithdrawTarget::NewestFromTty { tty: "/dev/pts/3".to_owned() },
        }),
        Method::ConversationCompact(ConversationCompact {
            command_id,
            conversation_id,
            focus: None,
        }),
        Method::PresenceSet(PresenceSet {
            command_id,
            conversation_id: Some(conversation_id),
            presence: Presence::Away,
        }),
        Method::TurnUndo(TurnUndo {
            command_id,
            conversation_id: Some(conversation_id),
            turn_id: None,
            dry_run: true,
            force: false,
            redo: false,
            list: false,
        }),
        Method::ConversationContext(ConversationContext { conversation_id, model: None }),
        Method::ConversationsSearch(ConversationsSearch {
            query: "rsync".to_owned(),
            ..ConversationsSearch::default()
        }),
        Method::ConversationSearch(ConversationSearch {
            conversation_id,
            query: "rsync".to_owned(),
            limit: None,
        }),
        Method::ConversationsWatch(ConversationsWatch { limit: None }),
    ]
}

#[test]
fn every_method_needs_the_scope_the_protocol_names() {
    let methods = every_method();
    assert_eq!(methods.len(), 31, "one request per method but conversation.diff");
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
            ("models.list", ScopeName::Read),
            ("projects.list", ScopeName::Read),
            ("admin.project_add", ScopeName::Admin),
            ("admin.project_remove", ScopeName::Admin),
            ("admin.status", ScopeName::Admin),
            ("admin.config_reload", ScopeName::Admin),
            ("admin.login_openai", ScopeName::Admin),
            ("sandbox.explain", ScopeName::Read),
            ("sandbox.surface_respond", ScopeName::Approve),
            ("admin.sandbox_check", ScopeName::Admin),
            ("prompt.withdraw", ScopeName::Operate),
            ("conversation.compact", ScopeName::Operate),
            ("presence.set", ScopeName::Operate),
            ("turn.undo", ScopeName::Operate),
            ("conversation.context", ScopeName::Read),
            ("conversations.search", ScopeName::Read),
            ("conversation.search", ScopeName::Read),
            ("conversations.watch", ScopeName::Read),
        ]
    );
}

/// An undo writes the user's files, so it needs `operate`: a model-side peer holds
/// `read` only and cannot undo. A phone holds `operate`; the handler refuses it itself.
#[test]
fn turn_undo_needs_operate_scope() {
    let method = every_method().into_iter().find(|method| method.name() == "turn.undo").unwrap();
    assert_eq!(scope(&method), ScopeName::Operate);
    assert_eq!(ScopeName::for_method(&method), ScopeName::Operate);
    for peer in [PeerSide::ModelSide, PeerSide::Unknown] {
        let refused = authorize(Origin::Shell, peer, &method);
        assert!(
            matches!(refused, Err(DaemonError::ModelSidePeer { method: "turn.undo" })),
            "{refused:?}"
        );
    }
    assert!(authorize(Origin::Shell, PeerSide::User, &method).is_ok());
    assert!(authorize(Origin::Cli, PeerSide::User, &method).is_ok());
}

#[test]
fn presence_set_refused_for_model_side_peer() {
    let method = every_method().into_iter().find(|method| method.name() == "presence.set").unwrap();
    for peer in [PeerSide::ModelSide, PeerSide::Unknown] {
        let refused = authorize(Origin::Shell, peer, &method);
        assert!(
            matches!(refused, Err(DaemonError::ModelSidePeer { method: "presence.set" })),
            "{refused:?}"
        );
    }
    assert!(authorize(Origin::Shell, PeerSide::User, &method).is_ok());
    assert!(authorize(Origin::Phone, PeerSide::User, &method).is_ok(), "a phone may operate");
}

#[test]
fn local_surfaces_hold_every_scope() {
    for surface in [Origin::Shell, Origin::Cli, Origin::Proxy] {
        assert_eq!(granted(surface, PeerSide::User), ScopeName::ALL, "{surface:?}");
        for method in every_method() {
            assert!(
                authorize(surface, PeerSide::User, &method).is_ok(),
                "{surface:?} {}",
                method.name()
            );
        }
    }
}

#[test]
fn a_phone_may_read_operate_and_approve_but_never_administer_or_type() {
    assert_eq!(
        granted(Origin::Phone, PeerSide::User),
        [ScopeName::Read, ScopeName::Operate, ScopeName::Approve]
    );
    for method in every_method() {
        let allowed = authorize(Origin::Phone, PeerSide::User, &method);
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
fn a_declared_phone_below_a_hidden_shell_gets_read_only() {
    // NOTE: the origin is what the client declares in `hello`, so a process of the
    // model's commands can say `phone`. The walk decides, never the declared origin.
    for peer in [PeerSide::ModelSide, PeerSide::Unknown] {
        assert_eq!(granted(Origin::Phone, peer), [ScopeName::Read], "{peer:?}");
        for method in every_method() {
            let allowed = authorize(Origin::Phone, peer, &method);
            match scope(&method) {
                ScopeName::Read if !user_side_only(&method) => {
                    assert!(allowed.is_ok(), "{}", method.name());
                }
                ScopeName::Read | ScopeName::Operate | ScopeName::Approve => assert!(
                    matches!(&allowed, Err(DaemonError::ModelSidePeer { method: name })
                        if *name == method.name()),
                    "{peer:?} {}: {allowed:?}",
                    method.name()
                ),
                _ => assert!(
                    matches!(&allowed, Err(DaemonError::Forbidden { method: name, .. })
                        if *name == method.name()),
                    "{peer:?} {}: {allowed:?}",
                    method.name()
                ),
            }
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

    use proptest::prelude::*;

    use crate::methods::conversation_subscribe::{
        MAX_BYTES, MAX_EVENTS, fits, newest_that_fit, newest_within, snapshot,
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

    #[test]
    fn a_newest_event_over_the_bound_leaves_no_event_and_a_cursor_that_reaches_it() {
        let (kept, older) = newest_that_fit(vec![steered(1, 10), steered(2, MAX_BYTES)]);

        assert!(older);
        assert!(kept.is_empty());
        let snapshot = snapshot(summary(), kept, older, Seq::new(2));
        // The cursor continues before the number after the high-water mark, so the
        // first history page holds the event that the snapshot left out.
        assert_eq!(snapshot.history_cursor, Some(cursor(Seq::new(3))));
        assert!(snapshot.events.is_empty());
    }

    #[test]
    fn a_snapshot_writes_each_event_as_json_at_most_once() {
        // 16 KiB events: 64 of them fill the bytes, far below the 128 events.
        let events: Vec<_> = (1..=MAX_EVENTS as u64).map(|seq| steered(seq, 16 * 1024)).collect();
        let total = events.len();

        let mut encodes = 0;
        let (kept, older) = newest_within(events.clone(), |envelope| {
            encodes += 1;
            json_len(envelope)
        });

        assert!(older);
        assert!(encodes <= total, "{encodes} encodes for {total} events");
        let mut before = 0;
        let (old_kept, _) = removed_one_by_one(events, |envelope| {
            before += 1;
            json_len(envelope)
        });
        assert!(before > total, "the old way wrote {before} events");
        assert_eq!(kept, old_kept);
    }

    fn json_len(envelope: &EventEnvelope) -> usize {
        serde_json::to_vec(envelope).unwrap().len()
    }

    /// The way before one pass: remove the oldest event until the rest fit, and write
    /// the rest as JSON again after each removal.
    fn removed_one_by_one(
        mut events: Vec<EventEnvelope>,
        mut len: impl FnMut(&EventEnvelope) -> usize,
    ) -> (Vec<EventEnvelope>, bool) {
        let mut cut = false;
        loop {
            let bytes = events.iter().map(&mut len).fold(0, usize::saturating_add);
            if events.len() <= MAX_EVENTS && bytes <= MAX_BYTES {
                return (events, cut);
            }
            events.remove(0);
            cut = true;
        }
    }

    proptest! {
        // NOTE: the lengths come from the list, not from JSON, so the old way, which
        // measures again after each removal, stays fast on large events.
        #[test]
        fn one_pass_keeps_what_removing_one_by_one_keeps(
            lens in prop::collection::vec(
                prop_oneof![
                    Just(10_usize),
                    Just(MAX_BYTES / MAX_EVENTS),
                    0_usize..300 * 1024,
                    Just(MAX_BYTES),
                    Just(MAX_BYTES + 1),
                    Just(usize::MAX),
                ],
                0..140,
            ),
        ) {
            let events: Vec<_> = (1..=lens.len() as u64).map(|seq| steered(seq, 0)).collect();
            let len = |envelope: &EventEnvelope| lens[envelope.seq.get() as usize - 1];

            let mut measured = 0;
            let kept = newest_within(events.clone(), |envelope| {
                measured += 1;
                len(envelope)
            });

            prop_assert_eq!(&kept, &removed_one_by_one(events, len));
            prop_assert!(measured <= lens.len());
        }
    }
}

#[test]
fn a_model_side_peer_or_a_gone_one_may_only_read() {
    for peer in [PeerSide::ModelSide, PeerSide::Unknown] {
        for surface in [Origin::Shell, Origin::Cli, Origin::Proxy] {
            assert_eq!(granted(surface, peer), [ScopeName::Read]);
            for method in every_method() {
                let allowed = authorize(surface, peer, &method);
                if scope(&method) == ScopeName::Read && !user_side_only(&method) {
                    assert!(allowed.is_ok(), "{}", method.name());
                } else {
                    assert!(
                        matches!(&allowed, Err(DaemonError::ModelSidePeer { method: name }) if *name == method.name()),
                        "{}: {allowed:?}",
                        method.name()
                    );
                }
            }
        }
    }
}

#[test]
fn always_allow_only_from_own_terminal() {
    let call_id = CallId::from_uuid(id(40));
    let answer = |decision| {
        Method::ApprovalRespond(ApprovalRespond {
            command_id: CommandId::from_uuid(id(41)),
            conversation_id: ConversationId::from_uuid(id(42)),
            call_id,
            decision,
        })
    };
    let always = answer(ApprovalDecision::AllowAlways);
    // A phone holds `approve`, so efrd refuses its `a` itself, and the question stays
    // open: no receipt keeps the refusal.
    assert!(authorize(Origin::Phone, PeerSide::User, &always).is_ok());
    let refused = remote_always(ApprovalDecision::AllowAlways, Origin::Phone, call_id);
    assert!(
        matches!(&refused, Some(DaemonError::Rejected { body }) if body.code == ErrorCode::Forbidden),
        "{refused:?}"
    );
    // A "yes" from a phone is the conversation's to judge.
    assert!(remote_always(ApprovalDecision::Allow, Origin::Phone, call_id).is_none());
    // The user's own terminal gives it.
    for surface in [Origin::Shell, Origin::Cli, Origin::Proxy] {
        assert!(remote_always(ApprovalDecision::AllowAlways, surface, call_id).is_none());
        assert!(authorize(surface, PeerSide::User, &always).is_ok());
    }
    // A model-side peer holds no `approve` at all, whatever it declares.
    for surface in [Origin::Shell, Origin::Cli, Origin::Proxy, Origin::Phone] {
        let allowed = authorize(surface, PeerSide::ModelSide, &always);
        assert!(matches!(allowed, Err(DaemonError::ModelSidePeer { .. })), "{allowed:?}");
    }
}

#[test]
fn sandbox_explain_needs_read_scope_only() {
    let method =
        every_method().into_iter().find(|method| method.name() == "sandbox.explain").unwrap();
    assert_eq!(scope(&method), ScopeName::Read);
    assert!(authorize(Origin::Shell, PeerSide::ModelSide, &method).is_ok());
    assert!(authorize(Origin::Phone, PeerSide::User, &method).is_ok());
    let respond = every_method()
        .into_iter()
        .find(|method| method.name() == "sandbox.surface_respond")
        .unwrap();
    assert!(authorize(Origin::Shell, PeerSide::ModelSide, &respond).is_err());
}

/// Search reads the text of every project and finds where the index replaced a
/// secret, so a process of the model's commands may not search, though it holds
/// `read`; the user's own processes and a user's phone may (DQ43).
#[test]
fn model_peer_may_not_search() {
    let searches: Vec<Method> = every_method().into_iter().filter(user_side_only).collect();
    let names: Vec<&str> = searches.iter().map(Method::name).collect();
    assert_eq!(names, ["conversations.search", "conversation.search"]);
    for method in &searches {
        assert_eq!(scope(method), ScopeName::Read);
        for surface in [Origin::Shell, Origin::Cli, Origin::Proxy, Origin::Phone] {
            for peer in [PeerSide::ModelSide, PeerSide::Unknown] {
                let refused = authorize(surface, peer, method);
                assert!(
                    matches!(&refused, Err(DaemonError::ModelSidePeer { method: name }) if *name == method.name()),
                    "{surface:?} {peer:?}: {refused:?}"
                );
            }
            assert!(authorize(surface, PeerSide::User, method).is_ok(), "{surface:?}");
        }
    }
}

/// The two methods of the tailnet listener that run before a device proved its key.
fn proof_methods() -> [Method; 2] {
    let device_id = efr_protocol::DeviceId::from_uuid(id(9));
    [
        Method::DeviceEnroll(efr_protocol::DeviceEnroll {
            token: SecretText::new("token".to_owned()),
            name: "phone".to_owned(),
            public_key: Base64Bytes::new(vec![4; 65]),
        }),
        Method::DeviceProve(efr_protocol::DeviceProve {
            device_id,
            signature: Base64Bytes::new(vec![0x30, 0x00]),
        }),
    ]
}

/// A remote connection gets no scope from the `/proc` walk: before its proof only
/// `hello`, `device.enroll` and `device.prove` pass, and after it the device's scopes,
/// at most the phone's.
#[test]
fn a_remote_connection_holds_nothing_before_its_proof_and_its_device_scopes_after() {
    for method in every_method().iter().chain(&proof_methods()) {
        let before = authorize_remote(None, method);
        if before_proof(method) {
            assert!(before.is_ok(), "{}", method.name());
        } else {
            assert!(
                matches!(&before, Err(DaemonError::ProofNeeded { method: name }) if *name == method.name()),
                "{}: {before:?}",
                method.name()
            );
            assert_eq!(before.unwrap_err().code(), ErrorCode::Unauthorized);
        }
        let every = authorize_remote(Some(&ScopeName::ALL), method);
        let phone = scope(method) != ScopeName::Admin && scope(method) != ScopeName::Terminal;
        assert_eq!(every.is_ok(), phone || before_proof(method), "{}: {every:?}", method.name());
        let read = authorize_remote(Some(&[ScopeName::Read]), method);
        let reads = scope(method) == ScopeName::Read;
        assert_eq!(read.is_ok(), reads || before_proof(method), "{}: {read:?}", method.name());
    }
}

/// The Unix socket refuses the methods of the tailnet listener for every peer.
#[test]
fn the_proof_methods_are_remote_only() {
    for method in proof_methods() {
        assert!(remote_only(&method) && before_proof(&method), "{}", method.name());
        assert_eq!(scope(&method), ScopeName::Read);
    }
    assert!(every_method().iter().all(|method| !remote_only(method)));
}

/// The walk of `/proc` that each search makes for its peer (DQ43): at most 2 ms at
/// p95. The walk goes up from this test to pid 1, past a hidden shell that it never
/// meets, the longest walk of a user-side peer. `just bench-search` runs it.
#[test]
#[ignore = "a bench: run it with just bench-search"]
#[expect(clippy::print_stdout, reason = "a bench prints its numbers")]
fn bench_search_peer_walk() {
    use std::time::{Duration, Instant};

    use crate::sandbox::peers;

    let pid = std::process::id();
    let mut times: Vec<Duration> = (0..1_000)
        .map(|_| {
            let started = Instant::now();
            let side = peers::side(Some(pid), &[u32::MAX - 1], 1, None);
            let took = started.elapsed();
            assert_eq!(side, PeerSide::User);
            took
        })
        .collect();
    times.sort();
    let at = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
    let ms = |time: Duration| time.as_secs_f64() * 1000.0;
    println!(
        "bench_search_peer_walk: 1000 walks, p50 {:.3} ms, p95 {:.3} ms, max {:.3} ms (budget p95 2 ms)",
        ms(at(0.5)),
        ms(at(0.95)),
        ms(times[times.len() - 1])
    );
    assert!(at(0.95) <= Duration::from_millis(2), "the p95 of a walk is {:?}", at(0.95));
}
