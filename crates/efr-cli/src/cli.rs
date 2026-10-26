//! The command line: what `efr` accepts, as clap derive types.
//!
//! The zsh plugin is the main caller, so its four calls are the contract this file
//! must keep. The plugin passes the shell context, the last command line, the recent
//! command lines and the prompt in `EFR_CONTEXT`, `EFR_LAST_COMMAND`,
//! `EFR_RECENT_COMMANDS` and `EFR_PROMPT`, so that no other user can read them in the
//! command line:
//!
//! - `efr send`
//! - `efr send --steer` (without `EFR_LAST_COMMAND` and `EFR_RECENT_COMMANDS`)
//! - `efr new`
//! - `efr compact` (`,compact`, with the focus in `EFR_PROMPT`)
//! - `efr away` (`,away`, with no prompt and no context)
//!
//! The flags `--context-json` and `--last-command` and the prompt words do the same by
//! hand, and each wins over its variable. For a paste of `,` lines, `efr send` and `efr
//! new` also get the prompts to queue behind the first one in `EFR_QUEUED_PROMPTS`,
//! which has no flag.
//!
//! The plugin also hands over the terminal's turn settings, as `EFR_MODE`, `EFR_MODEL`
//! and `EFR_EFFORT`, to `efr send`, `efr new` and `efr settings`. `--mode`, `--model`
//! and `--effort` do the same by hand and win over the variables.

use std::convert::Infallible;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use clap::builder::{PossibleValuesParser, TypedValueParser as _};
use clap::{ArgGroup, Args, Parser, Subcommand};
use efr_protocol::{ConversationId, JobId, Mode, TurnId};

