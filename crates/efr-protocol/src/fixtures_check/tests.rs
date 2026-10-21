use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use pretty_assertions::assert_eq;

use super::{
    FIXTURES_DIR, all, always_saved_samples, approval_decision_samples, daemon_roots,
    draft_part_samples, event_path, event_samples, floor_edit_exit_sample, grant_samples,
    input_wait_samples, launch_samples, method_samples, method_stem, models_list_sample,
    route_scope_samples, switch_samples, undo_refused_samples, withdraw_target_samples,
};
use crate::{
    AlwaysSaved, ApprovalDecision, ApprovedBy, AttachHow, BlockReason, BusProxyOff, DraftPart,
    Event, ExitKind, FullRefusal, Grant, InputWait, JudgeKind, Launch, Method, Mode, ModelSource,
    NewReason, NotSavedReason, RootSource, RouteScope, ShellCommandEnd, Switch, UndoAction,
    UndoReason, UndoRefused, WithdrawTarget,
};

const BLESS: &str = "cargo test -p efr-protocol --lib -- --ignored --exact \
                     fixtures_check::tests::bless_fixtures";

fn read(path: &str) -> String {
    fs::read_to_string(Path::new(FIXTURES_DIR).join(path)).unwrap_or_else(|err| {
        panic!("cannot read fixtures/v1/{path}: {err}; to create it run {BLESS}")
    })
}

/// Every file under `dir`, as paths relative to `root` with `/` separators.
fn files_under(root: &Path, dir: &Path, found: &mut BTreeSet<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_under(root, &path, found);
        } else {
            let relative = path.strip_prefix(root).unwrap();
            let parts: Vec<_> = relative.iter().map(|part| part.to_str().unwrap()).collect();
            found.insert(parts.join("/"));
        }
    }
}

/// The position of a method in the enum. Adding a method without an arm here does not
/// compile, and an arm without a sample fails `every_method_has_a_params_sample`.
fn method_index(method: &Method) -> usize {
    match method {
        Method::Hello(_) => 0,
        Method::ConversationsList(_) => 1,
        Method::ConversationSubscribe(_) => 2,
        Method::ConversationHistory(_) => 3,
        Method::PromptSend(_) => 4,
        Method::TurnInterrupt(_) => 5,
        Method::TurnSteer(_) => 6,
        Method::ApprovalRespond(_) => 7,
        Method::PtyAttach(_) => 8,
        Method::PtyWrite(_) => 9,
        Method::PtyResize(_) => 10,
        Method::InputRespond(_) => 11,
        Method::LeaseReport(_) => 12,
        Method::ModelsList(_) => 13,
        Method::ProjectsList(_) => 14,
        Method::AdminProjectAdd(_) => 15,
        Method::AdminProjectRemove(_) => 16,
        Method::AdminStatus(_) => 17,
        Method::AdminConfigReload(_) => 18,
        Method::AdminLoginOpenAi(_) => 19,
        Method::AdminLoginApiKey(_) => 20,
        Method::AdminLogout(_) => 21,
        Method::SandboxExplain(_) => 22,
        Method::SandboxSurfaceRespond(_) => 23,
        Method::AdminSandboxCheck(_) => 24,
        Method::ConversationDiff(_) => 25,
        Method::PromptWithdraw(_) => 26,
        Method::ConversationCompact(_) => 27,
        Method::PresenceSet(_) => 28,
        Method::TurnUndo(_) => 29,
        Method::JobsList(_) => 30,
        Method::JobsStop(_) => 31,
        Method::ConversationContext(_) => 32,
        Method::ConversationsRecent(_) => 33,
        Method::ConversationResume(_) => 34,
    }
}

const METHOD_COUNT: usize = 35;

