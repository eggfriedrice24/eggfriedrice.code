//! `jobs` and `job_stop`: the model reads and stops the background jobs of its
//! conversation (efr's auto spec, section 13.8, phase C).
//!
//! Both are daemon tools, like the settings tool: they read efrd's job table, which
//! `efr-tools` cannot reach. Both declare no requirement, so the engine runs them with
//! no question in every mode for a turn from this machine; a phone turn gets the
//! question that every tool gets there. They act only on the jobs of the call's
//! conversation.
//!
//! The definitions here are final. Until the job runtime exists, no job starts: a
//! `shell` call with `background: true` and each call of these tools answer
//! [`NOT_READY`].

use efr_conversation::{ToolCall, ToolOutcome};
use efr_permissions::Requirements;
use efr_protocol::JobId;
use efr_provider::ToolDefinition;
use serde::Deserialize;
use serde_json::{Value, json};

/// The name of the tool that lists the jobs and reads their output.
pub(crate) const JOBS: &str = "jobs";

/// The name of the tool that stops a job.
pub(crate) const JOB_STOP: &str = "job_stop";

/// What the model reads while background jobs cannot start.
pub(crate) const NOT_READY: &str =
    "background jobs are not ready in this build of efr; run the command in the foreground";

/// The most lines of output that `jobs` shows with `tail`.
pub(crate) const MAX_TAIL_LINES: u32 = 200;

/// What `jobs` tells the model about itself.
const JOBS_DESCRIPTION: &str = "List the background jobs of this conversation, or read \
    the output of one. Without id: one line per job with its id, state, command, start, \
    time left and output size. With id: that job's line, then its output from offset (in \
    bytes), or else its last tail lines (default 50, at most 200), at most 32 KiB. It only \
    reads. The notice of a job's end comes by itself, so do not poll with it.";

/// What `job_stop` tells the model about itself.
const JOB_STOP_DESCRIPTION: &str = "Stop one background job of this conversation, such as \
    j3. It answers with the job's state and the last 50 lines of its output; a job that \
    ended already answers with its final state. Stop a job when its work is done or no \
    longer needed.";

/// The input of `jobs`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JobsInput {
    #[serde(default)]
    pub(crate) id: Option<JobId>,
    #[serde(default)]
    pub(crate) tail: Option<u32>,
    #[serde(default)]
    pub(crate) offset: Option<u64>,
}

/// The input of `job_stop`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JobStopInput {
    pub(crate) id: JobId,
}

/// The schema of a job id, as both tools take it.
fn id_schema(description: &str) -> Value {
    json!({
        "type": "string",
        "pattern": "^j[1-9][0-9]{0,8}$",
        "description": description,
    })
}

/// The definition of `jobs` for the provider request.
pub(crate) fn jobs_definition() -> ToolDefinition {
    let schema = json!({
        "type": "object",
        "properties": {
            "id": id_schema("The job, such as j3. Absent: list every job of this conversation."),
            "tail": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_TAIL_LINES,
                "description": "With id: how many of the last lines of the output to show; \
                    default 50."
            },
            "offset": {
                "type": "integer",
                "minimum": 0,
                "description": "With id: show the output from this byte on, in place of \
                    the last lines."
            }
        },
        "additionalProperties": false
    });
    ToolDefinition::function(JOBS, JOBS_DESCRIPTION, schema)
}

/// The definition of `job_stop` for the provider request.
pub(crate) fn job_stop_definition() -> ToolDefinition {
    let schema = json!({
        "type": "object",
        "properties": {
            "id": id_schema("The job to stop, such as j3.")
        },
        "required": ["id"],
        "additionalProperties": false
    });
    ToolDefinition::function(JOB_STOP, JOB_STOP_DESCRIPTION, schema)
}

/// True for the name of one of the two job tools.
pub(crate) fn is_job_tool(name: &str) -> bool {
    name == JOBS || name == JOB_STOP
}

/// What a call of a job tool needs: nothing, for an input that matches its schema. An
/// input that does not is the text that the model reads.
pub(crate) fn requirements(call: &ToolCall) -> Result<Requirements, String> {
    match call.name.as_str() {
        JOBS => {
            let input = parse::<JobsInput>(JOBS, &call.input)?;
            if input.id.is_none() && (input.tail.is_some() || input.offset.is_some()) {
                return Err(format!("tail and offset of {JOBS} need an id"));
            }
            if input.tail.is_some_and(|tail| tail == 0 || tail > MAX_TAIL_LINES) {
                return Err(format!("tail of {JOBS} is from 1 to {MAX_TAIL_LINES}"));
            }
        }
        _ => {
            parse::<JobStopInput>(JOB_STOP, &call.input)?;
        }
    }
    Ok(Requirements::none())
}

/// Runs a call of a job tool. No job starts before the job runtime, so each call
/// answers [`NOT_READY`], and names the job that it asked for.
pub(crate) fn invoke(call: &ToolCall) -> ToolOutcome {
    let id = match call.name.as_str() {
        JOBS => parse::<JobsInput>(JOBS, &call.input).ok().and_then(|input| input.id),
        _ => parse::<JobStopInput>(JOB_STOP, &call.input).ok().map(|input| input.id),
    };
    match id {
        Some(id) => ToolOutcome::error(format!("{NOT_READY}; no job {id} runs")),
        None => ToolOutcome::error(NOT_READY),
    }
}

/// The input of the tool `name`, or the text that the model reads.
fn parse<T: for<'de> Deserialize<'de>>(name: &str, input: &Value) -> Result<T, String> {
    T::deserialize(input)
        .map_err(|error| format!("the input of the {name} tool does not match its schema: {error}"))
}

#[cfg(test)]
mod tests;
