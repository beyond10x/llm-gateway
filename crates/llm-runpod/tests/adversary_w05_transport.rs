//! Adversary cases for `ConnectorsRunpod`, wave 2026-10-08-w05, against the same fixture of the
//! `connectors` CLI that `tests/transport.rs` drives (`tests/fixture/connectors.rs`).
//!
//! No case runs the real `connectors`, opens a socket, reads a key or calls Runpod.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use llm_provision::{CreateRequest, DeploymentSpec, Dispatch, HostingProvider, Identifier};
use llm_runpod::{
    CloudType, ConnectorsBinding, ConnectorsRunpod, CreateAnswer, ManualClock, NetworkVolume,
    PodRequest, RunpodModel, RunpodProvider, RunpodTransport, TerminateAnswer, Thinking,
    VllmSettings,
};
use serde_json::{Value, json};

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("adversary-w05-transport")
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

    fn calls_of(&self, key: &str) -> Vec<Value> {
        fs::read_to_string(self.dir.join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("call line"))
            .filter(|call| call["key"] == json!(key))
            .collect()
    }
}

fn input(call: &Value) -> Value {
    serde_json::from_str(call["input"].as_str().expect("input recorded")).expect("input json")
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

/// A pod named as this adapter names every pod of the alias `qwen`, created for `request_id`.
fn qwen_pod(id: &str, status: &str, request_id: &str) -> Value {
    json!({
        "id": id, "name": "b10x-llm-qwen", "desiredStatus": status,
        "env": {"B10X_LLM_REQUEST": request_id, "B10X_LLM_OWNER": "owner-a", "B10X_LLM_EPOCH": "1"},
        "gpu": {"id": "NVIDIA A40", "count": 1},
        "lastStartedAt": "2026-10-08T10:00:00.000Z",
    })
}

fn request(cached: bool) -> PodRequest {
    PodRequest {
        name: "b10x-llm-qwen".to_owned(),
        image: "vllm/vllm-openai:v0.11.0".to_owned(),
        gpu_type: "NVIDIA A40".to_owned(),
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
        network_volume_id: cached.then(|| "vol123".to_owned()),
        volume_mount_path: cached.then(|| "/workspace".to_owned()),
        data_center_ids: Vec::new(),
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

/// The binding's timeout bounds one CLI call (`ConnectorsBinding::timeout`, spec
/// `ConnectorsBinding.timeout_ms`: "a call still running then is killed and its answer is
/// lost"). A CLI that prints its answer and exits while a process it started keeps the inherited
/// stdout open (an auto-started connectors owner, a helper) must not hold the transport, and the
/// pool's mutex above it, past that bound: `wait` joins the stdout reader with no deadline.
#[test]
fn a_cli_whose_stdout_outlives_it_does_not_outlive_the_timeout() {
    let fixture = Fixture::new("stdout_held");
    let mut answer = listed(&json!([qwen_pod("abc123", "RUNNING", "req-1")]));
    answer["hold_stdout_ms"] = json!(8_000);
    fixture.script(&json!({"operations invoke pods.list": [answer]}));
    let mut transport = ConnectorsRunpod::new(fixture.binding(Duration::from_millis(1_500)));

    let started = Instant::now();
    let _ = transport.list_pods();
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(5),
        "one list with a 1.5 s per-call timeout took {elapsed:?}"
    );
}

/// The pod name is the alias (`pod_name(resource_name)`, `pool.rs` sets `resource_name` to the
/// alias), so it is not unique per create: Runpod's `Pod.name` "does not need to be unique", and
/// connectors' `docs/catalog-runpod.md` resolves an `unknown` create by name only "Give every pod
/// a unique `name`". An `unknown` create whose one same-named pod carries another request's tag
/// was not made by this create, and must not be answered `Created`.
#[test]
fn an_unknown_create_is_not_resolved_to_a_pod_of_another_request() {
    let fixture = Fixture::new("unknown_other_request");
    fixture.script(&json!({
        "operations invoke pod.create": [failed("unknown", "outcome_unknown")],
        "operations invoke pods.list": [listed(&json!([qwen_pod("old999", "EXITED", "req-0")]))],
    }));
    let mut transport = ConnectorsRunpod::new(fixture.binding(Duration::from_secs(20)));

    let answer = transport.create_pod(&request(false));

    assert_eq!(
        answer,
        CreateAnswer::Lost,
        "the only pod named b10x-llm-qwen was created for req-0, not for this req-1 create"
    );
}

/// The same, through the provider: the controller adopts an `Accepted` resource of its own owner
/// (`controller.rs`, `Dispatch::Accepted` arm), so the earlier request's pod would become this
/// record's pod.
#[test]
fn an_unknown_create_through_the_provider_does_not_accept_an_earlier_requests_pod() {
    let fixture = Fixture::new("provider_unknown_other_request");
    fixture.script(&json!({
        "operations invoke pod.create": [failed("unknown", "outcome_unknown")],
        "operations invoke pods.list": [listed(&json!([qwen_pod("old999", "RUNNING", "req-0")]))],
    }));
    let mut provider = RunpodProvider::new(
        ConnectorsRunpod::new(fixture.binding(Duration::from_secs(20))),
        Identifier::new("runpod").expect("provider"),
        Identifier::new("acct").expect("account"),
        BTreeMap::from([(Identifier::new("qwen").expect("alias"), provider_model())]),
        Arc::new(ManualClock::new(0)),
    );
    let outcome = provider.create(&CreateRequest {
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
    });

    assert_eq!(
        outcome.dispatch,
        Dispatch::Unknown,
        "accepted {:?}",
        outcome.resource.map(|resource| resource.key.incarnation)
    );
}

/// llmgw's create body (`src/runpod.rs` `create_body` at `048ebd8`) sends `"volumeInGb": 0`.
/// Runpod's pinned `PodCreateInput.volumeInGb` defaults to 20, and connectors
/// `docs/catalog-runpod.md` documents "default 20; `0` for none". `volumeInGb` is in
/// `POD_CREATE_BODY_KEYS` and in v0.36.0's admitted twelve, yet the transport never sends it, so
/// every pod without a network volume gets a 20 GB pod volume llmgw never asked for.
#[test]
fn the_create_body_asks_for_no_pod_volume_as_llmgw_does() {
    let fixture = Fixture::new("volume_in_gb");
    fixture.script(&json!({"operations invoke pod.create": [failed("refused", "invalid_input")]}));
    let mut transport = ConnectorsRunpod::new(fixture.binding(Duration::from_secs(20)));
    let _ = transport.create_pod(&request(false));

    let body = input(&fixture.calls_of("operations invoke pod.create")[0])["body"].clone();
    assert_eq!(body["volumeInGb"], json!(0), "{body}");
}

/// Terminate's mapping is the claim (`docs/hosting.md` "The production transport"): `refused`
/// with `not_found` (Runpod's 404) is `Refused`, never `Terminated`; `unknown`, `not_attempted`
/// and a timeout are `Lost`. `tests/transport.rs` pins only the `applied` case, so a mutant
/// mapping `refused` to `Terminated` passes it.
#[test]
fn terminate_maps_404_to_refused_and_every_uncertain_answer_to_lost() {
    let cases = [
        (
            "not_found",
            failed("refused", "not_found"),
            TerminateAnswer::Refused,
        ),
        (
            "unknown",
            failed("unknown", "outcome_unknown"),
            TerminateAnswer::Lost,
        ),
        (
            "not_attempted",
            failed("not_attempted", "not_granted"),
            TerminateAnswer::Lost,
        ),
        (
            "timeout",
            json!({"sleep_ms": 5_000, "stdout": {}}),
            TerminateAnswer::Lost,
        ),
        (
            "unparseable",
            json!({"raw": "{\"mutation\": {\"classif"}),
            TerminateAnswer::Lost,
        ),
    ];
    for (name, answer, expected) in cases {
        let fixture = Fixture::new(&format!("terminate_{name}"));
        fixture.script(&json!({"operations invoke pod.terminate": [answer]}));
        let mut transport = ConnectorsRunpod::new(fixture.binding(Duration::from_millis(1_500)));
        assert_eq!(transport.terminate_pod("abc123"), expected, "{name}");
        assert_eq!(
            fixture.calls_of("operations invoke pod.terminate").len(),
            1,
            "{name}: sent once"
        );
    }
}

/// connectors `approvals issue` takes "a new absolute output path inside an existing private
/// directory" (`docs/local-approvals.md`). The spec's `ConnectorsBinding.work_directory` is a
/// plain `String`, and the transport joins it as given.
#[test]
fn the_proof_output_is_absolute_for_a_relative_work_directory() {
    let fixture = Fixture::new("relative_work");
    fixture.script(&json!({"operations invoke pod.terminate": [failed("refused", "not_found")]}));
    let cwd = std::env::current_dir().expect("cwd");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace");
    assert_eq!(cwd, Path::new(env!("CARGO_MANIFEST_DIR")));
    let relative = Path::new("../..").join(
        fixture
            .dir
            .join("work")
            .strip_prefix(workspace)
            .expect("fixture under the workspace"),
    );
    assert!(relative.is_dir(), "{}", relative.display());
    let mut binding = fixture.binding(Duration::from_secs(20));
    binding.work_directory = relative;
    let mut transport = ConnectorsRunpod::new(binding);

    let _ = transport.terminate_pod("abc123");

    let issue = &fixture.calls_of("approvals issue pod.terminate")[0];
    assert!(issue.get("proof_output_refused").is_none(), "{issue}");
}
