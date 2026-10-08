//! The production transport, `ConnectorsRunpod`, against a fixture of the `connectors` CLI
//! (`tests/fixture/connectors.rs`) and a loopback HTTP fixture of a pod's vLLM server.
//!
//! The interface is connectors v0.37.0's: `docs/catalog-runpod.md` (operations, classification,
//! body keys, invoke lines), `docs/local-approvals.md` (prepare → issue → invoke with the proof)
//! and `ess/domains/cli.yaml` (the JSON each command prints). No test runs the real `connectors`,
//! opens a socket beyond loopback, or reads a key.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};

use llm_provision::{
    CreateOutcome, CreateRequest, DeploymentSpec, Dispatch, HostingProvider, Identifier,
    ResourceKey,
};
use llm_runpod::{
    CloudType, ConnectorsBinding, ConnectorsRunpod, CreateAnswer, ManualClock, NetworkVolume,
    POD_CREATE_BODY_KEYS, PodRequest, PodStatus, Probe, ProbeTarget, RunpodModel, RunpodProvider,
    RunpodTransport, TerminateAnswer, Thinking, Unserviceable, VllmSettings, started_at_ms,
};
use serde_json::{Value, json};

const KEY: &str = "vllm-key-0123456789abcdef";

/// One test's fixture directory under cargo's own temporary target directory.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("transport")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("work")).expect("fixture dir");
        std::os::unix::fs::symlink(
            env!("CARGO_BIN_EXE_connectors-fixture"),
            dir.join("connectors"),
        )
        .expect("fixture link");
        Self { dir }
    }

    fn script(&self, script: &Value) {
        fs::write(self.dir.join("script.json"), script.to_string()).expect("script");
    }

    fn binding(&self, timeout: Duration) -> ConnectorsBinding {
        ConnectorsBinding {
            executable: self.dir.join("connectors"),
            adapter: "gpu".to_owned(),
            connection: "conn-1".to_owned(),
            work_directory: self.dir.join("work"),
            timeout,
        }
    }

    fn transport(&self) -> ConnectorsRunpod {
        ConnectorsRunpod::new(self.binding(Duration::from_secs(20)))
    }

    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(self.dir.join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("call line"))
            .collect()
    }

    fn calls_of(&self, key: &str) -> Vec<Value> {
        self.calls()
            .into_iter()
            .filter(|call| call["key"] == json!(key))
            .collect()
    }
}

fn arg<'a>(call: &'a Value, name: &str) -> Option<&'a str> {
    let argv = call["argv"].as_array()?;
    let index = argv.iter().position(|arg| arg == name)?;
    argv.get(index + 1)?.as_str()
}

fn input(call: &Value) -> Value {
    serde_json::from_str(call["input"].as_str().expect("input recorded")).expect("input json")
}

fn invoked(classification: &str, result: &Value) -> Value {
    json!({"stdout": {
        "adapter": "gpu", "operation": "pod.create", "revision": "revision-7",
        "result": result,
        "mutation": {"classification": classification, "replayed": false},
    }})
}

fn failed(classification: &str, code: &str) -> Value {
    json!({"exit": 2, "stdout": {"error": {"code": code, "data": {
        "kind": "provider", "code": code,
        "mutation": {"classification": classification, "replayed": false},
    }}}})
}

fn listed(pods: &Value) -> Value {
    json!({"stdout": {
        "adapter": "gpu", "operation": "pods.list", "revision": "revision-7",
        "result": {"status": 200, "body": pods},
    }})
}

fn runpod_pod(id: &str, name: &str, status: &str) -> Value {
    json!({
        "id": id, "name": name, "desiredStatus": status,
        "env": {"B10X_LLM_REQUEST": "req-1", "B10X_LLM_OWNER": "owner-a", "B10X_LLM_EPOCH": "1"},
        "gpu": {"id": "NVIDIA A40", "count": 1},
        "lastStartedAt": "2026-10-08T10:00:00.000Z",
    })
}

fn request(gpu: &str) -> PodRequest {
    PodRequest {
        name: "b10x-llm-qwen".to_owned(),
        image: "vllm/vllm-openai:v0.11.0".to_owned(),
        gpu_type: gpu.to_owned(),
        gpu_count: 1,
        cloud_type: CloudType::Secure,
        container_disk_gb: 40,
        ports: vec!["8000/http".to_owned()],
        interruptible: false,
        env: BTreeMap::from([("B10X_LLM_REQUEST".to_owned(), "req-1".to_owned())]),
        docker_entrypoint: vec![
            "vllm".to_owned(),
            "serve".to_owned(),
            "Qwen/Qwen3".to_owned(),
        ],
        network_volume_id: Some("vol123".to_owned()),
        volume_mount_path: Some("/workspace".to_owned()),
        data_center_ids: vec!["EU-RO-1".to_owned()],
    }
}

// ---- Acceptance 1: the operations, the approval flow, get by id, the body ----