/// The parsed command line.
#[derive(Debug, Parser)]
#[command(
    name = "efr",
    version,
    about = "Talk to the efr daemon: send prompts, follow replies, check status",
    long_about = None,
    after_help = "In zsh, the efr plugin runs these for you: `, <prompt>`, `,new`, `,! <text>`, `,compact [focus]`, `,context`, `,resume [N]` and `,away`, and `,mode`, `,model` and `,effort` set the terminal's turn settings."
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// One `efr` command.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Send a prompt to this terminal's conversation and follow the reply.
    Send(SendArgs),
    /// Start a new conversation for this terminal with its first prompt.
    New(NewArgs),
    /// Show whether the daemon runs, and its health.
    Status,
    /// List recent conversations, or show the events of one.
    History(HistoryArgs),
    /// Find a recent conversation, or make one this terminal's conversation again.
    ///
    /// Without an argument it opens a live search picker in fzf, below the command line,
    /// when fzf 0.46 or newer is installed: type to search the titles and the text of
    /// your conversations; Enter continues one here, Ctrl-O prints it, Esc closes.
    /// Without fzf it lists the recent conversations, the ones of the current project
    /// first, with their number, title, project, age and turns. With N (one or two
    /// digits) or an id (or at least 4 characters of one), the next prompt in that
    /// conversation's project continues it, its hidden shell starts in its last
    /// directory and its jobs report here. When another open terminal holds it, efr
    /// asks first. Other words search: the picker opens with them, or without fzf the
    /// best matches print.
    Resume(ResumeArgs),
    /// Delete a conversation and everything that efr keeps of it.
    #[command(subcommand)]
    Conversation(ConversationCommand),
    /// Find the conversations whose titles or text match some words, best first.
    ///
    /// efrd searches the titles, your prompts, the model's answers and the commands that
    /// the agent ran, never the output of a command. Each row shows the matching text and
    /// the start of the conversation's id, which efr resume takes. Other local users can
    /// read the words of a command line in the process list; the words of ,resume stay
    /// out of it.
    Search(SearchArgs),
    /// Show what the last turn changed in the files of its project and $SCRATCH.
    Diff(DiffArgs),
    /// Take back what the last turn changed in files and branches.
    ///
    /// It shows what it will do (restore, trash, delete, ref) and asks first. Every
    /// file that it overwrites or removes goes to efr's undo trash.
    Undo(UndoArgs),
    /// Make room in the model's context of this terminal's conversation now.
    ///
    /// efrd writes a summary of the earlier turns and keeps the newest ones word for
    /// word. It starts no turn.
    Compact(CompactArgs),
    /// Show what fills the model's context of this terminal's conversation.
    ///
    /// The model's last count, then efr's estimate of the next request by part (the
    /// system prompt, the tools, the summary, the prompts, the answers, the tool
    /// outputs), the largest items, the compactions and the cache use. It shows no text
    /// of a message.
    Context(ContextArgs),
    /// Tell efr that you are away from this terminal's conversation.
    ///
    /// A question then waits only a short time (presence.away_wait_secs), and its call
    /// does not run. Your next input in the conversation ends this.
    Away(AwayArgs),
    /// List the background jobs of this terminal's conversation, or stop one.
    ///
    /// A job is a command that the model started with background: true. It runs on
    /// after its turn, until it ends, its time limit passes or someone stops it.
    Jobs(JobsArgs),
    /// Answer or stop a command that still runs in the hidden shell of this terminal's
    /// conversation.
    ///
    /// A shell call that reached its timeout can leave its command running, such as an
    /// rsync that waits for a password. The hidden shell then takes no other call.
    /// efr shell shows the command and the line that it waits at: Enter answers it,
    /// Ctrl+C stops it, a second Ctrl+C closes the hidden shell, and Esc leaves it. Type
    /// a password for such a command only here, never at your shell's prompt.
    Shell(ShellArgs),
    /// Show the mode, model and effort that a prompt would use, with where each comes
    /// from.
    Settings(TurnSettingsArgs),
    /// List the models that a prompt may name.
    Models(ModelsArgs),
    /// Log in to a model provider: the OpenAI subscription in a browser, or an API key.
    #[command(subcommand)]
    Login(LoginCommand),
    /// Forget the stored login of a model provider.
    Logout(LogoutArgs),
    /// Show, check, edit and change config.toml.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// List, add and remove the projects that the permission modes trust.
    #[command(subcommand)]
    Project(ProjectCommand),
    /// Show where efr keeps its files, and where the daemon keeps them.
    Paths(PathsArgs),
    /// Check the sandbox of the auto mode, or explain what it does with a path.
    #[command(subcommand)]
    Sandbox(SandboxCommand),
    /// Pair a phone with efrd over Tailscale, and list or revoke the paired devices.
    #[command(subcommand)]
    Device(DeviceCommand),
}

/// The `efr device` commands.
#[derive(Debug, Subcommand)]
pub(crate) enum DeviceCommand {
    /// Pair a phone: show a QR code for the efr app, then the phone's facts, and ask.
    ///
    /// efrd must listen on the tailnet: set remote.enabled = true in config.toml,
    /// restart efrd, and keep Tailscale on, here and on the phone. Scan the code with the
    /// efr app. The code is good for one phone and for 5 minutes. When the phone sends
    /// it, efr shows the phone's name, its Tailscale node and user, and the fingerprint
    /// of its key. Compare the fingerprint with the one that the app shows, then press
    /// y to pair the phone or n to refuse it. Ctrl+C stops the pairing.
    Add(DeviceAddArgs),
    /// List the paired devices, the revoked ones too, the oldest first.
    List(DeviceListArgs),
    /// Revoke a device: it can never connect again, and its open connections close.
    ///
    /// A revoke cannot be undone; pair the phone again with efr device add.
    Revoke(DeviceRevokeArgs),
}

/// The arguments of `efr device add`.
#[derive(Debug, Args)]
pub(crate) struct DeviceAddArgs {
    /// The name of the device in efr device list. Without it, the name that the phone
    /// sends.
    #[arg(long, value_name = "NAME")]
    pub(crate) name: Option<String>,
    /// Swap the dark and the light parts of the code, for a terminal with a light
    /// background.
    #[arg(long)]
    pub(crate) invert: bool,
}

