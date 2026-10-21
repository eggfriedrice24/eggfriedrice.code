use efr_conversation::Toolbox as _;
use efr_permissions::Requirements;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{JOB_STOP, JOBS, NOT_READY, job_stop_definition, jobs_definition};
use efr_protocol::{
    CallId, ConversationId, Event, JobId, JobInfo, JobState, Launch, Origin, TurnId,
};
use efr_store::Batch;

use crate::tools::tests::{call, toolbox};

#[test]
fn the_job_tools_take_a_job_id_and_nothing_else() {
    let jobs = jobs_definition();
    assert_eq!(jobs.name, JOBS);
    assert_eq!(jobs.input_schema["additionalProperties"], json!(false));
    assert_eq!(jobs.input_schema["properties"]["tail"]["maximum"], json!(200));
    assert_eq!(jobs.input_schema.get("required"), None, "jobs alone lists every job");
    let stop = job_stop_definition();
    assert_eq!(stop.name, JOB_STOP);
    assert_eq!(stop.input_schema["required"], json!(["id"]));
    assert_eq!(stop.input_schema["properties"]["id"]["pattern"], json!("^j[1-9][0-9]{0,8}$"));
    assert!(!jobs.description.contains("auto"), "the tools work in every mode");
    assert!(!stop.description.contains("auto"));
}

/// Both tools declare nothing, so the engine runs them with no question in every mode
/// for a turn from this machine (a phone turn gets the question of every tool, which
/// the engine's own tests cover). A toolbox with no job table answers that no job runs.
#[tokio::test]
async fn jobs_tool_needs_no_question() {
    let home = tempfile::tempdir().unwrap();
    let toolbox = toolbox(home.path());
    for (name, input) in [
        (JOBS, json!({})),
        (JOBS, json!({"id": "j3", "tail": 20})),
        (JOBS, json!({"id": "j3", "offset": 4096})),
        (JOB_STOP, json!({"id": "j1"})),
    ] {
        let call = call(name, input, home.path());
        assert_eq!(toolbox.requirements(&call).await, Ok(Requirements::none()), "{name}");
        let outcome = toolbox.invoke(call, &mut |_: &str, _: u64| {}).await;
        assert!(outcome.is_error, "{name}");
        assert!(outcome.output.starts_with(NOT_READY), "{}", outcome.output);
    }
    for (name, input) in [
        (JOBS, json!({"tail": 5})),
        (JOBS, json!({"id": "j3", "tail": 201})),
        (JOBS, json!({"id": "3"})),
        (JOBS, json!({"all": true})),
        (JOB_STOP, json!({})),
        (JOB_STOP, json!({"id": "j0"})),
    ] {
        let refused = toolbox.requirements(&call(name, input.clone(), home.path())).await;
        assert!(refused.is_err(), "{name} {input}");
    }
}

#[tokio::test]
async fn job_stop_of_ended_job_returns_state() {
    let home = tempfile::tempdir().unwrap();
    let clock = efr_test_support::TestClock::new();
    let store = efr_test_support::TestStore::open(clock.shared()).await.unwrap();
    let dirs = efr_stdx::paths::Dirs::new(
        home.path().join("config"),
        home.path().join("data"),
        home.path().join("state"),
        home.path().join("runtime"),
    )
    .unwrap();
    let (table, _) = crate::jobs::JobTable::new(crate::jobs::JobParts {
        clock: clock.shared(),
        writer: store.writer().clone(),
        readers: store.readers().clone(),
        dirs,
        launcher: home.path().join("efr-sbx"),
        spawn_env: crate::jobs::SpawnEnv::default(),
        full: crate::full::FullTurns::default(),
        host_ports: std::sync::Arc::new(crate::sandbox::seams::TestHostPorts::new()),
    });
    let id = |n: u128| uuid::Uuid::from_u128(n);
    // NOTE: the conversation of the tools' test calls.
    let conversation = ConversationId::from_uuid(id(1));
    let now = efr_stdx::time::Clock::now(&clock);
    let output = home.path().join("j1.out");
    std::fs::write(&output, "built\ndone\n").unwrap();
    let job = JobInfo {
        id: JobId::nth(1),
        conversation_id: conversation,
        turn_id: TurnId::from_uuid(id(2)),
        call_id: CallId::from_uuid(id(9)),
        command: "make".to_owned(),
        launch: Launch::contained(),
        state: JobState::Running,
        started_at: now,
        ends_by: now,
        ended_at: None,
        exit_code: None,
        stopped_by: None,
        output,
        output_bytes: 0,
        watch: None,
        wakes: 0,
        forwards: Vec::new(),
        moved_from: None,
    };
    let batch = Batch::new()
        .event(
            conversation,
            Event::ConversationCreated { origin: Origin::Shell, tty: None, route: None },
        )
        .event(
            conversation,
            Event::JobStarted {
                turn_id: job.turn_id,
                call_id: job.call_id,
                job: job.clone(),
                launcher_pid: 0,
                launcher_start: 0,
            },
        )
        .event(
            conversation,
            Event::JobEnded {
                job_id: JobId::nth(1),
                state: JobState::Exited,
                exit_code: Some(0),
                stopped_by: None,
                output_bytes: 11,
                ran_secs: 3,
                tail: "built\ndone".to_owned(),
                surface_changes: Vec::new(),
                quarantined: false,
                survivors: Vec::new(),
                launcher_lost: false,
                output_truncated: false,
                blocked: Vec::new(),
                blocked_more: 0,
                bus_refused: Vec::new(),
                bus_refused_more: 0,
            },
        );
    store.writer().append(batch).await.unwrap();

    let stop = call(JOB_STOP, json!({"id": "j1"}), home.path());
    let outcome = super::invoke(Some(&table), &stop).await;
    assert!(!outcome.is_error, "{}", outcome.output);
    assert!(
        outcome.output.starts_with("job j1 had ended already\nj1 exited: make (exited with exit 0"),
        "{}",
        outcome.output
    );
    assert!(outcome.output.ends_with("built\ndone"), "{}", outcome.output);
    let unknown =
        super::invoke(Some(&table), &call(JOB_STOP, json!({"id": "j2"}), home.path())).await;
    assert!(unknown.is_error);
    assert!(unknown.output.starts_with("no job j2 in this conversation"), "{}", unknown.output);
    let listed = super::invoke(Some(&table), &call(JOBS, json!({}), home.path())).await;
    assert!(listed.output.starts_with("j1 exited: make"), "{}", listed.output);
}