/// The position of an event kind in the enum, for the same purpose as `method_index`.
fn event_index(event: &Event) -> usize {
    match event {
        Event::ConversationCreated { .. } => 0,
        Event::PromptQueued { .. } => 1,
        Event::PromptHeld { .. } => 2,
        Event::TurnStarted { .. } => 3,
        Event::ScopeChanged { .. } => 4,
        Event::AssistantMessageUpdated { .. } => 5,
        Event::AssistantMessageCompleted { .. } => 6,
        Event::ToolCallStarted { .. } => 7,
        Event::ToolCallOutputUpdated { .. } => 8,
        Event::ToolCallInputChanged { .. } => 9,
        Event::ToolCallCompleted { .. } => 10,
        Event::ApprovalRequested { .. } => 11,
        Event::ApprovalResolved { .. } => 12,
        Event::ApprovalExpired { .. } => 13,
        Event::TurnSteered { .. } => 14,
        Event::TurnInterruptRequested { .. } => 15,
        Event::TurnInterrupted { .. } => 16,
        Event::TurnCompleted { .. } => 17,
        Event::TurnFailed { .. } => 18,
        Event::TurnCancelled { .. } => 19,
        Event::ShellStarted { .. } => 20,
        Event::ShellExited { .. } => 21,
        Event::CwdChanged { .. } => 22,
        Event::LoginCompleted { .. } => 23,
        Event::ExitRequested { .. } => 24,
        Event::ExitJudged { .. } => 25,
        Event::SandboxSurfaceChanged { .. } => 26,
        Event::SurfaceQuestionRequested { .. } => 27,
        Event::SurfaceQuestionAnswered { .. } => 28,
        Event::TurnSurfaceReport { .. } => 29,
        Event::SandboxUnavailable { .. } => 30,
        Event::Unknown { .. } => 31,
        Event::PromptWithdrawn { .. } => 32,
        Event::SteeringDelivered { .. } => 33,
        Event::SteeringWithdrawn { .. } => 34,
        Event::ConversationCompacted(_) => 35,
        Event::LogoutCompleted { .. } => 36,
        Event::ExitReviewStarted { .. } => 37,
        Event::ClassifierPaused { .. } => 38,
        Event::PresenceChanged { .. } => 39,
        Event::CacheDeleteApplied { .. } => 40,
        Event::HomeFileChanged { .. } => 41,
        Event::SnapshotTaken { .. } => 42,
        Event::UndoApplied { .. } => 43,
        Event::JobStarted { .. } => 44,
        Event::JobWatchMatched { .. } => 45,
        Event::JobEnded { .. } => 46,
        Event::JobNotice { .. } => 47,
        Event::JobNoticeHeld { .. } => 48,
        Event::AlwaysRuleSaved { .. } => 49,
        Event::TerminalAttached { .. } => 50,
        Event::JobPortForwarded { .. } => 51,
        Event::ShellCommandDetached { .. } => 52,
        Event::ShellCommandInputChanged { .. } => 53,
        Event::ShellCommandEnded { .. } => 54,
    }
}

const EVENT_COUNT: usize = 55;

/// The position of an input wait in the enum, for the same purpose as `method_index`.
fn input_wait_index(input: InputWait) -> usize {
    match input {
        InputWait::None => 0,
        InputWait::Visible => 1,
        InputWait::Hidden => 2,
    }
}

const INPUT_WAIT_COUNT: usize = 3;

/// The position of a mode in the enum, for the same purpose as `method_index`.
fn mode_index(mode: Mode) -> usize {
    match mode {
        Mode::Manual => 0,
        Mode::Cautious => 1,
        Mode::Auto => 2,
        Mode::Full => 3,
    }
}

const MODE_COUNT: usize = 4;

/// The position of a judge in the enum, for the same purpose as `method_index`.
fn judge_kind_index(judge: JudgeKind) -> usize {
    match judge {
        JudgeKind::Classifier => 0,
        JudgeKind::User => 1,
        JudgeKind::Floor => 2,
        JudgeKind::Always => 3,
        JudgeKind::PresenceExpired => 4,
        JudgeKind::Full => 5,
    }
}

const JUDGE_KIND_COUNT: usize = 6;

/// The position of a full mode refusal in the enum, for the same purpose as
/// `method_index`.
fn full_refusal_index(refusal: FullRefusal) -> usize {
    match refusal {
        FullRefusal::Root => 0,
        FullRefusal::SnapshotOff => 1,
        FullRefusal::UndoOff => 2,
        FullRefusal::NoZsh => 3,
    }
}

const FULL_REFUSAL_COUNT: usize = 4;