#[test]
fn create_prepares_issues_and_invokes_pod_create_with_a_proof_for_its_exact_input() {
    let fixture = Fixture::new("create_flow");
    fixture.script(&json!({"operations invoke pod.create": [invoked(
        "applied",
        &json!({"status": 201, "body": runpod_pod("abc123", "b10x-llm-qwen", "RUNNING")}),
    )]}));
    let mut transport = fixture.transport();

    let answer = transport.create_pod(&request("NVIDIA A40"));

    let CreateAnswer::Created(pod) = answer else {
        panic!("an applied create is Created, got {answer:?}");
    };
    assert_eq!(pod.id, "abc123");
    assert_eq!(pod.name, "b10x-llm-qwen");
    assert_eq!(pod.status, PodStatus::Running);
    assert_eq!(pod.gpu_type.as_deref(), Some("NVIDIA A40"));

    let keys: Vec<String> = fixture
        .calls()
        .iter()
        .map(|call| call["key"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        keys,
        [
            "operations describe pod.create",
            "approvals prepare pod.create",
            "approvals issue pod.create",
            "operations invoke pod.create",
        ],
        "describe, then prepare, issue and invoke the one write"
    );
    let prepare = &fixture.calls_of("approvals prepare pod.create")[0];
    let issue = &fixture.calls_of("approvals issue pod.create")[0];
    let invoke = &fixture.calls_of("operations invoke pod.create")[0];
    for call in [prepare, issue, invoke] {
        assert_eq!(arg(call, "--adapter"), Some("gpu"));
        assert_eq!(arg(call, "--connection"), Some("conn-1"));
        assert_eq!(arg(call, "--operation"), Some("pod.create"));
        assert_eq!(arg(call, "--schema"), Some("schema-pod.create"));
        assert_eq!(arg(call, "--revision"), Some("revision-7"));
        assert_eq!(arg(call, "--output"), Some("json"));
        assert_eq!(
            call["input"], invoke["input"],
            "one exact input for all three"
        );
    }
    assert!(issue.get("subject_mismatch").is_none());
    assert!(issue.get("proof_output_refused").is_none());
    assert!(invoke.get("proof_mismatch").is_none(), "{invoke}");
    assert_eq!(arg(invoke, "--approval-file"), arg(issue, "--proof-output"));
    let proof = arg(issue, "--proof-output").expect("proof path");
    assert!(
        Path::new(proof).starts_with(fixture.dir.join("work")),
        "the proof is a new file in the private work directory"
    );
    let key = arg(invoke, "--idempotency-key").expect("idempotency key");
    assert!(!key.is_empty() && key.len() <= 256);
}

#[test]
fn the_create_body_maps_every_request_field_to_a_declared_key() {
    let fixture = Fixture::new("create_body");
    fixture.script(&json!({"operations invoke pod.create": [failed("refused", "invalid_input")]}));
    let mut transport = fixture.transport();
    let _ = transport.create_pod(&request("NVIDIA A40"));

    let invoke = &fixture.calls_of("operations invoke pod.create")[0];
    assert!(
        invoke.get("body_keys_refused").is_none(),
        "connectors v0.37.0 admits every key sent: {invoke}"
    );
    let document = input(invoke);
    let body = document["body"].as_object().expect("body object");
    assert_eq!(
        document.as_object().map(serde_json::Map::len),
        Some(1),
        "body only"
    );
    assert_eq!(body["name"], "b10x-llm-qwen");
    assert_eq!(body["imageName"], "vllm/vllm-openai:v0.11.0");
    assert_eq!(body["gpuTypeIds"], json!(["NVIDIA A40"]));
    assert_eq!(body["gpuCount"], 1);
    assert_eq!(body["cloudType"], "SECURE");
    assert_eq!(body["containerDiskInGb"], 40);
    assert_eq!(body["ports"], json!(["8000/http"]));
    assert_eq!(body["interruptible"], false);
    assert_eq!(body["env"], json!({"B10X_LLM_REQUEST": "req-1"}));
    assert_eq!(body["dataCenterIds"], json!(["EU-RO-1"]));
    assert_eq!(body["volumeMountPath"], "/workspace");
    // As llmgw (src/runpod.rs:792-793 at 048ebd8): the vLLM argv is the entrypoint and
    // `dockerStartCmd` is `[]`, which "keeps the image's" start command (connectors v0.37.0
    // docs/catalog-runpod.md:84). All three keys are in v0.37.0's `body_keys`.
    assert_eq!(
        body["dockerEntrypoint"],
        json!(["vllm", "serve", "Qwen/Qwen3"])
    );
    assert_eq!(body["dockerStartCmd"], json!([]));
    assert_eq!(body["networkVolumeId"], "vol123");
    for key in body.keys() {
        assert!(
            POD_CREATE_BODY_KEYS.contains(&key.as_str()),
            "{key} is not in POD_CREATE_BODY_KEYS"
        );
    }

    // An optional value that is absent is left out, never sent empty; `dockerStartCmd: []` stays.
    let fixture = Fixture::new("create_body_minimal");
    fixture.script(&json!({"operations invoke pod.create": [failed("refused", "invalid_input")]}));
    let mut minimal = request("NVIDIA A40");
    minimal.docker_entrypoint.clear();
    minimal.network_volume_id = None;
    minimal.volume_mount_path = None;
    minimal.data_center_ids.clear();
    let _ = fixture.transport().create_pod(&minimal);
    let body = input(&fixture.calls_of("operations invoke pod.create")[0])["body"].clone();
    assert_eq!(body["dockerStartCmd"], json!([]), "{body}");
    for absent in [
        "dockerEntrypoint",
        "networkVolumeId",
        "volumeMountPath",
        "dataCenterIds",
    ] {
        assert!(body.get(absent).is_none(), "{absent} sent empty: {body}");
    }
}

#[test]
fn the_body_key_constant_is_the_specified_pod_create_body() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../contracts/schema/schema/types/llm-gateway.runpod.PodCreateBody.schema.json"
    ))
    .expect("generated schema");
    let declared: Vec<&str> = schema["$defs"]["llm-gateway.runpod.PodCreateBody"]["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .map(String::as_str)
        .collect();
    let mut constant = POD_CREATE_BODY_KEYS.to_vec();
    let mut declared_sorted = declared.clone();
    constant.sort_unstable();
    declared_sorted.sort_unstable();
    assert_eq!(constant, declared_sorted);
}

