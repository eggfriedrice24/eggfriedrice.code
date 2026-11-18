//! Pushes to the paired phones over a `TestDaemon`: the capability of `hello`,
//! `device.push_set` and `admin.push_test`.

use efr_protocol::{
    AdminPushTest, AdminPushTestResult, DevicePushSet, DevicePushSetResult, ErrorCode, Method,
};
use efr_test_daemon::{ClientError, TestDaemon};

/// The code of the daemon's refusal of `result`.
fn refusal<T: std::fmt::Debug>(result: Result<T, ClientError>) -> ErrorCode {
    match result {
        Err(ClientError::Server { body }) => body.code,
        other => panic!("expected a refusal from the daemon, got {other:?}"),
    }
}

#[tokio::test]
async fn the_unix_socket_takes_no_push_token_and_a_daemon_with_no_push_tests_none() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    assert_eq!(client.hello().capabilities.push, None);

    let set = client
        .call::<DevicePushSetResult>(Method::DevicePushSet(DevicePushSet { token: None }))
        .await;
    assert_eq!(refusal(set), ErrorCode::Forbidden, "only the tailnet listener takes a token");
    let test = client.call::<AdminPushTestResult>(Method::AdminPushTest(AdminPushTest {})).await;
    assert_eq!(refusal(test), ErrorCode::Invalid);

    drop(client);
    daemon.stop().await.unwrap();
}