/// The position of a model source in the enum, for the same purpose as `method_index`.
fn model_source_index(source: ModelSource) -> usize {
    match source {
        ModelSource::Builtin => 0,
        ModelSource::Config => 1,
    }
}

const MODEL_SOURCE_COUNT: usize = 2;

/// The position of a root source in the enum, for the same purpose as `method_index`.
fn root_source_index(source: RootSource) -> usize {
    match source {
        RootSource::DirVariable => 0,
        RootSource::EfrHome => 1,
        RootSource::Xdg => 2,
        RootSource::RunUser => 3,
    }
}

const ROOT_SOURCE_COUNT: usize = 4;

/// The position of an exit kind in the enum, for the same purpose as `method_index`.
fn exit_kind_index(kind: ExitKind) -> usize {
    match kind {
        ExitKind::Write => 0,
        ExitKind::Host => 1,
        ExitKind::HostView => 2,
        ExitKind::Socket => 3,
        ExitKind::DesktopIpc => 4,
        ExitKind::Bus => 5,
        ExitKind::Device => 6,
        ExitKind::MaskedRead => 7,
        ExitKind::Destructive => 8,
        ExitKind::Outside => 9,
        ExitKind::Privilege => 10,
        ExitKind::Persistence => 11,
        ExitKind::Upload => 12,
        ExitKind::SyncedWrite => 13,
        ExitKind::AboveRoot => 14,
        ExitKind::Secret => 15,
        ExitKind::Config => 16,
    }
}

const EXIT_KIND_COUNT: usize = 17;

/// The position of a grant in the enum, for the same purpose as `method_index`.
fn grant_index(grant: &Grant) -> usize {
    match grant {
        Grant::Write { .. } => 0,
        Grant::Host { .. } => 1,
        Grant::OpenNetwork => 2,
        Grant::Socket { .. } => 3,
        Grant::Bus { .. } => 4,
        Grant::Desktop { .. } => 5,
        Grant::Device { .. } => 6,
        Grant::Unmask { .. } => 7,
        Grant::NetView => 8,
        Grant::Unknown => 9,
        Grant::BusFilter { .. } => 10,
    }
}

const GRANT_COUNT: usize = 11;

/// The position of a launch in the enum, for the same purpose as `method_index`.
fn launch_index(launch: &Launch) -> usize {
    match launch {
        Launch::Direct => 0,
        Launch::Contained { .. } => 1,
        Launch::Unsandboxed => 2,
    }
}

const LAUNCH_COUNT: usize = 3;

/// The position of a draft part in the enum, for the same purpose as `method_index`.
fn draft_part_index(part: &DraftPart) -> usize {
    match part {
        DraftPart::Text { .. } => 0,
        DraftPart::Reasoning { .. } => 1,
        DraftPart::ToolInput { .. } => 2,
        DraftPart::Context(_) => 3,
        DraftPart::Compacting { .. } => 4,
    }
}

const DRAFT_PART_COUNT: usize = 5;

/// The position of a withdraw target in the enum, for the same purpose as
/// `method_index`.
fn withdraw_target_index(target: &WithdrawTarget) -> usize {
    match target {
        WithdrawTarget::Turn { .. } => 0,
        WithdrawTarget::NewestFromTty { .. } => 1,
    }
}

const WITHDRAW_TARGET_COUNT: usize = 2;

/// The position of an undo action in the enum, for the same purpose as `method_index`.
fn undo_action_index(action: UndoAction) -> usize {
    match action {
        UndoAction::Restore => 0,
        UndoAction::Trash => 1,
        UndoAction::Delete => 2,
        UndoAction::Skip => 3,
        UndoAction::NotCovered => 4,
        UndoAction::Command => 5,
    }
}

const UNDO_ACTION_COUNT: usize = 6;

/// The position of an undo reason in the enum, for the same purpose as `method_index`.
fn undo_reason_index(reason: UndoReason) -> usize {
    match reason {
        UndoReason::ChangedAfterTurn => 0,
        UndoReason::NoRecordAfterTurn => 1,
        UndoReason::LeftOut => 2,
        UndoReason::Masked => 3,
        UndoReason::OverLimit => 4,
        UndoReason::NewInTurn => 5,
        UndoReason::LaterTurnNotUndone => 6,
        UndoReason::NoMirror => 7,
        UndoReason::ObjectsMissing => 8,
        UndoReason::OutsideTurnCalls => 9,
        UndoReason::AlreadyDone => 10,
        UndoReason::RootReplaced => 11,
        UndoReason::Unknown => 12,
        UndoReason::CallUnfinished => 13,
        UndoReason::SnapshotFailed => 14,
        UndoReason::StoreChanged => 15,
        UndoReason::StoreFull => 16,
        UndoReason::StagedOnly => 17,
    }
}