/// The constant is connectors v0.37.0's `pod.create` `body_keys`
/// (`adapters/catalog/providers/runpod/operations.json` at `v0.37.0`), as a set.
#[test]
fn the_body_key_constant_is_connectors_v0_37_0_body_keys() {
    let v0_37_0: BTreeSet<&str> = [
        "name",
        "imageName",
        "gpuTypeIds",
        "gpuCount",
        "containerDiskInGb",
        "volumeInGb",
        "volumeMountPath",
        "ports",
        "env",
        "cloudType",
        "dataCenterIds",
        "interruptible",
        "dockerStartCmd",
        "dockerEntrypoint",
        "networkVolumeId",
    ]
    .into_iter()
    .collect();
    let constant: BTreeSet<&str> = POD_CREATE_BODY_KEYS.iter().copied().collect();
    assert_eq!(constant.len(), POD_CREATE_BODY_KEYS.len(), "no key twice");
    assert_eq!(constant, v0_37_0);
}

#[test]
fn list_invokes_pods_list_and_reads_every_pod() {
    let fixture = Fixture::new("list");
    fixture.script(&json!({"operations invoke pods.list": [listed(&json!([
        runpod_pod("abc123", "b10x-llm-qwen", "RUNNING"),
        runpod_pod("def456", "other", "EXITED"),
        runpod_pod("ghi789", "b10x-llm-old", "TERMINATED"),
    ]))]}));
    let listing = fixture.transport().list_pods().expect("listing");

    assert!(listing.complete);
    let statuses: Vec<(&str, PodStatus)> = listing
        .pods
        .iter()
        .map(|pod| (pod.id.as_str(), pod.status))
        .collect();
    assert_eq!(
        statuses,
        [
            ("abc123", PodStatus::Running),
            ("def456", PodStatus::Exited),
            ("ghi789", PodStatus::Terminated),
        ]
    );
    assert_eq!(
        listing.pods[0]
            .env
            .get("B10X_LLM_OWNER")
            .map(String::as_str),
        Some("owner-a")
    );
    let invoke = &fixture.calls_of("operations invoke pods.list")[0];
    assert_eq!(input(invoke), json!({}));
    assert!(
        arg(invoke, "--approval-file").is_none(),
        "a read carries no proof"
    );
    assert!(fixture.calls_of("approvals prepare pods.list").is_empty());
}

#[test]
fn get_is_pods_list_with_the_id_filter_and_reads_last_started_at() {
    let fixture = Fixture::new("get");
    fixture.script(&json!({"operations invoke pods.list": [listed(&json!([
        runpod_pod("abc123", "b10x-llm-qwen", "RUNNING"),
    ]))]}));
    let started = fixture.transport().container_started_at("abc123");

    assert_eq!(started, started_at_ms("2026-10-08T10:00:00.000Z"));
    assert!(started.is_some());
    let invoke = &fixture.calls_of("operations invoke pods.list")[0];
    assert_eq!(input(invoke), json!({"id": "abc123"}));
}

#[test]
fn terminate_invokes_pod_terminate_with_a_proof_for_the_pod_id() {
    let fixture = Fixture::new("terminate");
    fixture.script(&json!({"operations invoke pod.terminate": [invoked(
        "applied",
        &json!({"status": 204, "body": null}),
    )]}));
    assert_eq!(
        fixture.transport().terminate_pod("abc123"),
        TerminateAnswer::Terminated
    );
    let invoke = &fixture.calls_of("operations invoke pod.terminate")[0];
    assert_eq!(input(invoke), json!({"podId": "abc123"}));
    assert!(invoke.get("proof_mismatch").is_none(), "{invoke}");
    assert_eq!(fixture.calls_of("approvals issue pod.terminate").len(), 1);
}

// ---- Acceptance 2: Refused only for `refused`; everything else is Lost ----

#[test]
fn a_refused_create_is_refused() {
    let fixture = Fixture::new("create_refused");
    fixture.script(&json!({"operations invoke pod.create": [failed("refused", "invalid_input")]}));
    assert_eq!(
        fixture.transport().create_pod(&request("NVIDIA A40")),
        CreateAnswer::Refused
    );
    assert!(
        fixture.calls_of("operations invoke pods.list").is_empty(),
        "a definite refusal needs no read"
    );
}

