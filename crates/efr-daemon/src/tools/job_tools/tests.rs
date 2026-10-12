use efr_conversation::Toolbox as _;
use efr_permissions::Requirements;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{JOB_STOP, JOBS, NOT_READY, job_stop_definition, jobs_definition};
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

#[tokio::test]
async fn the_job_tools_need_nothing_and_answer_not_ready() {
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
