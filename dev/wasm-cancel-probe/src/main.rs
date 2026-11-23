//! Execute the production cancellation bridge on a threadless WASI target.
#![forbid(unsafe_code)]
#[cfg(target_arch = "wasm32")]
#[path = "../../../src/cancel.rs"]
mod cancel;
#[cfg(target_arch = "wasm32")]
use enough::{Stop, StopReason};
#[cfg(target_arch = "wasm32")]
struct Live;
#[cfg(target_arch = "wasm32")]
impl Stop for Live {
    fn check(&self) -> Result<(), StopReason> {
        Ok(())
    }
}
#[cfg(target_arch = "wasm32")]
fn main() {
    assert!(
        Live.may_stop(),
        "the negative control must reach the live relay path"
    );
    let mut calls = 0;
    let result = cancel::with_relayed_stop(&Live, |token| {
        calls += 1;
        assert!(
            token.is_none(),
            "threadless Wasm must use boundary cancellation checks"
        );
        37
    });
    assert_eq!((calls, result), (1, 37));
    assert!(cancel::with_relayed_stop(&enough::Unstoppable, |token| {
        token.is_none()
    }));
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    panic!("run this probe on wasm32-wasip1");
}