#[test]
fn an_unknown_create_is_lost_after_one_listing_by_name_and_never_retried() {
    let fixture = Fixture::new("create_unknown");
    fixture.script(&json!({
        "operations invoke pod.create": [failed("unknown", "outcome_unknown")],
        "operations invoke pods.list": [listed(&json!([]))],
    }));
    assert_eq!(
        fixture.transport().create_pod(&request("NVIDIA A40")),
        CreateAnswer::Lost
    );
    assert_eq!(
        fixture.calls_of("operations invoke pod.create").len(),
        1,
        "never sent again"
    );
    let lists = fixture.calls_of("operations invoke pods.list");
    assert_eq!(lists.len(), 1, "one read resolves it");
    assert_eq!(input(&lists[0]), json!({"name": "b10x-llm-qwen"}));
}

#[test]
fn an_unknown_create_that_the_listing_finds_is_created() {
    let fixture = Fixture::new("create_unknown_found");
    fixture.script(&json!({
        "operations invoke pod.create": [failed("unknown", "outcome_unknown")],
        "operations invoke pods.list": [listed(&json!([runpod_pod("abc123", "b10x-llm-qwen", "RUNNING")]))],
    }));
    let answer = fixture.transport().create_pod(&request("NVIDIA A40"));
    assert!(
        matches!(&answer, CreateAnswer::Created(pod) if pod.id == "abc123"),
        "{answer:?}"
    );
    assert_eq!(fixture.calls_of("operations invoke pod.create").len(), 1);
}

#[test]
fn every_create_answer_that_is_not_a_definite_refusal_is_lost() {
    let cases = [
        ("timeout", json!({"sleep_ms": 5000, "stdout": {}})),
        (
            "exit_no_answer",
            json!({"exit": 1, "raw": "connectors: broken pipe"}),
        ),
        (
            "unparseable",
            json!({"raw": "{\"result\": {\"status\": 20"}),
        ),
        ("not_attempted", failed("not_attempted", "not_granted")),
        (
            "applied_without_pod",
            invoked("applied", &json!({"status": 201, "body": {}})),
        ),
        (
            "no_classification",
            json!({"stdout": {"result": {"status": 201}}}),
        ),
    ];
    for (name, answer) in cases {
        let fixture = Fixture::new(&format!("create_lost_{name}"));
        fixture.script(&json!({
            "operations invoke pod.create": [answer],
            "operations invoke pods.list": [listed(&json!([]))],
        }));
        let mut transport = ConnectorsRunpod::new(fixture.binding(Duration::from_millis(1500)));
        assert_eq!(
            transport.create_pod(&request("NVIDIA A40")),
            CreateAnswer::Lost,
            "{name}"
        );
        assert_eq!(
            fixture.calls_of("operations invoke pod.create").len(),
            1,
            "{name}: never retried"
        );
    }
}

fn provider_model() -> RunpodModel {
    RunpodModel {
        hf_model: "Qwen/Qwen3".to_owned(),
        image: Identifier::new("vllm-image").expect("image"),
        gpu_types: vec!["NVIDIA A40".to_owned(), "NVIDIA L40S".to_owned()],
        cloud_type: CloudType::Secure,
        container_disk_gb: 40,
        data_center_ids: Vec::new(),
        cache: None::<NetworkVolume>,
        api_key_secret: "qwen_key".to_owned(),
        vllm: VllmSettings {
            max_model_len: 8192,
            max_num_seqs: 8,
            gpu_memory_utilization: 0.9,
            thinking: Thinking::Off,
            reasoning_effort: None,
            sampling: None,
            extra_args: Vec::new(),
        },
        startup_deadline_ms: 600_000,
        idle_timeout_ms: 600_000,
        crash_restart_limit: 2,
        crash_window_ms: 600_000,
    }
}

fn provider(fixture: &Fixture) -> RunpodProvider<ConnectorsRunpod> {
    RunpodProvider::new(
        ConnectorsRunpod::new(fixture.binding(Duration::from_secs(20))),
        Identifier::new("runpod").expect("provider"),
        Identifier::new("acct").expect("account"),
        BTreeMap::from([(Identifier::new("qwen").expect("alias"), provider_model())]),
        Arc::new(ManualClock::new(0)),
    )
}

fn create_request() -> CreateRequest {
    CreateRequest {
        spec: DeploymentSpec {
            id: Identifier::new("qwen-deployment").expect("id"),
            provider: Identifier::new("runpod").expect("provider"),
            account: Identifier::new("acct").expect("account"),
            model: Identifier::new("qwen").expect("model"),
            image: Identifier::new("vllm-image").expect("image"),
            resource_name: Identifier::new("qwen").expect("name"),
            requested_lifetime_ms: 3_600_000,
        },
        owner: Identifier::new("owner-a").expect("owner"),
        epoch: 1,
        request_id: Identifier::new("req-1").expect("request"),
    }
}

/// The planted inversion: were `unknown` read as `Refused`, the provider's GPU fallback
/// (`provider.rs`, `fn create`) would submit a second create on the next GPU, and this fails.
#[test]
fn an_unknown_create_through_the_provider_submits_no_second_create() {
    let fixture = Fixture::new("provider_unknown");
    fixture.script(&json!({
        "operations invoke pod.create": [failed("unknown", "outcome_unknown")],
        "operations invoke pods.list": [listed(&json!([]))],
    }));
    let outcome: CreateOutcome = provider(&fixture).create(&create_request());
    assert_eq!(outcome.dispatch, Dispatch::Unknown);
    assert_eq!(
        fixture.calls_of("operations invoke pod.create").len(),
        1,
        "a lost create ends the attempt: no second GPU"
    );
}

