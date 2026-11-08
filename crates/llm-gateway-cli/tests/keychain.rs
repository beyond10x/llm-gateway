//! story:keychain-references: a credential named by a `keychain:<service>/<account>` reference,
//! read once at start from the OS keychain.
//!
//! No real keychain is read: the test binary installs a fixed in-memory keychain with
//! `install_secret_source` before its first lookup, and every endpoint is a loopback fixture.
//! Nothing leaves the loopback interface, no Runpod API is called and no credential is real.

use llm_gateway_cli::{
    EntryValue, KeychainRef, LookupFailure, OsKeychain, Routes, SecretSource,
    install_secret_source, parse, start, vllm_keys,
};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex, Once},
    thread,
    time::Duration,
};

/// The value the fixed keychain holds for `keychain:test-gateway/pod:fixture`.
const POD_KEY: &str = "pod-key-from-the-test-keychain-not-a-secret";
/// The value it holds for `keychain:test-gateway/vllm`.
const VLLM_KEY: &str = "vllm-key-from-the-test-keychain-not-a-secret";
const WAIT: Duration = Duration::from_secs(10);

/// A keychain of fixed entries. `keychain:test-gateway/sealed` is unavailable, every other
/// entry it does not name is missing.
struct FixedKeychain;

impl SecretSource for FixedKeychain {
    fn lookup(&self, reference: &KeychainRef) -> Result<EntryValue, LookupFailure> {
        let value = match (reference.service(), reference.account()) {
            ("test-gateway", "pod:fixture") => format!("{POD_KEY}\n"),
            ("test-gateway", "vllm") => VLLM_KEY.to_owned(),
            ("test-gateway", "spaced") => "two words\n".to_owned(),
            ("test-gateway", "sealed") => return Err(LookupFailure::Unavailable),
            _ => return Err(LookupFailure::Missing),
        };
        Ok(EntryValue::new(value.into_bytes()))
    }
}

fn keychain() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| assert!(install_secret_source(Arc::new(FixedKeychain))));
}

/// A loopback endpoint answering every request with one chat completion, recording each head.
struct Endpoint {
    address: SocketAddr,
    heads: Arc<Mutex<Vec<String>>>,
}

impl Endpoint {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let heads = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&heads);
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    return;
                };
                connection.set_read_timeout(Some(WAIT)).unwrap();
                let mut raw = Vec::new();
                let mut chunk = [0_u8; 8192];
                let (end, length) = loop {
                    let Ok(read) = connection.read(&mut chunk) else {
                        break (None, 0);
                    };
                    if read == 0 {
                        break (None, 0);
                    }
                    raw.extend_from_slice(&chunk[..read]);
                    if let Some(end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
                        let length: usize = head
                            .split("\r\n")
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .and_then(|value| value.trim().parse().ok())
                            .unwrap_or(0);
                        break (Some(end), length);
                    }
                };
                let Some(end) = end else { continue };
                while raw.len() < end + 4 + length {
                    let Ok(read) = connection.read(&mut chunk) else {
                        break;
                    };
                    if read == 0 {
                        break;
                    }
                    raw.extend_from_slice(&chunk[..read]);
                }
                recording
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&raw[..end]).into_owned());
                let answer = r#"{"id":"hosted-1","object":"chat.completion"}"#;
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    answer.len()
                );
                drop(connection.write_all(head.as_bytes()));
                drop(connection.write_all(answer.as_bytes()));
            }
        });
        Self { address, heads }
    }

    fn heads(&self) -> Vec<String> {
        self.heads.lock().unwrap().clone()
    }
}

/// A loopback gateway without ingress authentication serving `pod` from `endpoint` with the
/// credential the keychain entry `account` holds.
fn document(endpoint: SocketAddr, account: &str) -> String {
    format!(
        "listen = \"127.0.0.1:0\"\ningress_auth = \"none\"\n\n[hosted.pod]\nwires = [\"chat\"]\n\
         base_url = \"http://{endpoint}/v1\"\nauth_kind = \"bearer\"\n\
         credential = \"keychain:test-gateway/{account}\"\nbilling_kind = \"self-hosted\"\n"
    )
}

