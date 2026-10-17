//! The pod connection's TLS client (spec/domains/deployment.yaml, the pod endpoint rule).
//!
//! A pod is reached at the Runpod proxy's `https://<pod id>-8000.proxy.runpod.net/v1/` (llm's
//! Runpod description), so every connection the binary opens to a pod, the relay's and the
//! readiness probe's, is TLS: `rustls` on its `ring` provider, with the server's certificate
//! verified, chain and host name, before [`ProxyConnector`] hands the connection back. Nothing
//! is written to the pod before that, so a certificate that does not verify costs no request
//! byte and no key.
//!
//! The shipped connector ([`ProxyConnector::shipped`]) trusts exactly the Mozilla root program
//! as `webpki-roots` compiles it in. It reads no certificate store, file or environment
//! variable, and no document key or option reaches it. [`ProxyConnector::with_test_seam`] is
//! the test seam: other roots, and every pod host mapped to one loopback address.

use crate::relaying::PodConnector;
use llm_gateway::{RelayStream, read_within_timeout};
use rustls::{
    ClientConfig, ClientConnection, RootCertStore, StreamOwned,
    pki_types::{CertificateDer, ServerName},
};
use std::{
    fmt,
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    sync::Arc,
    time::{Duration, Instant},
};

/// How long opening a pod connection may take, the TCP connection and the TLS handshake together.
pub const POD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one read or one write on a relayed pod connection may wait, once it is open.
pub const POD_IO_TIMEOUT: Duration = Duration::from_secs(120);
/// The port of an `https://` authority that names none.
pub(crate) const HTTPS_PORT: u16 = 443;

/// Opens TLS connections to pods: the shipped binary's pod connector.
#[derive(Clone)]
pub struct ProxyConnector {
    config: Arc<ClientConfig>,
    /// The test seam's loopback address every pod host is connected at; `None` resolves the host.
    loopback: Option<SocketAddr>,
    /// Built by [`Self::with_test_seam`].
    seam: bool,
}

impl fmt::Debug for ProxyConnector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxyConnector")
            .field("test_seam", &self.seam)
            .field("loopback", &self.loopback)
            .finish_non_exhaustive()
    }
}

fn config(roots: RootCertStore) -> Arc<ClientConfig> {
    let mut config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            // The ring provider supports both safe default versions, TLS 1.2 and 1.3.
            .unwrap_or_else(|_| unreachable!("ring supports TLS 1.2 and 1.3"))
            .with_root_certificates(roots)
            .with_no_client_auth();
    // vLLM behind the Runpod proxy is spoken to in HTTP/1.1, as the relay writes it.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_owned())
}

impl ProxyConnector {
    /// The shipped connector: the roots `webpki-roots` compiles in, each pod host resolved and
    /// connected at its own port (443 when the endpoint names none).
    pub fn shipped() -> Self {
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        Self {
            config: config(roots),
            loopback: None,
            seam: false,
        }
    }

    /// The test seam: the same client trusting exactly `roots` (DER certificates), and, when
    /// `loopback` is given, every pod host connected at that loopback address while the
    /// certificate is still verified for the pod's own host name. Only tests call this; the
    /// shipped binary's document, command line and environment cannot reach it.
    ///
    /// # Errors
    /// `InvalidInput` for no root, a root that is not a usable DER certificate, or a `loopback`
    /// address that is not a loopback one.
    pub fn with_test_seam(roots: &[Vec<u8>], loopback: Option<SocketAddr>) -> io::Result<Self> {
        if loopback.is_some_and(|address| !address.ip().is_loopback()) {
            return Err(invalid(
                "the test seam maps pod hosts to a loopback address only",
            ));
        }
        if roots.is_empty() {
            return Err(invalid("the test seam needs at least one root"));
        }
        let mut store = RootCertStore::empty();
        for root in roots {
            store
                .add(CertificateDer::from(root.clone()))
                .map_err(|error| invalid(&format!("a test root is not usable: {error}")))?;
        }
        Ok(Self {
            config: config(store),
            loopback,
            seam: true,
        })
    }

