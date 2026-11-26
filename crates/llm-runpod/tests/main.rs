//! The integration tests of this crate, one test binary: each file beside this one is a module.
//! A test that needs its own process gets one from cargo-nextest, which runs every test alone.

mod adversary;
mod adversary2;
mod adversary_secure_cloud;
mod adversary_w02;
mod adversary_w04_hold;
mod adversary_w05_transport;
mod adversary_w05_transport_pass2;
mod adversary_w12_profiles;
mod adversary_w12_spend;
mod adversary_w12_spend_pass2;
mod openapi;
mod profiles_fallback;
mod refusing;
mod rest;
mod round2;
mod runpod;
mod security_w06_probe_buffer;
mod security_w12_spend;
mod spend;
mod transport;
mod wake_status;
