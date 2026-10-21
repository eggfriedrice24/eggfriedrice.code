//! `shell`: a command line in the conversation's hidden zsh, through
//! `efr_shell::CommandRunner`.

mod declare;
mod prompts;
mod reads;
mod words;
mod writes;

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use efr_protocol::{BlockReason, Blocked, BusKind, BusRefusal, InputWait, Needs, SandboxSummary};
use efr_shell::{
    CommandResult, CommandRunner, Completion, OutputUpdate, RunMode, RunProgress, RunRequest,
    ShellError,
};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::Deserialize;
use serde_json::Value;

use crate::output::{DEFAULT_OUTPUT_LIMIT, truncate_middle};
use crate::tool::parse_input;
use crate::{Tool, ToolContext, ToolError, ToolOutputSink, ToolRequirements, ToolResult, ToolSpec};

/// The input of `shell`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ShellInput {
    /// The command line, as you would type it at a zsh prompt. It may span several
    /// lines.
    command: String,
    /// How many seconds to wait for the command to end before answering with what is
    /// known; the command keeps running after that. Default 30, at most 600.
    #[serde(default)]
    timeout_seconds: Option<u64>,
    /// True to type the command into a shell that runs inside the hidden one (one you
    /// started with `sudo -i`, `bash` or `ssh`), which has no efr integration. The auto
    /// mode refuses it.
    #[serde(default)]
    nested_shell: bool,
    /// Only in the auto mode, after a command failed in the sandbox for access (a
    /// read-only file system, no network, permission denied). Ask for the narrowest
    /// member that fits, with a short reason; efr decides. efr already reads what the
    /// line itself shows, such as sudo, a push or systemctl, so leave that out. Other
    /// modes ignore it.
    #[serde(default)]
    #[schemars(with = "NeedsInput")]
    needs: Option<Needs>,
    /// True starts the line as a background job and answers at once with the job's id
    /// and its output file. The job runs on after this turn, at a low priority and with
    /// no terminal, until it ends, max_minutes passes or job_stop stops it. You get a
    /// notice when it ends; do not poll. Outside auto, the user is asked before each
    /// job starts. Not with timeout_seconds or nested_shell.
    #[serde(default)]
    background: bool,
    /// The time limit of the job in minutes, from 1 to 1440; default 60. efr cuts a
    /// longer limit to its own maximum (480 unless the user changed it). Only with
    /// background.
    #[serde(default)]
    #[schemars(range(min = 1, max = 1440))]
    max_minutes: Option<u32>,
    /// Wakes you when a line of the job's output matches pattern, a regular expression.
    /// Only with background.
    #[serde(default)]
    watch: Option<WatchInput>,
}

/// The `watch` member of a `shell` call: a pattern that wakes the model when a line of
/// the job's output matches it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WatchInput {
    /// A regular expression of at most 200 characters, such as `error|panic`.
    #[schemars(length(min = 1, max = 200))]
    pub pattern: String,
    /// How many matches may wake you, from 1 to 10; default 1. efr cuts a higher
    /// number to its own maximum.
    #[serde(default)]
    #[schemars(range(min = 1, max = 10))]
    pub max_wakes: Option<u8>,
}

impl WatchInput {
    /// The longest pattern, in characters.
    pub const MAX_PATTERN_CHARS: usize = 200;
    /// The most wakes that a call may ask for.
    pub const MAX_WAKES: u8 = 10;
}

/// What a `shell` call with `background: true` asks for, besides its line.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct JobRequest {
    /// The time limit in minutes that the model asked for, from 1 to
    /// [`JobRequest::MAX_MINUTES`]; `None` takes the default of `jobs.default_minutes`.
    pub max_minutes: Option<u32>,
    /// The watch pattern.
    pub watch: Option<WatchInput>,
}

impl JobRequest {
    /// The longest time limit that a call may name, in minutes. The daemon cuts it to
    /// `jobs.max_minutes`.
    pub const MAX_MINUTES: u32 = 1_440;
}

/// The job that the `shell` call with `input` asks for; `None` for a call that runs in
/// the foreground. An input that does not match the schema, or whose job members do
/// not fit together, is the error that the model reads.
pub fn job_request(input: &Value) -> Result<Option<JobRequest>, ToolError> {
    let input: ShellInput = parse_input(ShellTool::NAME, input)?;
    let line = words::split(&input.command);
    check_job(&input, interactive(&input, &line))?;
    Ok(input
        .background
        .then_some(JobRequest { max_minutes: input.max_minutes, watch: input.watch }))
}

/// The answer of a `shell` call that started the job `id` with `command`, whose output
/// goes to `output` (as the model reads it, such as `$SCRATCH/jobs/j3.out`) and which
/// ends after `minutes` at most. `clamped` says that efr cut a longer limit that the
/// model asked for (efr's auto spec, section 13.8).
pub fn job_started_text(
    id: &str,
    command: &str,
    output: &str,
    minutes: u32,
    clamped: bool,
) -> String {
    let command: String = {
        let flat = command.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.chars().count() <= JOB_COMMAND_CHARS {
            flat
        } else {
            let mut cut: String = flat.chars().take(JOB_COMMAND_CHARS).collect();
            cut.push_str("...");
            cut
        }
    };
    let unit = if minutes == 1 { "minute" } else { "minutes" };
    let limit = if clamped {
        format!("time limit: {minutes} {unit} (the most that efr allows)")
    } else {
        format!("time limit: {minutes} {unit}")
    };
    format!(
        "job {id} started: {command}\noutput: {output}\n{limit}\nYou will get a notice when it \
         ends. Do not poll. Do other work, or end your response."
    )
}

