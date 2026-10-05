use std::path::PathBuf;

use efr_protocol::Mode;
use efr_scope::Repo;
use insta::assert_snapshot;

use super::LiveState;

fn minimal() -> LiveState {
    LiveState {
        cwd: PathBuf::from("/home/u"),
        oldpwd: None,
        last_command: None,
        last_status: None,
        repo: None,
        home: PathBuf::from("/home/u"),
        host: None,
        os: None,
        ssh: false,
        scratch: PathBuf::from("/home/u/.local/share/efr/scratch/2026-10-04-hello-0a1b2c3d"),
        agent_cwd: None,
        mode: Mode::Cautious,
    }
}

fn full() -> LiveState {
    LiveState {
        cwd: PathBuf::from("/home/u/p/efr"),
        oldpwd: Some(PathBuf::from("/home/u")),
        last_command: Some("cargo test -p efr-store".to_owned()),
        last_status: Some(101),
        repo: Some(Repo { root: PathBuf::from("/home/u/p/efr"), branch: Some("main".to_owned()) }),
        host: Some("box".to_owned()),
        os: Some("Arch Linux".to_owned()),
        agent_cwd: Some(PathBuf::from("/etc")),
        mode: Mode::Auto,
        ..minimal()
    }
}

#[test]
fn a_full_live_state() {
    assert_snapshot!(full().render());
}

#[test]
fn a_minimal_live_state() {
    assert_snapshot!(minimal().render());
}

#[test]
fn a_detached_head_over_ssh_with_only_a_status() {
    let state = LiveState {
        repo: Some(Repo { root: PathBuf::from("/srv/site"), branch: None }),
        cwd: PathBuf::from("/srv/site/public"),
        last_status: Some(0),
        ssh: true,
        os: Some("Debian GNU/Linux 13".to_owned()),
        mode: Mode::Manual,
        ..minimal()
    };
    assert_snapshot!(state.render());
}

#[test]
fn a_multi_line_command_goes_in_a_block() {
    let state = LiveState {
        last_command: Some("for f in *.log; do\n  gzip \"$f\"\ndone".to_owned()),
        last_status: Some(1),
        ..minimal()
    };
    assert_snapshot!(state.render());
}

#[test]
fn a_long_command_is_cut() {
    let state = LiveState { last_command: Some(format!("echo {}", "x".repeat(500))), ..minimal() };
    let text = state.render();
    assert!(text.contains(" [cut]`."), "{text}");
    assert!(!text.contains(&"x".repeat(400)), "{text}");
}

#[test]
fn the_hidden_shell_is_left_out_when_it_is_where_the_user_is() {
    let state = LiveState { agent_cwd: Some(PathBuf::from("/home/u")), ..minimal() };
    assert!(!state.render().contains("Your hidden shell"));
}

#[test]
fn every_mode_says_what_it_means_for_the_calls() {
    for (mode, says) in [
        (Mode::Manual, "Permission mode: manual. Every call asks the user"),
        (Mode::Cautious, "Permission mode: cautious. Reads and read-only commands run at once"),
        (Mode::Auto, "Permission mode: auto. Calls outside the auto list ask the user."),
    ] {
        let text = LiveState { mode, ..minimal() }.render();
        assert!(text.contains(says), "{mode}: {text}");
        assert!(text.ends_with(".\n</live_state>"), "{text}");
    }
}

#[test]
fn debug_leaves_out_the_last_command() {
    let state = LiveState { last_command: Some("export TOKEN=hunter2".to_owned()), ..minimal() };
    assert!(!format!("{state:?}").contains("hunter2"));
}