/// The other half: a definite refusal is the only answer that moves on to the next GPU.
#[test]
fn a_refused_create_through_the_provider_tries_the_next_gpu() {
    let fixture = Fixture::new("provider_refused");
    fixture
        .script(&json!({"operations invoke pod.create": [failed("refused", "provider_refused")]}));
    let outcome = provider(&fixture).create(&create_request());
    assert_eq!(outcome.dispatch, Dispatch::Rejected);
    let gpus: Vec<Value> = fixture
        .calls_of("operations invoke pod.create")
        .iter()
        .map(|call| input(call)["body"]["gpuTypeIds"].clone())
        .collect();
    assert_eq!(gpus, [json!(["NVIDIA A40"]), json!(["NVIDIA L40S"])]);
}

// ---- Acceptance 3: a listing is complete only when it is whole ----

#[test]
fn a_listing_that_is_not_whole_is_never_complete() {
    let throttled = json!({"exit": 2, "stdout": {"error": {"code": "rate_limited", "data": {
        "kind": "provider", "code": "rate_limited", "service_code": "rate_limited",
        "retry_after_seconds": 30}}}});
    let cases = [
        (
            "truncated",
            json!({"raw": "{\"result\": {\"status\": 200, \"body\": [{\"id\": \"abc"}),
        ),
        ("throttled", throttled),
        ("interrupted", json!({"sleep_ms": 5000, "stdout": {}})),
        ("killed", json!({"exit": 137})),
        ("not_an_array", listed(&json!({"pods": []}))),
        (
            "one_unreadable_pod",
            listed(&json!([runpod_pod("abc123", "b10x-llm-qwen", "RUNNING"), {"id": 7}])),
        ),
        (
            "unknown_status",
            listed(&json!([runpod_pod("abc123", "b10x-llm-qwen", "PAUSED")])),
        ),
    ];
    for (name, answer) in cases {
        let fixture = Fixture::new(&format!("list_partial_{name}"));
        fixture.script(&json!({"operations invoke pods.list": [answer]}));
        let mut transport = ConnectorsRunpod::new(fixture.binding(Duration::from_millis(1500)));
        let complete = transport.list_pods().is_ok_and(|listing| listing.complete);
        assert!(
            !complete,
            "{name}: a listing that is not whole claimed complete"
        );
    }
}

// ---- Acceptance 4: the readiness probe ----

/// One loopback HTTP answer; returns the request it received.
fn serve_once(status: &str, body: &str) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback");
    let endpoint = format!("http://{}/v1/", listener.local_addr().expect("addr"));
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer).expect("read");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
        }
        stream.write_all(response.as_bytes()).expect("write");
        String::from_utf8_lossy(&request).into_owned()
    });
    (endpoint, handle)
}

fn probe(transport: &mut ConnectorsRunpod, endpoint: &str) -> Probe {
    transport.probe_ready(&ProbeTarget {
        pod_id: "abc123",
        model: Some("qwen"),
        endpoint: Some(endpoint),
    })
}

