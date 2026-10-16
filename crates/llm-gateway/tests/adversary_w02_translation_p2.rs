//! Adversary, story:gateway-translation pass 2: the usage record of a translated request whose
//! client went away before any answer. The translator is a fixture that speaks no protocol: it
//! waits for the writer to report the client gone and then returns `Ok(())` having written
//! nothing, as the binary's `LlmTranslator` does when `TranslatedAnswer::gone` cancels its turn
//! (`ClientSink::finish`, `if self.gone { return Ok(()) }`).

use llm_gateway::{
    Disposition, Gateway, GatewayConfig, Label, OwnerToken, RefusalCode, Relay, RelayModel,
    RelayStream, RelayTarget, RelayTargets, RouteInventory, SharedSecretVerifier, TargetRefusal,
    ToolCalling, TranslatedAnswer, Translation, TranslationFailure, TranslationRequest, Translator,
    UsageRecord, UsageRecords, Wire,
};
use std::{
    io::{self, Write},
    net::TcpStream,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const OWNER: &str = "owner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WAIT: Duration = Duration::from_secs(10);

struct Waits;

impl Translator for Waits {
    fn prepare(
        &self,
        _request: &TranslationRequest<'_>,
    ) -> Result<Box<dyn Translation>, RefusalCode> {
        Ok(Box::new(WaitsRun))
    }
}

struct WaitsRun;

impl Translation for WaitsRun {
    fn run(
        self: Box<Self>,
        _target: &dyn RelayTarget,
        answer: &mut dyn TranslatedAnswer,
    ) -> Result<(), TranslationFailure> {
        let deadline = Instant::now() + WAIT;
        while !answer.gone() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
}

struct Targets;

struct Target;

impl RelayTargets for Targets {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(Target))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

impl RelayTarget for Target {
    fn authority(&self) -> &'static str {
        "pod.example.test:8000"
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        Err(io::Error::other(
            "the translated path opens no byte relay connection",
        ))
    }
}

#[derive(Default)]
struct Kept(Mutex<Vec<UsageRecord>>);

impl UsageRecords for Kept {
    fn record(&self, record: &UsageRecord) {
        self.0.lock().unwrap().push(record.clone());
    }
}

/// `Disposition::Relayed` is "the target's answer reached the client", and `UsageRecord::status`
/// "the status line the client received" (`spec/domains/telemetry.yaml`, `docs/gateway.md`
/// "Counters and usage records"). A non-streamed translated request whose client closed before
/// any answer is recorded `Relayed` with status 0: no answer reached anybody and no status line
/// was written, and `llmgw_route_requests_total` counts it as a relayed call. Reached by every
/// client that gives up on a non-streamed translated answer (a timeout, Ctrl-C), the path the
/// correction round added (`relay.rs` `Ok(()) if answer.gone => Ok(())`).
///
/// Re-pinned in correction round 2. Asserted first: such a request is not recorded `Relayed`
/// with status 0. Decided instead: the record is kept and the contract says it
/// (`spec/domains/telemetry.yaml` Disposition and `status`, `docs/gateway.md` "Counters and
/// usage records"): `Relayed`, status 0, no target status and 0 response bytes mean no answer
/// was written because the client left.
#[test]
fn adv_p2_a_client_gone_before_a_translated_answer_is_recorded_relayed_with_status_0() {
    let kept = Arc::new(Kept::default());
    let model = RelayModel::new(
        Label::new("code").unwrap(),
        Label::new("served").unwrap(),
        vec![Wire::Messages],
    )
    .unwrap()
    .with_tool_calling(ToolCalling::Absent);
    let relay = Relay::new(vec![model], Arc::new(Targets) as Arc<dyn RelayTargets>)
        .unwrap()
        .with_translator(Arc::new(Waits))
        .with_records(Arc::clone(&kept) as Arc<dyn UsageRecords>);
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
    let body = "{\"model\":\"code\",\"messages\":[{\"role\":\"user\",\"content\":\"Hi\"}]}";
    let mut stream = TcpStream::connect(handle.local_addr()).unwrap();
    write!(
        stream,
        "POST /v1/chat/completions HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    drop(stream);
    let deadline = Instant::now() + WAIT;
    while kept.0.lock().unwrap().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    handle.shutdown();
    let records = kept.0.lock().unwrap().clone();
    let [record] = records.as_slice() else {
        panic!("expected one usage record, got {records:#?}");
    };
    assert!(
        record.disposition == Disposition::Relayed
            && record.status == 0
            && record.target_status.is_none()
            && record.refusal.is_none()
            && record.response_bytes == 0,
        "a client gone before any answer is recorded {:?} with status {} (target status {:?}, {} bytes)",
        record.disposition,
        record.status,
        record.target_status,
        record.response_bytes
    );
}
