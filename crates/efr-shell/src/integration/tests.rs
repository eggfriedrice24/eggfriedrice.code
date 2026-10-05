use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use pretty_assertions::assert_eq;

use super::{EDITOR, EDITOR_FILE, INTEGRATION, ZSHENV, args, install, supports};

/// The exact sequences the scanner and a ghostty screen expect, as the script writes
/// them in zsh's `$'...'` quoting.
#[test]
fn the_script_emits_the_ghostty_sequence_set() {
    for sequence in [
        r"$'\e]133;A;cl=line\a'",
        r"$'\e]133;P;k=s\a\e]133;B\a'",
        r"$'\e]133;B\a'",
        r"$'\e]133;C\a'",
        r#"$'\e]133;D;'"${st}"$'\a'"#,
        r"$'\e]133;D\a'",
        r#"$'\e]7;kitty-shell-cwd://'"${HOST}${PWD}"$'\a'"#,
    ] {
        assert!(INTEGRATION.contains(sequence), "missing {sequence}");
    }
}

#[test]
fn the_script_puts_its_precmd_hook_first_and_its_preexec_hook_last() {
    assert!(
        INTEGRATION
            .contains("precmd_functions=(_efr_hs_precmd ${precmd_functions:#_efr_hs_precmd})")
    );
    assert!(
        INTEGRATION
            .contains("preexec_functions=(${preexec_functions:#_efr_hs_preexec} _efr_hs_preexec)")
    );
}

#[test]
fn input_left_over_by_a_command_is_thrown_away_before_its_end_mark() {
    let drain = INTEGRATION.find("    _efr_hs_drain\n").expect("precmd drains the input");
    let end = INTEGRATION.find(r#"$'\e]133;D;'"${st}"$'\a'"#).unwrap();
    assert!(drain < end, "the drain runs before D");
    assert!(INTEGRATION.contains("while builtin read -s -t 0 -k 1 junk 2>/dev/null; do :; done"));
}

#[test]
fn the_script_binds_the_keys_the_session_types() {
    assert!(INTEGRATION.contains(r"bindkey -M $keymap $'\e[efr-clear~' _efr_hs_clear_line"));
    assert!(INTEGRATION.contains(r"bindkey -M $keymap $'\e[200~' _efr_hs_bracketed_paste"));
    assert!(INTEGRATION.contains(r"bindkey -M $keymap $'\e[efr-cancel~' send-break"));
    assert!(
        INTEGRATION.contains(r"bindkey -M $keymap $'\e[efr-forget~' _efr_hs_forget_credentials")
    );
}

/// The user's .zshrc may load efr.plugin.zsh, whose functions are named `_efr_*`; a
/// shared name would let the plugin replace a hook of the hidden shell.
#[test]
fn every_name_of_the_script_is_private_to_the_hidden_shell() {
    let mut rest = INTEGRATION;
    while let Some(at) = rest.find("_efr_") {
        let name: String =
            rest[at..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        assert!(name.starts_with("_efr_hs_"), "{name} is not private to the hidden shell");
        rest = &rest[at + name.len()..];
    }
}

/// The script turns off the same pagers as the daemon's environment, after the
/// user's startup files.
#[test]
fn the_script_turns_off_the_pagers_the_environment_turns_off() {
    let assignments: Vec<String> =
        crate::env::PAGERS.iter().map(|(name, value)| format!("{name}={value}")).collect();
    let export = format!("builtin export {}", assignments.join(" "));
    assert!(INTEGRATION.contains(&export), "missing {export}");
}

/// The script points the same editor variables as the daemon's environment at the
/// stub, after the user's startup files, and finds the stub under the name it is
/// written under.
#[test]
fn the_script_points_the_editors_the_environment_names_at_the_stub() {
    let assignments: Vec<String> =
        crate::env::EDITORS.iter().map(|name| format!("{name}=$_efr_hs_editor")).collect();
    let export = format!("builtin export {}", assignments.join(" "));
    assert!(INTEGRATION.contains(&export), "missing {export}");
    let stub = format!("builtin typeset -g _efr_hs_editor=${{${{(%):-%x}}:A:h}}/{EDITOR_FILE}");
    assert!(INTEGRATION.contains(&stub), "missing {stub}");
}

#[test]
fn the_editor_fails_and_says_how_to_go_on() {
    assert!(EDITOR.starts_with("#!/bin/sh\n"));
    assert!(EDITOR.contains(
        "'efr: there is no editor in the hidden shell; pass the text another way, such as git commit -m, a file, or ask the user to edit it' >&2"
    ));
    assert!(EDITOR.ends_with("exit 1\n"));
}

/// The stub runs: it prints its message on stderr, nothing on stdout, and fails,
/// whatever file it is asked to edit.
#[tokio::test]
async fn the_installed_editor_runs_and_fails() {
    let dir = tempfile::tempdir().unwrap();
    install(dir.path()).unwrap();
    let editor = dir.path().join(EDITOR_FILE);
    let mode = std::fs::metadata(&editor).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    let output = efr_stdx::process::command(&editor, dir.path())
        .arg("COMMIT_EDITMSG")
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "efr: there is no editor in the hidden shell; pass the text another way, such as git commit -m, a file, or ask the user to edit it\n"
    );
}

#[test]
fn the_shim_restores_zdotdir_and_sources_the_users_zshenv() {
    assert!(ZSHENV.contains("ZDOTDIR=$_EFR_USER_ZDOTDIR"));
    assert!(ZSHENV.contains("builtin unset ZDOTDIR"));
    assert!(ZSHENV.contains("${ZDOTDIR:-$HOME}/.zshenv"));
    assert!(ZSHENV.contains("efr-integration.zsh"));
}

#[test]
fn neither_script_carries_ghosttys_licence_text() {
    for script in [ZSHENV, INTEGRATION] {
        assert!(!script.contains("GNU General Public License"));
        assert!(!script.contains("ghostty_"));
    }
}

#[test]
fn install_writes_both_files_and_replaces_old_copies() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("zsh");
    install(&target).unwrap();
    std::fs::write(target.join("efr-integration.zsh"), "old").unwrap();
    install(&target).unwrap();
    assert_eq!(std::fs::read_to_string(target.join(".zshenv")).unwrap(), ZSHENV);
    assert_eq!(std::fs::read_to_string(target.join("efr-integration.zsh")).unwrap(), INTEGRATION);
    assert_eq!(std::fs::read_to_string(target.join("efr-editor")).unwrap(), EDITOR);
}

#[test]
fn only_a_zsh_gets_the_integration() {
    assert!(supports(Path::new("/usr/bin/zsh")));
    assert!(supports(Path::new("/usr/local/bin/zsh-5.9")));
    assert!(!supports(Path::new("/bin/bash")));
    assert!(!supports(Path::new("/bin/sh")));
}

#[test]
fn the_shell_is_interactive_and_a_login_shell_when_asked() {
    assert_eq!(args(true), ["-l", "-i"]);
    assert_eq!(args(false), ["-i"]);
}
