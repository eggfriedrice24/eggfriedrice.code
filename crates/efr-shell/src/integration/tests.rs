use std::path::Path;

use pretty_assertions::assert_eq;

use super::{INTEGRATION, ZSHENV, args, install, supports};

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
        INTEGRATION.contains("precmd_functions=(_efr_precmd ${precmd_functions:#_efr_precmd})")
    );
    assert!(
        INTEGRATION.contains("preexec_functions=(${preexec_functions:#_efr_preexec} _efr_preexec)")
    );
}

#[test]
fn the_script_binds_the_keys_the_session_types() {
    assert!(INTEGRATION.contains(r"bindkey -M $keymap $'\e[200~' _efr_bracketed_paste"));
    assert!(INTEGRATION.contains(r"bindkey -M $keymap $'\e[efr-cancel~' send-break"));
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
