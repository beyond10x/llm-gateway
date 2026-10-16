//! Adversary pass on story:ingress-auth-optional (wave 2026-10-16-w01): `IngressAuth::None`
//! driven against the story's rule, `docs/gateway.md` ("the relay", step 1) and the concept page.
//! Every target is a loopback fixture pod; nothing leaves the machine.

use llm_gateway::{
    Disposition, Gateway, GatewayConfig, GatewayHandle, IngressAuth, Label, NoOwnerVerifier,
    OwnerToken, OwnerVerifier, Relay, RelayModel, RelayStream, RelayTarget, RelayTargets,
    RouteInventory, SharedSecretVerifier, TargetRefusal, UsageRecord, UsageRecords, Wire,
};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

const OWNER: &str = "owner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const GUESS: &str = "a-guess-0123456789abcdef0123456789abcdef";
const WAIT: Duration = Duration::from_secs(10);
const BODY: &str = r#"{"model":"open","messages":[]}"#;

/// A loopback pod answering `count` requests with `200 {}`, keeping every head it read.
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
        drop(TcpStream::connect(&self.authority));
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

#[derive(Default)]
struct Records(Mutex<Vec<UsageRecord>>);

impl UsageRecords for Records {
    fn record(&self, record: &UsageRecord) {
        self.0.lock().unwrap().push(record.clone());
    }
}

fn owner_verifier() -> Arc<dyn OwnerVerifier> {
    Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    )
}

fn model() -> RelayModel {
    RelayModel::new(
        Label::new("open").unwrap(),
        Label::new("served-open").unwrap(),
        vec![Wire::Chat, Wire::Responses, Wire::Messages],
    )
    .unwrap()
}

struct Open {
    handle: Option<GatewayHandle>,
    addr: SocketAddr,
}

