use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use efr_stdx::rng::Rng as _;
use efr_test_support::TestRng;
use pretty_assertions::assert_eq;
use secrecy::ExposeSecret as _;

use super::{Pkce, challenge, state};

/// The octets, verifier and challenge of RFC 7636, appendix B.
const RFC_OCTETS: [u8; 32] = [
    116, 24, 223, 180, 151, 153, 224, 37, 79, 250, 96, 125, 216, 173, 187, 186, 22, 212, 37, 77,
    105, 214, 191, 240, 91, 88, 5, 88, 83, 132, 141, 121,
];
const RFC_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const RFC_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

fn is_unreserved(text: &str) -> bool {
    text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

#[test]
fn the_rfc_7636_octets_make_its_verifier_and_challenge() {
    let pkce = Pkce::from_bytes(&RFC_OCTETS);
    assert_eq!(pkce.verifier().expose_secret(), RFC_VERIFIER);
    assert_eq!(pkce.challenge(), RFC_CHALLENGE);
}

#[test]
fn the_challenge_is_the_s256_of_the_verifier() {
    assert_eq!(challenge(RFC_VERIFIER), RFC_CHALLENGE);
}

#[test]
fn a_generated_verifier_is_64_bytes_of_url_safe_base64() {
    let pkce = Pkce::generate(&TestRng::new(7));
    let verifier = pkce.verifier().expose_secret();
    assert_eq!(verifier.len(), 86);
    assert!(is_unreserved(verifier), "{verifier}");
    assert_eq!(pkce.challenge(), challenge(verifier));
    assert_eq!(pkce.challenge().len(), 43);
}

#[test]
fn a_generated_verifier_encodes_the_first_64_bytes_of_the_generator() {
    let mut bytes = [0_u8; 64];
    TestRng::new(7).fill_bytes(&mut bytes);
    let pkce = Pkce::generate(&TestRng::new(7));
    assert_eq!(pkce.verifier().expose_secret(), URL_SAFE_NO_PAD.encode(bytes));
}

#[test]
fn different_seeds_give_different_verifiers() {
    let first = Pkce::generate(&TestRng::new(1));
    let second = Pkce::generate(&TestRng::new(2));
    assert_ne!(first.verifier().expose_secret(), second.verifier().expose_secret());
}

#[test]
fn the_state_is_32_bytes_of_url_safe_base64() {
    let rng = TestRng::new(9);
    let first = state(&rng);
    let second = state(&rng);
    assert_eq!(first.expose_secret().len(), 43);
    assert!(is_unreserved(first.expose_secret()));
    assert_ne!(first.expose_secret(), second.expose_secret());
}

#[test]
fn debug_shows_the_challenge_but_not_the_verifier() {
    let pkce = Pkce::from_bytes(&RFC_OCTETS);
    let debug = format!("{pkce:?}");
    assert!(!debug.contains(RFC_VERIFIER), "{debug}");
    assert!(debug.contains(RFC_CHALLENGE), "{debug}");
}
