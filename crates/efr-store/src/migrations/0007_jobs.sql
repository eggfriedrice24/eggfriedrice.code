-- The background jobs of each conversation (phase C), a projection of the
-- `job_started`, `job_ended` and `job_watch_matched` events, and the turns that a job
-- notice started. Rebuilt from the log like the other projections.
CREATE TABLE jobs (
    conversation_id TEXT NOT NULL,
    -- The number of the job's id: 3 for `j3`. Ids count per conversation.
    number INTEGER NOT NULL,
    turn_id TEXT NOT NULL,
    call_id TEXT NOT NULL,
    command TEXT NOT NULL,
    launch TEXT NOT NULL CHECK (json_valid(launch)),
    -- The snake_case wire name of the job's state, such as `running`.
    state TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ends_by INTEGER NOT NULL,
    ended_at INTEGER,
    exit_code INTEGER,
    stopped_by TEXT,
    output_bytes INTEGER NOT NULL,
    -- The launcher's process id and the start time of its process, from the
    -- `job_started` event: a restart signals the launcher only when both still match,
    -- and never reads them from a file that the job can write.
    launcher_pid INTEGER NOT NULL,
    launcher_start INTEGER NOT NULL,
    -- The sequence number of the `job_started` event.
    start_seq INTEGER NOT NULL,
    -- The whole `efr_protocol::JobInfo` as JSON, kept in step with the columns.
    info TEXT NOT NULL CHECK (json_valid(info)),
    PRIMARY KEY (conversation_id, number)
) STRICT;

CREATE INDEX jobs_state ON jobs (state);
CREATE INDEX jobs_start_seq ON jobs (start_seq);

-- One row per event that a job notice tells about (`job_ended`, `job_watch_matched`),
-- so a start of efrd finds the notices that no turn took yet without a scan of the log.
CREATE TABLE job_notices (
    -- The sequence number of the event that the notice tells about.
    source_seq INTEGER PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    number INTEGER NOT NULL,
    -- When the event was recorded.
    at INTEGER NOT NULL,
    -- The turn whose `job_notice` took it; NULL while it waits.
    taken_by TEXT
) STRICT;

CREATE INDEX job_notices_waiting ON job_notices (at) WHERE taken_by IS NULL;

-- 1 for a turn that a job notice started, not a prompt. Its `command_id` is NULL.
ALTER TABLE turns ADD COLUMN wake INTEGER NOT NULL DEFAULT 0;

CREATE INDEX turns_wake ON turns (started_at) WHERE wake = 1;
