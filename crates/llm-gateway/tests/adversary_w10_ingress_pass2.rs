//! Adversary pass 2 on story:ingress-auth-optional (wave 2026-10-16-w01): the correction that lets
//! a repeated `authorization` through the head parser (`reads_no_credential` in `server.rs`) only
//! on the wire paths of a relaying gateway under `IngressAuth::None`. Each case is written so a
//! parser that let the repeat through anywhere else would authenticate the first owner token and
//! answer something other than `credential-malformed`. Every target is a loopback fixture pod.

use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, IngressAuth, Label, NoOwnerVerifier, OwnerToken,
    OwnerVerifier, Relay, RelayModel, RelayStream, RelayTarget, RelayTargets, RouteInventory,
    SharedSecretVerifier, TargetRefusal, Wire,
};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

const OWNER: &str = "owner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WAIT: Duration = Duration::from_secs(10);
const BODY: &str = r#"{"model":"open","messages":[]}"#;

/// A loopback pod answering up to `count` requests with `200 {}`, keeping every head it read.
struct Pod {
    authority: String,
    heads: Arc<Mutex<Vec<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl Pod {
    fn start(count: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let heads = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&heads);
        let thread = thread::spawn(move || {
            for _ in 0..count {
                let Ok((mut connection, _)) = listener.accept() else {
                    return;
                };
                connection.set_read_timeout(Some(WAIT)).unwrap();
                let mut buffer = Vec::new();
                let mut chunk = [0_u8; 8192];
                let end = loop {
                    if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                        break end;
                    }
                    match connection.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                    }
                };
                let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
                let wanted: usize = head
                    .split("\r\n")
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().ok())?
                    })
                    .unwrap_or(0);
                let mut body = buffer.len() - (end + 4);
                while body < wanted {
                    match connection.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => body += read,
                    }
                }
                seen.lock().unwrap().push(head);
                drop(connection.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
                ));
            }
        });
        Self {
            authority,
            heads,
            thread: Some(thread),
        }
    }

    fn heads(&self) -> Vec<String> {
        self.heads.lock().unwrap().clone()
    }
}

impl Drop for Pod {
    fn drop(&mut self) {
        // Wakes the accept loop if it is still waiting, so the thread can end.
        for _ in 0..64 {
            if TcpStream::connect(&self.authority).is_err() {
                break;
            }
            if self.thread.as_ref().is_some_and(JoinHandle::is_finished) {
                break;
            }
        }
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

struct Source(String);

struct Target(String);

impl RelayTarget for Target {
    fn authority(&self) -> &str {
        &self.0
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        let stream = TcpStream::connect(&self.0)?;
        stream.set_read_timeout(Some(WAIT))?;
        stream.set_write_timeout(Some(WAIT))?;
        Ok(Box::new(stream))
    }
}

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(Target(self.0.clone())))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

fn secret() -> Arc<dyn OwnerVerifier> {
    Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    )
}

fn no_owner() -> Arc<dyn OwnerVerifier> {
    Arc::new(NoOwnerVerifier)
}

fn model() -> RelayModel {
    RelayModel::new(
        Label::new("open").unwrap(),
        Label::new("served-open").unwrap(),
        vec![Wire::Chat, Wire::Responses, Wire::Messages],
    )
    .unwrap()
}

struct Running {
    handle: Option<GatewayHandle>,
    addr: SocketAddr,
}

impl Running {
    fn start(ingress: IngressAuth, verifier: Arc<dyn OwnerVerifier>, pod: Option<&Pod>) -> Self {
        let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
        config.read_timeout = WAIT;
        config.ingress_auth = ingress;
        let inventory = RouteInventory::new(Vec::new()).unwrap();
        let handle = match pod {
            Some(pod) => {
                let relay =
                    Relay::new(vec![model()], Arc::new(Source(pod.authority.clone()))).unwrap();
                Gateway::bind_with_relay(config, verifier, inventory, relay).unwrap()
            }
            None => Gateway::bind(config, verifier, inventory).unwrap(),
        };
        handle.mark_ready();
        let addr = handle.local_addr();
        Self {
            handle: Some(handle),
            addr,
        }
    }

