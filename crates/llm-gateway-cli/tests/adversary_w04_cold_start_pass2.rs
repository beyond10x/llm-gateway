//! Adversary cases for story:cold-start-hold, pass 2: the relay when the stop of a failed pod is
//! not confirmed. Composed through `start_relaying` with `EmulatedRunpod`, a manual pool clock
//! and a loopback address nothing answers on. No pod is started, no Runpod API is called and
//! nothing leaves the loopback interface.

mod support;

use llm_gateway::RelayStream;
use llm_gateway_cli::{Deployment, PodConnector, load, start_relaying};
use llm_runpod::{EmulatedRunpod, ManualClock};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::Arc,
    thread,
    time::Duration,
};
use support::{Fixture, OWNER_SECRET, document, mutate, with_vllm_key};

const WAIT: Duration = Duration::from_secs(20);
const CHAT: &str = "/v1/chat/completions";
const SMALL_CHAT: &str = "{\"model\":\"small\",\"messages\":[]}";

fn deployment(fixture: &Fixture, lines: &str) -> Deployment {
    let secret = fixture.owner_secret();
    let key = fixture.vllm_key();
    let text = with_vllm_key(&document("127.0.0.1:0", &secret), &key);
    let text = mutate(
        &text,
        "max_model_len = 1024\n",
        &format!("max_model_len = 1024\n{lines}"),
    );
    load(&fixture.config(&text)).unwrap()
}

struct Nowhere(SocketAddr);

impl PodConnector for Nowhere {
    fn connect(&self, _authority: &str) -> io::Result<Box<dyn RelayStream>> {
        let stream = TcpStream::connect(self.0)?;
        stream.set_read_timeout(Some(WAIT))?;
        Ok(Box::new(stream))
    }
}

fn nowhere() -> Arc<dyn PodConnector> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    Arc::new(Nowhere(address))
}

fn post(address: SocketAddr) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "POST {CHAT} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{SMALL_CHAT}",
        SMALL_CHAT.len()
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    answer
}

fn header<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
    raw.split("\r\n\r\n")
        .next()?
        .split("\r\n")
        .skip(1)
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
        })
}

/// `spec/domains/deployment.yaml` (row W6), as this correction rewrote it: `model-cold-start`
/// is for "a request whose bound pod is still starting when its hold budget passes"; a request
/// arriving while the previous pod is still being stopped "waits unbound"; "any other pool
/// refusal is `target-unavailable` too". The correction deleted the clause that made a request
/// still facing a stop at its budget `model-cold-start`.
///
/// Here pod1 misses its startup deadline and its stop cannot be confirmed (it vanished and
/// every listing is partial). A request that arrives afterwards is never bound to a starting
/// pod: no pod is starting, nothing is created, and the model is not "still starting". The
/// relay answers it `model-cold-start` with `retry-after: 30` all the same, and keeps doing so
/// for every request until the stop is confirmed.
#[test]
fn adv2_w6_a_request_that_waits_out_its_hold_on_an_unconfirmed_stop_is_not_model_cold_start() {
    let fixture = Fixture::new("adv2-w04-unconfirmed-stop");
    let deployment = deployment(
        &fixture,
        "start_wait_seconds = 10\nrequest_hold_seconds = 2\n",
    );
    let runpod = EmulatedRunpod::new();
    runpod.ready_after(u32::MAX);
    let clock = ManualClock::new(1_000);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        nowhere(),
        Arc::new(clock.clone()),
    )
    .unwrap();
    let address = running.local_addr();
    let first = thread::spawn(move || post(address));
    thread::sleep(Duration::from_millis(700));
    assert_eq!(runpod.create_calls(), 1);
    runpod.vanish("pod1");
    runpod.partial_listing(true);
    clock.advance(11_000);
    let failed = first.join().unwrap();
    assert!(
        failed.contains("\"code\":\"target-unavailable\""),
        "the request held on pod1: {failed}"
    );
    let later = post(address);
    running.shutdown();
    assert_eq!(runpod.create_calls(), 1, "a create during an owed stop");
    assert!(
        later.contains("\"code\":\"target-unavailable\""),
        "a request that never had a starting pod to wait on was answered: {later}"
    );
    assert_eq!(header(&later, "retry-after"), None, "{later}");
}
