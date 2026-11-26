//! The integration tests of this crate, one test binary: each file beside this one is a module.
//! A test that needs its own process gets one from cargo-nextest, which runs every test alone.

mod adversary;
mod adversary_claude_code_wire;
mod adversary_local_open;
mod adversary_pass_2;
mod adversary_passthrough;
mod adversary_public_listing;
mod adversary_relay;
mod adversary_w02_translation_p2;
mod adversary_w03_relay_read_interrupted;
mod adversary_w05_tool_calling;
mod adversary_w06_pass_2;
mod adversary_w06_profiles;
mod adversary_w10_base_path;
mod adversary_w10_ingress;
mod adversary_w10_ingress_pass2;
mod adversary_w10_static_headers;
mod adversary_w10_static_headers_pass_2;
mod adversary_w12_wake;
mod client_credentials;
mod dependency_boundary;
mod gateway;
mod index_page;
mod lifecycle_routes;
mod observability;
mod prometheus_metrics;
mod public_listing;
mod relayed_session;
mod security_w06_head_buffer;
mod service_account_ingress;
mod spec_declarations;
mod translation_port;
mod wire_relay;