/// The most characters of the command that the answer of a job start names.
const JOB_COMMAND_CHARS: usize = 200;

/// Why the job members of `input` do not fit together, or `Ok` (efr's auto spec,
/// section 13.8): `max_minutes` and `watch` need `background: true`, and a job takes
/// no `timeout_seconds`, no `nested_shell` and no line that may wait for input at the
/// terminal (`interactive`), because its stdin is `/dev/null`.
fn check_job(input: &ShellInput, interactive: bool) -> Result<(), ToolError> {
    const NEEDS_BACKGROUND: &str = "needs background: true";
    const JOB: &str = "background: true";
    let refuse =
        |member: &'static str, reason: &'static str| Err(ToolError::InvalidJob { member, reason });
    if !input.background {
        if input.max_minutes.is_some() {
            return refuse("max_minutes", NEEDS_BACKGROUND);
        }
        if input.watch.is_some() {
            return refuse("watch", NEEDS_BACKGROUND);
        }
        return Ok(());
    }
    if input.timeout_seconds.is_some() {
        return refuse(
            JOB,
            "does not take timeout_seconds; a job runs until it ends or max_minutes passes",
        );
    }
    if input.nested_shell {
        return refuse(JOB, "does not take nested_shell; run that line in the foreground");
    }
    if interactive {
        return refuse(
            JOB,
            "cannot run a line that may wait for input at the terminal, such as sudo, ssh or \
             an editor: a job has no terminal; run that part in the foreground",
        );
    }
    if input.max_minutes.is_some_and(|minutes| minutes == 0 || minutes > JobRequest::MAX_MINUTES) {
        return refuse("max_minutes", "is from 1 to 1440");
    }
    if let Some(watch) = &input.watch {
        let chars = watch.pattern.chars().count();
        if chars == 0 || chars > WatchInput::MAX_PATTERN_CHARS {
            return refuse("watch.pattern", "is 1 to 200 characters");
        }
        if watch.max_wakes.is_some_and(|wakes| wakes == 0 || wakes > WatchInput::MAX_WAKES) {
            return refuse("watch.max_wakes", "is from 1 to 10");
        }
    }
    Ok(())
}

/// True when the line of `input` (split as `line`) may wait for input at the terminal:
/// a nested shell, or a program such as `sudo`, `ssh` or an editor. A line that the
/// split cannot read whole is also scanned without its quotes.
fn interactive(input: &ShellInput, line: &words::Line) -> bool {
    let commands = programs_of(line);
    let mut interactive = input.nested_shell
        || commands.iter().any(|(program, _)| INTERACTIVE.contains(&program.as_str()));
    if line.opaque {
        interactive |= programs(&input.command).iter().any(|program| INTERACTIVE.contains(program));
    }
    interactive
}

/// The schema of `needs` as the model sees it: [`Needs`] with the limits that
/// `Needs::check` holds, and what each member is for. The descriptions steer the model
/// to the narrowest member: a model that does not know that a bus or a write grant
/// exists asks to run outside the sandbox, with the user's full rights.
struct NeedsInput;

/// What each member of `needs` is for, as the model reads it.
const NEEDS_MEMBERS: &[(&str, &str)] = &[
    (
        "write",
        "Paths to write outside the sandbox's write roots, such as one file in ~. A grant \
         opens only these paths.",
    ),
    (
        "hosts",
        "Hosts that the command needs, such as gitlab.com. A server that the user's browser \
         must reach runs as a background job: in auto, efr forwards its port unless \
         jobs.forward_ports is off. For a tool that does not use HTTPS_PROXY, such as psql, \
         name localhost or the host with its port.",
    ),
    ("sockets", "Unix sockets to connect to, such as a tool's own socket."),
    (
        "bus",
        "A message bus: system for systemctl, hostnamectl, timedatectl and other D-Bus \
         clients of the system; session for the user's own services.",
    ),
    ("device", "A device node, such as /dev/nvme0n1."),
    ("unmask", "Masked paths to read, such as a project's .env, which read as empty."),
    (
        "outside",
        "Runs the whole line outside the sandbox with the user's full rights: files, \
         secrets and network. Ask for it only when no other member fits, such as for a \
         debugger that attaches to a process or git over ssh, and say why in reason.",
    ),
    ("reason", "Why the command needs this, in one sentence. The user reads it."),
];

impl JsonSchema for NeedsInput {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("Needs")
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        let mut schema = Needs::input_schema();
        if let Some(Value::Object(properties)) = schema.get_mut("properties") {
            for (name, text) in NEEDS_MEMBERS {
                if let Some(Value::Object(member)) = properties.get_mut(*name) {
                    member.insert("description".to_owned(), Value::from(*text));
                }
            }
        }
        match schema {
            Value::Object(map) => Schema::from(map),
            _ => Schema::default(),
        }
    }
}

/// How a call's line is typed: into a nested shell with sentinels, or into the hidden
/// zsh as its marks allow.
fn mode(input: &ShellInput) -> RunMode {
    if input.nested_shell { RunMode::Sentinel } else { RunMode::Auto }
}