impl Open {
    fn start(
        verifier: Arc<dyn OwnerVerifier>,
        pod: Option<&Pod>,
        records: Option<Arc<Records>>,
    ) -> Self {
        let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
        config.read_timeout = WAIT;
        config.ingress_auth = IngressAuth::None;
        let inventory = RouteInventory::new(Vec::new()).unwrap();
        let handle = match pod {
            Some(pod) => {
                let mut relay =
                    Relay::new(vec![model()], Arc::new(Source(pod.authority.clone()))).unwrap();
                if let Some(records) = records {
                    relay = relay.with_records(records);
                }
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
    fn ask(&self, method: &str, path: &str, headers: &[&str], body: &str) -> (u16, String) {
        let mut raw = format!("{method} {path} HTTP/1.1\r\nhost: gateway\r\n");
        for header in headers {
            raw.push_str(header);
            raw.push_str("\r\n");
        }
        if !body.is_empty() {
            raw.push_str("content-type: application/json\r\n");
            raw.push_str("content-length: ");
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

impl Drop for Open {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.shutdown();
        }
    }
}

/// The story's rule: under `none` "no `authorization` header is read, and one sent is ignored";
/// docs/gateway.md: "a missing, malformed or wrong credential is no refusal". A client behind a
/// mesh whose sidecar adds its own `authorization` beside the client's sends two.
#[test]
fn adv_none_a_repeated_authorization_on_a_wire_path_is_no_refusal() {
    let pod = Pod::start(1);
    let gateway = Open::start(Arc::new(NoOwnerVerifier), Some(&pod), None);
    let answered = gateway.ask(
        "POST",
        "/v1/chat/completions",
        &[
            "authorization: Bearer one-0123456789abcdef",
            "authorization: Bearer two-0123456789abcdef",
        ],
        BODY,
    );
    assert_eq!(answered, (200, String::new()), "pod saw {:?}", pod.heads());
    assert_eq!(pod.heads().len(), 1);
}

/// Acceptance 2, second half, which no test of the unit drives: with `none` and an owner secret
/// the owner-only surface is served to that secret only.
#[test]
fn adv_none_with_a_secret_serves_the_owner_surface_to_that_secret_only() {
    let gateway = Open::start(owner_verifier(), None, None);
    let owner = format!("authorization: Bearer {OWNER}");
    let guess = format!("authorization: Bearer {GUESS}");
    assert_eq!(gateway.ask("GET", "/metrics", &[&owner], "").0, 200);
    assert_eq!(gateway.ask("GET", "/v1/routes", &[&owner], "").0, 200);
    assert_eq!(
        gateway.ask("GET", "/metrics", &[], ""),
        (401, "credential-absent".to_owned())
    );
    assert_eq!(
        gateway.ask("GET", "/v1/routes", &[&guess], ""),
        (401, "credential-rejected".to_owned())
    );
}

/// The story's rule: only "a wire path of a gateway composed with a relay" opens. Without a
/// relay, and on any spelling that is not one of the three literal paths, nothing opens.
#[test]
fn adv_none_opens_only_the_three_literal_wire_paths_of_a_relaying_gateway() {
    let bare = Open::start(Arc::new(NoOwnerVerifier), None, None);
    for path in ["/v1/chat/completions", "/v1/responses", "/v1/messages"] {
        assert_eq!(
            bare.ask("POST", path, &[], BODY),
            (401, "credential-absent".to_owned()),
            "no relay: {path}"
        );
    }
    let pod = Pod::start(0);
    let open = Open::start(Arc::new(NoOwnerVerifier), Some(&pod), None);
    for path in [
        "/V1/chat/completions",
        "/v1/chat/completions/",
        "//v1/chat/completions",
        "/v1/chat/completions%00",
        "/v1/Messages",
        "/v1/responses/x",
        "/v1/routes/open",
        "/v1/unknown",
    ] {
        assert_eq!(
            open.ask("POST", path, &[], BODY),
            (401, "credential-absent".to_owned()),
            "{path}"
        );
        assert_eq!(
            open.ask("GET", path, &[], ""),
            (401, "credential-absent".to_owned()),
            "GET {path}"
        );
    }
    assert!(pod.heads().is_empty());
}

/// Under `none` with no owner, the owner-only surface answers exactly as a gateway with a
/// secret answers a guess: whether an owner exists cannot be told from outside.
#[test]
fn adv_none_without_an_owner_is_indistinguishable_from_one_with_a_secret() {
    let none = Open::start(Arc::new(NoOwnerVerifier), None, None);
    let secret = Open::start(owner_verifier(), None, None);
    let guess = format!("authorization: Bearer {GUESS}");
    for (method, path) in [
        ("GET", "/metrics"),
        ("HEAD", "/metrics"),
        ("GET", "/v1/routes"),
        ("GET", "/v1/routes/open"),
        ("POST", "/v1/unknown"),
        ("DELETE", "/metrics"),
    ] {
        for headers in [
            vec![],
            vec![guess.as_str()],
            vec!["authorization: Basic eDp5"],
            vec!["authorization: Bearer"],
            vec!["authorization: bearer short"],
        ] {
            assert_eq!(
                none.ask(method, path, &headers, ""),
                secret.ask(method, path, &headers, ""),
                "{method} {path} {headers:?}"
            );
        }
    }
}

/// An unauthenticated model call is still recorded, and no client `authorization` (the owner's
/// own included) reaches the target.
#[test]
fn adv_none_records_an_unauthenticated_call_and_forwards_no_client_credential() {
    let pod = Pod::start(3);
    let records = Arc::new(Records::default());
    let gateway = Open::start(owner_verifier(), Some(&pod), Some(Arc::clone(&records)));
    let owner = format!("authorization: Bearer {OWNER}");
    for (path, headers) in [
        ("/v1/chat/completions", vec![]),
        ("/v1/responses?api-version=1", vec![owner.as_str()]),
        (
            "/v1/messages",
            vec!["authorization: Basic eDp5", "x-api-key: client-key"],
        ),
    ] {
        assert_eq!(
            gateway.ask("POST", path, &headers, BODY),
            (200, String::new()),
            "{path}"
        );
    }
    assert_eq!(
        gateway.ask("GET", "/v1/messages", &[], ""),
        (405, "method-not-allowed".to_owned())
    );
    let heads = pod.heads();
    assert_eq!(heads.len(), 3);
    for head in &heads {
        let lower = head.to_ascii_lowercase();
        assert!(!lower.contains("authorization"), "{head}");
        assert!(!lower.contains("x-api-key"), "{head}");
        assert!(!head.contains(OWNER), "{head}");
    }
    let records = records.0.lock().unwrap().clone();
    let summary: Vec<_> = records
        .iter()
        .map(|record| {
            (
                record.model.as_deref(),
                record.wire,
                record.disposition,
                record.status,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (Some("open"), Wire::Chat, Disposition::Relayed, 200),
            (Some("open"), Wire::Responses, Disposition::Relayed, 200),
            (Some("open"), Wire::Messages, Disposition::Relayed, 200),
            (None, Wire::Messages, Disposition::Refused, 405),
        ]
    );
}

/// The concept page's numbered order of decision: the unit inserted its new section inside the
/// list, so step 6 now renders under "Model calls without a credential", cut off from 1 to 5.
#[test]
fn adv_the_concept_pages_decision_order_is_one_unbroken_list() {
    let page = include_str!("../../../website/docs/concepts/single-owner-gateway.md");
    let start = page.find("## The order a request is decided in").unwrap();
    let rest = &page[start + 3..];
    let section = &rest[..rest.find("\n## ").unwrap_or(rest.len())];
    for step in 1..=6 {
        assert!(
            section.contains(&format!("\n{step}. **")),
            "step {step} is not in the section:\n{section}"
        );
    }
}
