//! Adversary pass 2 on story:ingress-auth-optional (wave 2026-10-16-w01): the correction that lets
//! a repeated `authorization` through the head parser (`reads_no_credential` in `server.rs`) only
//! on the wire paths of a relaying gateway under `IngressAuth::None`. Each case is written so a
//! parser that let the repeat through anywhere else would authenticate the first owner token and
//! answer something other than `credential-malformed`. Every target is a loopback fixture pod.

use crate::upstream::{self, Upstream};
use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, IngressAuth, Label, NoOwnerVerifier, OwnerToken,
    OwnerVerifier, Relay, RelayModel, RelayTarget, RelayTargets, RouteInventory,
    SharedSecretVerifier, TargetRefusal, Wire,
};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

const OWNER: &str = "owner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WAIT: Duration = Duration::from_secs(10);
const BODY: &str = r#"{"model":"open","messages":[]}"#;

/// A loopback pod answering up to `count` calls of its test with `200 {}`, keeping every head it
/// read. It answers only its own test's calls (`crate::upstream`).
struct Pod {
    upstream: Upstream,
}

impl Pod {
    fn start(count: usize) -> Self {
        Self {
            upstream: Upstream::scripted(
                (0..count)
                    .map(|_| upstream::bytes(upstream::OK.to_vec()))
                    .collect(),
            ),
        }
    }

    fn heads(&self) -> Vec<String> {
        self.upstream
            .seen()
            .iter()
            .map(|raw| upstream::head_of(raw))
            .collect()
    }
}

struct Source(upstream::Target);

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(self.0.clone()))
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
        // Bound to every address, not loopback alone: a gateway that authenticates nobody and
        // listens on loopback only opens every path (story:local-open-mode), which
        // `adversary_local_open.rs` holds. This one is reached over loopback all the same.
        let mut config = GatewayConfig::new("0.0.0.0:0".parse().unwrap());
        config.read_timeout = WAIT;
        config.ingress_auth = ingress;
        let inventory = RouteInventory::new(Vec::new()).unwrap();
        let handle = match pod {
            Some(pod) => {
                let relay =
                    Relay::new(vec![model()], Arc::new(Source(pod.upstream.target(WAIT)))).unwrap();
                Gateway::bind_with_relay(config, verifier, inventory, relay).unwrap()
            }
            None => Gateway::bind(config, verifier, inventory).unwrap(),
        };
        handle.mark_ready();
        let addr = SocketAddr::from(([127, 0, 0, 1], handle.local_addr().port()));
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
