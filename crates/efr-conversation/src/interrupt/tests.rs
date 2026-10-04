use super::Interrupt;

#[tokio::test]
async fn raising_wakes_a_waiting_turn() {
    let interrupt = Interrupt::new();
    let waiting = interrupt.clone();
    let task = tokio::spawn(async move { waiting.raised().await });
    assert!(!interrupt.is_raised());

    interrupt.raise();

    task.await.expect("the waiter wakes");
    assert!(interrupt.is_raised());
}

#[tokio::test]
async fn a_raised_interrupt_completes_at_once_and_raising_twice_is_once() {
    let interrupt = Interrupt::new();
    interrupt.raise();
    interrupt.raise();
    interrupt.raised().await;
    assert!(interrupt.is_raised());
}