#[test]
fn the_probe_sends_the_models_vllm_key_as_a_bearer_to_endpoint_models() {
    let fixture = Fixture::new("probe_ready");
    let mut transport = fixture.transport();
    transport.set_vllm_key("qwen", KEY.as_bytes().to_vec());
    let (endpoint, server) = serve_once("200 OK", r#"{"object":"list","data":[{"id":"qwen"}]}"#);

    assert_eq!(
        probe(&mut transport, &endpoint),
        Probe::Ready {
            served_models: vec!["qwen".to_owned()]
        }
    );
    let request = server.join().expect("server");
    assert!(
        request.starts_with("GET /v1/models HTTP/1.1\r\n"),
        "{request}"
    );
    assert!(
        request
            .lines()
            .any(|line| line.eq_ignore_ascii_case(&format!("authorization: Bearer {KEY}"))),
        "{request}"
    );
    assert!(
        fixture.calls().is_empty(),
        "the probe goes to the pod, not to connectors"
    );
}

#[test]
fn the_probe_maps_each_answer() {
    let cases = [
        ("401 Unauthorized", Probe::Refused),
        ("403 Forbidden", Probe::Refused),
        ("503 Service Unavailable", Probe::NotReady),
        ("502 Bad Gateway", Probe::NotReady),
    ];
    for (status, expected) in cases {
        let fixture = Fixture::new("probe_answers");
        let mut transport = fixture.transport();
        transport.set_vllm_key("qwen", KEY.as_bytes().to_vec());
        let (endpoint, server) = serve_once(status, "{}");
        assert_eq!(probe(&mut transport, &endpoint), expected, "{status}");
        server.join().expect("server");
    }
    // Nobody listening, no address, and an `https://` endpoint this crate has no TLS client for.
    let fixture = Fixture::new("probe_unreachable");
    let mut transport = fixture.transport();
    let closed = TcpListener::bind("127.0.0.1:0").expect("loopback");
    let endpoint = format!("http://{}/v1/", closed.local_addr().expect("addr"));
    drop(closed);
    assert_eq!(probe(&mut transport, &endpoint), Probe::Unreachable);
    assert_eq!(
        transport.probe_ready(&ProbeTarget {
            pod_id: "abc123",
            model: Some("qwen"),
            endpoint: None
        }),
        Probe::Unreachable
    );
    assert_eq!(
        probe(&mut transport, "https://abc123-8000.proxy.runpod.net/v1/"),
        Probe::Unreachable
    );
}

#[test]
fn the_transport_debug_prints_no_key() {
    let fixture = Fixture::new("debug");
    let mut transport = fixture.transport();
    transport.set_vllm_key("qwen", KEY.as_bytes().to_vec());
    let printed = format!("{transport:?}");
    assert!(!printed.contains(KEY), "{printed}");
    assert!(!printed.contains("0123456789abcdef"), "{printed}");
    let bytes = format!("{:?}", KEY.as_bytes());
    assert!(
        !printed.contains(bytes.trim_matches(['[', ']'])),
        "the key's bytes: {printed}"
    );
    assert!(
        printed.contains("qwen"),
        "the alias is not secret: {printed}"
    );
}

// ---- Acceptance 5: crash-loop detection reads lastStartedAt moving forward ----

#[test]
fn last_started_at_parses_runpods_utc_timestamps() {
    assert_eq!(started_at_ms("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(started_at_ms("1970-01-01T00:00:01.5Z"), Some(1_500));
    assert_eq!(
        started_at_ms("2024-07-12T19:14:40.144Z"),
        Some(1_720_811_680_144)
    );
    for bad in [
        "",
        "2024-07-12",
        "2024-07-12T19:14:40+02:00",
        "2024-13-12T19:14:40Z",
    ] {
        assert_eq!(started_at_ms(bad), None, "{bad}");
    }
}

#[test]
fn a_last_started_at_moving_forward_counts_as_a_restart() {
    let at = |stamp: &str| {
        listed(&json!([{
            "id": "abc123", "name": "b10x-llm-qwen", "desiredStatus": "RUNNING",
            "env": {"B10X_LLM_REQUEST": "req-1", "B10X_LLM_OWNER": "owner-a", "B10X_LLM_EPOCH": "1"},
            "lastStartedAt": stamp,
        }]))
    };
    let fixture = Fixture::new("crash_loop");
    // Each inventory is one full listing and one get by id; the get carries the start time.
    fixture.script(&json!({"operations invoke pods.list": [
        at("2026-10-08T10:00:00.000Z"), at("2026-10-08T10:00:00.000Z"),
        at("2026-10-08T10:00:00.000Z"), at("2026-10-08T10:00:00.000Z"),
        at("2026-10-08T10:00:00.000Z"), at("2026-10-08T10:01:00.000Z"),
        at("2026-10-08T10:01:00.000Z"), at("2026-10-08T10:02:00.000Z"),
    ]}));
    let mut provider = provider(&fixture);
    let key = ResourceKey {
        provider: Identifier::new("runpod").expect("provider"),
        account: Identifier::new("acct").expect("account"),
        name: Identifier::new("b10x-llm-qwen").expect("name"),
        incarnation: Identifier::new("abc123").expect("incarnation"),
    };
    let _ = provider.inventory();
    let _ = provider.inventory();
    assert_eq!(
        provider.unserviceable(&key),
        None,
        "an unchanged start is no restart"
    );
    let _ = provider.inventory();
    assert_eq!(
        provider.unserviceable(&key),
        None,
        "one restart is below the limit of two"
    );
    let _ = provider.inventory();
    assert_eq!(
        provider.unserviceable(&key),
        Some(Unserviceable::CrashLoop),
        "two forward moves inside the window are a crash loop"
    );
}

// ---- connectors' connection evidence expires: revalidate once, repeat once ----
//
// connectors v0.37.0 docs/local-catalog-provider.md:479-486: evidence lasts
// `evidence_lifetime_ms` (60 s when omitted, at most 300 000 ms); then an invoke is refused
// `not_granted` at `admission` with `next_action: revalidate_connection` until
// `connections revalidate` renews it, and "The invoke does not revalidate on its own".

fn refused_at_admission(classification: Option<&str>) -> Value {
    let mut data = json!({
        "kind": "admission", "code": "not_granted", "stage": "admission",
        "next_action": "revalidate_connection",
    });
    if let Some(classification) = classification {
        data["mutation"] = json!({"classification": classification, "replayed": false});
    }
    json!({"exit": 2, "stdout": {"error": {"code": "not_granted", "data": data}}})
}

#[test]
fn a_read_refused_at_admission_revalidates_the_connection_once_and_is_repeated_once() {
    let fixture = Fixture::new("revalidate_read");
    fixture.script(&json!({"operations invoke pods.list": [
        refused_at_admission(None),
        listed(&json!([runpod_pod("abc123", "b10x-llm-qwen", "RUNNING")])),
    ]}));
    let listing = fixture.transport().list_pods().expect("listing");

    assert!(listing.complete, "the repeated read is whole");
    assert_eq!(listing.pods.len(), 1);
    assert_eq!(fixture.calls_of("operations invoke pods.list").len(), 2);
    let revalidations = fixture.calls_of("connections revalidate ");
    assert_eq!(revalidations.len(), 1, "revalidated once");
    assert_eq!(arg(&revalidations[0], "--adapter"), Some("gpu"));
    assert_eq!(arg(&revalidations[0], "--connection"), Some("conn-1"));
    assert_eq!(
        arg(&revalidations[0], "--expected-revision"),
        Some("conn-rev-3"),
        "the revision connections describe reports"
    );
}

#[test]
fn a_second_refusal_at_admission_is_not_repeated_again() {
    let fixture = Fixture::new("revalidate_read_twice");
    fixture.script(&json!({"operations invoke pods.list": [refused_at_admission(None)]}));
    let complete = fixture
        .transport()
        .list_pods()
        .is_ok_and(|listing| listing.complete);

    assert!(!complete);
    assert_eq!(fixture.calls_of("operations invoke pods.list").len(), 2);
    assert_eq!(fixture.calls_of("connections revalidate ").len(), 1);
}

#[test]
fn a_create_refused_at_admission_before_dispatch_is_repeated_once_with_a_fresh_proof() {
    let fixture = Fixture::new("revalidate_create");
    fixture.script(&json!({"operations invoke pod.create": [
        refused_at_admission(Some("not_attempted")),
        invoked("applied", &json!({"status": 201, "body": runpod_pod("abc123", "b10x-llm-qwen", "RUNNING")})),
    ]}));
    let answer = fixture.transport().create_pod(&request("NVIDIA A40"));

    assert!(
        matches!(&answer, CreateAnswer::Created(pod) if pod.id == "abc123"),
        "{answer:?}"
    );
    let invokes = fixture.calls_of("operations invoke pod.create");
    assert_eq!(invokes.len(), 2);
    assert_eq!(fixture.calls_of("connections revalidate ").len(), 1);
    assert_eq!(
        fixture.calls_of("approvals issue pod.create").len(),
        2,
        "a proof each"
    );
    for invoke in &invokes {
        assert!(invoke.get("proof_mismatch").is_none(), "{invoke}");
    }
    assert_eq!(invokes[0]["input"], invokes[1]["input"], "the same body");
    assert_ne!(
        arg(&invokes[0], "--idempotency-key"),
        arg(&invokes[1], "--idempotency-key"),
        "the repeat is a new attempt"
    );
}

#[test]
fn a_write_refused_after_admission_or_classified_otherwise_is_never_repeated() {
    let mut dispatched = refused_at_admission(Some("unknown"));
    dispatched["stdout"]["error"]["data"]["stage"] = json!("admission");
    let mut later = refused_at_admission(Some("not_attempted"));
    later["stdout"]["error"]["data"]["stage"] = json!("dispatch");
    for (name, answer) in [("dispatched", dispatched), ("later_stage", later)] {
        let fixture = Fixture::new(&format!("revalidate_never_{name}"));
        fixture.script(&json!({
            "operations invoke pod.create": [answer],
            "operations invoke pods.list": [listed(&json!([]))],
        }));
        assert_eq!(
            fixture.transport().create_pod(&request("NVIDIA A40")),
            CreateAnswer::Lost,
            "{name}"
        );
        assert_eq!(
            fixture.calls_of("operations invoke pod.create").len(),
            1,
            "{name}"
        );
        assert!(
            fixture.calls_of("connections revalidate ").is_empty(),
            "{name}"
        );
    }
}

// ---- connectors' published framing, and a stale descriptor ----
//
// connectors v0.37.0 `contracts/cli/v1alpha1/semantics.md:178-181`: a success is one
// `{"ok":true,"result":…}` envelope on stdout; a failure leaves stdout empty and writes one
// `{"ok":false,"error":{"code":"failure","data":…}}` envelope to stderr. The fixture frames the
// bare answers the cases above script; these cases script the frames themselves.

#[test]
fn an_answer_outside_connectors_framing_is_not_read() {
    // The `applied` create answer, unframed on stdout: not connectors' success framing.
    let fixture = Fixture::new("unframed_success");
    fixture.script(&json!({
        "operations invoke pod.create": [{"raw": json!({
            "result": {"status": 201, "body": runpod_pod("abc123", "b10x-llm-qwen", "RUNNING")},
            "mutation": {"classification": "applied", "replayed": false},
        }).to_string()}],
        "operations invoke pods.list": [listed(&json!([]))],
    }));
    assert_eq!(
        fixture.transport().create_pod(&request("NVIDIA A40")),
        CreateAnswer::Lost
    );

    // A `refused` failure framed correctly but printed on stdout, not stderr.
    let fixture = Fixture::new("failure_on_stdout");
    fixture.script(
        &json!({"operations invoke pod.create": [{"exit": 1, "raw": json!({
        "ok": false, "error": {"code": "failure", "data": {
            "code": "invalid_input", "stage": "dispatch",
            "mutation": {"classification": "refused", "replayed": false}}},
    }).to_string()}]}),
    );
    assert_eq!(
        fixture.transport().create_pod(&request("NVIDIA A40")),
        CreateAnswer::Lost,
        "a failure is read from stderr only"
    );
}

#[test]
fn a_failure_on_stderr_reaches_no_debug_output() {
    let fixture = Fixture::new("stderr_debug");
    fixture.script(
        &json!({"operations invoke pods.list": [{"exit": 1, "stderr": {
            "ok": false, "error": {"code": "failure", "data": {
                "code": "forbidden", "stage": "dispatch", "service_reason": KEY}},
        }}]}),
    );
    let mut transport = fixture.transport();
    transport.set_vllm_key("qwen", KEY.as_bytes().to_vec());
    let complete = transport.list_pods().is_ok_and(|listing| listing.complete);
    assert!(!complete);
    let printed = format!("{transport:?}");
    assert!(!printed.contains(KEY), "{printed}");
}

fn described(operation: &str, revision: &str) -> Value {
    json!({"stdout": {
        "adapter": "gpu", "revision": revision, "schema": format!("schema-{operation}-{revision}"),
        "operation": {"id": operation}, "source": "owner", "stale": false,
    }})
}

fn refused_before_dispatch(code: &str, stage: &str, next_action: &str) -> Value {
    json!({"exit": 1, "stdout": {"error": {"code": "failure", "data": {
        "kind": "operational", "code": code, "stage": stage, "next_action": next_action,
    }}}})
}

#[test]
fn a_stale_description_refreshes_the_descriptor_once_and_repeats_the_read_once() {
    let fixture = Fixture::new("stale_description_read");
    fixture.script(&json!({
        "operations describe pods.list": [described("pods.list", "revision-7"), described("pods.list", "revision-8")],
        "operations invoke pods.list": [
            refused_before_dispatch("stale_description", "admission", "refresh_description"),
            listed(&json!([runpod_pod("abc123", "b10x-llm-qwen", "RUNNING")])),
        ],
    }));
    let listing = fixture.transport().list_pods().expect("listing");

    assert!(listing.complete);
    assert_eq!(fixture.calls_of("operations describe pods.list").len(), 2);
    let invokes = fixture.calls_of("operations invoke pods.list");
    assert_eq!(invokes.len(), 2);
    assert_eq!(arg(&invokes[0], "--revision"), Some("revision-7"));
    assert_eq!(
        arg(&invokes[1], "--revision"),
        Some("revision-8"),
        "the refreshed revision"
    );
    assert_eq!(
        arg(&invokes[1], "--schema"),
        Some("schema-pods.list-revision-8")
    );
    assert!(fixture.calls_of("connections revalidate ").is_empty());
}

#[test]
fn a_lifecycle_conflict_at_admission_refreshes_revalidates_and_repeats_a_create_once() {
    let fixture = Fixture::new("lifecycle_conflict_create");
    fixture.script(&json!({
        "operations describe pod.create": [described("pod.create", "revision-7"), described("pod.create", "revision-8")],
        "operations invoke pod.create": [
            refused_before_dispatch("lifecycle_conflict", "admission", "revalidate_connection"),
            invoked("applied", &json!({"status": 201, "body": runpod_pod("abc123", "b10x-llm-qwen", "RUNNING")})),
        ],
    }));
    let answer = fixture.transport().create_pod(&request("NVIDIA A40"));

    assert!(
        matches!(&answer, CreateAnswer::Created(pod) if pod.id == "abc123"),
        "{answer:?}"
    );
    let invokes = fixture.calls_of("operations invoke pod.create");
    assert_eq!(invokes.len(), 2);
    assert_eq!(arg(&invokes[1], "--revision"), Some("revision-8"));
    assert_eq!(fixture.calls_of("connections revalidate ").len(), 1);
    assert_eq!(
        fixture.calls_of("approvals issue pod.create").len(),
        2,
        "a proof each"
    );
    let issued = fixture.calls_of("approvals issue pod.create");
    assert_eq!(
        arg(&issued[1], "--revision"),
        Some("revision-8"),
        "the proof is for the new revision"
    );
    for invoke in &invokes {
        assert!(invoke.get("proof_mismatch").is_none(), "{invoke}");
    }
}

#[test]
fn a_stale_descriptor_refused_twice_or_after_dispatch_is_not_repeated_again() {
    // Refused again after the refresh: an incomplete listing, one refresh, two invokes.
    let fixture = Fixture::new("stale_twice");
    fixture.script(&json!({"operations invoke pods.list": [
        refused_before_dispatch("stale_description", "admission", "refresh_description"),
    ]}));
    let complete = fixture
        .transport()
        .list_pods()
        .is_ok_and(|listing| listing.complete);
    assert!(!complete);
    assert_eq!(fixture.calls_of("operations describe pods.list").len(), 2);
    assert_eq!(fixture.calls_of("operations invoke pods.list").len(), 2);

    // A lifecycle_conflict outside admission, and a stale_description that carries a dispatched
    // classification: a write is never repeated.
    let mut dispatched = refused_before_dispatch("stale_description", "dispatch", "none");
    dispatched["stdout"]["error"]["data"]["mutation"] =
        json!({"classification": "unknown", "replayed": false});
    let cases = [
        (
            "conflict_later",
            refused_before_dispatch("lifecycle_conflict", "dispatch", "stop_owner"),
        ),
        ("dispatched", dispatched),
    ];
    for (name, answer) in cases {
        let fixture = Fixture::new(&format!("stale_never_{name}"));
        fixture.script(&json!({
            "operations invoke pod.create": [answer],
            "operations invoke pods.list": [listed(&json!([]))],
        }));
        assert_eq!(
            fixture.transport().create_pod(&request("NVIDIA A40")),
            CreateAnswer::Lost,
            "{name}"
        );
        assert_eq!(
            fixture.calls_of("operations invoke pod.create").len(),
            1,
            "{name}"
        );
        assert_eq!(
            fixture.calls_of("operations describe pod.create").len(),
            1,
            "{name}"
        );
    }
}
