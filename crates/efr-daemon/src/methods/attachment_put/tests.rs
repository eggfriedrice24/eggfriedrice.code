use efr_protocol::{NAME_MAX_BYTES, fit_name};

use super::check_name;

#[test]
fn a_name_is_one_plain_file_name_that_fits() {
    for name in ["shot.png", "a b.txt", ".env.png", "ü.png", &"x".repeat(NAME_MAX_BYTES)] {
        assert!(check_name(name).is_ok(), "{name}");
    }
    let long = "x".repeat(NAME_MAX_BYTES + 1);
    for name in ["", ".", "..", "a/b.png", "a\nb.png", "a\0b", "a\u{7f}b", &long] {
        assert!(check_name(name).is_err(), "{name:?}");
    }
    assert!(check_name(&fit_name(&long)).is_ok(), "the rule of the clients fits");
}
