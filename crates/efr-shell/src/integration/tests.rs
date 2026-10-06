use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use pretty_assertions::assert_eq;

use super::{CHILD, CHILD_FILE, EDITOR, EDITOR_FILE, INTEGRATION, ZSHENV, args, install, supports};

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

/// The hidden shell runs no hook of the user (efr's auto spec, 5.7): its own hooks are
/// the only ones, set again at every prompt and every command.
#[test]
fn the_script_keeps_only_its_own_hooks() {
    for line in [
        "precmd_functions=(_efr_hs_precmd)",
        "preexec_functions=(_efr_hs_preexec)",
        "chpwd_functions=(_efr_hs_report_pwd)",
        "periodic_functions=()",
        "zshaddhistory_functions=()",
        "PS1='%# '",
        "RPS1=",
        "builtin setopt no_prompt_subst",
        "for name in precmd preexec chpwd periodic zshaddhistory; do",
        "builtin zstyle -d zle-$name widgets",
        "builtin zle -D zle-$name 2>/dev/null",
    ] {
        assert!(INTEGRATION.contains(line), "missing {line}");
    }
    let strip = INTEGRATION.find("builtin zle -D zle-$name").unwrap();
    let own = INTEGRATION.find("add-zle-hook-widget line-init _efr_hs_line_init").unwrap();
    assert!(strip < own, "the user's widgets go before efr's is added");
}

#[test]
fn the_script_freezes_the_terminal_settings() {
    assert!(INTEGRATION.contains("\n  builtin ttyctl -f\n"));
}

/// The fixed line names the functions that the script defines and the variable that
/// holds them, which nothing can change.
#[test]
fn the_wrapper_check_matches_the_scripts_functions() {
    let check = crate::run::WRAPPER_CHECK;
    for name in ["_efr_hs_sbx", "_efr_hs_sbx_apply", "_efr_hs_sbx_snapshot"] {
        assert!(INTEGRATION.contains(&format!("\n{name}() {{\n")), "{name} is not defined");
        assert!(check.contains(&format!("${{functions[{name}]-}}")), "{name} is not checked");
    }
    assert!(INTEGRATION.contains(
        r#"builtin typeset -gr _efr_hs_sbx_src="$functions[_efr_hs_sbx]$functions[_efr_hs_sbx_apply]$functions[_efr_hs_sbx_snapshot]""#
    ));
    assert!(check.contains(r#"== "$_efr_hs_sbx_src""#));
    assert!(check.ends_with(r" ]] && \_efr_hs_sbx "));
    // The source is read only after the last of the three is defined.
    let src = INTEGRATION.find("builtin typeset -gr _efr_hs_sbx_src=").unwrap();
    let last = INTEGRATION.find("\n_efr_hs_sbx_snapshot() {\n").unwrap();
    assert!(last < src);
}

/// The wrapper's environment names match the ones the session passes.
#[test]
fn the_script_reads_the_sandbox_variables_once() {
    for name in [crate::env::SANDBOX_DIR, crate::env::SANDBOX_LAUNCHER] {
        assert!(INTEGRATION.contains(&format!("${{{name}-}}")), "{name} is not read");
    }
    assert!(INTEGRATION.contains("builtin unset _EFR_HS_SBX_DIR _EFR_HS_SBX_BIN"));
}

/// The end mark that the wrapper prints is the one the scanner reads.
#[test]
fn the_wrapper_prints_the_sandbox_end_mark() {
    assert!(INTEGRATION.contains(r#"$'\e]133;efr-sbx;'"$(<$dir/nonce)"$'\a'"#));
}

#[test]
fn the_child_script_writes_every_record_kind_on_descriptor_3() {
    for kind in
        ["efr-records v1 cwd", "export", "unset", "func", "unfunc", "alias", "unalias", "end"]
    {
        assert!(CHILD.contains(kind), "missing {kind}");
    }
    assert!(CHILD.contains(r#"{ builtin print -rN -- "${(@)out}" } 2>/dev/null >&3"#));
    assert!(CHILD.contains("builtin trap '_efr_child_records $?' EXIT"));
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
    for script in [ZSHENV, INTEGRATION, CHILD] {
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
    assert_eq!(std::fs::read_to_string(target.join(CHILD_FILE)).unwrap(), CHILD);
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