fn chat(gateway: SocketAddr) -> String {
    let body = r#"{"model":"pod","messages":[{"role":"user","content":"hi"}]}"#;
    let mut stream = TcpStream::connect(gateway).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "POST /v1/chat/completions HTTP/1.1\r\nhost: gateway\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = Vec::new();
    drop(stream.read_to_end(&mut answer));
    String::from_utf8_lossy(&answer).into_owned()
}

/// What this proves: a hosted endpoint whose credential is a keychain reference is sent the
/// entry's value, its trailing newline trimmed, as its bearer; the reference is read at start,
/// once, and the value appears nowhere but in the endpoint's `authorization` header.
#[test]
fn keychain_a_hosted_endpoint_is_sent_the_entrys_value_as_its_bearer() {
    keychain();
    let endpoint = Endpoint::start();
    let deployment = parse(&document(endpoint.address, "pod:fixture")).unwrap();
    assert!(
        !format!("{deployment:?}").contains(POD_KEY),
        "the loaded document holds the reference, never the value"
    );
    let gateway = start(&deployment).unwrap();
    let answer = chat(gateway.local_addr());
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(!answer.contains(POD_KEY), "{answer}");
    gateway.shutdown();
    let heads = endpoint.heads();
    assert_eq!(heads.len(), 1, "{heads:?}");
    assert!(
        heads[0]
            .split("\r\n")
            .any(|line| line.eq_ignore_ascii_case(&format!("authorization: Bearer {POD_KEY}"))),
        "{}",
        heads[0]
    );
}

/// What this proves: an entry the keychain does not hold, a keychain it cannot read and a value
/// that is not one token each refuse the start with the endpoint credential's code, naming the
/// document key and quoting neither the reference nor the value.
#[test]
fn keychain_an_unreadable_entry_refuses_the_start_and_quotes_nothing() {
    keychain();
    let endpoint = Endpoint::start();
    for (account, code) in [
        ("absent", "endpoint-credential:unreadable"),
        ("sealed", "endpoint-credential:unreadable"),
        ("spaced", "endpoint-credential:not-a-token"),
    ] {
        let deployment = parse(&document(endpoint.address, account)).unwrap();
        let Err(refusal) = start(&deployment) else {
            panic!("{account}: the start was not refused");
        };
        assert_eq!(refusal.code(), code, "{account}");
        let text = refusal.to_string();
        assert!(text.contains("hosted.pod.credential:"), "{account}: {text}");
        for leak in ["test-gateway", account, "two words"] {
            assert!(!text.contains(leak), "{account}: {text}");
        }
    }
    assert!(endpoint.heads().is_empty());
}

/// What this proves: a Runpod model's `vllm_api_key` reference yields its vLLM key, held
/// redacted.
#[test]
fn keychain_a_models_vllm_key_is_read_from_its_entry() {
    keychain();
    let deployment = parse(
        "listen = \"127.0.0.1:0\"\ningress_auth = \"none\"\n\n[providers.runpod]\n\
         kind = \"runpod-vllm\"\n\n[models.small]\nprovider = \"runpod\"\nwires = [\"chat\"]\n\
         context_window = 65536\nhf_model = \"example/small-model\"\n\
         image = \"vllm/vllm-openai:v0.27.1\"\ngpu_types = [\"NVIDIA L40S\"]\n\
         max_model_len = 1024\nvllm_api_key = \"keychain:test-gateway/vllm\"\n",
    )
    .unwrap();
    let keys = vllm_keys(&deployment).unwrap();
    assert_eq!(keys.get("small").unwrap().expose(), VLLM_KEY.as_bytes());
    assert!(!format!("{keys:?}").contains(VLLM_KEY));
}

/// What this proves: a routes document endpoint's `keychain:` secret is admitted and read from
/// the keychain; an `env:` secret stays refused.
#[test]
fn keychain_a_routes_endpoint_reads_its_keychain_secret() {
    keychain();
    let routes = |secret: &str| {
        format!(
            "format: llm.routes/1\nendpoints:\n  pod:\n    base_url: https://pod.example.test/v1\n    \
             protocol: chat-completions\n    auth: {{kind: bearer, secret: \"{secret}\"}}\n\
             routes:\n  small: pod/served\n"
        )
    };
    let parsed = Routes::parse(&routes("keychain:test-gateway/pod:fixture")).unwrap();
    let credentials = parsed.read_credentials(None).unwrap();
    assert_eq!(credentials["pod"].expose(), POD_KEY.as_bytes());
    let refused = Routes::parse(&routes("env:POD_KEY")).unwrap_err();
    assert_eq!(refused.code(), "config:value");
}

