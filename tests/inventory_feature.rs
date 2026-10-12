use zencodec::decode::{DecodeJob as _, DecoderConfig as _};

#[cfg(not(feature = "inventory"))]
#[test]
fn disabled_inventory_is_typed_unsupported() {
    use zencodec::{ErrorCategory, RequestError, UnsupportedOperation};
    let err = zenavif::AvifDecoderConfig::new()
        .job()
        .inventory(b"")
        .unwrap_err();
    assert_eq!(
        err.error().category(),
        ErrorCategory::Request(RequestError::Unsupported(UnsupportedOperation::Inventory))
    );
}

#[cfg(feature = "inventory")]
#[test]
fn every_poll_retains_one_shot_timeout() {
    use enough::{Stop, StopReason};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct AtPoll {
        polls: Arc<AtomicUsize>,
        fail_at: usize,
    }
    impl Stop for AtPoll {
        fn check(&self) -> Result<(), StopReason> {
            if self.polls.fetch_add(1, Ordering::Relaxed) == self.fail_at {
                Err(StopReason::TimedOut)
            } else {
                Ok(())
            }
        }
    }
    for bytes in [
        b"".as_slice(),
        b"malformed".as_slice(),
        b"\0\0\0\x14ftypavif\0\0\0\0avif".as_slice(),
    ] {
        let polls = Arc::new(AtomicUsize::new(0));
        let run = |fail_at| {
            zenavif::AvifDecoderConfig::new()
                .job()
                .with_stop(zencodec::StopToken::new(AtPoll {
                    polls: polls.clone(),
                    fail_at,
                }))
                .inventory(bytes)
        };
        run(usize::MAX).unwrap();
        let total = polls.load(Ordering::Relaxed);
        assert!(total > 0);
        for fail_at in 0..total {
            polls.store(0, Ordering::Relaxed);
            let error = run(fail_at).unwrap_err();
            assert_eq!(
                error.error().category(),
                zencodec::ErrorCategory::Stopped(StopReason::TimedOut),
                "poll {fail_at}/{total}"
            );
        }
    }
}
