//! `shell`: a command line in the conversation's hidden zsh, through
//! `efr_shell::CommandRunner`.

mod declare;
mod reads;
mod words;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use efr_protocol::InputWait;
use efr_shell::{
    CommandResult, CommandRunner, Completion, OutputUpdate, RunMode, RunProgress, RunRequest,
    ShellError,
};
use schemars::JsonSchema;
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
    /// started with `sudo -i`, `bash` or `ssh`), which has no efr integration.
    #[serde(default)]
    nested_shell: bool,
}

/// Runs a command line in the conversation's hidden zsh: one long-lived interactive
/// shell with the user's environment and startup files, whose working directory and
/// variables carry over from call to call. The user does not see its screen; a person
/// who follows the turn can answer a command that waits for input.
///
/// It declares the command line, `interactive` for commands that may wait for input
/// at the terminal (`sudo`, `ssh`, an editor, or any nested shell) and `network` for
/// commands that usually reach the network (`curl`, `git pull`, package installs).
/// Both come from the command's program names and are a heuristic: the engine judges
/// the command line itself too.
///
/// It also declares the paths the line names, resolved against the hidden shell's
/// directory: every operand of every program as a read (so `cat ~/.ssh/id_ed25519`
/// meets the secrets rule although `cat` may run freely), recursive searches, listings
/// and globs as reads of everything below their directory, the working directory of a
/// search that names no path, and output redirections as writes. What the text cannot
/// show, such as the files a script opens, it cannot declare; the engine asks for a
/// line it cannot read.
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

    fn render(&self, result: &CommandResult, timeout: Duration) -> ToolResult {
        let cut = truncate_middle(&result.output, self.output_limit);
        let mut text = cut.text;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        let cwd = result.cwd_after.display();
        let seconds = timeout.as_secs();
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
             starts in the user's working directory. The user does not see this shell. \
             The answer has the output, the exit code and the directory after the \
             command. A command still running at the timeout keeps running, and the next \
             call waits for it to end. When a command waits for input (a sudo password, \
             a [Y/n] question), the user can answer it in their terminal while they \
             follow the turn: an answer to a question shows in the output, a password \
             never does. A command that waits for a password while nobody follows the \
             turn is stopped at once. Set nested_shell when the command must go to a \
             shell you started inside this one, such as sudo -i or ssh.",
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
        let mut interactive = input.nested_shell
            || commands.iter().any(|(program, _)| INTERACTIVE.contains(&program.as_str()));
        let mut network = commands.iter().any(|(program, args)| reaches_network(program, args))
            || networked_git(&input.command);
        if line.opaque {
            // NOTE: the split may miss a program inside quotes or a substitution, so the
            // coarse scan that ignores quotes adds what it finds.
            let coarse = programs(&input.command);
            interactive |= coarse.iter().any(|program| INTERACTIVE.contains(program));
            network |= coarse.iter().any(|program| NETWORK.contains(program));
        }
        let declared = declare::declared(&line, ctx.command_dir(), ctx.home.path());
        let mut requirements = ToolRequirements::none()
            .with_command(input.command)
            .with_command_dir(ctx.command_dir())
            .with_interactive(interactive)
            .with_network(network);
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

    async fn invoke(
        &self,
        ctx: ToolContext,
        input: Value,
        out: &mut dyn ToolOutputSink,
    ) -> Result<ToolResult, ToolError> {
        let input: ShellInput = parse_input(Self::NAME, &input)?;
        let timeout = input
            .timeout_seconds
            .map_or(self.default_timeout, Duration::from_secs)
            .min(self.max_timeout);
        let mode = if input.nested_shell { RunMode::Sentinel } else { RunMode::Auto };
        let request = RunRequest::new(input.command, ctx.cwd.clone())
            .with_timeout(timeout)
            .with_mode(mode)
            .with_call(ctx.ids.call_id);
        let mut progress = Relay { out };
        match self.runner.run_command(ctx.ids.conversation_id, request, &mut progress).await {
            Ok(result) => Ok(self.render(&result, timeout)),
            Err(ShellError::Busy { .. }) => Ok(ToolResult::error(
                "The shell is busy: another command of this conversation is still running, \
                 or an unfinished command line waits in it. Try again once it has ended; if \
                 it does not end, tell the user, who can stop it by interrupting the turn.",
            )),
            Err(ShellError::NotReady { .. }) => Ok(ToolResult::error(format!(
                "The shell did not reach its prompt within {}s, so the command was not run: \
                 an earlier command is still running. If it is a shell you started inside \
                 this one (sudo -i, bash, ssh), call again with nested_shell set.",
                timeout.as_secs()
            ))),
            Err(ShellError::Exited { .. }) => Ok(ToolResult::error(
                "The shell exited while the command ran. The next call starts a new shell in \
                 the user's working directory; variables and the directory of the old one are \
                 gone.",
            )),
            Err(ShellError::InvalidCommand { reason }) => Ok(ToolResult::error(format!(
                "The command cannot be typed into a shell: {reason}."
            ))),
            Err(source) => Err(ToolError::Shell { source }),
        }
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

    fn input_changed(&mut self, wait: InputWait) {
        self.out.input_changed(wait);
    }

    fn can_answer_hidden(&mut self) -> bool {
        self.out.can_answer_hidden()
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

/// True when `program` with `args` usually reaches the network.
fn reaches_network(program: &str, args: &[String]) -> bool {
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
    for segment in line.split([';', '|', '&', '\n', '(', ')', '`']) {
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

/// True when a `git` command in `line` uses a subcommand that reaches a remote.
fn networked_git(line: &str) -> bool {
    line.split([';', '|', '&', '\n', '(', ')', '`']).any(|segment| {
        let mut words = segment.split_whitespace().skip_while(|word| *word != "git").skip(1);
        while let Some(word) = words.next() {
            match word {
                // The options of git itself that take the next word as their value.
                "-C" | "-c" => {
                    words.next();
                }
                option if option.starts_with('-') => {}
                subcommand => return GIT_NETWORK.contains(&subcommand),
            }
        }
        false
    })
}

#[cfg(test)]
mod tests;