/// Runs a command line in the conversation's hidden zsh: one long-lived interactive
/// shell with the user's environment and startup files, whose working directory and
/// variables carry over from call to call. The user does not see its screen; a person
/// who follows the turn can answer a command that waits for input. A call that the user
/// approved because it may wait for input keeps running past its timeout while such a
/// person follows, up to [`ToolContext::interactive_limit`].
///
/// It declares the command line, `interactive` for commands that may wait for input
/// at the terminal (`sudo`, `ssh`, an editor, or any nested shell), `may_prompt` for
/// commands that may ask for a password through ssh (`rsync` to another host, `scp`,
/// `git` over the network, a remote shell in `-e` or `GIT_SSH_COMMAND`) and `network`
/// for commands that usually reach the network (`curl`, `git pull`, package installs).
/// They come from the command's words and are a heuristic: the engine judges the
/// command line itself too.
///
/// It also declares the paths the line names, resolved against the hidden shell's
/// directory: every operand of every program as a read (so `cat ~/.ssh/id_ed25519`
/// meets the secrets rule although `cat` may run freely), recursive searches, listings
/// and globs as reads of everything below their directory, the working directory of a
/// search that names no path, and output redirections and the operands of the writer
/// programs (`rm`, `rmdir`, `mkdir`, `touch`, `mv`, `cp`, `ln`, `chmod`, `truncate`,
/// `tee`, and `git rm`, `git mv` and `git worktree add`) as writes. What the text
/// cannot show, such as the files a script or a build writes, it cannot declare; the
/// engine asks for a line it cannot read.
#[derive(Debug, Clone)]
pub struct ShellTool {
    runner: Arc<dyn CommandRunner>,
    output_limit: usize,
    default_timeout: Duration,
    max_timeout: Duration,
}

impl ShellTool {
    /// The tool's name.
    pub const NAME: &'static str = "shell";

