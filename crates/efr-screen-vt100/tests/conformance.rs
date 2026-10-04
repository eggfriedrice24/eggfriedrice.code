//! The backend conformance suite of `efr-screen`, run against vt100.

#[test]
fn conformance() {
    efr_screen::conformance::run("vt100", efr_screen_vt100::factory);
}
