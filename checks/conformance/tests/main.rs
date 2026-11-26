//! The integration tests of this crate, one test binary: each file beside this one is a module.
//! A test that needs its own process gets one from cargo-nextest, which runs every test alone.

mod adversary_spec_diff;
mod adversary_spec_diff_2;
mod spec_diff;