    /// `(status, code)` of one request with `headers` (each `name: value`) and `body`.
    fn ask(&self, method: &str, target: &str, headers: &[&str], body: &str) -> (u16, String) {
        let mut raw = String::new();
        raw.push_str(method);
        raw.push(' ');
        raw.push_str(target);
        raw.push_str(" HTTP/1.1\r\nhost: gateway\r\n");
        for header in headers {
            raw.push_str(header);
            raw.push_str("\r\n");
        }
        if !body.is_empty() {
            raw.push_str("content-type: application/json\r\ncontent-length: ");
            raw.push_str(&body.len().to_string());
            raw.push_str("\r\n");
        }
        raw.push_str("\r\n");
        raw.push_str(body);
        let mut stream = TcpStream::connect(self.addr).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        drop(stream.write_all(raw.as_bytes()));
        let mut answer = Vec::new();
        drop(stream.read_to_end(&mut answer));
        let text = String::from_utf8_lossy(&answer).into_owned();
        let status = text
            .split(' ')
            .nth(1)
            .and_then(|status| status.parse().ok())
            .unwrap_or(0);
        let code = text
            .split("\"code\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or("")
            .to_owned();
        (status, code)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.shutdown();
        }
    }
}

fn owner_twice(first: &'static str, second: &'static str) -> [String; 2] {
    [
        format!("{first}: Bearer {OWNER}"),
        format!("{second}: Bearer {OWNER}"),
    ]
}

fn malformed() -> (u16, String) {
    (401, "credential-malformed".to_owned())
}

/// The fix commit: "Every other path still refuses one credential-malformed"; docs/gateway.md:
/// under `none`, "every other path still authenticates". Two copies of the owner's own token
/// make the probe sharp: a parser that let the repeat through off the wire paths would
/// authenticate the first copy and serve the owner surface.
#[test]
fn adv2_none_a_repeated_owner_token_off_the_wire_paths_stays_credential_malformed() {
    let pod = Pod::start(0);
    for verifier in [secret(), no_owner()] {
        let gateway = Running::start(IngressAuth::None, verifier, Some(&pod));
        for (first, second) in [
            ("authorization", "authorization"),
            ("Authorization", "AUTHORIZATION"),
        ] {
            let headers = owner_twice(first, second);
            let headers: Vec<&str> = headers.iter().map(String::as_str).collect();
            for (method, target, body) in [
                ("GET", "/metrics", ""),
                ("GET", "/v1/routes", ""),
                ("GET", "/v1/routes/open", ""),
                ("POST", "/v1/unknown", BODY),
                ("GET", "/v1/unknown", ""),
                ("POST", "/v1/chat/completions/", BODY),
                ("POST", "/V1/chat/completions", BODY),
                ("POST", "/v1/messages/x", BODY),
                ("POST", "/v1/models", BODY),
                ("POST", "/", BODY),
                ("POST", "/metrics?x=/v1/messages", ""),
            ] {
                assert_eq!(
                    gateway.ask(method, target, &headers, body),
                    malformed(),
                    "{method} {target} with {first} and {second}"
                );
            }
        }
    }
    assert!(pod.heads().is_empty(), "{:?}", pod.heads());
}

/// `bearer` is unchanged by the correction: a repeated owner token to a wire path is still
/// ambiguous and still refused, and nothing reaches the pod.
#[test]
fn adv2_bearer_a_repeated_owner_token_on_a_wire_path_stays_credential_malformed() {
    let pod = Pod::start(3);
    let gateway = Running::start(IngressAuth::Bearer, secret(), Some(&pod));
    let headers = owner_twice("authorization", "Authorization");
    let headers: Vec<&str> = headers.iter().map(String::as_str).collect();
    for target in ["/v1/chat/completions", "/v1/responses", "/v1/messages?x=1"] {
        assert_eq!(
            gateway.ask("POST", target, &headers, BODY),
            malformed(),
            "{target}"
        );
    }
    drop(gateway);
    assert!(pod.heads().is_empty(), "{:?}", pod.heads());
}