/// The arguments of `efr device list`.
#[derive(Debug, Args)]
pub(crate) struct DeviceListArgs {
    /// Print the daemon's answer as JSON, for scripts.
    #[arg(long)]
    pub(crate) json: bool,
}

/// The arguments of `efr device revoke`.
#[derive(Debug, Args)]
pub(crate) struct DeviceRevokeArgs {
    /// The device: its id, or at least 4 characters of the start of it, as efr device
    /// list shows it.
    #[arg(value_name = "ID")]
    pub(crate) id: String,
}

/// The `efr conversation` commands.
#[derive(Debug, Subcommand)]
pub(crate) enum ConversationCommand {
    /// Delete a conversation: its events, its search text, its recordings, its
    /// $SCRATCH, its snapshots and its undo trash. Nothing takes a delete back.
    ///
    /// efr asks first on a terminal. efrd stops the conversation's background jobs and
    /// closes its hidden shell first. When another open terminal holds the
    /// conversation, or a turn runs in it, efr asks again; --force deletes without these
    /// questions.
    Delete(ConversationDeleteArgs),
}

/// The arguments of `efr conversation delete`.
#[derive(Debug, Args)]
pub(crate) struct ConversationDeleteArgs {
    /// The conversation: its id, or at least 4 characters of the start of it, as efr
    /// search and efr resume show it.
    #[arg(value_name = "ID")]
    pub(crate) id: String,
    /// Delete with no question. Without a terminal, efr deletes only with --yes.
    #[arg(long, short = 'y')]
    pub(crate) yes: bool,
    /// Delete also when another open terminal holds the conversation, and stop its
    /// running turn first.
    #[arg(long)]
    pub(crate) force: bool,
}

/// The `efr sandbox` commands.
#[derive(Debug, Subcommand)]
pub(crate) enum SandboxCommand {
    /// Run the sandbox checks now and print each one with its result and fix.
    Check,
    /// Show whether a command in the auto sandbox can read and write PATH, and why.
    Explain {
        /// The path; relative to the current directory.
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
    /// List the always-allow rules of config.toml ([[sandbox.allow]]), or remove one.
    Allow {
        #[command(subcommand)]
        command: Option<AllowCommand>,
    },
}

/// The `efr sandbox allow` commands.
#[derive(Debug, Subcommand)]
pub(crate) enum AllowCommand {
    /// Remove rule N of sandbox.allow from config.toml, after a question.
    Remove {
        /// The number of the rule, as efr sandbox allow lists it.
        #[arg(value_name = "N")]
        index: usize,
    },
}

/// The arguments of `efr paths`.
#[derive(Debug, Args)]
pub(crate) struct PathsArgs {
    /// Print the same facts as JSON.
    #[arg(long)]
    pub(crate) json: bool,
}

/// The arguments of `efr send`.
#[derive(Debug, Args)]
pub(crate) struct SendArgs {
    /// Add the text to the running turn instead of queueing a new prompt. A running
    /// turn keeps its settings, so a steer takes none.
    #[arg(long, conflicts_with_all = ["mode", "model", "effort"])]
    pub(crate) steer: bool,

    /// The user's shell as the zsh plugin observed it, as a JSON object [env:
    /// EFR_CONTEXT]
    #[arg(long, value_name = "JSON")]
    pub(crate) context_json: Option<String>,

    /// The last command line of the user's shell, for the turn's context only; it is
    /// never stored [env: EFR_LAST_COMMAND]
    #[arg(long, value_name = "TEXT", conflicts_with = "steer")]
    pub(crate) last_command: Option<LastCommand>,

    /// Send to this conversation instead of the terminal's active one.
    #[arg(long, value_name = "ID")]
    pub(crate) conversation: Option<ConversationId>,

    #[command(flatten)]
    pub(crate) settings: TurnSettingsArgs,

    /// The prompt; the words are joined with spaces [env: EFR_PROMPT]
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "PROMPT")]
    pub(crate) prompt: Vec<String>,
}

