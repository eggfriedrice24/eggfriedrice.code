use std::path::PathBuf;

use efr_protocol::{Mode, ModeFallback};
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
        moved_from: None,
        mode: Mode::Cautious,
        fallback: None,
        model: "gpt-5.5".to_owned(),
        effort: None,
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
        effort: Some("high".to_owned()),
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
fn a_move_of_the_user_since_the_last_prompt_is_one_line() {
    let state = LiveState {
        cwd: PathBuf::from("/home/u/p/efr"),
        moved_from: Some(PathBuf::from("/home/u")),
        ..minimal()
    };
    let text = state.render();
    assert!(
        text.contains(
            "\nThe user moved from /home/u to /home/u/p/efr since the last prompt; your hidden \
             shell moved with them.\n"
        ),
        "{text}"
    );
    assert!(!text.contains("Your hidden shell is in"), "{text}");

    let shell_there = LiveState { agent_cwd: Some(PathBuf::from("/home/u/p/efr")), ..state };
    assert_eq!(shell_there.render(), text, "a shell where the user is moved with them");
    assert!(!minimal().render().contains("moved"), "no move, no line");
}

#[test]
fn a_hidden_shell_that_could_not_move_says_where_it_stayed() {
    let state = LiveState {
        cwd: PathBuf::from("/home/u/p/efr"),
        moved_from: Some(PathBuf::from("/home/u")),
        agent_cwd: Some(PathBuf::from("/etc")),
        ..minimal()
    };
    assert_snapshot!(state.render());
}

#[test]
fn debug_leaves_out_the_last_command() {
    let state = LiveState { last_command: Some("export TOKEN=hunter2".to_owned()), ..minimal() };
    assert!(!format!("{state:?}").contains("hunter2"));
}

#[test]
fn the_preamble_shows_the_last_command_with_its_secrets_redacted() {
    let key = format!("sk-ant-api03-{}", "a".repeat(400));
    let state = LiveState {
        last_command: Some(format!("export TOKEN=hunter2 && curl -H 'x-api-key: {key}' x")),
        last_status: Some(0),
        ..minimal()
    };

    let text = state.render();

    assert!(
        text.contains(
            "The user's last command was `export TOKEN=[redacted] && curl -H 'x-api-key: \
             [redacted]' x`; it exited with status 0."
        ),
        "{text}"
    );
    assert!(!text.contains("hunter2") && !text.contains("aaaa"), "{text}");
}

/// The part of the preamble from the mode line on.
fn settings_part(state: &LiveState) -> String {
    let text = state.render();
    let start = text.find("Permission mode:").unwrap();
    text[start..].to_owned()
}

#[test]
fn the_settings_of_a_turn_with_its_effort() {
    let state = LiveState {
        mode: Mode::Manual,
        model: "gpt-5.4".to_owned(),
        effort: Some("xhigh".to_owned()),
        ..minimal()
    };
    assert_snapshot!(settings_part(&state));
}

#[test]
fn the_settings_of_a_turn_that_leaves_the_effort_to_the_backend() {
    assert_snapshot!(settings_part(&minimal()));
}

#[test]
fn the_three_modes_and_who_changes_the_settings_are_in_every_preamble() {
    for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
        let text = LiveState { mode, ..minimal() }.render();
        for says in [
            "efr has three permission modes and no others:",
            "\n- manual: every read, write, command and network access asks the user",
            "\n- cautious: reads outside secrets, the read-only commands, and writes in \
             $SCRATCH and in the turn's registered project run at once",
            "\n- auto: shell commands run at once in a sandbox.",
            "call shell again with the narrowest needs that fits and a reason; the user \
             decides.",
            "needs.bus a message bus for systemctl",
            "journalctl needs no grant",
            "Ask for needs.outside only when nothing narrower fits, and say why in the reason",
            "A command that must run outside the sandbox (sudo, a push) must be alone in its \
             call",
            "nested_shell is not available in auto.",
            "Only the user changes this terminal's mode, model and effort, with the lines \
             ,mode ,model and ,effort",
            "in sticky mode a bare line such as mode auto works too",
            "never say that you switched or will switch one",
            "Your settings tool changes only the defaults in config.toml, after the user \
             approves its diff",
        ] {
            assert!(text.contains(says), "{mode}: {says:?} in {text}");
        }
        assert_eq!(text.matches("\n- ").count(), 3, "{text}");
        // The mode line names the mode, and the list alone says what it means.
        assert!(text.contains(&format!("\nPermission mode: {mode}\n")), "{text}");
        assert_eq!(text.matches("ask the user, except what").count(), 0, "{text}");
        assert_eq!(text.matches("except what the user's own rules allow").count(), 1, "{text}");
        assert!(text.ends_with(".\n</live_state>"), "{text}");
    }
}

#[test]
fn debug_shows_the_model_and_the_effort() {
    let state = LiveState { effort: Some("high".to_owned()), ..minimal() };
    let debug = format!("{state:?}");
    assert!(debug.contains("gpt-5.5") && debug.contains("high"), "{debug}");
}

#[test]
fn a_turn_that_fell_back_says_which_mode_it_runs_as_and_why() {
    let state = LiveState {
        fallback: Some(ModeFallback {
            asked: Mode::Auto,
            reason: "bubblewrap is not installed".to_owned(),
        }),
        ..minimal()
    };
    let text = state.render();
    assert!(
        text.contains(
            "Permission mode: cautious\nThe user asked for auto, but this turn runs as \
             cautious: bubblewrap is not installed.\n"
        ),
        "{text}"
    );
    assert!(!minimal().render().contains("The user asked for"));
}

#[test]
fn the_auto_rule_promises_nothing_of_a_later_phase() {
    let text = minimal().render();
    for later in ["phase", "reviewer", "proxy", "undo", "registries"] {
        assert!(!text.contains(later), "{later:?} in {text}");
    }
}
