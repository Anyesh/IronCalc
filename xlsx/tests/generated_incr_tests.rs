#![cfg(feature = "incr")]
#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

// Per-file tests of the incremental evaluator, produced by build.rs over the
// same workbooks as `generated_tests.rs`: the lockstep differential, and the
// read path under the read audit.
include!(concat!(env!("OUT_DIR"), "/generated_incr_tests.rs"));