    /// The tool over `runner` (the daemon's `ShellSessions`), with a 32 KiB output
    /// limit, a 30 second default timeout and a 10 minute maximum.
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        ShellTool {
            runner,
            output_limit: DEFAULT_OUTPUT_LIMIT,
            default_timeout: RunRequest::DEFAULT_TIMEOUT,
            max_timeout: Duration::from_secs(600),
        }
    }

    /// Sets the most bytes of output the model sees.
    #[must_use]
    pub fn with_output_limit(mut self, output_limit: usize) -> Self {
        self.output_limit = output_limit;
        self
    }

    /// Sets the timeout of a call that names none, and the largest a call may name.
    #[must_use]
    pub fn with_timeouts(mut self, default: Duration, max: Duration) -> Self {
        self.default_timeout = default;
        self.max_timeout = max;
        self
    }

    /// How long a call with `input` waits for its command.
    fn timeout(&self, input: &ShellInput) -> Duration {
        input
            .timeout_seconds
            .map_or(self.default_timeout, Duration::from_secs)
            .min(self.max_timeout)
    }

    /// The model's answer for `result`, after the command ran for `waited`, with what
    /// the sandbox reports after it. `contained` is true for a call that ran in the
    /// auto sandbox, not in the exit child.
    fn render(&self, result: &CommandResult, waited: Duration, contained: bool) -> ToolResult {
        let mut summary = result.sandbox.as_ref().map(|sandbox| sandbox.summary.clone());
        let mut tainted = summary.as_ref().is_some_and(|summary| !summary.survivors.is_empty());
        if result.completion == Completion::SandboxFailed {
            match sandbox_failure(result) {
                // The client shows why on its own line (efr's auto spec, section 14.7).
                SandboxFailure::NotStarted(reason) => {
                    summary.get_or_insert_default().setup_error = Some(reason);
                }
                // NOTE: a launcher lost after the start may leave the call's processes
                // stopped or running on the shell's terminal (efr's auto spec, 3.13).
                SandboxFailure::Lost(_) => tainted = true,
            }
        }
        let rendered = self
            .render_run(result, waited, contained)
            .with_sandbox(summary)
            .with_sandbox_failed(result.completion == Completion::SandboxFailed)
            .with_shell_tainted(tainted);
        match &result.sandbox {
            Some(sandbox) if result.completion != Completion::SandboxFailed => {
                let failed =
                    result.completion == Completion::Finished && result.exit_code != Some(0);
                let mut notes = Vec::new();
                if let Some(hidden) = &sandbox.hidden_cwd {
                    notes.push(format!(
                        "[efr: the shell is in {}, which the sandbox hides; this call started \
                         in $SCRATCH.]",
                        hidden.display()
                    ));
                }
                if sandbox.started && !sandbox.state_kept {
                    notes.push("[efr: the shell state of this call was not kept.]".to_owned());
                }
                notes.push(sandbox_notes(&sandbox.summary, failed));
                notes.retain(|note| !note.is_empty());
                let notes = notes.join("\n");
                if notes.is_empty() {
                    rendered
                } else {
                    ToolResult { output: format!("{}\n{notes}", rendered.output), ..rendered }
                }
            }
            _ => rendered,
        }
    }

    /// The model's answer for the run of `result`, after the command ran for `waited`.
    fn render_run(&self, result: &CommandResult, waited: Duration, contained: bool) -> ToolResult {
        let cut = truncate_middle(&result.output, self.output_limit);
        let mut text = cut.text;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        let cwd = result.cwd_after.display();
        let seconds = waited.as_secs();
        let is_error = match result.completion {
            Completion::Finished => {
                let status =
                    result.exit_code.map_or_else(|| "unknown".to_owned(), |code| code.to_string());
                text.push_str(&format!("[exit code {status}, cwd {cwd}]"));
                result.exit_code != Some(0)
            }
            Completion::NotStarted => {
                text.push_str(&format!(
                    "[the command did not run: the line was empty, did not parse, or was \
                     unfinished (an unclosed quote) and was cancelled; cwd {cwd}]"
                ));
                true
            }
            Completion::Interactive => {
                text.push_str(&format!(
                    "[still running after {seconds}s and waiting for input. The user can \
                     answer it in their terminal only while they follow the turn, and nobody \
                     did in time. The command keeps running, and the next call waits for it \
                     to end. cwd {cwd}. The screen ends with:]\n"
                ));
                text.push_str(result.screen_tail.as_deref().unwrap_or_default());
                false
            }
            // NOTE: the user is never asked about a full-screen program, so the text
            // must not say that they could have answered it.
            Completion::FullScreen => {
                text.push_str(&format!(
                    "[still running after {seconds}s: a full-screen program (an editor, a \
                     pager, top) waits at the terminal. The user cannot reach it from their \
                     terminal yet, so it runs until it ends by itself, and the next call \
                     waits for that. Tell the user. Prefer commands that do not take over the \
                     screen. cwd {cwd}. The screen ends with:]\n"
                ));
                text.push_str(result.screen_tail.as_deref().unwrap_or_default());
                false
            }
            // NOTE: sandboxed code can fake a password prompt, so efr never types a
            // secret into a contained call; only an approved exit runs outside.
            Completion::Unanswered if contained => {
                text.push_str(&format!(
                    "[stopped: the command asked for a secret inside the sandbox; efr does \
                     not type secrets into sandboxed commands; ask with needs.outside if it \
                     needs one. cwd {cwd}]"
                ));
                true
            }
            Completion::SandboxFailed => {
                match sandbox_failure(result) {
                    SandboxFailure::NotStarted(reason) => text.push_str(&format!(
                        "[the sandbox could not start: {reason}. The command did not run. cwd \
                         {cwd}]"
                    )),
                    SandboxFailure::Lost(reason) => text.push_str(&format!(
                        "[the sandbox's launcher failed: {reason}. The command may have run; \
                         check what it did before you run it again. cwd {cwd}]"
                    )),
                }
                true
            }
            Completion::Unanswered => {
                text.push_str(&format!(
                    "[stopped: the command asked for hidden input, such as a password, and no \
                     user could answer it at a terminal, so efr interrupted it. Ask the user \
                     to run the command in their own terminal, or to follow this turn in \
                     their terminal while you try again. cwd {cwd}]"
                ));
                true
            }
            _ => {
                text.push_str(&format!(
                    "[still running after {seconds}s; the next call waits for it to end. cwd \
                     {cwd}. The screen ends with:]\n"
                ));
                text.push_str(result.screen_tail.as_deref().unwrap_or_default());
                false
            }
        };
        ToolResult::ok(text)
            .with_truncated(cut.truncated || result.truncated)
            .with_exit_code(result.exit_code)
            .with_error(is_error)
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::for_input::<ShellInput>(
            Self::NAME,
            "Run a command line in this conversation's own hidden zsh. The shell lives \
             as long as the conversation: cd, exported variables and aliases carry over \
             between calls, and it has the user's environment and startup files. It \
             starts in the user's working directory. The user does not see this shell, \
             and nobody can use a full-screen program in it: no editor works there, and \
             a command that opens one (git commit without -m, crontab -e) fails at once \
             with a message that says so. Give a command its text yourself (git commit \
             -m) or use write_file. In the auto permission mode the line runs in a \
             sandbox, and efr decides what leaves it. Run what the user asked; do not ask \
             the user for permission first. If the command fails for access (read-only \
             file system, no network, a 403 from efr's proxy when it is on, permission \
             denied), call shell again with the same \
             line and the narrowest needs that fits, and a short reason. needs.outside runs \
             the whole line with the user's full rights, so ask for it only when no \
             narrower member fits. A command that runs outside the sandbox does not see \
             exports or functions that sandboxed commands made, and must be alone in its \
             line apart from read-only helpers such as echo. In auto, nested_shell is \
             refused. background: true starts the line as a job and answers at once with a \
             job id and an output file; the job runs until it ends, max_minutes passes \
             (default 60, at most 480, unless the user changed them), or job_stop stops it. \
             You get a notice when it ends; do not poll and do not sleep. watch wakes you on \
             an output line that matches a pattern. Processes that a normal call starts with \
             &, nohup or setsid stop when the call ends. The \
             answer has the output, the exit code and the directory after the command. A \
             command still running at the timeout keeps running, and the next call waits \
             for it to end. When a command waits for input (a sudo password, a [Y/n] \
             question), the user can answer it in their terminal while they follow the \
             turn. On a plain terminal an answer to a question shows in the output and a \
             password never does; behind a program that runs another one on a terminal of \
             its own (sudo, ssh, docker exec), the program on that inner terminal decides \
             whether an answer is shown. A call that the user approved because it may \
             wait for input keeps running past timeout_seconds while the user follows the \
             turn, up to their interactive limit (60 minutes unless they changed it). A \
             command that waits for a password while nobody follows the turn is stopped \
             at once. Set nested_shell when the command must go to a shell you started \
             inside this one, such as sudo -i or ssh.",
        )
    }

    fn requirements(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<ToolRequirements, ToolError> {
        let input: ShellInput = parse_input(Self::NAME, input)?;
        let line = words::split(&input.command);
        let commands = programs_of(&line);
        let interactive = interactive(&input, &line);
        let may_prompt = prompts::may_prompt(&line, &input.command);
        let mut network = commands.iter().any(|(program, args)| reaches_network(program, args))
            || networked_git(&input.command);
        if line.opaque {
            // NOTE: the split may miss a program inside quotes or a substitution, so the
            // coarse scan that ignores quotes adds what it finds.
            let coarse = programs(&input.command);
            network |= coarse.iter().any(|program| NETWORK.contains(program));
        }
        if let Some(needs) = &input.needs {
            needs.check().map_err(|source| ToolError::InvalidNeeds { source })?;
        }
        check_job(&input, interactive)?;
        let declared = declare::declared(&line, ctx.command_dir(), ctx.home.path());
        let timeout = self.timeout(&input);
        let mut requirements = ToolRequirements::none()
            .with_command(input.command)
            .with_command_dir(ctx.command_dir())
            .with_interactive(interactive)
            .with_may_prompt(may_prompt)
            .with_network(network)
            .with_needs(input.needs)
            .with_nested(input.nested_shell)
            .with_timeout(timeout)
            .with_background(input.background)
            .with_max_minutes(input.max_minutes)
            .with_watch(input.watch.is_some());
        for path in declared.reads {
            requirements = requirements.with_read(path);
        }
        for path in declared.trees {
            requirements = requirements.with_read_tree(path);
        }
        for path in declared.writes {
            requirements = requirements.with_write(path);
        }
        Ok(requirements)
    }

    /// A call takes a manual input unless its line goes to a nested shell or starts a
    /// shell or a REPL: the hidden shell refuses one there, because the text would run
    /// as a command line once the command ends. A background job takes none either: its
    /// stdin is `/dev/null`, and the call answers at once. An input that does not parse
    /// takes none; its call fails anyway.
    fn takes_manual_input(&self, input: &Value) -> bool {
        parse_input::<ShellInput>(Self::NAME, input).is_ok_and(|input| {
            !input.background && efr_shell::takes_manual_answers(mode(&input), &input.command)
        })
    }

    async fn invoke(
        &self,
        ctx: ToolContext,
        input: Value,
        out: &mut dyn ToolOutputSink,
    ) -> Result<ToolResult, ToolError> {
        let input: ShellInput = parse_input(Self::NAME, &input)?;
        let timeout = self.timeout(&input);
        let mode = mode(&input);
        // NOTE: the hard or the soft flag: the hidden shell keeps a password wait of a
        // command left running only for a flagged line that a person approved (DS52).
        let line = words::split(&input.command);
        let flagged = interactive(&input, &line) || prompts::may_prompt(&line, &input.command);
        let request = RunRequest::new(input.command, ctx.cwd.clone())
            .with_timeout(timeout)
            .with_mode(mode)
            .with_call(ctx.ids.call_id)
            .with_forget_credentials(ctx.forget_credentials)
            .with_forget_before(ctx.forget_before)
            .with_interactive_limit(ctx.interactive_limit)
            .with_input_limit(ctx.input_limit)
            .with_approval(flagged, ctx.approved_by)
            .with_sandbox(ctx.sandbox.clone());
        let mut progress = Relay { out };
        let started = ctx.clock.now();
        match self.runner.run_command(ctx.ids.conversation_id, request, &mut progress).await {
            Ok(result) => {
                // NOTE: a call that a person followed may have run past its timeout, and
                // the model must learn how long it really waited.
                let ran =
                    Duration::try_from(ctx.clock.now().duration_since(started)).unwrap_or_default();
                let contained = ctx.sandbox.as_ref().is_some_and(efr_shell::SandboxRun::contained);
                Ok(self.render(&result, timeout.max(ran), contained))
            }
            // NOTE: nothing clears a busy shell from here: an interrupt reaches the shell
            // only for a call in flight, and this one never started.
            Err(ShellError::Busy { .. }) => Ok(ToolResult::error(
                "The shell is busy: an unfinished command line or another run holds it, and \
                 nobody can clear it from here now. The command was not run. Tell the user; \
                 a new conversation (,new in their terminal) gets a fresh shell.",
            )),
            Err(ShellError::NotReady { fresh, .. }) => {
                Ok(ToolResult::error(not_ready_message(fresh, timeout)))
            }
            Err(ShellError::Exited { .. }) => Ok(ToolResult::error(
                "The shell exited while the command ran. The next call starts a new shell in \
                 the user's working directory; variables and the directory of the old one are \
                 gone.",
            )),
            Err(ShellError::NoIntegration { .. }) => Ok(ToolResult::error(NO_INTEGRATION)),
            Err(ShellError::InvalidCommand { reason }) => Ok(ToolResult::error(format!(
                "The command cannot be typed into a shell: {reason}."
            ))),
            Err(source) => Err(ToolError::Shell { source }),
        }
    }
}

