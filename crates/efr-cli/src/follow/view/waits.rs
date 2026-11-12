//! A command that waits for input in the view: the answer line, the hint after a
//! silence, the manual input line, what the user types, and the texts of each wait. The
//! live slice of the terminal look owns this file: an `impl TurnView` block next to
//! `view.rs`.
//!
//! The answer line takes the place of the input row, under the last lines of the
//! command's output (TM-Live 21-23): `?` in the `warning` role in bold, what the
//! command waits for in `warning`, two spaces and the keys muted, such as `? the command
//! waits for input  enter answers · ctrl+c stops the command`. Under it, `› ` and what
//! the user types; nothing of a password shows, not even dots. After a silence of a
//! command that takes a manual input, one muted row offers `Ctrl+\\` (TM-Live 24).

use efr_protocol::{CallId, InputWait};
use efr_render::{Glyphs, RenderOptions, render_trace};

use super::{About, AnswerKind, Ask, MOVE_WAITS, Step, TurnView, echo_edit};
use crate::follow::SILENCE;
use crate::format::{self, Tone};
use crate::terminal::Size;

impl TurnView {
    /// Call `call_id` began or stopped waiting for input; `looks_secret` when a visible
    /// wait's prompt looks like a password prompt behind another program.
    pub(super) fn input_changed(
        &mut self,
        call_id: CallId,
        input: InputWait,
        looks_secret: bool,
        size: Size,
        can_ask: bool,
    ) -> Step {
        // NOTE: a call that waits for input cannot move, so its wait ends the question
        // of the background key and asks for the answer in its place.
        let waits = matches!(input, InputWait::Hidden | InputWait::Visible);
        let closed = match self.jobs.moving.take_if(|moving| *moving == call_id && waits) {
            Some(_) => self.answer_line(About::Move, false, MOVE_WAITS, size),
            None => Step::default(),
        };
        let step = self.input_changed_now(call_id, input, looks_secret, size, can_ask);
        Step { err: format!("{}{}", closed.err, step.err), ..step }
    }

    /// [`input_changed`](Self::input_changed) once a question of the background key
    /// about the call is closed.
    pub(super) fn input_changed_now(
        &mut self,
        call_id: CallId,
        input: InputWait,
        looks_secret: bool,
        size: Size,
        can_ask: bool,
    ) -> Step {
        let approval_pending = self.question_pending();
        let moving = self.jobs.moving.is_some();
        let (running, replaced) = self.running(call_id);
        if running.asking.is_some_and(AnswerKind::guards) {
            running.guarding = true;
        }
        let settled = replaced || running.reads_keys();
        running.wait = input;
        running.asking = None;
        running.typed.clear();
        running.stirred();
        let kind = match input {
            InputWait::Hidden => AnswerKind::Hidden,
            InputWait::Visible if looks_secret => AnswerKind::Masked,
            InputWait::Visible => AnswerKind::Visible,
            // NOTE: the question of the background key reads the keys meanwhile; they go
            // back to the call after its answer ([`keys_back`](Self::keys_back)).
            _ if moving => return self.commit(String::new()),
            // No wait, or one this build does not know: nothing to ask, but a call that
            // asked for a password keeps the keys quiet until it completes.
            _ if running.guarding => {
                let mut step = self.commit(String::new());
                step.ask = Some(Ask::Discard(call_id));
                return step;
            }
            _ if settled && !replaced => {
                let step = self.commit(String::new());
                return self.keys_free(Some(call_id), step);
            }
            _ => return Step { settled, ..self.commit(String::new()) },
        };
        // NOTE: a prompt in the sandbox can be a fake, so efr asks for no answer that may
        // be a secret there: not a hidden one, and not one whose prompt looks like a
        // password prompt. efr-shell stops such a command at once.
        if kind != AnswerKind::Visible && self.contained.contains_key(&call_id) {
            return Step { settled, ..self.note(HIDDEN_INPUT_SANDBOXED, size) };
        }
        if !can_ask {
            let note = if kind.hidden() { HIDDEN_INPUT_ELSEWHERE } else { VISIBLE_INPUT_ELSEWHERE };
            return Step { settled, ..self.note(note, size) };
        }
        // One question at a time, so an approval and an input never overlap. Tool calls
        // run one after another, so an approval does not come while a command runs.
        // The line of a command left running in the hidden shell keeps the keys too.
        if approval_pending || self.left_reads_keys() {
            return Step { settled, ..Step::default() };
        }
        self.ask_for(call_id, kind, size)
    }