/// docs/gateway.md: only "a wire path of a gateway composed with a relay" skips authentication.
/// A gateway bound without a relay under `none` keeps refusing a repeated credential there.
#[test]
fn adv2_none_without_a_relay_a_repeated_owner_token_on_a_wire_path_stays_credential_malformed() {
    for verifier in [secret(), no_owner()] {
        let gateway = Running::start(IngressAuth::None, verifier, None);
        let headers = owner_twice("authorization", "authorization");
        let headers: Vec<&str> = headers.iter().map(String::as_str).collect();
        for (method, target) in [
            ("POST", "/v1/chat/completions"),
            ("GET", "/v1/responses"),
            ("POST", "/v1/messages"),
        ] {
            assert_eq!(
                gateway.ask(method, target, &headers, BODY),
                malformed(),
                "{method} {target}"
            );
        }
    }
}

/// Under `none` a wire path of a relaying gateway takes any number of `authorization` lines, in
/// any case, well-formed or not, and none of them reaches the pod. The query string is not part
/// of the path the parser asks about.
#[test]
fn adv2_none_repeated_case_variant_and_malformed_authorizations_on_a_wire_path_are_ignored() {
    let pod = Pod::start(6);
    for verifier in [secret(), no_owner()] {
        let gateway = Running::start(IngressAuth::None, verifier, Some(&pod));
        let headers = [
            "Authorization: Bearer one-0123456789abcdef0123456789abcdef",
            "AUTHORIZATION: not-a-credential",
            "aUtHoRiZaTiOn:",
            "authorization: Basic eDp5",
        ];
        for target in [
            "/v1/chat/completions",
            "/v1/responses?api-version=1",
            "/v1/messages?",
        ] {
            assert_eq!(
                gateway.ask("POST", target, &headers, BODY),
                (200, String::new()),
                "{target}"
            );
        }
        for method in ["GET", "HEAD", "PUT"] {
            assert_eq!(
                gateway.ask(method, "/v1/chat/completions", &headers, ""),
                (405, "method-not-allowed".to_owned()).then_head(method),
                "{method}"
            );
        }
    }
    let heads = pod.heads();
    assert_eq!(heads.len(), 6, "{heads:?}");
    for head in heads {
        let lower = head.to_ascii_lowercase();
        assert!(
            !lower.contains("authorization")
                && !lower.contains("not-a-credential")
                && !lower.contains("edp5")
                && !lower.contains("one-0123"),
            "{head}"
        );
    }
}

/// Only `authorization` may repeat on an open wire path: a repeated framing header beside it is
/// still `request-malformed`, whichever comes first.
#[test]
fn adv2_none_a_repeated_authorization_does_not_carry_another_repeated_header_through() {
    let pod = Pod::start(0);
    let gateway = Running::start(IngressAuth::None, secret(), Some(&pod));
    for headers in [
        [
            "authorization: Bearer a",
            "authorization: Bearer b",
            "x-note: one",
            "x-note: two",
        ],
        [
            "x-note: one",
            "authorization: Bearer a",
            "x-note: two",
            "authorization: Bearer b",
        ],
        [
            "authorization: Bearer a",
            "content-type: text/plain",
            "authorization: Bearer b",
            "content-length: 1",
        ],
    ] {
        assert_eq!(
            gateway.ask("POST", "/v1/chat/completions", &headers, BODY),
            (400, "request-malformed".to_owned()),
            "{headers:?}"
        );
    }
    assert!(pod.heads().is_empty(), "{:?}", pod.heads());
}

/// A `HEAD` answer carries no body, so its code is not readable from the answer.
trait ThenHead {
    fn then_head(self, method: &str) -> Self;
}

impl ThenHead for (u16, String) {
    fn then_head(self, method: &str) -> Self {
        if method == "HEAD" {
            (self.0, String::new())
        } else {
            self
        }
    }
}
