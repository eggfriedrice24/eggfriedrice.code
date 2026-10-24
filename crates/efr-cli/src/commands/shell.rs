//! `efr shell [conversation]`: the command that a shell call left running in the
//! conversation's hidden shell, which takes no other call until it ends. It shows the
//! command and the row that it waits at, answers it and stops it. The plugin's `,shell`
//! runs it, also in sticky mode: it is the one place where the user types an answer for
//! such a command, never the shell's own prompt.
//!
//! It reads `shell.status`, which reads efrd's live state: `the hidden shell is free`,
//! else one line for the command, such as `the hidden shell runs: rsync -a ... (for 4m
//! 10s; you approved it)`, and the row that it waits at. On a terminal it then
//! subscribes to the conversation and stays inline until the command ends:
//!
//! - Enter sends the typed line with `input.respond` and the command's call id: a hidden
//!   answer for a hidden wait, an answer that is not shown for a visible wait that looks
//!   like a password prompt, a shown one for a plain question. A command in the sandbox
//!   gets no answer that may be a secret; efr-shell stops it.
//! - Ctrl+C sends `shell.interrupt` (SIGINT to the command). A second Ctrl+C asks `close
//!   the hidden shell? [y/n]`, and `y` sends it with `close`, for a command that ignores
//!   SIGINT.
//! - Ctrl+B, while the command waits for nothing, sends `call.background` with the
//!   command's call id (OQS16): the command moves into a background job and the hidden
//!   shell is free. A command outside the sandbox first gets efrd's preview and the
//!   question of the turn view's Ctrl+B; only `y` moves it.
//! - Esc leaves the command as it is.
//!
//! The key reader holds the terminal in its own mode; every way out stops it, also an
//! error, SIGTERM and SIGHUP, and it throws away what is still unread before echo comes
//! back. Without a terminal on stdin it shows the command and ends, and it refuses to
//! read a password. The command and its prompt row come from programs, so they go
//! through `format::one_line`. Nothing here is a full screen: each line is written once,
//! and only the echo of a shown answer is written again in place.

use efr_protocol::{
    CallBackground, CallBackgroundResult, ConversationId, ConversationSubscribe,
    ConversationSubscribeItem, ErrorCode, Event, InputRespond, InputRespondResult, InputWait,
    Method, Origin, ShellInterrupt, ShellInterruptResult, ShellStatus, ShellStatusResult,
};
use efr_transport::client::{Client, ClientError, ItemStream};
use futures::StreamExt as _;
use serde_json::Value;

use crate::answer::{AnswerLine, Edit};
use crate::cli::ShellArgs;
use crate::commands::history::{Picked, pick, resolve};
use crate::context::Context;
use crate::error::CliError;
use crate::follow::since_then;
use crate::format::hidden_shell::{self, approver, secret_wait};
use crate::format::{self, Tone, jobs};
use crate::keys::{ESC, Key, KeyReader};
use crate::live::effective_width;
use crate::output::Output;

/// What the line of the command starts with.
const RUNS: &str = "the hidden shell runs: ";

/// What starts the row that the command waits at.
const BAR: &str = "  \u{2502} ";

/// The line under a hidden wait.
const HIDDEN: &str = "type the answer and press Enter; it is not shown, and the agent sees it only if the program prints it";

/// The line under a visible wait whose prompt looks like a password prompt.
const MASKED: &str = "this looks like a password prompt behind another program: your typing is not shown here, and the agent sees it only if that program shows it";

/// The line under a plain visible wait.
const VISIBLE: &str = "type the answer and press Enter; the agent sees it if the program shows it";

/// The line under a wait in the sandbox that may be for a secret.
const SANDBOXED: &str =
    "sandbox: the command asked for a secret; efr does not type secrets into the sandbox";

/// The keys while the command waits for an answer.
const ANSWER_KEYS: &str = "enter answers \u{b7} ctrl+c stops it \u{b7} esc leaves it";

/// The keys while the command waits for nothing.
const RUN_KEYS: &str = "it waits for no input now \u{b7} ctrl+b moves it to a background job \u{b7} ctrl+c stops it \u{b7} esc leaves it";

/// Ctrl+B, which moves the command into a background job.
const CTRL_B: u8 = 0x02;

/// The question of a second Ctrl+C.
const CLOSE_QUESTION: &str = "close the hidden shell? [y/n]";