    /// Asks for an answer line of `kind` for the running call `call_id`, below its last
    /// output line.
    pub(super) fn ask_for(&mut self, call_id: CallId, kind: AnswerKind, size: Size) -> Step {
        let Some(running) = self.running.as_mut().filter(|running| running.call_id == call_id)
        else {
            return Step::default();
        };
        running.asking = Some(kind);
        running.held = false;
        let prompt = format::one_line(&running.tail);
        let mut step = if self.terminal() {
            self.commit(String::new())
        } else {
            let options = self.options_at(size);
            let command = self.commands.get(&call_id).map(String::as_str);
            let mut err = String::new();
            if !prompt.is_empty() {
                err.push_str(&render_trace(&prompt, &options));
            }
            err.push_str(&piece_row(
                &kind.line(command.and_then(program), options.glyphs()),
                &options,
            ));
            let mut err = self.raw_err(err);
            // A shown answer is echoed on the next line as it is typed.
            if kind.shown() {
                err.push_str(&echo_prefix(options.glyphs()));
                self.echo_line = true;
            }
            Step { err, ..Step::default() }
        };
        step.ask = Some(Ask::Input { call_id, kind });
        step
    }

    /// The call that may offer `Ctrl+\` once it has been silent long enough, with its
    /// activity count, which changes at every sign of life: a call of this turn that
    /// takes a manual input, runs and reports no wait, while no key is read for it and
    /// no approval waits, and whose line that offers `Ctrl+\` is not shown yet.
    pub(crate) fn silence(&self) -> Option<(CallId, u64)> {
        let running = self.running.as_ref()?;
        let quiet = running.quiet()
            && !running.hinted
            && !self.question_pending()
            && !self.left_reads_keys();
        quiet.then_some((running.call_id, running.activity))
    }

    /// Call `call_id` has been silent since its activity count was last read: shows the
    /// line that offers `Ctrl+\`. Nothing when it is no longer the silent call.
    pub(crate) fn silent(&mut self, call_id: CallId, size: Size) -> Step {
        if self.silence().map(|(silent, _)| silent) != Some(call_id) {
            return Step::default();
        }
        if let Some(running) = &mut self.running {
            running.hinted = true;
        }
        if self.terminal() {
            return self.commit(String::new());
        }
        let header = self.due_header(size);
        let step = self.note(&silence_hint(self.options.glyphs()), size);
        Step { err: format!("{header}{}", step.err), ..step }
    }

    /// The call whose line that offers `Ctrl+\` is shown now; the follow loop waits for
    /// the key exactly while there is one.
    pub(crate) fn manual_offer(&self) -> Option<CallId> {
        let running = self.running.as_ref()?;
        let offered = running.quiet()
            && running.hinted
            && !self.question_pending()
            && !self.left_reads_keys();
        offered.then_some(running.call_id)
    }

    /// The user pressed `Ctrl+\` while the line for call `call_id` was shown: asks for a
    /// line that is not shown as it is typed and goes as a manual answer.
    pub(crate) fn manual(&mut self, call_id: CallId, size: Size) -> Step {
        // NOTE: a manual line may hold a password, which never goes into the sandbox.
        if self.manual_offer() != Some(call_id) || self.contained.contains_key(&call_id) {
            return Step::default();
        }
        if let Some(running) = &mut self.running {
            running.hinted = false;
        }
        self.ask_for(call_id, AnswerKind::Manual, size)
    }

    /// True while a manual answer line is open, which `Ctrl+\` closes.
    pub(crate) fn manual_open(&self) -> bool {
        self.running.as_ref().is_some_and(|running| running.asking == Some(AnswerKind::Manual))
    }

    /// The user pressed `Ctrl+\` again while a manual answer line was open: the line
    /// closes unsent, its keys stop, and the line that offers `Ctrl+\` comes back.
    pub(crate) fn manual_cancelled(&mut self, size: Size) -> Step {
        if !self.manual_open() {
            return Step::default();
        }
        let mut call = None;
        if let Some(running) = &mut self.running {
            running.asking = None;
            running.typed.clear();
            running.hinted = true;
            call = Some(running.call_id);
        }
        let step = self.note(MANUAL_CANCELLED, size);
        self.manual_closed(call, step)
    }

    /// `step` for the moment a manual answer line of call `call` closes. The keys of a
    /// call that kept them are read and thrown away from now on instead of kept: the
    /// line may have held a password, and a second copy typed ahead for a prompt that
    /// efr cannot see must never start a later answer line that is shown. The call's
    /// silence still offers `Ctrl+\` for the next manual line. Otherwise the keys stop.
    pub(super) fn manual_closed(&self, call: Option<CallId>, step: Step) -> Step {
        match call.filter(|call| self.retained == Some(*call)) {
            Some(call) => Step { ask: Some(Ask::Discard(call)), ..step },
            None => Step { settled: true, ..step },
        }
    }