    /// Opens one TLS connection to `authority` (`host[:port]`) and completes its handshake,
    /// certificate verification included, before returning: nothing else is written to the pod
    /// before this returns. `connect` is one deadline over the whole opening, the TCP connection
    /// and every handshake read and write together, so a peer that answers slowly or not at all
    /// cannot stretch it; `io` bounds each read and write after the opening.
    fn open(
        &self,
        authority: &str,
        connect: Duration,
        io: Duration,
    ) -> io::Result<StreamOwned<ClientConnection, BoundedSocket>> {
        let deadline = Instant::now() + connect;
        let (host, port) =
            split(authority, HTTPS_PORT).ok_or_else(|| invalid("not a host[:port] authority"))?;
        let name = ServerName::try_from(host.to_owned())
            .map_err(|_| invalid("the pod host is not a server name"))?;
        let mut socket = match self.loopback {
            Some(address) => TcpStream::connect_timeout(&address, remaining(deadline)?)?,
            None => dial(host, port, deadline)?,
        };
        socket.set_nodelay(true)?;
        let mut connection = ClientConnection::new(Arc::clone(&self.config), name)
            .map_err(|error| io::Error::other(error.to_string()))?;
        {
            let mut bounded = Deadlined {
                socket: &mut socket,
                deadline,
            };
            while connection.is_handshaking() {
                connection.complete_io(&mut bounded)?;
            }
        }
        socket.set_read_timeout(Some(io))?;
        socket.set_write_timeout(Some(io))?;
        Ok(StreamOwned::new(connection, BoundedSocket(socket)))
    }
}

/// The socket under an open pod connection. Each socket read is bounded by the socket's read
/// timeout measured from its first attempt: an interrupted read (a signal, a stop and continue)
/// is retried with what is left of that timeout. `rustls` retries an `Interrupted` read itself
/// with no deadline, restarting the timeout each time, so none may reach it.
struct BoundedSocket(TcpStream);

impl Read for BoundedSocket {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut socket = &self.0;
        read_within_timeout(&self.0, || socket.read(buffer))
    }
}

impl Write for BoundedSocket {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// What is left of `deadline`; `TimedOut` once it has passed.
fn remaining(deadline: Instant) -> io::Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "opening the pod connection took longer than its connect bound",
        ))
    } else {
        Ok(left)
    }
}

/// The socket during the handshake: each read and write waits at most what is left of the
/// opening's deadline, so the handshake as a whole ends by it.
struct Deadlined<'a> {
    socket: &'a mut TcpStream,
    deadline: Instant,
}

impl Read for Deadlined<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.socket
            .set_read_timeout(Some(remaining(self.deadline)?))?;
        self.socket.read(buffer)
    }
}

impl Write for Deadlined<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.socket
            .set_write_timeout(Some(remaining(self.deadline)?))?;
        self.socket.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.socket.flush()
    }
}

/// `host` and port of `host[:port]`, the port `default` when none is written; an IPv6 literal is
/// written `[addr]:port`.
pub(crate) fn split(authority: &str, default: u16) -> Option<(&str, u16)> {
    if authority.is_empty() || authority.contains(['/', '@', ' ']) {
        return None;
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(port) => port.parse().ok()?,
            None if after.is_empty() => default,
            None => return None,
        };
        return Some((host, port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Some((host, port.parse().ok()?)),
        Some(_) => None,
        None => Some((authority, default)),
    }
}

/// Resolves `host` and connects to the first address that answers before `deadline`.
pub(crate) fn dial(host: &str, port: u16, deadline: Instant) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "the pod host has no address");
    for address in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&address, remaining(deadline)?) {
            Ok(socket) => return Ok(socket),
            Err(error) => last = error,
        }
    }
    Err(last)
}

impl PodConnector for ProxyConnector {
    fn connect(&self, authority: &str) -> io::Result<Box<dyn RelayStream>> {
        Ok(Box::new(self.open(
            authority,
            POD_CONNECT_TIMEOUT,
            POD_IO_TIMEOUT,
        )?))
    }

    fn connect_probe(
        &self,
        authority: &str,
        timeout: Duration,
    ) -> io::Result<Box<dyn RelayStream>> {
        Ok(Box::new(self.open(authority, timeout, timeout)?))
    }
}

#[cfg(test)]
mod tests {
    use super::split;

    #[test]
    fn an_authority_is_a_host_and_a_port_that_defaults_to_443() {
        assert_eq!(
            split("pod1-8000.proxy.runpod.net", 443),
            Some(("pod1-8000.proxy.runpod.net", 443))
        );
        assert_eq!(split("localhost:8443", 443), Some(("localhost", 8443)));
        assert_eq!(split("[::1]:8443", 443), Some(("::1", 8443)));
        assert_eq!(split("[::1]", 443), Some(("::1", 443)));
        for refused in ["", "a:b", "user@pod:443", "pod/v1", "::1", "pod:99999"] {
            assert_eq!(split(refused, 443), None, "{refused:?}");
        }
    }
}
