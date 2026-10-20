//! The state copies of cron and linger (efr's auto spec 10.8, plan PB20): a named
//! change of the user's crontab or of the linger setting changes no file that efrd can
//! read, so efrd reads the state with the program that sets it, before the call and
//! after it, and keeps both in the home store as the entries `@crontab` and `@linger`.
//!
//! `crontab -l` gives three states: the text of the crontab (also an empty one),
//! missing (no crontab, which `crontab -l` says with "no crontab for"), or an error,
//! which is "not covered". `loginctl show-user <uid> -p Linger --value` gives `yes` or
//! `no`. Undo reads the state again and compares it with the state after the turn: a
//! difference is a conflict, and undo leaves it as it is. Else it sets the state from
//! before the turn with `crontab -` (the text on its stdin), `crontab -r`, or
//! `loginctl enable-linger <uid>` or `disable-linger <uid>`. These are command undos.
//!
//! efrd runs only the programs of [`TrustedPrograms`], with fixed arguments and never a
//! word of the model. [`STATE_COPIES`] turns the copies off for every machine; the
//! forms that need them then stay with the user.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use efr_permissions::{CoverAsk, Coverage};
use efr_protocol::UndoReason;
use efr_snapshot::{Expect, HomeEntry, HomeState, HomeStep, blob_id};
use efr_stdx::time::Clock;
use tokio::io::AsyncWriteExt as _;

use crate::trusted_programs::{Program, TrustedPrograms};

/// True when efrd copies the crontab and the linger setting. False keeps `crontab` and
/// `loginctl` with the user on every machine.
pub(crate) const STATE_COPIES: bool = true;

/// How long one run of `crontab` or `loginctl` may take.
const RUN_TIMEOUT: Duration = Duration::from_secs(10);

/// The largest crontab that a copy takes.
const MAX_STATE_BYTES: usize = 1024 * 1024;

/// What `crontab -l` says on stderr when the user has no crontab, with cronie and with
/// the cron of Debian and Ubuntu, in the C locale.
const NO_CRONTAB: &str = "no crontab for";

/// One state that a command reads and sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateCopy {
    /// The user's crontab, `@crontab`.
    Crontab,
    /// The linger setting of the user, `@linger`.
    Linger,
}

impl StateCopy {
    /// The state that `ask` names, or `None` for an ask of a file.
    pub(crate) fn of(ask: &CoverAsk) -> Option<StateCopy> {
        match ask {
            CoverAsk::CronTable => Some(StateCopy::Crontab),
            CoverAsk::Linger(_) => Some(StateCopy::Linger),
            _ => None,
        }
    }

    /// The state of the entry `path`, or `None` for a file.
    pub(crate) fn from_path(path: &Path) -> Option<StateCopy> {
        [StateCopy::Crontab, StateCopy::Linger].into_iter().find(|copy| copy.path() == path)
    }

    /// Its name: `crontab` or `linger`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            StateCopy::Crontab => "crontab",
            StateCopy::Linger => "linger",
        }
    }

    /// The path of its entry in a copy: `@crontab` or `@linger`.
    pub(crate) fn path(self) -> PathBuf {
        PathBuf::from(format!("@{}", self.name()))
    }

    fn program(self) -> Program {
        match self {
            StateCopy::Crontab => Program::Crontab,
            StateCopy::Linger => Program::Loginctl,
        }
    }
}

/// Whether efrd can copy the state of `ask` before a call: only with [`STATE_COPIES`]
/// and its program found, and for a linger form that names a user, only when that user
/// is efrd's own. [`Coverage::Unknown`] for an ask of a file.
pub(crate) fn coverage(programs: &TrustedPrograms, ask: &CoverAsk) -> Coverage {
    if let CoverAsk::Linger(Some(user)) = ask
        && *user != uid()
        && Some(user.as_str()) != own_name()
    {
        return Coverage::NotAllowed;
    }
    match StateCopy::of(ask) {
        None => Coverage::Unknown,
        Some(copy) if STATE_COPIES && programs.has(copy.program()) => Coverage::Covered,
        Some(_) => Coverage::NotAllowed,
    }
}

/// The user database that names efrd's own user.
const PASSWD: &str = "/etc/passwd";

/// The name of efrd's own user in [`PASSWD`], read once. `None` for a user that only a
/// network directory knows: a linger form that names it then stays with the user.
fn own_name() -> Option<&'static str> {
    static NAME: OnceLock<Option<String>> = OnceLock::new();
    NAME.get_or_init(|| name_of(&uid(), fs::read_to_string(PASSWD).ok().as_deref())).as_deref()
}

/// The name of the line of `passwd` whose uid is `uid`.
fn name_of(uid: &str, passwd: Option<&str>) -> Option<String> {
    passwd?.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        (fields.nth(1)? == uid).then(|| name.to_owned())
    })
}

/// The programs that a state copy runs, and the clock of their time limit.
#[derive(Clone, Copy)]
pub(crate) struct Runner<'a> {
    pub(crate) programs: &'a TrustedPrograms,
    pub(crate) clock: &'a dyn Clock,
}