/// What the echo of a shown answer starts with.
const ECHO: &str = "> ";

/// How an answer is read and sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// Echo off: never shown, sent as a hidden answer.
    Hidden,
    /// A visible wait that looks like a password prompt: never shown, sent as visible.
    Masked,
    /// A plain question: shown as it is typed, sent as visible.
    Visible,
}

/// The command in the hidden shell, as `efr shell` knows it.
#[derive(Debug)]
struct Busy {
    call_id: efr_protocol::CallId,
    command: String,
    contained: bool,
    input: InputWait,
    looks_secret: bool,
    prompt: String,
}

impl Busy {
    /// The answer that its wait takes here; `None` without a wait, and for a wait in the
    /// sandbox that may be for a secret.
    fn answer(&self) -> Option<Answer> {
        if self.contained && secret_wait(self.input, self.looks_secret) {
            return None;
        }
        match self.input {
            InputWait::Hidden => Some(Answer::Hidden),
            InputWait::Visible if self.looks_secret => Some(Answer::Masked),
            InputWait::Visible => Some(Answer::Visible),
            _ => None,
        }
    }
}

pub(crate) async fn run(ctx: &Context, out: &mut Output, args: &ShellArgs) -> Result<(), CliError> {
    let client = ctx.connect(Origin::Cli, None).await?;
    let picked = match args.conversation.as_deref() {
        Some(query) => Some((resolve(&client, query).await?, None)),
        None => pick(&client, ctx.tty.as_deref()).await?.map(|(id, picked)| (id, Some(picked))),
    };
    let Some((conversation_id, picked)) = picked else {
        return out.out("no conversations yet\n");
    };
    picked_line(ctx, out, picked);
    let method = Method::ShellStatus(ShellStatus { conversation_id });
    let status: ShellStatusResult = client.call(method).await?;
    let Some(busy) = status.busy else {
        return out.out("the hidden shell is free\n");
    };
    let mut line = format!("{RUNS}{}", format::one_line(&busy.command));
    let mut facts = vec![format!("for {}", format::took(since_then(busy.since, ctx.clock.now())))];
    if busy.contained {
        facts.push("in the sandbox".to_owned());
    }
    facts.extend(busy.approved_by.and_then(approver).map(str::to_owned));
    line.push_str(&format!(" ({})\n", facts.join("; ")));
    out.out(&line)?;
    let mut busy = Busy {
        call_id: busy.call_id,
        command: busy.command,
        contained: busy.contained,
        input: busy.input,
        looks_secret: busy.looks_secret,
        prompt: busy.prompt,
    };
    if !ctx.keys.available() {
        return without_keys(out, &busy);
    }
    let method = Method::ConversationSubscribe(ConversationSubscribe {
        conversation_id,
        after_seq: None,
        answers_input: true,
        drafts: false,
    });
    let stream = client.stream(method).await?;
    // NOTE: the reader is held here before anything can fail, so every way out stops it
    // and the terminal gets its modes back.
    let mut reader = ctx.keys.start()?;
    let mut session = Session { ctx, client: &client, conversation_id, line: AnswerLine::new() };
    let result = session.run(out, &mut busy, stream, &mut reader).await;
    reader.stop_discarding().await;
    result
}

/// Writes the muted line on stderr that says which conversation the command picked.
fn picked_line(ctx: &Context, out: &mut Output, picked: Option<Picked>) {
    if let Some(picked) = picked {
        let options = ctx.render_options(effective_width(ctx.screen.size()));
        let line = format!("{}; efr shell <id> shows another", picked.words());
        out.err(&format!("{}\n", format::paint(&line, Tone::Dim, &options)));
    }
}

/// Without a terminal on stdin no key can be read: the wait shows, and a password is
/// refused.
fn without_keys(out: &mut Output, busy: &Busy) -> Result<(), CliError> {
    if busy.input == InputWait::None {
        return Ok(());
    }
    write_prompt(out, busy)?;
    if !busy.contained && secret_wait(busy.input, busy.looks_secret) {
        return Err(CliError::ShellNeedsTerminal);
    }
    out.err("efr cannot ask for its input here: stdin is not a terminal\n");
    Ok(())
}