/// The arguments of `efr new`.
#[derive(Debug, Args)]
pub(crate) struct NewArgs {
    /// The user's shell as the zsh plugin observed it, as a JSON object [env:
    /// EFR_CONTEXT]
    #[arg(long, value_name = "JSON")]
    pub(crate) context_json: Option<String>,

    /// The last command line of the user's shell, for the turn's context only; it is
    /// never stored [env: EFR_LAST_COMMAND]
    #[arg(long, value_name = "TEXT")]
    pub(crate) last_command: Option<LastCommand>,

    #[command(flatten)]
    pub(crate) settings: TurnSettingsArgs,

    /// The first prompt of the new conversation; the words are joined with spaces
    /// [env: EFR_PROMPT]
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "PROMPT")]
    pub(crate) prompt: Vec<String>,
}

/// The turn settings that a prompt asks for. Each one left out comes from its
/// variable, and without that from the daemon's config.
#[derive(Debug, Clone, Default, Args)]
pub(crate) struct TurnSettingsArgs {
    /// The permission mode of the turn [env: EFR_MODE]
    #[arg(long, value_name = "MODE", value_parser = mode_parser())]
    pub(crate) mode: Option<Mode>,

    /// The model of the turn, one that `efr models` lists [env: EFR_MODEL]
    #[arg(long, value_name = "MODEL")]
    pub(crate) model: Option<String>,

    /// The reasoning effort of the turn, one that the model takes [env: EFR_EFFORT]
    #[arg(long, value_name = "EFFORT")]
    pub(crate) effort: Option<String>,
}

/// Reads a mode by its wire name, and lists the names in the help and in the error.
fn mode_parser() -> impl clap::builder::TypedValueParser<Value = Mode> {
    PossibleValuesParser::new(Mode::ALL.map(Mode::as_str)).try_map(|name| name.parse::<Mode>())
}

/// The arguments of `efr models`.
#[derive(Debug, Args)]
pub(crate) struct ModelsArgs {
    /// Print only the model ids, one per line, for completion.
    #[arg(long)]
    pub(crate) names: bool,
}

/// A command line from the user's shell, as `--last-command` or `EFR_LAST_COMMAND`
/// gives it. It can hold a secret (`export TOKEN=...`), so `Debug` shows only its
/// length.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct LastCommand(String);

impl LastCommand {
    #[cfg(test)]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Debug for LastCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LastCommand(<{} bytes>)", self.0.len())
    }
}

impl From<String> for LastCommand {
    fn from(text: String) -> Self {
        LastCommand(text)
    }
}

impl FromStr for LastCommand {
    type Err = Infallible;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Ok(LastCommand(text.to_owned()))
    }
}

/// The arguments of `efr history`.
#[derive(Debug, Args)]
pub(crate) struct HistoryArgs {
    /// The conversation to show: its id, or the start of it. Without one, the recent
    /// conversations are listed; with --verbose, the newest conversation of this
    /// terminal shows, else the newest of all.
    #[arg(value_name = "CONVERSATION")]
    pub(crate) conversation: Option<String>,

    /// The most conversations or events to show.
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<u32>,

    /// Continue from a cursor that an earlier page printed.
    #[arg(long, value_name = "CURSOR")]
    pub(crate) cursor: Option<String>,

    /// Also show the facts that efr recorded for each exit from the auto sandbox.
    #[arg(long, short = 'v')]
    pub(crate) verbose: bool,
}

/// The arguments of `efr resume`.
#[derive(Debug, Args)]
pub(crate) struct ResumeArgs {
    /// N, the number of a row of the list; a conversation: its id, or the start of it;
    /// or words to search for. Without them, the picker or the list. Words on the
    /// command line show in the process list; the words of ,resume stay out of it.
    #[arg(value_name = "N|CONVERSATION|WORDS")]
    pub(crate) words: Vec<String>,

    /// Move the conversation of N or an id here also when another open terminal holds
    /// it, with no question.
    #[arg(long)]
    pub(crate) take: bool,
}

