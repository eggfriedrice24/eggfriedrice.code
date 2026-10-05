//! Helpers that more than one module of the test binary needs.

/// True when the tests that drive a real zsh may run (`EFR_TEST_ZSH=1`); otherwise
/// `test` says on stderr that it skips.
#[expect(clippy::print_stderr, reason = "a skipped test says why, as atuin's e2e tests do")]
pub(crate) fn zsh_enabled(test: &str) -> bool {
    let on = efr_stdx::env::flag(efr_stdx::env::Var::TestZsh).unwrap_or(false);
    if !on {
        eprintln!("skipping {test}: set EFR_TEST_ZSH=1 to run the tests that drive a real zsh");
    }
    on
}