/// The model's answer for a call of full mode in a hidden shell without efr's zsh
/// integration, which could not clear sudo's cached password before the command.
const NO_INTEGRATION: &str = "full mode needs efr's zsh integration in the hidden shell, \
     so it can clear sudo's cached password; this shell started without it. The command \
     was not run. Tell the user.";

/// The model's answer for a call whose shell did not reach a ready prompt within
/// `timeout`, so the command was not run. `fresh` is true when no command ran in the
/// shell yet: then no earlier command can hold it.
pub fn not_ready_message(fresh: bool, timeout: Duration) -> String {
    let seconds = timeout.as_secs();
    if fresh {
        format!(
            "The shell did not show a ready prompt within {seconds}s, so the command was not \
             run. No command ran in this shell yet: the user's startup files (.zshrc) may \
             wait for input or draw a prompt that efr cannot read. Tell the user."
        )
    } else {
        format!(
            "The shell did not reach its prompt within {seconds}s, so the command was not \
             run: an earlier command is still running. If it is a shell you started inside \
             this one (sudo -i, bash, ssh), call again with nested_shell set."
        )
    }
}

/// The line after the output of a contained call that failed (spec 4.3). efr never
/// reads the output for a denial, because the command wrote it: injected text must
/// not steer the model toward a grant. The note lists no limits of the sandbox, so the
/// model does not read it as a refusal.
const SANDBOX_NOTE: &str = "[efr: this ran in the auto sandbox (writes in the turn's \
                            project, registered projects that the command names, $SCRATCH, \
                            /tmp and the tool caches; secrets read as empty). If it failed \
                            for access, call shell again with the narrowest needs; efr \
                            decides.]";