/// The state of `copy` now, as a copy's entry with its bytes: the text of a state,
/// missing, or not covered when the program failed or efrd found none.
pub(crate) async fn read(runner: Runner<'_>, copy: StateCopy) -> (HomeEntry, Option<Vec<u8>>) {
    let entry = |state: HomeState| HomeEntry::new(copy.path(), state, true);
    let not_covered = (entry(HomeState::NotCovered(UndoReason::Unknown)), None);
    if !STATE_COPIES {
        return not_covered;
    }
    let args: Vec<String> = match copy {
        StateCopy::Crontab => vec!["-l".to_owned()],
        StateCopy::Linger => {
            vec![
                "show-user".to_owned(),
                uid(),
                "--property=Linger".to_owned(),
                "--value".to_owned(),
            ]
        }
    };
    let Some(output) = run(runner, copy.program(), &args, None).await else { return not_covered };
    match copy {
        StateCopy::Crontab if output.status.success() => {
            if output.stdout.len() > MAX_STATE_BYTES {
                return (entry(HomeState::NotCovered(UndoReason::OverLimit)), None);
            }
            (entry(HomeState::State), Some(output.stdout))
        }
        StateCopy::Crontab if String::from_utf8_lossy(&output.stderr).contains(NO_CRONTAB) => {
            (entry(HomeState::Missing), None)
        }
        StateCopy::Linger if output.status.success() => {
            match String::from_utf8_lossy(&output.stdout).trim() {
                answer @ ("yes" | "no") => {
                    (entry(HomeState::State), Some(format!("{answer}\n").into_bytes()))
                }
                _ => not_covered,
            }
        }
        _ => {
            tracing::info!(
                program = copy.program().name(),
                "a state copy could not read the state"
            );
            not_covered
        }
    }
}

/// What a command undo of one state came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Undone {
    /// The state is the one before the turn now, or it was already.
    Applied,
    /// The state is not the one after the turn: undo left it as it is.
    Changed,
}

/// The state that `now` read: `Some(None)` when it is missing, `None` when efrd could
/// not read it.
fn current(now: &(HomeEntry, Option<Vec<u8>>)) -> Option<Option<&[u8]>> {
    match (&now.0.state, &now.1) {
        (HomeState::Missing, _) => Some(None),
        (HomeState::State, Some(text)) => Some(Some(text.as_slice())),
        _ => None,
    }
}

/// True when the state `current` is what the turn left, as `expect` says.
fn expected(expect: &Expect, current: Option<&[u8]>) -> bool {
    match (expect, current) {
        (Expect::Any, _) => true,
        (Expect::Missing, None) => true,
        (Expect::Blob(id), Some(text)) => blob_id(text) == *id,
        _ => false,
    }
}

/// True when the state that `now` read is no longer what the turn left, so undo will
/// leave it (the preview of `efr undo` shows it as a skip). False when efrd could not
/// read it: the apply decides then.
pub(crate) fn changed_after(step: &HomeStep, now: &(HomeEntry, Option<Vec<u8>>)) -> bool {
    current(now).is_some_and(|current| !expected(&step.expect, current))
}

/// Sets the state of `step` back, when it is what the step expects: `bytes` is the
/// state before the turn (the step's blob), `None` for a missing crontab. `now` is the
/// state that [`read`] found just before. Before it changes a crontab, `keep` gets the
/// text of now, for the undo trash.
pub(crate) async fn undo(
    runner: Runner<'_>,
    step: &HomeStep,
    now: &(HomeEntry, Option<Vec<u8>>),
    bytes: Option<Vec<u8>>,
    keep: impl FnOnce(&[u8]) -> io::Result<()>,
) -> io::Result<Undone> {
    let Some(copy) = StateCopy::from_path(&step.path) else { return Ok(Undone::Changed) };
    let Some(current) = current(now) else { return Ok(Undone::Changed) };
    let target = match (&step.write, &bytes) {
        (HomeState::State, Some(text)) => Some(text.as_slice()),
        (HomeState::Missing, _) => None,
        _ => return Ok(Undone::Changed),
    };
    if current == target {
        return Ok(Undone::Applied);
    }
    if !expected(&step.expect, current) {
        return Ok(Undone::Changed);
    }
    let (args, input): (Vec<String>, Option<&[u8]>) = match (copy, target) {
        (StateCopy::Crontab, Some(text)) => (vec!["-".to_owned()], Some(text)),
        (StateCopy::Crontab, None) => (vec!["-r".to_owned()], None),
        (StateCopy::Linger, Some(text)) => {
            let verb = match String::from_utf8_lossy(text).trim() {
                "yes" => "enable-linger",
                "no" => "disable-linger",
                _ => return Ok(Undone::Changed),
            };
            (vec![verb.to_owned(), uid()], None)
        }
        (StateCopy::Linger, None) => return Ok(Undone::Changed),
    };
    if let (StateCopy::Crontab, Some(text)) = (copy, current) {
        keep(text)?;
    }
    let output = run(runner, copy.program(), &args, input)
        .await
        .ok_or_else(|| io::Error::other(format!("{} could not run", copy.program().name())))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first = stderr.lines().next().unwrap_or_default();
        return Err(io::Error::other(format!("{} failed: {first}", copy.program().name())));
    }
    Ok(Undone::Applied)
}

/// efrd's own uid, as `loginctl` takes it.
fn uid() -> String {
    rustix::process::getuid().as_raw().to_string()
}

/// Runs `program` with `args` and `input` on its stdin, within [`RUN_TIMEOUT`];
/// `None` when it is not trusted, cannot start or takes longer.
async fn run(
    runner: Runner<'_>,
    program: Program,
    args: &[String],
    input: Option<&[u8]>,
) -> Option<std::process::Output> {
    let mut command = runner.programs.command(program)?;
    command.args(args);
    if input.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    let mut child = command.spawn().ok()?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        let fed = Clock::timeout(&runner.clock, RUN_TIMEOUT, stdin.write_all(input)).await;
        if !matches!(fed, Ok(Ok(()))) {
            return None;
        }
        drop(stdin);
    }
    match Clock::timeout(&runner.clock, RUN_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => Some(output),
        _ => {
            tracing::warn!(program = program.name(), "a trusted program did not end in time");
            None
        }
    }
}

#[cfg(test)]
mod tests;