/// Writes the row that the command waits at, when it has one.
fn write_prompt(out: &mut Output, busy: &Busy) -> Result<(), CliError> {
    if busy.prompt.trim().is_empty() {
        return Ok(());
    }
    out.out(&format!("{BAR}{}\n", format::one_line(&busy.prompt)))
}

/// One run of `efr shell` on a terminal.
struct Session<'a> {
    ctx: &'a Context,
    client: &'a Client,
    conversation_id: ConversationId,
    /// What the user typed so far: zeroed when it is sent, cleared or dropped.
    line: AnswerLine,
}

/// Where the keys go now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keys {
    /// Into the answer line of the wait.
    Answer(Answer),
    /// Nowhere, but Esc: the command waits for nothing that can be typed here.
    Watch,
    /// To the question of a second Ctrl+C.
    Close,
    /// To the question of Ctrl+B, whose `y` moves the command with the preview's token.
    Move(u64),
}

impl Session<'_> {
    async fn run(
        &mut self,
        out: &mut Output,
        busy: &mut Busy,
        mut stream: ItemStream<Value>,
        reader: &mut KeyReader,
    ) -> Result<(), CliError> {
        let mut keys = self.show_wait(out, busy)?;
        let mut interrupt = self.ctx.interrupt.wait();
        let mut ending = self.ctx.terminate.wait();
        let mut stopped = false;
        loop {
            tokio::select! {
                () = &mut interrupt => {
                    interrupt = self.ctx.interrupt.wait();
                    self.end_echo(out, keys)?;
                    if stopped {
                        out.out(&format!("{CLOSE_QUESTION}\n"))?;
                        keys = Keys::Close;
                        continue;
                    }
                    stopped = true;
                    if !self.interrupt(out, busy, false).await? {
                        return Ok(());
                    }
                }
                signal = &mut ending => return Err(CliError::Ended { signal }),
                key = reader.next() => {
                    let Some(key) = key else {
                        return Ok(());
                    };
                    match self.key(out, busy, keys, key).await? {
                        Step::Stay => {}
                        Step::Keys(next) => keys = next,
                        Step::Leave => return Ok(()),
                    }
                }
                item = stream.next() => match item {
                    Some(Ok(value)) => {
                        if let Some(next) = self.item(out, busy, keys, value)? {
                            keys = next;
                        } else {
                            return Ok(());
                        }
                    }
                    Some(Err(error)) => return Err(error.into()),
                    None => return Err(CliError::SubscriptionEnded),
                },
            }
        }
    }

    /// Shows the wait of the command and says which keys do what; where the keys go.
    fn show_wait(&mut self, out: &mut Output, busy: &Busy) -> Result<Keys, CliError> {
        // NOTE: what was typed for an earlier wait is zeroed and never goes to this one.
        drop(self.line.take());
        if busy.input == InputWait::None {
            out.out(&format!("{RUN_KEYS}\n"))?;
            return Ok(Keys::Watch);
        }
        write_prompt(out, busy)?;
        let Some(answer) = busy.answer() else {
            out.out(&format!("{SANDBOXED}\n{RUN_KEYS}\n"))?;
            return Ok(Keys::Watch);
        };
        let how = match answer {
            Answer::Hidden => HIDDEN,
            Answer::Masked => MASKED,
            Answer::Visible => VISIBLE,
        };
        out.out(&format!("{how}\n{ANSWER_KEYS}\n"))?;
        if answer == Answer::Visible {
            out.out(ECHO)?;
        }
        Ok(Keys::Answer(answer))
    }

    /// Ends the echo line of a shown answer before another line is written.
    fn end_echo(&self, out: &mut Output, keys: Keys) -> Result<(), CliError> {
        if keys == Keys::Answer(Answer::Visible) {
            out.out("\n")?;
        }
        Ok(())
    }

    /// One key.
    async fn key(
        &mut self,
        out: &mut Output,
        busy: &mut Busy,
        keys: Keys,
        key: Key,
    ) -> Result<Step, CliError> {
        if key == Key::Esc {
            if keys == Keys::Close {
                out.out("the hidden shell stays\n")?;
                return Ok(Step::Keys(self.show_wait(out, busy)?));
            }
            self.end_echo(out, keys)?;
            out.out(&format!("efr shell leaves it as it is; {}\n", hidden_shell::ELSEWHERE))?;
            return Ok(Step::Leave);
        }
        let byte = key.byte();
        match keys {
            Keys::Watch if byte == CTRL_B => self.background(out, busy, None).await,
            Keys::Watch => Ok(Step::Stay),
            Keys::Move(token) => match byte {
                b'y' | b'Y' => self.background(out, busy, Some(token)).await,
                b'n' | b'N' | ESC => {
                    out.out("the command runs on here\n")?;
                    Ok(Step::Keys(self.show_wait(out, busy)?))
                }
                _ => Ok(Step::Stay),
            },
            Keys::Close => match byte {
                b'y' | b'Y' => {
                    if self.interrupt(out, busy, true).await? {
                        Ok(Step::Keys(Keys::Watch))
                    } else {
                        Ok(Step::Leave)
                    }
                }
                b'n' | b'N' | ESC => {
                    out.out("the hidden shell stays\n")?;
                    Ok(Step::Keys(self.show_wait(out, busy)?))
                }
                _ => Ok(Step::Stay),
            },
            Keys::Answer(answer) => match self.line.key(byte) {
                Edit::Unchanged => Ok(Step::Stay),
                Edit::Changed => {
                    // NOTE: the echo is written again in place only on a terminal.
                    if answer == Answer::Visible && self.ctx.term.stdout_tty {
                        out.out(&format!("\r\x1b[2K{ECHO}{}", format::one_line(self.line.text())))?;
                    }
                    Ok(Step::Stay)
                }
                Edit::Submit => {
                    self.end_echo(out, keys)?;
                    self.respond(out, busy, answer).await?;
                    if answer == Answer::Visible {
                        out.out(ECHO)?;
                    }
                    Ok(Step::Stay)
                }
            },
        }
    }

    /// Sends the line to the command's wait. A password gets no note: the command's
    /// own output, and its next wait, show what came of it.
    async fn respond(
        &mut self,
        out: &mut Output,
        busy: &Busy,
        answer: Answer,
    ) -> Result<(), CliError> {
        let method = Method::InputRespond(InputRespond {
            conversation_id: self.conversation_id,
            call_id: busy.call_id,
            text: self.line.take(),
            hidden: answer == Answer::Hidden,
            manual: false,
        });
        match self.client.call::<InputRespondResult>(method).await {
            Ok(_) if answer == Answer::Visible => out.out("answer sent\n"),
            Ok(_) => Ok(()),
            Err(ClientError::Server { body })
                if matches!(body.code, ErrorCode::NotFound | ErrorCode::Conflict) =>
            {
                out.out("the command no longer waits for that input; nothing was sent\n")
            }
            Err(ClientError::Server { body }) => {
                out.out(&format!("the answer was not sent: {}\n", format::one_line(&body.message)))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Asks efrd to move the command into a background job (OQS16): with no `confirm`
    /// after Ctrl+B, with the preview's token after a `y`. A preview asks the question of
    /// the turn view's Ctrl+B; a refusal is a note, and the command stays here.
    async fn background(
        &mut self,
        out: &mut Output,
        busy: &Busy,
        confirm: Option<u64>,
    ) -> Result<Step, CliError> {
        let method = Method::CallBackground(CallBackground {
            conversation_id: self.conversation_id,
            call_id: busy.call_id,
            command_id: self.ctx.command_id(),
            confirm,
        });
        match self.client.call::<CallBackgroundResult>(method).await {
            Ok(CallBackgroundResult::Moved { job_id }) => {
                let command = format::one_line(&busy.command);
                out.out(&format!(
                    "{command} moved to the background as job {job_id}; efr jobs shows it\n"
                ))?;
                Ok(Step::Leave)
            }
            Ok(CallBackgroundResult::Preview { token, command, max_minutes, exits })
                if confirm.is_none() =>
            {
                let mut lines = vec![format!("{}? [y/n]", jobs::MOVE_TITLE)];
                lines.push(format!("{BAR}{}", jobs::command(&command, 60)));
                lines.push(jobs::move_risk(max_minutes));
                lines.extend(exits.iter().map(|exit| jobs::move_exit(exit, None)));
                out.out(&format!("{}\n", lines.join("\n")))?;
                Ok(Step::Keys(Keys::Move(token)))
            }
            Ok(_) => {
                out.out(&format!("{}\n", jobs::not_moved("press ctrl+b again")))?;
                Ok(Step::Keys(self.show_wait(out, busy)?))
            }
            Err(ClientError::Server { body }) => {
                out.out(&format!("{}\n", jobs::not_moved(&body.message)))?;
                Ok(Step::Keys(self.show_wait(out, busy)?))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Asks efrd to stop the command: SIGINT, or with `close` the end of the whole
    /// hidden shell. False when the command no longer runs.
    async fn interrupt(
        &mut self,
        out: &mut Output,
        busy: &Busy,
        close: bool,
    ) -> Result<bool, CliError> {
        let method = Method::ShellInterrupt(ShellInterrupt {
            conversation_id: self.conversation_id,
            call_id: busy.call_id,
            command_id: self.ctx.command_id(),
            close,
        });
        match self.client.call::<ShellInterruptResult>(method).await {
            Ok(_) if close => {
                out.out("efr closes the hidden shell; the next call starts a new one\n")?;
                Ok(true)
            }
            Ok(_) => {
                out.out("efr asked the command to stop; ctrl+c again closes the hidden shell\n")?;
                Ok(true)
            }
            Err(ClientError::Server { body }) if body.code == ErrorCode::NotFound => {
                out.out("the command no longer runs; the hidden shell is free\n")?;
                Ok(false)
            }
            Err(ClientError::Server { body }) => {
                out.out(&format!(
                    "the command was not stopped: {}\n",
                    format::one_line(&body.message)
                ))?;
                Ok(true)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// One item of the subscription: the events of the command change its wait or end
    /// it. `None` when it ended.
    fn item(
        &mut self,
        out: &mut Output,
        busy: &mut Busy,
        keys: Keys,
        value: Value,
    ) -> Result<Option<Keys>, CliError> {
        let events = match serde_json::from_value::<ConversationSubscribeItem>(value) {
            Ok(ConversationSubscribeItem::Event(envelope)) => vec![envelope],
            // NOTE: the events of a snapshot came before the subscription; in order, they
            // end at the command's state now, which `shell.status` read just before. Only
            // what changed after that read matters, and those are the newest events, so
            // the events that a cut snapshot leaves out are not read (`follow` reads them).
            Ok(ConversationSubscribeItem::Snapshot(snapshot)) => snapshot.events,
            Ok(_) | Err(_) => return Ok(Some(keys)),
        };
        // The newest wait of the command in these events, if any.
        let mut wait = None;
        for envelope in events {
            match envelope.event {
                Event::ShellCommandDetached { call_id, input, looks_secret, prompt, .. }
                | Event::ShellCommandInputChanged {
                    call_id, input, looks_secret, prompt, ..
                } if call_id == busy.call_id => {
                    wait = Some((input, looks_secret && input == InputWait::Visible, prompt));
                }
                Event::ShellCommandEnded { call_id, exit_code, how } if call_id == busy.call_id => {
                    self.end_echo(out, keys)?;
                    let command = format::one_line(&busy.command);
                    // NOTE: the last wait, also one of these events: efr stops a wait in
                    // the sandbox that may be for a secret at once, and that end is
                    // `limit` too (DS50).
                    let (input, looks_secret) = match &wait {
                        Some((input, looks_secret, _)) => (*input, *looks_secret),
                        None => (busy.input, busy.looks_secret),
                    };
                    let secret = busy.contained && secret_wait(input, looks_secret);
                    let note = hidden_shell::ended(&command, exit_code, how, secret);
                    out.out(&format!("{note}\n"))?;
                    return Ok(None);
                }
                _ => {}
            }
        }
        let Some((input, looks_secret, prompt)) = wait else {
            return Ok(Some(keys));
        };
        if (input, looks_secret, prompt.as_str())
            == (busy.input, busy.looks_secret, busy.prompt.as_str())
            && keys != Keys::Close
        {
            return Ok(Some(keys));
        }
        busy.input = input;
        busy.looks_secret = looks_secret;
        busy.prompt = prompt;
        if keys == Keys::Close {
            // The question of the second Ctrl+C stays; the new wait shows after it.
            return Ok(Some(keys));
        }
        self.end_echo(out, keys)?;
        Ok(Some(self.show_wait(out, busy)?))
    }
}

/// What a key leads to.
#[derive(Debug)]
enum Step {
    /// Nothing changes.
    Stay,
    /// The keys go elsewhere now.
    Keys(Keys),
    /// The user leaves; the command ends here.
    Leave,
}

#[cfg(test)]
mod tests;