/// The arguments of `efr search`.
///
/// The picker of `,resume` runs the two hidden forms in fzf: `--picker` prints the rows
/// for the query that fzf exports in `FZF_QUERY`, and `--preview` prints the matching
/// lines of one conversation. The query never reaches their arguments.
#[derive(Debug, Args)]
#[command(group = ArgGroup::new("hidden").args(["picker", "preview"]))]
pub(crate) struct SearchArgs {
    /// The most conversations to list: 10 by default, 200 at most.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..=200), conflicts_with = "hidden")]
    pub(crate) limit: Option<u32>,

    /// Print the daemon's answer as JSON, with the measures of the search.
    #[arg(long, conflicts_with = "hidden")]
    pub(crate) json: bool,

    /// Print the rows of the picker of ,resume for the query in FZF_QUERY.
    #[arg(long, hide = true, requires = "tty")]
    pub(crate) picker: bool,

    /// Print the lines of this conversation that match the query in FZF_QUERY, for the
    /// preview of the picker of ,resume. An empty id prints nothing.
    #[arg(long, hide = true, value_name = "ID", requires = "tty")]
    pub(crate) preview: Option<String>,

    /// The terminal of the picker.
    #[arg(long, hide = true, value_name = "TTY", requires = "hidden")]
    pub(crate) tty: Option<String>,

    /// The words to find. Every word must match in one prompt, answer or command; a
    /// "quoted phrase" matches as one; the last word matches as the start of a word.
    #[arg(value_name = "WORDS", conflicts_with = "hidden")]
    pub(crate) words: Vec<String>,
}

impl SearchArgs {
    /// True for the hidden forms that fzf runs: they write nothing to stderr, which fzf
    /// would draw over its rows.
    pub(crate) fn is_hidden(&self) -> bool {
        self.picker || self.preview.is_some()
    }
}

impl Command {
    /// True when the command writes nothing to stderr: no log, no config warning and no
    /// error message.
    pub(crate) fn is_quiet(&self) -> bool {
        matches!(self, Command::Search(args) if args.is_hidden())
    }
}

/// The arguments of `efr diff`.
#[derive(Debug, Args)]
pub(crate) struct DiffArgs {
    /// Show this turn instead of the last one.
    #[arg(long, value_name = "ID")]
    pub(crate) turn: Option<TurnId>,

    /// The conversation whose last turn shows: its id, or the start of it. Without it,
    /// this terminal's conversation shows, else the newest of all.
    #[arg(long, value_name = "ID")]
    pub(crate) conversation: Option<String>,

    /// List the files that changed, with their counts of lines, instead of the diff.
    #[arg(long)]
    pub(crate) stat: bool,

    /// With --stat, add up every turn of the conversation instead of one turn. A turn
    /// that efr undo took back does not count. A path is relative to the project of
    /// its turn, so the same path in two projects counts as one file.
    #[arg(long, requires = "stat", conflicts_with = "turn")]
    pub(crate) all: bool,
}

/// The arguments of `efr undo`.
#[derive(Debug, Args)]
pub(crate) struct UndoArgs {
    /// Undo this turn instead of the newest one that is not undone.
    #[arg(long, value_name = "ID")]
    pub(crate) turn: Option<TurnId>,

    /// The conversation whose turn to undo: its id, or the start of it. Without it,
    /// this terminal's conversation, else the newest of all.
    #[arg(long, value_name = "ID")]
    pub(crate) conversation: Option<String>,

    /// List the turns that can be undone, and which are undone, and change nothing.
    #[arg(long, conflicts_with_all = ["dry_run", "yes", "force", "redo"])]
    pub(crate) list: bool,

    /// Show what the undo does, and change nothing.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Undo without the question.
    #[arg(long, short = 'y')]
    pub(crate) yes: bool,

    /// Also overwrite a file that changed after the turn, and undo a turn while a
    /// newer turn is not undone.
    #[arg(long)]
    pub(crate) force: bool,

    /// Take back the newest undo of the conversation.
    #[arg(long)]
    pub(crate) redo: bool,
}

