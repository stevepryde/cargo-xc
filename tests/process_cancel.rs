//! The cancellation flag is process-global, so this lives in its own test
//! binary: nothing else here may spawn a managed child.

use cargo_xc::process::{self, Interrupted, Managed};

#[tokio::test]
async fn cancel_before_any_child_blocks_the_next_spawn() {
    assert!(!process::cancelled());
    // No child is running, so no receiver is subscribed to the flag; the
    // request must still stick.
    process::cancel();
    assert!(process::cancelled());

    let error = Managed::new("true")
        .status()
        .await
        .expect_err("a managed child must not start after cancellation");
    assert!(error.is::<Interrupted>(), "unexpected error: {error:#}");
}