const UNDO_REASON_COUNT: usize = 18;

/// The position of an undo refusal in the enum, for the same purpose as `method_index`.
fn undo_refused_index(refused: &UndoRefused) -> usize {
    match refused {
        UndoRefused::Off => 0,
        UndoRefused::NothingToUndo => 1,
        UndoRefused::TurnNotFound => 2,
        UndoRefused::LaterTurnsNotUndone { .. } => 3,
        UndoRefused::NoRedo => 4,
        UndoRefused::PhoneOrigin => 5,
        UndoRefused::StoreFailed => 6,
    }
}

const UNDO_REFUSED_COUNT: usize = 7;

/// The position of a block reason in the enum, for the same purpose as `method_index`.
fn block_reason_index(reason: BlockReason) -> usize {
    match reason {
        BlockReason::NotAllowed => 0,
        BlockReason::PrivateAddress => 1,
        BlockReason::SniMismatch => 2,
        BlockReason::UploadLimit => 3,
        BlockReason::Method => 4,
        BlockReason::IpLiteral => 5,
        BlockReason::NotTls => 6,
        BlockReason::BadRequest => 7,
        BlockReason::Limit => 8,
        BlockReason::Denied => 9,
        BlockReason::Unknown => 10,
    }
}

const BLOCK_REASON_COUNT: usize = 11;

/// The position of a route scope key in the enum, for the same purpose as
/// `method_index`.
fn route_scope_index(scope: &RouteScope) -> usize {
    match scope {
        RouteScope::Project(_) => 0,
        RouteScope::Outside => 1,
        RouteScope::Unknown => 2,
    }
}

const ROUTE_SCOPE_COUNT: usize = 3;

/// The position of a switch, and of the reason of a new conversation, for the same
/// purpose as `method_index`; a switch other than `New` counts once.
fn switch_index(switch: &Switch) -> usize {
    match switch {
        Switch::Back { .. } => 0,
        Switch::PickedUp { .. } => 1,
        Switch::New { reason: NewReason::Scope, .. } => 2,
        Switch::New { reason: NewReason::Idle { .. }, .. } => 3,
        Switch::New { reason: NewReason::Moved { .. }, .. } => 4,
        Switch::New { reason: NewReason::Unknown, .. } => 5,
        Switch::Unknown => 6,
    }
}

const SWITCH_COUNT: usize = 7;

/// The position of a way a conversation came to a terminal, for the same purpose as
/// `method_index`.
fn attach_how_index(how: AttachHow) -> usize {
    match how {
        AttachHow::PickedUp => 0,
        AttachHow::Resumed => 1,
        AttachHow::Unknown => 2,
    }
}

const ATTACH_HOW_COUNT: usize = 3;

/// The position of an approval answer in the enum, for the same purpose as
/// `method_index`.
fn approval_decision_index(decision: ApprovalDecision) -> usize {
    match decision {
        ApprovalDecision::Allow => 0,
        ApprovalDecision::Deny => 1,
        ApprovalDecision::AllowAlways => 2,
    }
}

const APPROVAL_DECISION_COUNT: usize = 3;

/// The position of an always rule's outcome in the enum, for the same purpose as
/// `method_index`.
fn always_saved_index(saved: &AlwaysSaved) -> usize {
    match saved {
        AlwaysSaved::Saved { .. } => 0,
        AlwaysSaved::NotSaved { .. } => 1,
        AlwaysSaved::Unknown => 2,
    }
}

const ALWAYS_SAVED_COUNT: usize = 3;

/// The position of a reason in the enum, for the same purpose as `method_index`.
fn not_saved_reason_index(reason: NotSavedReason) -> usize {
    match reason {
        NotSavedReason::Changed => 0,
        NotSavedReason::ReadOnly => 1,
        NotSavedReason::LoadCheck => 2,
        NotSavedReason::Io => 3,
        NotSavedReason::ReloadTimeout => 4,
        NotSavedReason::Unknown => 5,
    }
}