    /// `step` for the moment nothing reads keys for call `call` any more: the keys are
    /// kept for it when it is the retained call, and stop otherwise.
    pub(super) fn keys_free(&self, call: Option<CallId>, step: Step) -> Step {
        match call.filter(|call| self.retained == Some(*call)) {
            Some(call) => Step { ask: Some(Ask::Retain(call)), ..step },
            None => Step { settled: true, ..step },
        }
    }

    /// What the user typed so far for a shown answer, echoed below the prompt: in the
    /// live zone on a terminal, otherwise on stderr after `> `, where a removed
    /// character is erased with Backspace. An answer that is not shown is never passed
    /// here, and text passed while no shown answer is asked for is dropped.
    pub(crate) fn typed(&mut self, text: &str) -> Step {
        let terminal = self.terminal();
        let echo_line = self.echo_line;
        // NOTE: the line of a command left running in the hidden shell and the line of
        // the running call never ask at the same time.
        let (asking, typed) = match (self.left.as_deref_mut(), self.running.as_mut()) {
            (Some(left), _) if left.asking.is_some() => (left.asking, &mut left.typed),
            (_, Some(running)) => (running.asking, &mut running.typed),
            _ => return Step::default(),
        };
        if !asking.is_some_and(AnswerKind::shown) {
            return Step::default();
        }
        if terminal {
            text.clone_into(typed);
            return self.commit(String::new());
        }
        // A note since the last key ended the echo line; a new one starts empty.
        let (mut err, shown) = if echo_line {
            (String::new(), typed.as_str())
        } else {
            (echo_prefix(self.options.glyphs()), "")
        };
        err.push_str(&echo_edit(shown, text));
        text.clone_into(typed);
        self.echo_line = true;
        Step { err, ..Step::default() }
    }

    /// An answer line that is not shown starts with `count` characters typed ahead while
    /// the call asked nothing: a note says so, because nothing else on the screen does,
    /// and a stray key in front of a password fails it.
    pub(crate) fn typed_ahead(&mut self, count: usize, size: Size) -> Step {
        let unit = if count == 1 { "character" } else { "characters" };
        let line = format!("the answer starts with {count} {unit} typed ahead; Ctrl+U clears them");
        self.note(&line, size)
    }

    /// The answer of `kind` reached the command. A password, hidden or behind a relay,
    /// gets no note: the line under its prompt said how it goes, and the command's own
    /// output shows what came of it. Any other answer gets one, because its echo goes.
    pub(crate) fn answer_sent(&mut self, kind: AnswerKind, size: Size) -> Step {
        let note = (!kind.guards()).then_some(ANSWER_SENT);
        self.answer_note(note, size)
    }

    /// The daemon refused the answer because the command no longer waits for it.
    pub(crate) fn answer_refused(&mut self, size: Size) -> Step {
        self.answer_note(Some(ANSWER_REFUSED), size)
    }

    /// The daemon refused the answer for another reason, which `message` gives.
    pub(crate) fn answer_failed(&mut self, message: &str, size: Size) -> Step {
        let line = format!("the answer was not sent: {}", format::one_line(message));
        self.answer_note(Some(&line), size)
    }

    /// A note about an answer, if any; the echo of what was typed goes with it.
    /// A manual answer asks once: after it, the keys stop and the call's silence starts
    /// again.
    pub(super) fn answer_note(&mut self, text: Option<&str>, size: Size) -> Step {
        if let Some(left) = &mut self.left {
            left.typed.clear();
        }
        let mut freed = None;
        if let Some(running) = &mut self.running {
            running.typed.clear();
            if running.asking.is_some_and(AnswerKind::manual) {
                running.asking = None;
                running.stirred();
                freed = Some(running.call_id);
            }
        }
        let step = match text {
            Some(text) => self.note(text, size),
            None => self.commit(String::new()),
        };
        match freed {
            Some(call) => self.manual_closed(Some(call), step),
            None => step,
        }
    }
}

/// What a command waits for, in the answer line of a visible wait.
const VISIBLE: &str = "the command waits for input";

/// The keys of the answer line of a visible wait.
const VISIBLE_KEYS: &[&str] = &["enter answers", "ctrl+c stops the command"];

/// The keys of the answer line of a hidden wait: what is typed shows nowhere, so a fake
/// password prompt gets nothing on the screen either.
const HIDDEN_KEYS: &[&str] = &["typing is hidden", "enter answers"];

/// What a manual input line asks for: nothing reported a prompt.
const MANUAL: &str = "an input for the command";

/// The keys of a manual input line, which is not shown: it may be a password.
const MANUAL_KEYS: &[&str] = &["typing is not shown here", "ctrl+\\ cancels"];

