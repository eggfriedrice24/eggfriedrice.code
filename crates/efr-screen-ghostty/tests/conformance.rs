//! The efr-screen conformance suite against the ghostty backend: every fixture in
//! `crates/efr-screen/fixtures/`, driven through the real `ScreenActor`.

#[test]
fn conformance() {
    efr_screen::conformance::run("ghostty", efr_screen_ghostty::factory);
}