const NOT_SAVED_REASON_COUNT: usize = 6;

/// The position of a reason in the enum, for the same purpose as `method_index`.
fn bus_proxy_off_index(reason: BusProxyOff) -> usize {
    match reason {
        BusProxyOff::Config => 0,
        BusProxyOff::NoProgram => 1,
        BusProxyOff::SelfTest => 2,
        BusProxyOff::OldVersion => 3,
        BusProxyOff::Unknown => 4,
    }
}

const BUS_PROXY_OFF_COUNT: usize = 5;

/// The position of a way a command left in the hidden shell ended, for the same purpose
/// as `method_index`.
fn shell_command_end_index(how: ShellCommandEnd) -> usize {
    match how {
        ShellCommandEnd::Ended => 0,
        ShellCommandEnd::Stopped => 1,
        ShellCommandEnd::Limit => 2,
        ShellCommandEnd::Closed => 3,
        ShellCommandEnd::ShellExited => 4,
        ShellCommandEnd::Moved => 5,
        ShellCommandEnd::Unknown => 6,
    }
}

const SHELL_COMMAND_END_COUNT: usize = 7;

/// The position of an approver in the enum, for the same purpose as `method_index`.
fn approved_by_index(by: ApprovedBy) -> usize {
    match by {
        ApprovedBy::You => 0,
        ApprovedBy::YourRule => 1,
        ApprovedBy::Reviewer => 2,
        ApprovedBy::FullMode => 3,
        ApprovedBy::Unknown => 4,
    }
}

const APPROVED_BY_COUNT: usize = 5;

#[test]
fn every_sample_matches_its_frozen_file() {
    for fixture in all() {
        assert_eq!(
            read(&fixture.path),
            fixture.json,
            "fixtures/v1/{} differs from its sample; if the change is deliberate, run {BLESS} \
             and add a changelog line to docs/protocol.md",
            fixture.path
        );
    }
}

#[test]
fn every_frozen_file_round_trips_byte_for_byte() {
    for fixture in all() {
        let text = read(&fixture.path);
        let again = (fixture.round_trip)(&text)
            .unwrap_or_else(|err| panic!("fixtures/v1/{} does not decode: {err}", fixture.path));
        assert_eq!(again, text, "fixtures/v1/{} changes in a round trip", fixture.path);
    }
}

#[test]
fn every_file_has_a_sample_and_every_sample_has_a_file() {
    let root = Path::new(FIXTURES_DIR);
    let mut on_disk = BTreeSet::new();
    files_under(root, root, &mut on_disk);
    let samples: BTreeSet<String> = all().into_iter().map(|fixture| fixture.path).collect();
    assert_eq!(on_disk, samples);
}

#[test]
fn fixture_paths_are_unique() {
    let paths: Vec<String> = all().into_iter().map(|fixture| fixture.path).collect();
    let unique: BTreeSet<&String> = paths.iter().collect();
    assert_eq!(unique.len(), paths.len());
}

#[test]
fn every_method_has_a_params_sample() {
    let covered: BTreeSet<usize> = method_samples().iter().map(method_index).collect();
    assert_eq!(covered, (0..METHOD_COUNT).collect());
}

#[test]
fn every_method_has_a_result_or_item_fixture() {
    let paths: BTreeSet<String> = all().into_iter().map(|fixture| fixture.path).collect();
    for method in method_samples() {
        let stem = method_stem(&method);
        if method.is_stream() {
            let prefix = format!("{stem}_item_");
            assert!(
                paths.iter().any(|path| path.starts_with(&prefix)),
                "{stem} has no item fixture"
            );
            assert!(!paths.contains(&format!("{stem}_result.json")), "{stem} streams");
        } else {
            assert!(paths.contains(&format!("{stem}_result.json")), "{stem} has no result fixture");
        }
    }
}

#[test]
fn params_files_decode_as_their_method() {
    for method in method_samples() {
        let path = format!("{}_params.json", method_stem(&method));
        let decoded: Method = serde_json::from_str(&read(&path)).unwrap();
        assert_eq!(decoded.name(), method.name(), "{path}");
    }
}

