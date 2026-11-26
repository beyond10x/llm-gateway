//! A loopback port where nothing answers, for the probes that must find no server.

use std::net::{Ipv4Addr, SocketAddr};

/// A port that refuses every connection for as long as this value lives: a TCP socket bound to
/// `127.0.0.1:0` that never listens. No other socket can bind the port while it is held. A port
/// chosen by binding a listener and dropping it is free for another test to take before it is
/// probed, and the probe then reaches that test's server.
pub struct Refusing {
    _socket: rustix::fd::OwnedFd,
    pub address: SocketAddr,
}

impl Refusing {
    pub fn new() -> Self {
        use rustix::net::{AddressFamily, SocketType, bind, getsockname, socket};
        let socket = socket(AddressFamily::INET, SocketType::STREAM, None).expect("a socket");
        bind(&socket, &SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).expect("loopback");
        let address = SocketAddr::try_from(getsockname(&socket).expect("its name")).expect("inet");
        Self {
            _socket: socket,
            address,
        }
    }
}
