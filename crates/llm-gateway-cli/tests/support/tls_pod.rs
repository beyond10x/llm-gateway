//! A loopback `https` fixture pod (story:pod-proxy-tls): a TLS server on 127.0.0.1 whose
//! certificate a test root issues for a pod's proxy host name. It answers `GET /v1/models` with a
//! listing that names the model `small`, and any other request with [`POD_CHAT_ANSWER`], and
//! records every connection: the host name the client asked for, whether the handshake
//! completed, and every request byte it decrypted. Test roots are made here, at run time, by
//! `rcgen`; nothing is read from the host's certificate store.

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

/// The proxy host llm's Runpod description gives the fixture pod `podsmall1`
/// (`running_small_pod`): `https://podsmall1-8000.proxy.runpod.net/v1/`.
pub const POD_HOST: &str = "podsmall1-8000.proxy.runpod.net";

/// What the fixture pod answers a relayed request with.
pub const POD_CHAT_ANSWER: &str = r#"{"id":"chat-1","object":"chat.completion","model":"small","choices":[{"index":0,"message":{"role":"assistant","content":"over tls"},"finish_reason":"stop"}]}"#;

/// What the fixture pod answers the readiness probe with.
const MODELS_ANSWER: &str = r#"{"object":"list","data":[{"id":"small","object":"model"}]}"#;

const WAIT: Duration = Duration::from_secs(10);

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A certificate authority made for one test.
pub struct TestRoot {
    /// The root certificate, DER.
    pub der: Vec<u8>,
    issuer: Issuer<'static, KeyPair>,
}

impl TestRoot {
    pub fn new(name: &str) -> Self {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.distinguished_name.push(DnType::CommonName, name);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let key = KeyPair::generate().unwrap();
        let certificate = params.self_signed(&key).unwrap();
        Self {
            der: certificate.der().to_vec(),
            issuer: Issuer::new(params, key),
        }
    }

    /// A server configuration presenting a certificate for `host` that this root issued.
    pub fn server(&self, host: &str) -> Arc<ServerConfig> {
        let mut params = CertificateParams::new(vec![host.to_owned()]).unwrap();
        params.distinguished_name.push(DnType::CommonName, host);
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let key = KeyPair::generate().unwrap();
        let certificate = params.signed_by(&key, &self.issuer).unwrap();
        let config = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![certificate.der().clone()],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            )
            .unwrap();
        Arc::new(config)
    }
}

/// One connection the fixture pod accepted.
#[derive(Debug, Clone)]
pub struct Seen {
    /// The host name the client sent (SNI), once the handshake read it.
    pub server_name: Option<String>,
    /// Whether the TLS handshake completed.
    pub handshake: bool,
    /// Whether the connection was served with the second, untrusted certificate.
    pub switched: bool,
    /// Every request byte the fixture decrypted.
    pub request: Vec<u8>,
}

impl Seen {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.request).into_owned()
    }
}

/// When the fixture pod presents a second certificate instead of its first, from the
/// connection after the readiness probe it answered once `armed` returns `true`.
pub struct Switch {
    pub to: Arc<ServerConfig>,
    pub armed: Box<dyn Fn() -> bool + Send + Sync>,
}

/// A running loopback `https` fixture pod.
pub struct TlsPod {
    pub address: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl TlsPod {
    /// Serves every connection with `config`, or with `switch.to` once it has switched.
    pub fn start(config: Arc<ServerConfig>, switch: Option<Switch>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&seen);
        let switch = switch.map(Arc::new);
        let switched = Arc::new(AtomicBool::new(false));
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(connection) = connection else {
                    return;
                };
                let now_switched = switched.load(Ordering::SeqCst);
                let config = match (&switch, now_switched) {
                    (Some(switch), true) => Arc::clone(&switch.to),
                    _ => Arc::clone(&config),
                };
                let recording = Arc::clone(&recording);
                let switch = switch.clone();
                let switched = Arc::clone(&switched);
                thread::spawn(move || {
                    let seen = serve(connection, config, now_switched, |probe| {
                        if probe && switch.as_ref().is_some_and(|switch| (switch.armed)()) {
                            switched.store(true, Ordering::SeqCst);
                        }
                    });
                    recording.lock().unwrap().push(seen);
                });
            }
        });
        Self { address, seen }
    }

    /// Every connection served so far.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Reads one request's head and `content-length` body, keeping every byte read.
fn read_request(stream: &mut impl Read, raw: &mut Vec<u8>) {
    let mut chunk = [0_u8; 4096];
    loop {
        if let Some(end) = find(raw, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
            let length: usize = head
                .split("\r\n")
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
            if raw.len() >= end + 4 + length {
                return;
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => raw.extend_from_slice(&chunk[..read]),
        }
    }
}

/// One connection: the handshake, one request, one answer, and a clean TLS close.
/// `answered(probe)` is called once the answer is written.
fn serve(
    tcp: std::net::TcpStream,
    config: Arc<ServerConfig>,
    switched: bool,
    answered: impl FnOnce(bool),
) -> Seen {
    tcp.set_read_timeout(Some(WAIT)).unwrap();
    tcp.set_write_timeout(Some(WAIT)).unwrap();
    let connection = ServerConnection::new(config).unwrap();
    let mut stream = StreamOwned::new(connection, tcp);
    let mut handshake = true;
    while stream.conn.is_handshaking() {
        if stream.conn.complete_io(&mut stream.sock).is_err() {
            handshake = false;
            break;
        }
    }
    let server_name = stream.conn.server_name().map(str::to_owned);
    let mut request = Vec::new();
    if !handshake {
        // Whatever the client sent after a failed handshake, decrypted: nothing is expected.
        drop(stream.conn.reader().read_to_end(&mut request));
        return Seen {
            server_name,
            handshake,
            switched,
            request,
        };
    }
    read_request(&mut stream, &mut request);
    let probe = request.starts_with(b"GET /v1/models ");
    let body = if probe {
        MODELS_ANSWER
    } else {
        POD_CHAT_ANSWER
    };
    let answer = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    if stream.write_all(answer.as_bytes()).is_ok() {
        stream.conn.send_close_notify();
        drop(stream.flush());
        answered(probe);
    }
    Seen {
        server_name,
        handshake,
        switched,
        request,
    }
}
