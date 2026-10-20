//! The backend conformance suite of `efr-screen`, run against the vt100 backend.

#[test]
fn vt100() {
    efr_screen::conformance::run("vt100", efr_screen::vt100::factory);
}