/// The arguments of `efr compact`.
#[derive(Debug, Args)]
pub(crate) struct CompactArgs {
    /// The conversation to compact: its id, or the start of it. Without it, this
    /// terminal's conversation, else the newest of all.
    #[arg(long, value_name = "ID")]
    pub(crate) conversation: Option<String>,

    /// What the summary must keep, such as: the failing test and its fix; the words are
    /// joined with spaces [env: EFR_PROMPT]
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "FOCUS")]
    pub(crate) focus: Vec<String>,
}

/// The arguments of `efr context`.
#[derive(Debug, Args)]
pub(crate) struct ContextArgs {
    /// The conversation: its id, or the start of it. Without it, this terminal's
    /// conversation, else the newest of all.
    #[arg(value_name = "CONVERSATION")]
    pub(crate) conversation: Option<String>,

    /// Count for this model instead of the conversation's: its window, and whether the
    /// model's own items of earlier turns go back.
    #[arg(long, value_name = "MODEL")]
    pub(crate) model: Option<String>,

    /// Print the daemon's answer as JSON, for scripts.
    #[arg(long)]
    pub(crate) json: bool,
}

/// The arguments of `efr away`.
#[derive(Debug, Args)]
pub(crate) struct AwayArgs {
    /// The conversation: its id, or the start of it. Without it, this terminal's
    /// conversation, else the newest of all.
    #[arg(long, value_name = "ID")]
    pub(crate) conversation: Option<String>,
}

/// The arguments of `efr jobs`: a command, else the arguments of `efr jobs list`.
#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub(crate) struct JobsArgs {
    #[command(subcommand)]
    pub(crate) command: Option<JobsCommand>,
    #[command(flatten)]
    pub(crate) list: JobsListArgs,
}

/// The `efr jobs` commands.
#[derive(Debug, Subcommand)]
pub(crate) enum JobsCommand {
    /// List the jobs, newest first (also `efr jobs` alone).
    List(JobsListArgs),
    /// Stop a running job, and show how it ended and the last lines of its output.
    Stop(JobsStopArgs),
}

/// The arguments of `efr jobs list`.
#[derive(Debug, Clone, Default, Args)]
pub(crate) struct JobsListArgs {
    /// List the jobs of every conversation, each with the title of its conversation.
    #[arg(long, conflicts_with = "conversation")]
    pub(crate) all: bool,
    /// The conversation: its id, or the start of it. Without it, this terminal's
    /// conversation, else the newest of all.
    #[arg(long, value_name = "ID")]
    pub(crate) conversation: Option<String>,
}

/// The arguments of `efr jobs stop`.
#[derive(Debug, Args)]
pub(crate) struct JobsStopArgs {
    /// The job, such as j3.
    #[arg(value_name = "JOB")]
    pub(crate) job: JobId,
    /// The conversation: its id, or the start of it. Without it, this terminal's
    /// conversation, else the newest of all.
    #[arg(long, value_name = "ID")]
    pub(crate) conversation: Option<String>,
}

/// The arguments of `efr shell`.
#[derive(Debug, Args)]
pub(crate) struct ShellArgs {
    /// The conversation: its id, or the start of it. Without it, this terminal's
    /// conversation, else the newest of all.
    #[arg(value_name = "CONVERSATION")]
    pub(crate) conversation: Option<String>,
}

/// The providers `efr login` knows.
#[derive(Debug, Subcommand)]
pub(crate) enum LoginCommand {
    /// Log in to the OpenAI subscription in a browser. The daemon runs the login; this
    /// prints the URL to open and waits for the browser to finish.
    Openai,
    /// Store an OpenAI API key for the openai-api provider.
    ///
    /// On a terminal, efr asks for the key and does not show it; otherwise it reads the
    /// key from stdin. Never put a key on the command line: other users can read it in
    /// the process list, and it stays in the shell's history.
    OpenaiApi(KeyArgs),
    /// Store an Anthropic API key for the anthropic-api provider.
    ///
    /// On a terminal, efr asks for the key and does not show it; otherwise it reads the
    /// key from stdin. Never put a key on the command line: other users can read it in
    /// the process list, and it stays in the shell's history.
    #[command(alias = "anthropic-api")]
    Anthropic(KeyArgs),
}

