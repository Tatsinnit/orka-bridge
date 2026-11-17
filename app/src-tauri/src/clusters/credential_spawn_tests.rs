use super::*;
#[path = "../../tests/support/exec_credential_fixture.rs"]
mod fixture;
use std::{
    future::Future,
    task::{Context, Poll, Waker},
};

fn plugin() -> ExecConfig {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "client.authentication.k8s.io/v1",
        "command": "/nonexistent/synthetic-credential-plugin",
        "interactiveMode": "Never"
    }))
    .unwrap()
}

#[tokio::test]
async fn cancelled_guard_wait_never_attempts_plugin_spawn() {
    let held = crate::runtime::spawn_guard::tests::hold();
    let (close, receiver) = watch::channel(false);
    let mut launch = Box::pin(execute(
        plugin(),
        Instant::now() + Duration::from_secs(60),
        receiver,
    ));
    let mut context = Context::from_waker(Waker::noop());
    assert!(
        launch.as_mut().poll(&mut context).is_pending(),
        "plugin spawn entered the private descriptor setup window"
    );
    close.send_replace(true);
    assert!(matches!(
        launch.as_mut().poll(&mut context),
        Poll::Ready(Err(AccessOutcome::Unavailable))
    ));
    drop(held);
}

#[tokio::test(start_paused = true)]
async fn ten_second_plugin_limit_includes_guard_wait() {
    let held = crate::runtime::spawn_guard::tests::hold();
    let (_close, receiver) = watch::channel(false);
    let mut launch = Box::pin(execute(
        plugin(),
        Instant::now() + Duration::from_secs(60),
        receiver,
    ));
    let mut context = Context::from_waker(Waker::noop());
    assert!(launch.as_mut().poll(&mut context).is_pending());
    tokio::time::advance(Duration::from_secs(10)).await;
    assert!(matches!(
        launch.as_mut().poll(&mut context),
        Poll::Ready(Err(AccessOutcome::Unavailable))
    ));
    drop(held);
}

#[tokio::test(start_paused = true)]
async fn guard_wait_and_plugin_share_one_ten_second_budget() {
    let version = "client.authentication.k8s.io/v1";
    let mut auth = fixture::exec(version, fixture::response(version));
    auth["exec"]["args"] = serde_json::json!(["hang"]);
    let exec = serde_json::from_value(auth["exec"].take()).unwrap();
    let held = crate::runtime::spawn_guard::tests::hold();
    let (_close, receiver) = watch::channel(false);
    let start = Instant::now();
    let mut launch = Box::pin(execute(exec, start + Duration::from_secs(60), receiver));
    let mut context = Context::from_waker(Waker::noop());
    assert!(launch.as_mut().poll(&mut context).is_pending());
    tokio::time::advance(Duration::from_secs(6)).await;
    drop(held);
    // Enter the real child phase before allowing the test clock to auto-advance.
    assert!(launch.as_mut().poll(&mut context).is_pending());
    assert!(matches!(launch.await, Err(AccessOutcome::Unavailable)));
    assert_eq!(start.elapsed(), Duration::from_secs(10));
}

#[tokio::test(start_paused = true)]
async fn earlier_caller_deadline_includes_guard_wait() {
    let held = crate::runtime::spawn_guard::tests::hold();
    let (_close, receiver) = watch::channel(false);
    let mut launch = Box::pin(execute(
        plugin(),
        Instant::now() + Duration::from_millis(100),
        receiver,
    ));
    let mut context = Context::from_waker(Waker::noop());
    assert!(launch.as_mut().poll(&mut context).is_pending());
    tokio::time::advance(Duration::from_millis(100)).await;
    assert!(matches!(
        launch.as_mut().poll(&mut context),
        Poll::Ready(Err(AccessOutcome::Unavailable))
    ));
    drop(held);
}