/// What this proves: each keychain key of the document admits a well-formed reference and
/// refuses a malformed one, a credential named twice, a keychain key beside a connectors
/// connection and one on a Bedrock model, each as `config:value` or `config:schema`, quoting
/// nothing written.
#[test]
fn keychain_the_documents_keychain_keys_and_their_rules() {
    let provider = "listen = \"127.0.0.1:0\"\ningress_auth = \"none\"\n\n[providers.runpod]\n\
                    kind = \"runpod-vllm\"\napi_key = \"keychain:test-gateway/runpod\"\n\n\
                    [hosted.pod]\nwires = [\"chat\"]\nbase_url = \"https://pod.example.test/v1\"\n\
                    auth_kind = \"bearer\"\ncredential = \"keychain:test-gateway/pod:fixture\"\n";
    let deployment = parse(provider).unwrap();
    assert_eq!(
        deployment.providers["runpod"]
            .api_key
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("keychain:test-gateway/runpod")
    );
    assert_eq!(
        deployment.hosted["pod"]
            .credential_keychain
            .as_ref()
            .map(KeychainRef::account),
        Some("pod:fixture")
    );
    let connectors = "\n[providers.runpod.connectors]\nexecutable = \"/usr/bin/connectors\"\n\
                      adapter = \"runpod\"\nconnection = \"main\"\nwork_directory = \"/var/empty\"\n";
    for (name, text, code, key) in [
        (
            "malformed api_key",
            provider.replace("keychain:test-gateway/runpod", "keychain:not a ref"),
            "config:value",
            "providers.runpod.api_key",
        ),
        (
            "file api_key",
            provider.replace("keychain:test-gateway/runpod", "file:/run/key"),
            "config:value",
            "providers.runpod.api_key",
        ),
        (
            "api_key beside connectors",
            format!("{provider}{connectors}"),
            "config:value",
            "providers.runpod.api_key",
        ),
        (
            "credential twice",
            provider.replace(
                "credential = ",
                "credential_file = \"/run/key\"\ncredential = ",
            ),
            "config:value",
            "hosted.pod.credential",
        ),
        (
            "malformed credential",
            provider.replace("keychain:test-gateway/pod:fixture", "keychain:/secret-ish"),
            "config:value",
            "hosted.pod.credential",
        ),
        (
            "credential on an anonymous endpoint",
            provider.replace("auth_kind = \"bearer\"", "auth_kind = \"anonymous\""),
            "config:value",
            "hosted.pod.credential_file",
        ),
    ] {
        let refusal = parse(&text).unwrap_err();
        assert_eq!(refusal.code(), code, "{name}: {refusal}");
        let message = refusal.to_string();
        assert!(message.contains(key), "{name}: {message}");
        for leak in ["not a ref", "/run/key", "secret-ish"] {
            assert!(!message.contains(leak), "{name}: {message}");
        }
    }
    let bedrock = "listen = \"127.0.0.1:0\"\ningress_auth = \"none\"\n\n[hosted.claude]\n\
                   kind = \"bedrock\"\nregion = \"eu-central-1\"\n\
                   model_id = \"anthropic.claude-test\"\n\
                   credential = \"keychain:test-gateway/pod:fixture\"\n";
    let refusal = parse(bedrock).unwrap_err();
    assert!(
        refusal.to_string().contains("hosted.claude.credential"),
        "{refusal}"
    );
}

/// What this proves: the shipped keychain's command names the service and the account and
/// nothing else, so no value can reach an argument list.
#[test]
fn keychain_the_os_lookup_carries_only_the_two_names() {
    let reference = KeychainRef::parse("keychain:test-gateway/pod:fixture").unwrap();
    let (_, arguments) = OsKeychain::command(&reference);
    let names: Vec<&str> = arguments
        .iter()
        .map(String::as_str)
        .filter(|argument| argument.contains("test-gateway") || argument.contains("pod:"))
        .collect();
    assert_eq!(names, ["test-gateway", "pod:fixture"]);
}