/// The arguments of `efr login openai-api` and `efr login anthropic`.
#[derive(Debug, Args)]
pub(crate) struct KeyArgs {
    /// Read the key from OPENAI_API_KEY or ANTHROPIC_API_KEY of this shell. The daemon
    /// never reads these variables.
    #[arg(long)]
    pub(crate) from_env: bool,

    /// Store the key without a check with the provider, such as on a computer that is
    /// offline.
    #[arg(long)]
    pub(crate) no_check: bool,
}

/// The arguments of `efr logout`.
#[derive(Debug, Args)]
pub(crate) struct LogoutArgs {
    /// The provider: openai (the subscription), openai-api or anthropic.
    #[arg(value_name = "PROVIDER", value_parser = provider_parser())]
    pub(crate) provider: ProviderName,
}

/// A model provider by the name that a user types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderName {
    /// The OpenAI subscription: `openai` or `openai-subscription`.
    OpenAi,
    /// OpenAI with an API key: `openai-api`.
    OpenAiApi,
    /// Anthropic with an API key: `anthropic` or `anthropic-api`.
    Anthropic,
}

impl ProviderName {
    /// Every name, the short ones first.
    const NAMES: [(&'static str, ProviderName); 5] = [
        ("openai", ProviderName::OpenAi),
        ("openai-api", ProviderName::OpenAiApi),
        ("anthropic", ProviderName::Anthropic),
        ("openai-subscription", ProviderName::OpenAi),
        ("anthropic-api", ProviderName::Anthropic),
    ];

    /// The provider's id, as the daemon and the config name it.
    pub(crate) fn id(self) -> &'static str {
        match self {
            ProviderName::OpenAi => "openai-subscription",
            ProviderName::OpenAiApi => "openai-api",
            ProviderName::Anthropic => "anthropic-api",
        }
    }
}

/// Reads a provider by one of its names, and lists the names in the help and the
/// error.
fn provider_parser() -> impl clap::builder::TypedValueParser<Value = ProviderName> {
    PossibleValuesParser::new(ProviderName::NAMES.map(|(name, _)| name)).try_map(|name| {
        ProviderName::NAMES
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, provider)| *provider)
            .ok_or_else(|| format!("no provider is named {name:?}"))
    })
}

/// The `efr project` commands, which change projects.toml through the daemon.
#[derive(Debug, Subcommand)]
pub(crate) enum ProjectCommand {
    /// List the registered projects with their roots.
    List,
    /// Register a project: PATH, or without one the git work tree that holds the
    /// current directory (else the directory itself).
    Add {
        /// The project's root directory; relative to the current directory.
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,
        /// A name for the project; the last part of the root by default.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
    },
    /// Take the project with this root out of the registry.
    Remove {
        /// The project's root directory; relative to the current directory.
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
}

/// The `efr config` commands.
#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    /// Print every setting with where it comes from, then what the daemon reads.
    Show,
    /// Check a config file; errors name the line, the column and the key.
    Check {
        /// The file to check; config.toml in the config root by default.
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,
    },
    /// Open config.toml in $VISUAL or $EDITOR (vi by default), check it, then reload it.
    Edit,
    /// Print the JSON schema of config.toml.
    Schema,
    /// Ask the daemon to read config.toml again now.
    Reload,
    /// Set one key, such as model.name, keeping the file's comments and layout.
    Set {
        /// The dotted key, such as model.name or conversation.max_queued.
        #[arg(value_name = "KEY")]
        key: String,
        /// The value: text, a number, true or false, or a list as words separated by
        /// commas.
        #[arg(value_name = "VALUE", allow_hyphen_values = true)]
        value: String,
    },
    /// Remove one key, so its default applies again.
    Unset {
        /// The dotted key.
        #[arg(value_name = "KEY")]
        key: String,
    },
}

#[cfg(test)]
mod tests;
