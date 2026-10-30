//! story:byte-relay-client-gone, the pod's TLS connection: a client that leaves while the
//! gateway waits for the answer head of a pod reached over TLS (`ProxyConnector`, as a Runpod
//! pod is) shuts that connection down and releases the target, so a pod is not held generating
//! an answer nobody reads until its read timeout. Everything is loopback; nothing leaves the
//! machine.

mod support;

use llm_gateway::{
    Gateway, GatewayConfig, Label, OwnerToken, Relay, RelayModel, RelayStream, RelayTarget,
    RelayTargets, RouteInventory, SharedSecretVerifier, TargetRefusal, Wire,
};
use llm_gateway_cli::{PodConnector, ProxyConnector};
use rustls::{ServerConnection, StreamOwned};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use support::tls_pod::{POD_HOST, TestRoot};

const OWNER: &str = "owner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WAIT: Duration = Duration::from_secs(10);
/// Well inside the pod connection's 120 s read timeout, well past the gateway's 100 ms watch.
const BOUND: Duration = Duration::from_secs(3);

/// A TLS pod that reads one request and never answers: it reports how long after the request
/// arrived its connection ended, by the gateway's hand.
fn silent_tls_pod(root: &TestRoot) -> (std::net::SocketAddr, mpsc::Receiver<Duration>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let config = root.server(POD_HOST);
    let (ended, ends) = mpsc::channel();
    thread::spawn(move || {
        let Ok((tcp, _)) = listener.accept() else {
            return;
        };
        tcp.set_read_timeout(Some(WAIT * 3)).unwrap();
        let mut stream = StreamOwned::new(ServerConnection::new(config).unwrap(), tcp);
        let mut chunk = [0_u8; 4096];
        let mut request = Vec::new();
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(read) => request.extend_from_slice(&chunk[..read]),
            }
        }
        let arrived = Instant::now();
        // The body is short and follows the head; then nothing is answered.
        loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _sent = ended.send(arrived.elapsed());
    });
    (address, ends)
}

struct Pods {
    connector: ProxyConnector,
    released: Arc<AtomicUsize>,
}

impl RelayTargets for Pods {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(Pod {
            connector: self.connector.clone(),
            released: Arc::clone(&self.released),
        }))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

struct Pod {
    connector: ProxyConnector,
    released: Arc<AtomicUsize>,
}

impl RelayTarget for Pod {
    fn authority(&self) -> &str {
        POD_HOST
    }

    fn connect(&self) -> std::io::Result<Box<dyn RelayStream>> {
        self.connector.connect(POD_HOST)
    }
}

impl Drop for Pod {
    fn drop(&mut self) {
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn tls_a_client_that_left_shuts_the_pod_connection_down_and_releases_the_target() {
    let root = TestRoot::new("client gone test root");
    let (address, ends) = silent_tls_pod(&root);
    let connector =
        ProxyConnector::with_test_seam(std::slice::from_ref(&root.der), Some(address)).unwrap();
    let released = Arc::new(AtomicUsize::new(0));
    let targets: Arc<dyn RelayTargets> = Arc::new(Pods {
        connector,
        released: Arc::clone(&released),
    });
    let model = RelayModel::new(
        Label::new("code").unwrap(),
        Label::new("served-code").unwrap(),
        vec![Wire::Chat],
    )
    .unwrap();
    let relay = Relay::new(vec![model], targets).unwrap();
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    config.read_timeout = WAIT;
    let handle = Gateway::bind_with_relay(
        config,
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();

    let body = br#"{"model":"code","stream":false,"messages":[]}"#;
    let mut client = std::net::TcpStream::connect(handle.local_addr()).unwrap();
    let mut request = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(body);
    client.write_all(&request).unwrap();
    thread::sleep(Duration::from_millis(500));
    drop(client);

    let deadline = Instant::now() + BOUND;
    while Instant::now() < deadline && released.load(Ordering::SeqCst) == 0 {
        thread::sleep(Duration::from_millis(10));
    }
    let held_for = ends.recv_timeout(BOUND);
    let released = released.load(Ordering::SeqCst);
    handle.shutdown();
    assert_eq!(
        released, 1,
        "the TLS target was still held {BOUND:?} after the client left"
    );
    let held_for = held_for.unwrap_or_else(|_| {
        panic!("the pod's TLS connection was still open {BOUND:?} after the client left")
    });
    assert!(
        held_for < BOUND + Duration::from_millis(500),
        "the pod's connection ended {held_for:?} after the request"
    );
}
