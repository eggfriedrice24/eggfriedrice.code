use super::looks_secret;

#[test]
fn a_prompt_that_names_a_secret_looks_secret_in_any_case() {
    for line in [
        "Password:",
        "[sudo] password for u:",
        "Password for 'https://u@github.com':",
        "Enter passphrase for key '/home/u/.ssh/id_ed25519':",
        "PASSCODE: ",
        "Enter PIN for 'YubiKey':",
        "pin:",
        "Verification code: ",
        "Enter the one-time code from your app:",
        "One time code:",
        "u@host's password: ",
    ] {
        assert!(looks_secret(line), "{line}");
    }
}

#[test]
fn a_prompt_that_names_no_secret_does_not_look_secret() {
    for line in [
        "Proceed with installation? [Y/n] ",
        "PING example.org: continue? ",
        "spinning up workers... ",
        "pinned version 1.2? [y/N] ",
        "Enter the code to continue: ",
        "Name: ",
        "",
    ] {
        assert!(!looks_secret(line), "{line}");
    }
}