/// The answer line of a wait of `kind` for a command whose first program is `program`,
/// as pieces: `?` in the `warning` role in bold, what it waits for in `warning`, two
/// spaces, and the keys muted, apart by the separator of `glyphs`.
pub(super) fn answer_line(
    kind: AnswerKind,
    program: Option<&str>,
    glyphs: &Glyphs,
) -> Vec<(String, Tone)> {
    let program = program.unwrap_or("the command");
    let separator = format!(" {} ", glyphs.separator);
    let (what, keys) = match kind {
        AnswerKind::Visible => (VISIBLE.to_owned(), VISIBLE_KEYS.join(&separator)),
        AnswerKind::Hidden => (format!("password for {program}"), HIDDEN_KEYS.join(&separator)),
        // NOTE: the program on the inner terminal decides whether the answer shows, and
        // the agent sees it only if that program shows it.
        AnswerKind::Masked => (
            format!("looks like a password behind {program}"),
            format!("typing is not shown here; {program} decides the rest"),
        ),
        AnswerKind::Manual => (MANUAL.to_owned(), MANUAL_KEYS.join(&separator)),
    };
    vec![
        (glyphs.question.to_owned(), Tone::AttentionBold),
        (" ".to_owned(), Tone::Plain),
        (what, Tone::Attention),
        ("  ".to_owned(), Tone::Plain),
        (keys, Tone::Dim),
    ]
}

/// The first program of `command`, such as `sudo` of `sudo pacman -Syu`, for the answer
/// line; `None` for an empty command.
pub(crate) fn program(command: &str) -> Option<&str> {
    let word = command.split_whitespace().find(|word| !word.contains('='))?;
    Some(word.rsplit('/').next().unwrap_or(word))
}

/// `pieces` as one row with its newline, cut to the width on a terminal.
pub(crate) fn piece_row(pieces: &[(String, Tone)], options: &RenderOptions) -> String {
    let mut out = String::new();
    let mut left = format::columns(options).map(|(width, _)| width);
    for (text, tone) in pieces {
        let text = match (left, format::columns(options)) {
            (Some(room), Some((_, method))) => {
                let width = efr_render::text_width(text, method);
                if width > room {
                    let cut = format::cut(text, room, method);
                    out.push_str(&format::paint(&cut, *tone, options));
                    break;
                }
                left = Some(room - width);
                text.clone()
            }
            _ => text.clone(),
        };
        out.push_str(&format::paint(&text, *tone, options));
    }
    out.push('\n');
    out
}

/// The row under an answer line: `› ` in the `accent` role and what the user typed,
/// when it is shown; the mark alone when it is not.
pub(crate) fn echo_row(typed: Option<&str>, options: &RenderOptions) -> String {
    let mut out = format::paint(options.glyphs().prompt, Tone::Accent, options);
    out.push(' ');
    if let Some(typed) = typed {
        out.push_str(&format::one_line(typed));
    }
    out.push('\n');
    out
}

/// What the echo of a visible answer starts with when the output is not a terminal.
pub(crate) fn echo_prefix(glyphs: &Glyphs) -> String {
    format!("{} ", glyphs.prompt)
}

/// The row under a call that has printed nothing for [`SILENCE`] and reports no wait:
/// `no output for 10s · ctrl+\\ types an input for the command`.
pub(crate) fn silence_hint(glyphs: &Glyphs) -> String {
    format!(
        "no output for {} {} ctrl+\\ types an input for the command",
        format::elapsed(SILENCE),
        glyphs.separator
    )
}

/// The note when the user closed a manual answer line unsent.
pub(super) const MANUAL_CANCELLED: &str = "the input was not sent";

/// The note when a command waits for hidden input and no key can be read here.
pub(super) const HIDDEN_INPUT_ELSEWHERE: &str =
    "the command waits for hidden input, such as a password; efr cannot ask for it here";

/// The note when a contained call waits for hidden input, or for a visible answer whose
/// prompt looks like a password prompt: efr stops it and never asks (efr's auto spec,
/// sections 3.12 and 14.7).
pub(super) const HIDDEN_INPUT_SANDBOXED: &str =
    "sandbox: the command asked for a secret; efr does not type secrets into the sandbox";

/// The note when a command waits for visible input and no key can be read here.
pub(super) const VISIBLE_INPUT_ELSEWHERE: &str =
    "the command waits for input; efr cannot ask for it here";

/// The note after an answer reached the command.
pub(super) const ANSWER_SENT: &str = "answer sent";

/// The note when the daemon refused an answer because the wait was over.
pub(super) const ANSWER_REFUSED: &str =
    "the command no longer waits for that input; nothing was sent";
