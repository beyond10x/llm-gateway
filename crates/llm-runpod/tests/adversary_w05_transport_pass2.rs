//! Adversary pass 2 for `ConnectorsRunpod`, wave 2026-10-08-w05: the transport driven by the
//! output framing connectors v0.36.0 documents for its CLI, not the framing the fixture invents.
//!
//! connectors v0.36.0 `contracts/cli/v1alpha1/semantics.md`: "`--output json` emits exactly one
//! UTF-8 `{"ok":true,"result":...}` success envelope plus newline to stdout. Failure leaves stdout
//! empty and emits one `{"ok":false,"error":{"code":...,"data":...}}` envelope plus newline to
//! stderr". connectors' own CLI tests read it that way (`apps/connectors/tests/json_answers.rs`:
//! `serde_json::from_slice::<Value>(&output.stdout).unwrap()["result"]`, then
//! `result["schema"]`; `apps/connectors/tests/local_cli.rs`: errors from `output.stderr`, at
//! `["error"]["data"]["code"]`). The result types are `OperationDescribeResult` and
//! `OperationInvokeResult` (`ess/domains/cli.yaml`), whose `result` is the provider's answer.
//!
//! No case runs the real `connectors`, opens a socket, reads a key or calls Runpod.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use llm_runpod::{
    CloudType, ConnectorsBinding, ConnectorsRunpod, CreateAnswer, PodRequest, RunpodTransport,
};
use serde_json::{Value, json};

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("adversary-w05-transport-pass2")
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

    fn transport(&self) -> ConnectorsRunpod {
        ConnectorsRunpod::new(ConnectorsBinding {
            executable: self.dir.join("connectors"),
            adapter: "gpu".to_owned(),
            connection: "conn-1".to_owned(),
            work_directory: self.dir.join("work"),
            timeout: Duration::from_secs(10),
        })
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

/// `{"ok":true,"result":…}` on stdout, exit 0.
fn ok(result: &Value) -> Value {
    json!({"stdout": {"ok": true, "result": result}})
}

/// Exit 1, stdout empty, `{"ok":false,"error":{"code":"failure","data":…}}` on stderr.
fn failure(data: &Value) -> Value {
    json!({"exit": 1, "raw": "", "stderr": {"ok": false, "error": {"code": "failure", "data": data}}})
}

fn described(operation: &str) -> Value {
    ok(&json!({
        "adapter": "gpu", "revision": "revision-7", "schema": format!("schema-{operation}"),
        "operation": {"id": operation, "description": "", "contract": "operations/v1alpha1",
                      "profile": "resource", "input_schema": {}, "output_schema": {}},
        "source": "owner", "stale": false,
    }))
}

fn qwen_pod(id: &str) -> Value {
    json!({
        "id": id, "name": "b10x-llm-qwen", "desiredStatus": "RUNNING",
        "env": {"B10X_LLM_REQUEST": "req-1", "B10X_LLM_OWNER": "owner-a", "B10X_LLM_EPOCH": "1"},
        "gpu": {"id": "NVIDIA A40", "count": 1},
        "lastStartedAt": "2026-10-08T10:00:00.000Z",
    })
}

fn request() -> PodRequest {
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
        docker_entrypoint: Vec::new(),
        network_volume_id: None,
        volume_mount_path: None,
        data_center_ids: Vec::new(),
    }
}

/// `operations describe` and a `pods.list` invoke, both answered in the documented success
/// envelope. The listing is whole and holds the one pod.
#[test]
fn a_listing_in_connectors_success_envelope_is_read() {
    let fixture = Fixture::new("enveloped_list");
    fixture.script(&json!({
        "operations describe pods.list": [described("pods.list")],
        "operations invoke pods.list": [ok(&json!({
            "adapter": "gpu", "operation": "pods.list", "revision": "revision-7",
            "result": {"status": 200, "body": [qwen_pod("abc123")]},
        }))],
    }));
    let listing = fixture.transport().list_pods().expect("listing");

    assert_eq!(
        fixture.calls_of("operations invoke pods.list").len(),
        1,
        "the schema and revision were read from the enveloped describe, so the list was invoked"
    );
    assert!(listing.complete, "the enveloped answer is whole");
    assert_eq!(
        listing.pods.iter().map(|pod| pod.id.as_str()).collect::<Vec<_>>(),
        ["abc123"]
    );
}

/// A `pod.create` invoke answered `applied` in the documented success envelope
/// (`OperationInvokeResult`: `mutation` beside `result`, both under the envelope's `result`).
/// `operations describe` and the approval steps keep the fixture's default answers, so only the
/// invoke's framing differs from `tests/transport.rs`.
#[test]
fn an_applied_create_in_connectors_success_envelope_is_created() {
    let fixture = Fixture::new("enveloped_create");
    fixture.script(&json!({"operations invoke pod.create": [ok(&json!({
        "adapter": "gpu", "operation": "pod.create", "revision": "revision-7",
        "result": {"status": 201, "body": qwen_pod("abc123")},
        "mutation": {"classification": "applied", "replayed": false},
    }))]}));
    let answer = fixture.transport().create_pod(&request());

    assert_eq!(fixture.calls_of("operations invoke pod.create").len(), 1);
    assert!(
        matches!(&answer, CreateAnswer::Created(pod) if pod.id == "abc123"),
        "a pod Runpod created and connectors classified `applied` is {answer:?}"
    );
}

/// A create Runpod refused (`400`): connectors exits 1 with stdout empty and the `Failure` on
/// stderr, `mutation.classification = refused` in `error.data`. Nothing was created, so the
/// provider may try the next GPU: `Refused`.
#[test]
fn a_refused_create_reported_on_stderr_is_refused() {
    let fixture = Fixture::new("stderr_refused_create");
    fixture.script(&json!({"operations invoke pod.create": [failure(&json!({
        "kind": "operational", "code": "invalid_input", "stage": "dispatch",
        "next_action": "retry_explicitly", "service_code": "invalid_request",
        "mutation": {"classification": "refused", "replayed": false},
    }))]}));
    let answer = fixture.transport().create_pod(&request());

    assert_eq!(fixture.calls_of("operations invoke pod.create").len(), 1);
    assert_eq!(answer, CreateAnswer::Refused);
}

/// The correction for expired connection evidence, against the documented framing: a read
/// refused `not_granted` at `admission` (stderr) is followed by `connections describe`
/// (enveloped), one `connections revalidate`, and one repeat.
#[test]
fn a_read_refused_at_admission_on_stderr_revalidates_once() {
    let fixture = Fixture::new("stderr_not_granted_read");
    fixture.script(&json!({
        "operations invoke pods.list": [
            failure(&json!({
                "kind": "operational", "code": "not_granted", "stage": "admission",
                "next_action": "revalidate_connection",
            })),
            ok(&json!({
                "adapter": "gpu", "operation": "pods.list", "revision": "revision-7",
                "result": {"status": 200, "body": [qwen_pod("abc123")]},
            })),
        ],
        "connections describe ": [ok(&json!({"connection": {
            "summary": {"adapter": "gpu", "instance_id": "gpu-local", "connection": "conn-1",
                        "profile": "runpod.api-key", "revision": "conn-rev-3", "state": "pending"},
            "observed_at_ms": 0, "valid_until_ms": 0, "source": "owner", "stale": false,
        }}))],
    }));
    let _ = fixture.transport().list_pods();

    assert_eq!(
        fixture.calls_of("connections revalidate ").len(),
        1,
        "an expired connection is revalidated once"
    );
    assert_eq!(fixture.calls_of("operations invoke pods.list").len(), 2);
}
