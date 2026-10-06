use pretty_assertions::assert_eq;

use super::parse;

const NONCE: [u8; 16] = [
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];

#[test]
fn a_nonce_of_32_lowercase_hex_digits() {
    assert_eq!(parse(b"efr-sbx;00112233445566778899aabbccddeeff"), Some(NONCE));
}

#[test]
fn every_other_shape_is_not_the_mark() {
    for body in [
        &b"efr-sbx;"[..],
        b"efr-sbx",
        b"efr-sbx;00112233445566778899aabbccddeef",
        b"efr-sbx;00112233445566778899aabbccddeeff0",
        b"efr-sbx;00112233445566778899AABBCCDDEEFF",
        b"efr-sbx;00112233445566778899aabbccddeefg",
        b"efr-sbx;00112233445566778899aabbccddeeff;aid=1",
        b"efr-sbx; 0112233445566778899aabbccddeeff",
        b"efr-sbxx00112233445566778899aabbccddeeff",
        b"D;0",
    ] {
        assert_eq!(parse(body), None, "{}", String::from_utf8_lossy(body));
    }
}
