use intimatr::lifecycle::{Lifecycle, LifecycleError, LifecycleState};

#[test]
fn lifecycle_moves_through_normal_start_and_stop() {
    let lifecycle = Lifecycle::new();

    assert_eq!(lifecycle.state(), LifecycleState::Detached);
    lifecycle.attach().unwrap();
    assert_eq!(lifecycle.state(), LifecycleState::Attached);
    lifecycle.begin_bootstrap().unwrap();
    assert_eq!(lifecycle.state(), LifecycleState::Bootstrapping);
    lifecycle.mark_running().unwrap();
    assert_eq!(lifecycle.state(), LifecycleState::Running);

    assert!(lifecycle.begin_shutdown().unwrap());
    assert!(lifecycle.shutdown_requested());
    assert_eq!(lifecycle.state(), LifecycleState::Stopping);

    lifecycle.mark_stopped().unwrap();
    assert_eq!(lifecycle.state(), LifecycleState::Stopped);
}

#[test]
fn lifecycle_rejects_invalid_transitions() {
    let lifecycle = Lifecycle::new();
    let error = lifecycle
        .mark_running()
        .expect_err("detached runtime cannot become running directly");

    assert_eq!(
        error,
        LifecycleError::InvalidTransition {
            from: LifecycleState::Detached,
            to: LifecycleState::Running,
        }
    );
}

#[test]
fn repeated_shutdown_is_idempotent() {
    let lifecycle = Lifecycle::new();
    lifecycle.attach().unwrap();
    lifecycle.begin_bootstrap().unwrap();
    lifecycle.mark_running().unwrap();

    assert!(lifecycle.begin_shutdown().unwrap());
    assert!(!lifecycle.begin_shutdown().unwrap());
    lifecycle.mark_stopped().unwrap();
    assert!(!lifecycle.begin_shutdown().unwrap());
}

#[test]
fn failure_does_not_override_shutdown() {
    let lifecycle = Lifecycle::new();
    lifecycle.attach().unwrap();
    lifecycle.begin_bootstrap().unwrap();
    assert!(lifecycle.begin_shutdown().unwrap());

    lifecycle.mark_failed();

    assert_eq!(lifecycle.state(), LifecycleState::Stopping);
}
