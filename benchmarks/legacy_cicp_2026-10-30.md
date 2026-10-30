# Legacy CICP compatibility test — 2026-10-30

Source d9396593ca982102c8550920b5afecfe0416d7ed scopes a deprecated-setter
allowance to caller_cicp_wins_over_descriptor_color. The established metadata
and pixel assertions are unchanged. This remains a best-effort compatibility
control, not checked metadata retention qualification. No library code changed.

An isolated export with the committed lock passes the unchanged encode-feature
unit test (one passed, 187 filtered). The first media-encode Clippy invocation
then failed because the export lacked a compile-time fixture. No targets were
removed. Staging the existing paris_icc_exif_xmp.avif fixture with SHA-256
961bc38b61e60b7651fa20efa24269ae2f35e4958822a81c908c9bbf9b3f66e1
lets strict Clippy --features media-encode --lib --tests --examples pass.
The lock remains byte-identical. The additional zenav1-svt CI profile was not
rerun locally. Reproduce with just test-legacy-cicp after fixture provisioning.

Logs on r7900x: /home/lilith/tmp/native-timeline-ci-20261030/.
An initial export typo produced no build (lint.log). The corrected export's
lint-exported.log preserves the passing unit test and missing-fixture failure:
wrapper rc101, 183 seconds, peak RSS 0.99 GiB, minimum available 12535 MiB.
The unchanged-source retry is lint-fixtures.log: wrapper rc0, reported 0 seconds,
peak RSS 0.17 GiB, minimum available 13839 MiB. These are validation resources,
not codec performance measurements. Processes exited before releasing the slot.

PR79 still has a separate behavior conflict: issue63's malformed-EXIF dropping
change disagrees with malformed_track_exif_is_an_error_not_missing_metadata.
That code and test are untouched. This repair does not qualify merging or
releasing the full AVIF workspace.