/// How a sandboxed run failed, with the launcher's reason.
enum SandboxFailure {
    /// The command did not run: the setup failed, or the launcher never started it.
    NotStarted(String),
    /// The launcher failed after it started the command, which may have run.
    Lost(String),
}

/// Why a sandboxed run failed. The output is never read for a reason, because
/// sandboxed code may have written it.
fn sandbox_failure(result: &CommandResult) -> SandboxFailure {
    let Some(sandbox) = result.sandbox.as_ref() else {
        return SandboxFailure::NotStarted("the launcher ended before it started".to_owned());
    };
    match (&sandbox.setup_error, &sandbox.launch_error) {
        (Some(reason), _) => SandboxFailure::NotStarted(reason.trim().to_owned()),
        (None, Some(reason)) => SandboxFailure::Lost(reason.trim().to_owned()),
        (None, None) => SandboxFailure::Lost("the launcher ended without its result".to_owned()),
    }
}

/// What the launcher reported about a call, for the model: names only, never values.
fn sandbox_notes(summary: &SandboxSummary, failed: bool) -> String {
    let mut notes: Vec<String> = Vec::new();
    if summary.confined && failed {
        notes.push(SANDBOX_NOTE.to_owned());
    }
    if !summary.kept_out.is_empty() {
        notes.push(format!(
            "[efr: these exports stay in the sandbox: {}. Later contained calls see them; \
             the hidden shell and a command that runs outside the sandbox do not.]",
            summary.kept_out.join(", ")
        ));
    }
    if !summary.dropped.is_empty() {
        notes.push(format!(
            "[efr: these exports were dropped, and no later call sees them: {}.]",
            summary.dropped.join(", ")
        ));
    }
    if !summary.background_stopped.is_empty() {
        notes.push(format!(
            "[efr: these background jobs stopped when the command ended: {}. To keep a \
             process running, call shell with background: true.]",
            summary.background_stopped.join(", ")
        ));
    }
    if !summary.survivors.is_empty() {
        notes.push(format!(
            "[efr: these processes of the command could not be ended: {}. They may still \
             read the terminal, so the next call starts in a new hidden shell, in the \
             user's working directory, without the variables of this one.]",
            summary.survivors.join(", ")
        ));
    }
    // NOTE: the call's own limit is efrd's; the note names the key without the number.
    notes.extend(blocked_note(&summary.blocked, summary.blocked_more, 0));
    let quarantined: Vec<String> = summary
        .surface_changes
        .iter()
        .filter(|change| change.quarantined)
        .map(|change| change.path.display().to_string())
        .collect();
    if !quarantined.is_empty() {
        notes.push(format!(
            "[efr: the command changed git settings that run code; efr moved them out of \
             the way, and the user decides whether to keep them: {}.]",
            quarantined.join(", ")
        ));
    }
    notes.join("\n")
}

/// The model's note for the refusals of efr's network proxy in a call or a job (phase
/// D2, DP28): one sentence per reason, in the order in which the reasons first come,
/// each with the hosts that got it, and one for the refusals that `blocked` leaves out
/// (`more`). `max_upload_bytes` is the upload limit of the call; 0 leaves the number out.
/// `None` when the proxy refused nothing.
pub fn blocked_note(blocked: &[Blocked], more: u32, max_upload_bytes: u64) -> Option<String> {
    if blocked.is_empty() && more == 0 {
        return None;
    }
    let mut reasons: Vec<(BlockReason, Vec<String>)> = Vec::new();
    for entry in blocked {
        let host = format!("{}:{}", entry.host, entry.port);
        match reasons.iter_mut().find(|(reason, _)| *reason == entry.reason) {
            Some((_, hosts)) if !hosts.contains(&host) => hosts.push(host),
            Some(_) => {}
            None => reasons.push((entry.reason, vec![host])),
        }
    }
    let mut lines: Vec<String> = reasons
        .into_iter()
        .map(|(reason, hosts)| block_sentence(reason, &hosts.join(", "), max_upload_bytes))
        .collect();
    if more > 0 {
        let connections = if more == 1 { "connection" } else { "connections" };
        lines.push(format!("[efr: the network proxy also refused {more} more {connections}.]"));
    }
    Some(lines.join("\n"))
}

