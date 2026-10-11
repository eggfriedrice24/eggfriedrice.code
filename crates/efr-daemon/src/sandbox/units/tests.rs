use std::path::{Path, PathBuf};

use efr_permissions::{CoverAsk, Coverage};
use efr_protocol::{ConversationId, TurnId};
use efr_scope::{Git, Home};
use efr_snapshot::{DEFAULT_SNAPSHOT_TIMEOUT, HomeState, Limits, SnapshotParts, Snapshots};
use efr_test_support::TestClock;
use pretty_assertions::assert_eq;

use crate::home::{self, HomeLimits, Reader};
use crate::sandbox::units::{Prediction, coverage, predict, targets, unit_file, valid_name};

const LIMITS: HomeLimits = HomeLimits { max_files: 2000, max_bytes: 256 * 1024 * 1024 };

fn new_home() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(temp.path()).unwrap().join("home");
    std::fs::create_dir_all(&home).unwrap();
    (temp, home)
}

fn unit(home: &Path, name: &str, install: &str) -> PathBuf {
    let path = home.join(".config/systemd/user").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        format!(
            "[Unit]\nDescription=x\n\n[Service]\nExecStart=/usr/bin/true\n\n[Install]\n{install}\n"
        ),
    )
    .unwrap();
    path
}

fn states(prepared: &home::Prepared) -> Vec<(PathBuf, HomeState)> {
    prepared.entries.iter().map(|entry| (entry.path.clone(), entry.state.clone())).collect()
}

#[tokio::test]
async fn unit_enable_copies_wants_links() {
    let (_temp, home) = new_home();
    let file = unit(&home, "agent.service", "WantedBy=default.target");
    let dir = home.join(".config/systemd/user");
    let wants = dir.join("default.target.wants/agent.service");

    let before = home::prepare(&targets("agent.service", &home).unwrap(), &[], LIMITS).unwrap();
    assert_eq!(
        states(&before),
        [
            (file.clone(), HomeState::File { mode: mode(&file) }),
            (wants.clone(), HomeState::Missing),
        ]
    );
    let clock = TestClock::new();
    let store = Snapshots::new(SnapshotParts {
        dir: home.parent().unwrap().join("data/snapshots"),
        git: Git::new(clock.shared()).isolated(),
        home: Home::new(&home).unwrap(),
        clock: clock.shared(),
        excludes_file: None,
        timeout: DEFAULT_SNAPSHOT_TIMEOUT,
    });
    let id = |n: u128| uuid::Uuid::from_u128(n);
    let (conversation, turn) = (ConversationId::from_uuid(id(1)), TurnId::from_uuid(id(2)));
    let copy = store.home_before(conversation, turn, Reader::new(&before)).await.unwrap();

    // What `systemctl --user enable agent.service` writes.
    std::fs::create_dir_all(wants.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink("../agent.service", &wants).unwrap();
    let after = home::after(&before, &[], LIMITS);
    let deltas = store.home_after(&copy, Reader::new(&after)).await.unwrap();

    let changed: Vec<&Path> =
        deltas.iter().filter(|delta| delta.changed).map(|delta| delta.path.as_path()).collect();
    assert_eq!(changed, [wants.as_path()]);
    let plan = store
        .plan_undo(
            conversation,
            turn,
            &[],
            false,
            "u-0192f8a0-0000-7000-8000-000000000001",
            Limits::default(),
        )
        .await
        .unwrap();
    let steps: Vec<(PathBuf, HomeState)> =
        plan.home.steps.iter().map(|step| (step.path.clone(), step.write.clone())).collect();
    assert_eq!(steps, [(wants, HomeState::Missing)], "undo moves the new link to the trash");
}

fn mode(path: &Path) -> u32 {
    std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(path).unwrap().permissions())
        & 0o7777
}

#[test]
fn unit_prediction_rejects_path_names() {
    for name in [
        "../x.service",
        "a/b.service",
        "x",
        ".service",
        "foo@.service",
        "foo@bar.service",
        "x..y.service",
    ] {
        assert!(!valid_name(name), "{name}");
    }
    assert!(valid_name("syncthing.service"));
    assert!(valid_name("dev-disk-by\\x2duuid.mount"));

    let (_temp, home) = new_home();
    assert_eq!(unit_file("../../evil.service", &home), None);
    for install in [
        "WantedBy=../../evil.target",
        "Alias=a/b.service",
        "WantedBy=multi-user.target%i",
        "WantedBy=default.target \\",
        "Also=gone.service",
    ] {
        unit(&home, "x.service", install);
        assert_eq!(predict("x.service", &home), None, "{install}");
        assert_eq!(coverage("x.service", &home, &[], LIMITS), Coverage::NotAllowed, "{install}");
    }
    // A unit that efrd cannot find has no prediction either.
    assert_eq!(predict("absent.service", &home), None);

    // A config dir that is a link leads elsewhere: no prediction.
    let (_temp, home) = new_home();
    let elsewhere = home.join("dotfiles/systemd");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(home.join(".config/systemd")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, home.join(".config/systemd/user")).unwrap();
    std::fs::write(elsewhere.join("x.service"), "[Install]\nWantedBy=default.target\n").unwrap();
    assert_eq!(predict("x.service", &home), None);
}

#[test]
fn unit_prediction_reads_existing_link() {
    let (_temp, home) = new_home();
    unit(
        &home,
        "agent.service",
        "WantedBy=default.target timers.target\nAlias=ag.service\nAlso=helper.socket",
    );
    unit(&home, "helper.socket", "WantedBy=sockets.target");
    let dir = home.join(".config/systemd/user");
    let wants = dir.join("default.target.wants/agent.service");
    std::fs::create_dir_all(wants.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink("/usr/lib/systemd/user/agent.service", &wants).unwrap();

    let prediction = predict("agent.service", &home).unwrap();
    assert_eq!(
        prediction,
        Prediction {
            dir: dir.clone(),
            links: vec![
                dir.join("agent.service"),
                dir.join("helper.socket"),
                wants.clone(),
                dir.join("timers.target.wants/agent.service"),
                dir.join("ag.service"),
                dir.join("sockets.target.wants/helper.socket"),
            ],
        }
    );
    let prepared = home::prepare(&targets("agent.service", &home).unwrap(), &[], LIMITS).unwrap();
    let found = states(&prepared);
    let link = HomeState::Link { target: b"/usr/lib/systemd/user/agent.service".to_vec() };
    assert!(found.contains(&(wants, link)), "{found:?}");
    assert!(found.contains(&(dir.join("ag.service"), HomeState::Missing)), "{found:?}");
    assert!(
        found.iter().all(|(path, _)| path.starts_with(&dir)),
        "the link is copied as a link, never followed: {found:?}"
    );
    assert!(prepared.covers(&CoverAsk::HomeDir(dir)));
}
