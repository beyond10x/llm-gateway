//! The pod connection's TLS client (story:pod-proxy-tls). Stub: no TLS yet.

use crate::relaying::PodConnector;
use llm_gateway::RelayStream;
use std::{io, net::SocketAddr};

/// The shipped pod connector.
#[derive(Debug, Clone)]
pub struct ProxyConnector {
    _seam: Option<SocketAddr>,
}

impl ProxyConnector {
    pub fn shipped() -> Self {
        Self { _seam: None }
    }

    /// # Errors
    /// None yet.
    pub fn with_test_seam(_roots: &[Vec<u8>], loopback: Option<SocketAddr>) -> io::Result<Self> {
        Ok(Self { _seam: loopback })
    }
}

impl PodConnector for ProxyConnector {
    fn connect(&self, _authority: &str) -> io::Result<Box<dyn RelayStream>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the Runpod proxy needs TLS, which this binary does not speak yet",
        ))
    }
}
