use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use efr_permissions::{CoverAsk, Coverage};
use efr_protocol::{ConversationId, TurnId, UndoReason};
use efr_scope::{Git, Home};
use efr_snapshot::{
    DEFAULT_SNAPSHOT_TIMEOUT, HomeEntries, HomeState, HomeStep, Limits, SnapshotParts, Snapshots,
};
use efr_test_support::TestClock;
use pretty_assertions::assert_eq;

use crate::home::state::{self, Runner, StateCopy, Undone};
use crate::trusted_programs::TrustedPrograms;

/// A fake `crontab` that keeps the crontab in `crontab.state` next to itself, as cronie
/// does: `-l` prints it or says "no crontab for", `-` installs stdin, `-r` removes it.
/// Each run adds its arguments to `calls`.
const CRONTAB: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
echo "$*" >> "$dir/calls"
case "$1" in
  -l) [ -f "$dir/crontab.state" ] || { echo "no crontab for u" >&2; exit 1; }
      cat "$dir/crontab.state" ;;
  -r) [ -f "$dir/crontab.state" ] || { echo "no crontab for u" >&2; exit 1; }
      rm "$dir/crontab.state" ;;
  -) cat > "$dir/crontab.state" ;;
  *) exit 2 ;;
esac
"#;

/// A fake `loginctl` that keeps `yes` or `no` in `linger.state` next to itself.
const LOGINCTL: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
echo "$*" >> "$dir/calls"
case "$1" in
  show-user) cat "$dir/linger.state" ;;
  enable-linger) echo yes > "$dir/linger.state" ;;
  disable-linger) echo no > "$dir/linger.state" ;;
  *) exit 2 ;;
esac
"#;

struct Machine {
    _temp: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
    programs: TrustedPrograms,
    clock: TestClock,
}

impl Machine {
    /// A machine whose fake programs live in `bin` below the temp dir, which no write
    /// root holds.
    fn new() -> Machine {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let bin = root.join("bin");
        install(&bin, "crontab", CRONTAB);
        install(&bin, "loginctl", LOGINCTL);
        let programs = TrustedPrograms::in_test_dir(&bin, &[root.join("home")]);
        Machine { _temp: temp, root, bin, programs, clock: TestClock::new() }
    }