/// The model's note about the bus calls that efr's bus filter refused in a call or a
/// job (phase E, DE11), or that the filter stopped during it (DE8): one sentence that
/// names each refused call as data, with how to ask for the whole bus, and one that
/// says the filter stopped. `refused` comes from the filter's log, with each name,
/// member and path checked against the D-Bus grammar; `more` counts the refusals that
/// it leaves out. `None` when there is nothing to say.
pub fn bus_note(refused: &[BusRefusal], more: u32, stopped: bool) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    let count = refused
        .iter()
        .fold(u64::from(more), |count, entry| count.saturating_add(u64::from(entry.count)));
    if count > 0 {
        let calls: Vec<String> = refused.iter().map(refused_call).collect();
        let what = if count == 1 { "a call".to_owned() } else { format!("{count} calls") };
        let mut list = calls.join("; ");
        if more > 0 && !list.is_empty() {
            list.push_str(&format!("; {more} more"));
        }
        let listed = if list.is_empty() { String::new() } else { format!(": {list}") };
        lines.push(format!(
            "[efr: the bus filter refused {what}{listed}. For the whole bus for one command, \
             call shell again with needs.bus.]"
        ));
    }
    if stopped {
        lines.push(
            "[efr: the bus filter stopped during the command, so its bus calls after that \
             failed.]"
                .to_owned(),
        );
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// One refused call of [`bus_note`]: `bus=system name=N member=M path=P`, without an
/// empty member and path (a refusal of the bus itself), and with `(N times)`.
fn refused_call(entry: &BusRefusal) -> String {
    let bus = match entry.bus {
        BusKind::System => "system",
        BusKind::Session => "session",
        _ => "unknown",
    };
    let mut text = format!("bus={bus} name={}", entry.name);
    if !entry.member.is_empty() {
        text.push_str(&format!(" member={}", entry.member));
    }
    if !entry.path.is_empty() {
        text.push_str(&format!(" path={}", entry.path));
    }
    if entry.count > 1 {
        text.push_str(&format!(" ({} times)", entry.count));
    }
    text
}

/// The sentence of [`blocked_note`] for `reason` and the hosts `hosts`.
fn block_sentence(reason: BlockReason, hosts: &str, max_upload_bytes: u64) -> String {
    let refused = format!("[efr: the network proxy refused {hosts}:");
    match reason {
        BlockReason::NotAllowed => format!(
            "{refused} not on the allow list. To reach a host, call shell again with \
             needs.hosts.]"
        ),
        BlockReason::PrivateAddress => format!(
            "{refused} it resolves to a local or private address. A host grant does not \
             help.]"
        ),
        BlockReason::SniMismatch => format!(
            "{refused} the TLS server name was not the host of the connection. Do not try \
             another way.]"
        ),
        BlockReason::NotTls => format!(
            "{refused} the connection sent no TLS hello with a server name, or it asked for \
             Encrypted Client Hello. Do not try another way.]"
        ),
        BlockReason::IpLiteral => {
            format!("{refused} the command named an IP address. Do not try another way.]")
        }
        BlockReason::UploadLimit if max_upload_bytes > 0 => format!(
            "[efr: the network proxy cut {hosts}: the command sent more than \
             {max_upload_bytes} bytes, the upload limit of a call (network.max_upload_bytes).]"
        ),
        BlockReason::UploadLimit => format!(
            "[efr: the network proxy cut {hosts}: the command sent more than the upload limit \
             of a call (network.max_upload_bytes).]"
        ),
        BlockReason::Denied => format!(
            "{refused} efr denies this host for this command (the deny list, or a credential \
             that the sandbox cannot hide; a note above says which). A host grant does not \
             help. Do not try another way.]"
        ),
        BlockReason::Method => format!("{refused} plain http allows only GET and HEAD.]"),
        BlockReason::BadRequest => format!("{refused} the request was malformed.]"),
        BlockReason::Limit => format!("{refused} the command had too many open connections.]"),
        _ => format!("{refused} for a reason that this build does not know.]"),
    }
}

/// Passes what a run hears on to the call's output sink.
struct Relay<'a> {
    out: &'a mut dyn ToolOutputSink,
}

impl RunProgress for Relay<'_> {
    fn update(&mut self, update: &OutputUpdate) {
        self.out.update(&update.tail, update.bytes);
    }

    fn input_changed(&mut self, wait: InputWait, looks_secret: bool) {
        self.out.input_changed(wait, looks_secret);
    }

    fn can_answer_hidden(&mut self) -> bool {
        self.out.can_answer_hidden()
    }

    fn can_answer(&mut self) -> bool {
        self.out.can_answer()
    }
}

/// Programs that may wait for input at the terminal.
const INTERACTIVE: &[&str] = &[
    "sudo", "su", "doas", "pkexec", "passwd", "chsh", "ssh", "sftp", "ftp", "telnet", "vi", "vim",
    "nvim", "nano", "emacs", "less", "more", "man", "top", "htop", "btop",
];