#[test]
fn every_event_kind_has_a_sample() {
    let covered: BTreeSet<usize> = event_samples().iter().map(event_index).collect();
    assert_eq!(covered, (0..EVENT_COUNT).collect());
}

#[test]
fn every_input_wait_has_a_sample() {
    let covered: Vec<usize> = input_wait_samples().into_iter().map(input_wait_index).collect();
    assert_eq!(covered, (0..INPUT_WAIT_COUNT).collect::<Vec<_>>());
}

#[test]
fn every_mode_has_a_sample() {
    let covered: Vec<usize> = Mode::ALL.into_iter().map(mode_index).collect();
    assert_eq!(covered, (0..MODE_COUNT).collect::<Vec<_>>());
}

#[test]
fn every_judge_kind_has_a_sample() {
    let covered: Vec<usize> = JudgeKind::ALL.into_iter().map(judge_kind_index).collect();
    assert_eq!(covered, (0..JUDGE_KIND_COUNT).collect::<Vec<_>>());
    for judge in JudgeKind::ALL {
        assert_eq!(serde_json::to_value(judge).unwrap(), judge.as_str());
    }
}

#[test]
fn every_full_refusal_has_a_sample() {
    let covered: Vec<usize> = FullRefusal::ALL.into_iter().map(full_refusal_index).collect();
    assert_eq!(covered, (0..FULL_REFUSAL_COUNT).collect::<Vec<_>>());
}

#[test]
fn every_model_source_has_a_sample() {
    let covered: BTreeSet<usize> =
        models_list_sample().models.iter().map(|model| model_source_index(model.source)).collect();
    assert_eq!(covered, (0..MODEL_SOURCE_COUNT).collect());
}

#[test]
fn every_root_source_has_a_sample() {
    let roots = daemon_roots();
    let covered: BTreeSet<usize> = [roots.config, roots.data, roots.state, roots.runtime]
        .into_iter()
        .map(|root| root_source_index(root.source))
        .collect();
    assert_eq!(covered, (0..ROOT_SOURCE_COUNT).collect());
}

#[test]
fn every_exit_kind_has_a_sample() {
    let covered: Vec<usize> = ExitKind::ALL.into_iter().map(exit_kind_index).collect();
    assert_eq!(covered, (0..EXIT_KIND_COUNT).collect::<Vec<_>>());
}

#[test]
fn every_grant_and_launch_has_a_sample() {
    let grants: BTreeSet<usize> = grant_samples().iter().map(grant_index).collect();
    assert_eq!(grants, (0..GRANT_COUNT).collect());
    let launches: BTreeSet<usize> = launch_samples().iter().map(launch_index).collect();
    assert_eq!(launches, (0..LAUNCH_COUNT).collect());
}

#[test]
fn every_draft_part_has_a_sample() {
    let covered: Vec<usize> = draft_part_samples().iter().map(draft_part_index).collect();
    assert_eq!(covered, (0..DRAFT_PART_COUNT).collect::<Vec<_>>());
}

#[test]
fn every_withdraw_target_has_a_sample() {
    let covered: Vec<usize> = withdraw_target_samples().iter().map(withdraw_target_index).collect();
    assert_eq!(covered, (0..WITHDRAW_TARGET_COUNT).collect::<Vec<_>>());
}

#[test]
fn every_block_reason_has_a_sample() {
    let covered: Vec<usize> = BlockReason::ALL.into_iter().map(block_reason_index).collect();
    assert_eq!(covered, (0..BLOCK_REASON_COUNT).collect::<Vec<_>>());
}

#[test]
fn every_approval_decision_and_always_outcome_has_a_sample() {
    let decisions: Vec<usize> =
        approval_decision_samples().into_iter().map(approval_decision_index).collect();
    assert_eq!(decisions, (0..APPROVAL_DECISION_COUNT).collect::<Vec<_>>());
    let saved: Vec<usize> = always_saved_samples().iter().map(always_saved_index).collect();
    assert_eq!(saved, (0..ALWAYS_SAVED_COUNT).collect::<Vec<_>>());
    let reasons: Vec<usize> = NotSavedReason::ALL.into_iter().map(not_saved_reason_index).collect();
    assert_eq!(reasons, (0..NOT_SAVED_REASON_COUNT).collect::<Vec<_>>());
}