    fn runner(&self) -> Runner<'_> {
        Runner { programs: &self.programs, clock: &self.clock }
    }

    fn store(&self) -> Snapshots {
        Snapshots::new(SnapshotParts {
            dir: self.root.join("data/snapshots"),
            git: Git::new(self.clock.shared()).isolated(),
            home: Home::new(self.root.join("home")).unwrap(),
            clock: self.clock.shared(),
            excludes_file: None,
            timeout: DEFAULT_SNAPSHOT_TIMEOUT,
        })
    }

    fn set(&self, name: &str, text: Option<&str>) {
        let path = self.bin.join(name);
        match text {
            Some(text) => std::fs::write(path, text).unwrap(),
            None => {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    fn get(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.bin.join(name)).ok()
    }

    fn calls(&self) -> Vec<String> {
        self.get("calls").unwrap_or_default().lines().map(str::to_owned).collect()
    }

    /// A copy of `copy` before and after `change`, then the home step of its undo.
    async fn turn(&self, copy: StateCopy, change: impl FnOnce()) -> HomeStep {
        let store = self.store();
        let (conversation, turn) = ids();
        let before = state::read(self.runner(), copy).await;
        let made =
            store.home_before(conversation, turn, HomeEntries::new(vec![before])).await.unwrap();
        change();
        let after = state::read(self.runner(), copy).await;
        let deltas = store.home_after(&made, HomeEntries::new(vec![after])).await.unwrap();
        assert!(deltas.iter().all(|delta| delta.changed && delta.covered), "{deltas:?}");
        let plan =
            store.plan_undo(conversation, turn, &[], false, UNDO, Limits::default()).await.unwrap();
        let [step] = <[HomeStep; 1]>::try_from(plan.home.steps).unwrap();
        step
    }

    /// The command undo of `step`, with the text that it keeps for the undo trash.
    async fn undo(&self, step: &HomeStep) -> (Undone, Option<Vec<u8>>) {
        let copy = StateCopy::from_path(&step.path).unwrap();
        let now = state::read(self.runner(), copy).await;
        let bytes = match &step.blob {
            Some(blob) => Some(self.store().home_blob(blob).await.unwrap()),
            None => None,
        };
        let mut kept = None;
        let keep = |text: &[u8]| {
            kept = Some(text.to_vec());
            Ok(())
        };
        let undone = state::undo(self.runner(), step, &now, bytes, keep).await.unwrap();
        (undone, kept)
    }
}

const UNDO: &str = "u-0192f8a0-0000-7000-8000-000000000001";

fn ids() -> (ConversationId, TurnId) {
    let id = |n: u128| uuid::Uuid::from_u128(n);
    (ConversationId::from_uuid(id(1)), TurnId::from_uuid(id(2)))
}

fn install(dir: &Path, name: &str, script: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
async fn crontab_state_copy_and_undo() {
    let machine = Machine::new();
    machine.set("crontab.state", Some("0 * * * * backup\n"));

    let step = machine
        .turn(StateCopy::Crontab, || machine.set("crontab.state", Some("* * * * * x\n")))
        .await;
    let (undone, kept) = machine.undo(&step).await;

    assert_eq!(undone, Undone::Applied);
    assert_eq!(machine.get("crontab.state").as_deref(), Some("0 * * * * backup\n"));
    assert_eq!(kept.as_deref(), Some(b"* * * * * x\n".as_slice()), "the turn's crontab is kept");
    assert_eq!(machine.calls(), ["-l", "-l", "-l", "-"]);

    // A crontab that the turn made goes again with `crontab -r`.
    let machine = Machine::new();
    let step = machine
        .turn(StateCopy::Crontab, || machine.set("crontab.state", Some("@reboot x\n")))
        .await;
    assert_eq!(step.write, HomeState::Missing);
    let (undone, _) = machine.undo(&step).await;
    assert_eq!(undone, Undone::Applied);
    assert_eq!(machine.get("crontab.state"), None);
    assert_eq!(machine.calls().last().map(String::as_str), Some("-r"));
}

#[tokio::test]
async fn crontab_changed_after_turn_is_a_conflict() {
    let machine = Machine::new();
    machine.set("crontab.state", Some("0 * * * * backup\n"));
    let step = machine
        .turn(StateCopy::Crontab, || machine.set("crontab.state", Some("* * * * * x\n")))
        .await;

    let now = state::read(machine.runner(), StateCopy::Crontab).await;
    assert!(!state::changed_after(&step, &now), "the crontab is what the turn left");
    machine.set("crontab.state", Some("* * * * * mine\n"));
    let now = state::read(machine.runner(), StateCopy::Crontab).await;
    assert!(state::changed_after(&step, &now), "the preview shows a skip");
    let (undone, kept) = machine.undo(&step).await;

    assert_eq!(undone, Undone::Changed);
    assert_eq!(machine.get("crontab.state").as_deref(), Some("* * * * * mine\n"));
    assert_eq!(kept, None);
    assert!(!machine.calls().contains(&"-".to_owned()), "{:?}", machine.calls());
}

#[tokio::test]
async fn linger_state_copy_and_undo() {
    let machine = Machine::new();
    machine.set("linger.state", Some("no\n"));

    let step = machine.turn(StateCopy::Linger, || machine.set("linger.state", Some("yes\n"))).await;
    let (undone, kept) = machine.undo(&step).await;

    assert_eq!(undone, Undone::Applied);
    assert_eq!(machine.get("linger.state").as_deref(), Some("no\n"));
    assert_eq!(kept, None, "linger is no text for the undo trash");
    let uid = rustix::process::getuid().as_raw();
    let show = format!("show-user {uid} --property=Linger --value");
    assert_eq!(
        machine.calls(),
        [show.clone(), show.clone(), show, format!("disable-linger {uid}")]
    );
}

#[test]
fn linger_for_another_user_is_not_covered() {
    let machine = Machine::new();
    let uid = rustix::process::getuid().as_raw().to_string();
    let linger = |user: Option<&str>| CoverAsk::Linger(user.map(str::to_owned));

    assert_eq!(state::coverage(&machine.programs, &linger(None)), Coverage::Covered);
    assert_eq!(state::coverage(&machine.programs, &linger(Some(&uid))), Coverage::Covered);
    assert_eq!(
        state::coverage(&machine.programs, &linger(Some("efr-no-such-user"))),
        Coverage::NotAllowed
    );
}

#[test]
fn name_of_reads_the_line_of_the_uid() {
    let passwd = "root:x:0:0::/root:/bin/sh\nme:x:1000:1000::/home/me:/bin/zsh\n";

    assert_eq!(state::name_of("1000", Some(passwd)).as_deref(), Some("me"));
    assert_eq!(state::name_of("0", Some(passwd)).as_deref(), Some("root"));
    assert_eq!(state::name_of("1001", Some(passwd)), None);
    assert_eq!(state::name_of("1000", None), None);
}

#[tokio::test]
async fn state_copy_never_runs_a_program_from_a_write_root() {
    let machine = Machine::new();
    let home = machine.root.join("home");
    let project_bin = home.join("p/app/bin");
    install(&project_bin, "crontab", "#!/bin/sh\ntouch \"$(dirname \"$0\")/ran\"\n");
    let programs = TrustedPrograms::in_test_dir(&project_bin, std::slice::from_ref(&home));
    let runner = Runner { programs: &programs, clock: &machine.clock };

    let (entry, bytes) = state::read(runner, StateCopy::Crontab).await;

    assert_eq!(entry.state, HomeState::NotCovered(UndoReason::Unknown));
    assert_eq!(bytes, None);
    assert_eq!(state::coverage(&programs, &CoverAsk::CronTable), Coverage::NotAllowed);
    assert!(!project_bin.join("ran").exists(), "the program in the write root never ran");
    // The same program outside every write root counts.
    assert_eq!(state::coverage(&machine.programs, &CoverAsk::CronTable), Coverage::Covered);
}

#[tokio::test]
async fn crontab_missing_empty_and_error_are_three_states() {
    let machine = Machine::new();
    let read = || state::read(machine.runner(), StateCopy::Crontab);

    assert_eq!(read().await.0.state, HomeState::Missing);
    machine.set("crontab.state", Some(""));
    assert_eq!(read().await, (crontab_entry(HomeState::State), Some(Vec::new())));
    install(&machine.bin, "crontab", "#!/bin/sh\necho 'permission denied' >&2\nexit 1\n");
    assert_eq!(read().await.0.state, HomeState::NotCovered(UndoReason::Unknown));
}

fn crontab_entry(state: HomeState) -> efr_snapshot::HomeEntry {
    efr_snapshot::HomeEntry {
        path: PathBuf::from("@crontab"),
        state,
        floor: true,
        chain: Vec::new(),
        missing_parents: Vec::new(),
    }
}