/// Programs that usually reach the network. `pacman` reaches it only to sync, install
/// or upgrade; [`reaches_network`] reads its options when the line is plain.
const NETWORK: &[&str] = &[
    "curl", "wget", "ssh", "scp", "sftp", "rsync", "ftp", "telnet", "nc", "ncat", "ping", "dig",
    "nslookup", "host", "pacman", "yay", "paru", "apt", "apt-get", "dnf", "zypper", "brew", "pip",
    "pip3", "pipx", "npm", "pnpm", "yarn", "npx", "docker", "podman", "flatpak", "snap",
];

/// The git subcommands that reach a remote.
const GIT_NETWORK: &[&str] = &["clone", "fetch", "pull", "push", "ls-remote", "submodule"];

/// Words that run the program after them.
const WRAPPERS: &[&str] =
    &["sudo", "doas", "env", "command", "builtin", "exec", "nohup", "time", "nice"];

/// The program of each simple command of the split `line` with its arguments, past
/// leading assignments and wrappers such as `sudo` or `env` (which count themselves),
/// as base names. `command -v` and `command -V` only look a program up.
fn programs_of(line: &words::Line) -> Vec<(String, Vec<String>)> {
    let mut programs = Vec::new();
    for command in &line.commands {
        let mut rest = command.program_and_args();
        while let Some((first, args)) = rest.split_first() {
            let program = first.text.rsplit('/').next().unwrap_or(&first.text).to_owned();
            let args: Vec<String> = args.iter().map(|word| word.text.clone()).collect();
            let looks_up =
                program == "command" && args.iter().any(|arg| arg == "-v" || arg == "-V");
            let wraps = WRAPPERS.contains(&program.as_str()) && !looks_up;
            programs.push((program, args));
            if !wraps {
                break;
            }
            let next = rest[1..]
                .iter()
                .position(|word| !word.text.starts_with('-') && !words::is_assignment(&word.text));
            rest = match next {
                Some(at) => &rest[1 + at..],
                None => &[],
            };
        }
    }
    programs
}

/// The subcommands of `npm`, `pnpm` and `yarn` that run the project's own scripts
/// instead of fetching packages.
const NODE_LOCAL: &[&str] = &["run", "run-script", "test", "build", "lint", "start"];

/// True when `program` with `args` usually reaches the network.
fn reaches_network(program: &str, args: &[String]) -> bool {
    // NOTE: a script may reach the network as a build or a test may; the permission
    // modes judge running it, and only installing packages declares the network.
    if matches!(program, "npm" | "pnpm" | "yarn")
        && args.first().is_some_and(|first| NODE_LOCAL.contains(&first.as_str()))
    {
        return false;
    }
    if program != "pacman" {
        return NETWORK.contains(&program);
    }
    // Querying, searching and showing read the local databases; syncing, installing,
    // upgrading, downloading and refreshing the file database fetch from mirrors.
    let Some(operation) = args.iter().find(|arg| arg.starts_with('-')) else {
        return true;
    };
    if operation.starts_with("--") {
        return !matches!(operation.as_str(), "--query" | "--remove" | "--deptest" | "--database");
    }
    let letters = &operation[1..];
    match letters.chars().next() {
        Some('Q' | 'R' | 'T' | 'D') => false,
        Some('S') => {
            letters.contains(['y', 'u', 'w'])
                || !letters[1..].chars().any(|c| matches!(c, 's' | 'i' | 'l' | 'g' | 'p'))
        }
        Some('F') => letters.contains('y'),
        _ => true,
    }
}

/// The program of each simple command in `line`: split at `;`, `|`, `&`, newlines,
/// parentheses and backquotes, past leading `NAME=value` assignments and wrappers
/// such as `sudo` or `env` (which count themselves), as base names. Quoting is not
/// parsed, so it finds a program too many rather than too few, for a line that the
/// split could not read whole.
fn programs(line: &str) -> Vec<&str> {
    let mut programs = Vec::new();
    for segment in segments(line) {
        for word in segment.split_whitespace() {
            let word = word.trim_matches(['"', '\'', '$', '{', '}']);
            if word.is_empty() || words::is_assignment(word) || word.starts_with('-') {
                continue;
            }
            let program = word.rsplit('/').next().unwrap_or(word);
            programs.push(program);
            if !WRAPPERS.contains(&program) {
                break;
            }
        }
    }
    programs
}

/// The segments of `line` at `;`, `|`, `&`, newlines, parentheses and backquotes, as
/// the coarse scans read them, which do not parse quotes.
fn segments(line: &str) -> impl Iterator<Item = &str> {
    line.split([';', '|', '&', '\n', '(', ')', '`'])
}

/// The options of git itself that take the next word as their value.
const GIT_VALUE_OPTIONS: &[&str] = &[
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--config-env",
    "--attr-source",
    "--shallow-file",
    "--exec-path",
];

/// The subcommand of the first `git` of `segment`, past the options of git itself, with
/// the words after it.
fn git_subcommand(segment: &str) -> Option<(&str, std::str::SplitWhitespace<'_>)> {
    let mut words = segment.split_whitespace();
    words.by_ref().find(|word| *word == "git")?;
    loop {
        let word = words.next()?;
        if GIT_VALUE_OPTIONS.contains(&word) {
            words.next();
        } else if !word.starts_with('-') {
            return Some((word, words));
        }
    }
}

/// True when a `git` command in `line` uses a subcommand that reaches a remote.
fn networked_git(line: &str) -> bool {
    segments(line).any(|segment| {
        git_subcommand(segment).is_some_and(|(subcommand, _)| GIT_NETWORK.contains(&subcommand))
    })
}

#[cfg(test)]
mod tests;
