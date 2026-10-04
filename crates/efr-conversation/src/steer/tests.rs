use pretty_assertions::assert_eq;

use super::Steering;

#[test]
fn steering_is_taken_oldest_first_and_once() {
    let steering = Steering::default();
    let shared = steering.clone();
    assert!(!steering.is_waiting());

    shared.push("first".to_owned());
    shared.push("second".to_owned());

    assert!(steering.is_waiting());
    assert_eq!(steering.take(), vec!["first".to_owned(), "second".to_owned()]);
    assert!(steering.take().is_empty());
    assert!(!steering.is_waiting());
}
