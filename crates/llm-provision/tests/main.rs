//! The integration tests of this crate, one test binary: each file beside this one is a module.
//! A test that needs its own process gets one from cargo-nextest, which runs every test alone.

mod adversary_ownership;
mod adversary_pass2;
mod hosting;