#[test]
fn every_route_scope_switch_and_attach_way_has_a_sample() {
    let scopes: Vec<usize> = route_scope_samples().iter().map(route_scope_index).collect();
    assert_eq!(scopes, (0..ROUTE_SCOPE_COUNT).collect::<Vec<_>>());
    let switches: Vec<usize> = switch_samples().iter().map(switch_index).collect();
    assert_eq!(switches, (0..SWITCH_COUNT).collect::<Vec<_>>());
    let ways: Vec<usize> = AttachHow::ALL.into_iter().map(attach_how_index).collect();
    assert_eq!(ways, (0..ATTACH_HOW_COUNT).collect::<Vec<_>>());
}

#[test]
fn every_bus_proxy_off_reason_has_a_sample() {
    let covered: Vec<usize> = BusProxyOff::ALL.into_iter().map(bus_proxy_off_index).collect();
    assert_eq!(covered, (0..BUS_PROXY_OFF_COUNT).collect::<Vec<_>>());
}

#[test]
fn every_shell_command_end_and_approver_has_a_sample() {
    let ends: Vec<usize> = ShellCommandEnd::ALL.into_iter().map(shell_command_end_index).collect();
    assert_eq!(ends, (0..SHELL_COMMAND_END_COUNT).collect::<Vec<_>>());
    for how in ShellCommandEnd::ALL {
        assert_eq!(serde_json::to_value(how).unwrap(), how.as_str());
    }
    let approvers: Vec<usize> = ApprovedBy::ALL.into_iter().map(approved_by_index).collect();
    assert_eq!(approvers, (0..APPROVED_BY_COUNT).collect::<Vec<_>>());
    for by in ApprovedBy::ALL {
        assert_eq!(serde_json::to_value(by).unwrap(), by.as_str());
    }
}

#[test]
fn every_undo_action_reason_and_refusal_has_a_sample() {
    let actions: Vec<usize> = UndoAction::ALL.into_iter().map(undo_action_index).collect();
    assert_eq!(actions, (0..UNDO_ACTION_COUNT).collect::<Vec<_>>());
    let reasons: Vec<usize> = UndoReason::ALL.into_iter().map(undo_reason_index).collect();
    assert_eq!(reasons, (0..UNDO_REASON_COUNT).collect::<Vec<_>>());
    let refusals: Vec<usize> = undo_refused_samples().iter().map(undo_refused_index).collect();
    assert_eq!(refusals, (0..UNDO_REFUSED_COUNT).collect::<Vec<_>>());
}

#[test]
fn event_files_decode_as_the_kind_they_are_named_after() {
    for sample in event_samples() {
        let path = event_path(&sample);
        let decoded: Event = serde_json::from_str(&read(&path)).unwrap();
        let is_unknown = matches!(decoded, Event::Unknown { .. });
        assert_eq!(is_unknown, matches!(sample, Event::Unknown { .. }), "{path}");
        if !is_unknown {
            assert_eq!(format!("events/{}.json", decoded.kind()), path);
        }
    }
}

#[test]
fn record_action_edit_round_trips() {
    let path = "events/exit_requested_floor_edit.json";
    let text = read(path);
    let decoded: Event = serde_json::from_str(&text).unwrap();
    assert_eq!(decoded, floor_edit_exit_sample());
    let Event::ExitRequested { record, .. } = &decoded else {
        panic!("{path} is not an exit_requested event");
    };
    assert_eq!(record.action.edit.as_deref(), Some("export EDITOR=nvim\nalias k=kubectl\n"));
    assert!(record.facts.targets.iter().all(|target| target.backup_before_write));
    assert!(text.contains("\"edit\": "), "the payload is a member of the action");
}

/// Rewrites every fixture from its sample. Ignored, so it runs only when asked for.
#[test]
#[ignore = "rewrites fixtures/v1; run it on purpose (see the README)"]
fn bless_fixtures() {
    for fixture in all() {
        let path = Path::new(FIXTURES_DIR).join(&fixture.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, fixture.json).unwrap();
    }
}
