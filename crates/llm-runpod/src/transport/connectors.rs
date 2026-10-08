//! The production Runpod transport: the control plane through the `connectors` CLI.
//!
//! `beyond10x/connectors` v0.36.0 serves Runpod's REST API from a pinned `OpenAPI` document through
//! its catalog provider and keeps the Runpod API key in its keyring (connectors
//! `docs/catalog-runpod.md`). This transport runs that CLI as a child process, one call at a
//! time, and maps what it prints; it holds no HTTP client for the control plane and never sees the
//! Runpod key. The invocations, their inputs and the answer mapping are declared in
//! `spec/domains/runpod.yaml` (`ConnectorsBinding`, `ConnectorsOperation`, `CreateAnswer`,
//! `TerminateAnswer`, `PodCreateBody`).
//!
//! * Every call is `connectors --output json operations invoke --adapter <a> --connection <c>
//!   --operation <id> --schema <s> --revision <r> --input-file <document>`; `schema` and
//!   `revision` come from `operations describe` for that operation, read once.
//! * A write (`pod.create`, `pod.terminate`) is first `approvals prepare`d and `approvals
//!   issue`d for its exact input document, then invoked with `--approval-file <proof>
//!   --idempotency-key <key>` (connectors `docs/local-approvals.md`). It is never sent twice.
//! * A create is `Refused` only when connectors classifies it `refused`. An `unknown` create is
//!   resolved by one `pods.list` on the pod's unique name; everything else is `Lost`.

use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Map, Value, json};

use super::probe::{self, BearerKey};
use crate::{
    CreateAnswer, Pod, PodListing, PodRequest, PodStatus, Probe, ProbeTarget, RunpodTransport,
    TerminateAnswer,
};

/// Every key a `pod.create` `body` may carry, as Runpod's pinned `PodCreateInput` names it
/// (`llm-gateway.runpod.PodCreateBody`).
///
/// Source: connectors v0.36.0, `adapters/catalog/providers/runpod/operations.json`, `pod.create`
/// `body_keys` (the first twelve), plus `dockerStartCmd` and `networkVolumeId` from the pinned
/// `PodCreateInput` (connectors `adapters/runpod/upstream/runpod-rest-v1.json`, SHA-256
/// `9500a8989878d53d8731f27bf8dbbd57801b328c760bdb32c38ba36d5cb580db`). v0.36.0 refuses those
/// two as `invalid_input` before any request; a connectors release admitting them is requested
/// upstream. Move this list with the connectors release this crate is used with.
pub const POD_CREATE_BODY_KEYS: &[&str] = &[
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
    "networkVolumeId",
];

/// The connectors operation ids of the shipped Runpod selection set.
const POD_CREATE: &str = "pod.create";
const PODS_LIST: &str = "pods.list";
const POD_TERMINATE: &str = "pod.terminate";

/// The most a CLI answer may print before it is treated as unreadable.
const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
/// How long a probe of a pod may take, connect to last byte.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// What the transport is built from (`llm-gateway.runpod.ConnectorsBinding`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorsBinding {
    /// The `connectors` executable.
    pub executable: PathBuf,
    /// The adapter alias the connectors configuration binds the Runpod catalog provider to.
    pub adapter: String,
    /// The saved connectors connection id.
    pub connection: String,
    /// An existing private directory for each write's input document and approval proof.
    pub work_directory: PathBuf,
    /// The bound on one CLI call. A call still running then is killed, and its answer is lost.
    pub timeout: Duration,
}

/// The production Runpod transport over the `connectors` CLI.
///
/// The vLLM keys for the readiness probe are handed in by model alias
/// ([`set_vllm_key`](Self::set_vllm_key)); the transport reads no file for them, and its `Debug`
/// prints the aliases only.
pub struct ConnectorsRunpod {
    binding: ConnectorsBinding,
    keys: BTreeMap<String, BearerKey>,
    /// `(schema, revision)` per operation, from `operations describe`.
    described: BTreeMap<&'static str, (String, String)>,
}

impl fmt::Debug for ConnectorsRunpod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectorsRunpod")
            .field("binding", &self.binding)
            .field("keys", &self.keys.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// What one CLI call ended with.
struct Answer {
    /// `None` when the call was killed at the timeout, could not start, or ended by a signal.
    exit: Option<i32>,
    /// The JSON it printed, if it printed exactly one readable JSON document.
    json: Option<Value>,
}

impl Answer {
    fn lost() -> Self {
        Self {
            exit: None,
            json: None,
        }
    }

    /// connectors' `mutation.classification`, on success at the top level and on failure in
    /// `error.data` (connectors `docs/local-gitlab-merge.md`).
    fn classification(&self) -> Option<&str> {
        let json = self.json.as_ref()?;
        json.pointer("/mutation/classification")
            .or_else(|| json.pointer("/error/data/mutation/classification"))
            .and_then(Value::as_str)
    }

    /// The provider's answer body, only for a call that succeeded.
    fn body(&self) -> Option<&Value> {
        if self.exit != Some(0) {
            return None;
        }
        let result = self.json.as_ref()?.get("result")?;
        if let Some(status) = result.get("status")
            && !status
                .as_u64()
                .is_some_and(|status| (200..300).contains(&status))
        {
            return None;
        }
        result.get("body")
    }
}

/// A unique stem for one write's files in the work directory.
fn file_stem(operation: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("{operation}-{}-{nanos}-{sequence}", std::process::id())
}

impl ConnectorsRunpod {
    /// A transport over one connectors adapter and connection. Runs nothing yet.
    pub fn new(binding: ConnectorsBinding) -> Self {
        Self {
            binding,
            keys: BTreeMap::new(),
            described: BTreeMap::new(),
        }
    }

    /// Hands the transport one model's vLLM key, which the readiness probe of that model's pods
    /// sends as a bearer. Replaces any earlier key for the alias.
    pub fn set_vllm_key(&mut self, alias: &str, key: Vec<u8>) {
        self.keys.insert(alias.to_owned(), BearerKey::new(key));
    }

    /// Runs the CLI once with these arguments, after `--output json`, within the timeout.
    fn run(&self, args: &[&str]) -> Answer {
        let Ok(child) = Command::new(&self.binding.executable)
            .args(["--output", "json"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return Answer::lost();
        };
        self.wait(child)
    }

    fn wait(&self, mut child: Child) -> Answer {
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Answer::lost();
        };
        let reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            let limit = u64::try_from(MAX_OUTPUT_BYTES).unwrap_or(u64::MAX);
            let complete = (&mut stdout)
                .take(limit.saturating_add(1))
                .read_to_end(&mut bytes)
                .is_ok()
                && bytes.len() <= MAX_OUTPUT_BYTES;
            complete.then_some(bytes)
        });
        let deadline = Instant::now() + self.binding.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let printed = reader.join().ok().flatten();
        let Some(status) = status else {
            return Answer::lost();
        };
        Answer {
            exit: status.code(),
            json: printed.and_then(|bytes| serde_json::from_slice(&bytes).ok()),
        }
    }

    /// `(schema, revision)` for one operation, from `operations describe`, read once.
    fn describe(&mut self, operation: &'static str) -> Option<(String, String)> {
        if let Some(found) = self.described.get(operation) {
            return Some(found.clone());
        }
        let answer = self.run(&[
            "operations",
            "describe",
            "--adapter",
            &self.binding.adapter,
            "--operation",
            operation,
        ]);
        if answer.exit != Some(0) {
            return None;
        }
        let json = answer.json?;
        let schema = json.get("schema")?.as_str()?.to_owned();
        let revision = json.get("revision")?.as_str()?.to_owned();
        self.described
            .insert(operation, (schema.clone(), revision.clone()));
        Some((schema, revision))
    }

    /// Writes a new owner-only file in the work directory. `None` if it cannot.
    fn new_file(&self, name: &str, contents: &[u8]) -> Option<PathBuf> {
        let path = self.binding.work_directory.join(name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .ok()?;
        if file
            .write_all(contents)
            .and_then(|()| file.sync_all())
            .is_err()
        {
            let _ = fs::remove_file(&path);
            return None;
        }
        Some(path)
    }

    /// One read: `operations invoke` of `pods.list` with these filters.
    fn read(&mut self, filters: &Value) -> Answer {
        let Some((schema, revision)) = self.describe(PODS_LIST) else {
            return Answer::lost();
        };
        let Some(input) = self.new_file(
            &format!("{}.json", file_stem(PODS_LIST)),
            filters.to_string().as_bytes(),
        ) else {
            return Answer::lost();
        };
        let answer = self.run(&[
            "operations",
            "invoke",
            "--adapter",
            &self.binding.adapter,
            "--connection",
            &self.binding.connection,
            "--operation",
            PODS_LIST,
            "--schema",
            &schema,
            "--revision",
            &revision,
            "--input-file",
            &path_arg(&input),
        ]);
        let _ = fs::remove_file(&input);
        answer
    }

    /// One write: prepare and issue an approval for its exact input, then invoke it once with
    /// that proof. `None` when it was never invoked.
    fn write(
        &mut self,
        operation: &'static str,
        input: &Value,
        idempotency_key: &str,
    ) -> Option<Answer> {
        let (schema, revision) = self.describe(operation)?;
        let stem = file_stem(operation);
        let input_path = self.new_file(&format!("{stem}.json"), input.to_string().as_bytes())?;
        let proof_path = self.binding.work_directory.join(format!("{stem}.proof"));
        let answer = self.approve_and_invoke(
            operation,
            &schema,
            &revision,
            &input_path,
            &proof_path,
            idempotency_key,
        );
        let _ = fs::remove_file(&input_path);
        let _ = fs::remove_file(&proof_path);
        answer
    }

    fn approve_and_invoke(
        &self,
        operation: &str,
        schema: &str,
        revision: &str,
        input: &Path,
        proof: &Path,
        idempotency_key: &str,
    ) -> Option<Answer> {
        let input = path_arg(input);
        let proof = path_arg(proof);
        let target = [
            "--adapter",
            &self.binding.adapter,
            "--connection",
            &self.binding.connection,
            "--operation",
            operation,
            "--schema",
            schema,
            "--revision",
            revision,
            "--input-file",
            &input,
        ];
        let prepared = self.run(&[&["approvals", "prepare"][..], &target[..]].concat());
        if prepared.exit != Some(0) {
            return None;
        }
        let subject = prepared
            .json?
            .pointer("/preparation/subject_sha256")?
            .as_str()?
            .to_owned();
        let issued = self.run(
            &[
                &["approvals", "issue"][..],
                &target[..],
                &["--approve-subject", &subject, "--proof-output", &proof][..],
            ]
            .concat(),
        );
        if issued.exit != Some(0) {
            return None;
        }
        Some(
            self.run(
                &[
                    &["operations", "invoke"][..],
                    &target[..],
                    &[
                        "--approval-file",
                        &proof,
                        "--idempotency-key",
                        idempotency_key,
                    ][..],
                ]
                .concat(),
            ),
        )
    }

    /// One whole `pods.list` answer, or `None` when any part of it is not readable.
    fn listing(&mut self, filters: &Value) -> Option<Vec<Pod>> {
        let answer = self.read(filters);
        answer.body()?.as_array()?.iter().map(pod_of).collect()
    }

    /// One pod by id: `pods.list` with the `id` filter, and the raw entry with it.
    fn get(&mut self, pod_id: &str) -> Option<Value> {
        let answer = self.read(&json!({ "id": pod_id }));
        let pods = answer.body()?.as_array()?;
        let mut matching = pods
            .iter()
            .filter(|pod| pod.get("id").and_then(Value::as_str) == Some(pod_id));
        let found = matching.next()?.clone();
        matching.next().is_none().then_some(found)
    }
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// One pod of a `pods.list` answer, or `None` when any field this adapter relies on is missing
/// or of another shape.
fn pod_of(value: &Value) -> Option<Pod> {
    let status = match value.get("desiredStatus")?.as_str()? {
        "RUNNING" => PodStatus::Running,
        "EXITED" => PodStatus::Exited,
        "TERMINATED" => PodStatus::Terminated,
        _ => return None,
    };
    let env = match value.get("env") {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(Value::Object(env)) => env
            .iter()
            .map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
            .collect::<Option<_>>()?,
        Some(_) => return None,
    };
    Some(Pod {
        id: value.get("id")?.as_str()?.to_owned(),
        name: value.get("name")?.as_str()?.to_owned(),
        status,
        env,
        gpu_type: value
            .pointer("/gpu/id")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// The `pod.create` input: `{"body": …}` with every key [`POD_CREATE_BODY_KEYS`] names that the
/// request has a value for.
fn create_input(request: &PodRequest) -> Value {
    let mut body = Map::new();
    body.insert("name".to_owned(), json!(request.name));
    body.insert("imageName".to_owned(), json!(request.image));
    body.insert("gpuTypeIds".to_owned(), json!([request.gpu_type]));
    body.insert("gpuCount".to_owned(), json!(request.gpu_count));
    body.insert(
        "containerDiskInGb".to_owned(),
        json!(request.container_disk_gb),
    );
    if let Some(path) = &request.volume_mount_path {
        body.insert("volumeMountPath".to_owned(), json!(path));
    }
    body.insert("ports".to_owned(), json!(request.ports));
    body.insert("env".to_owned(), json!(request.env));
    body.insert("cloudType".to_owned(), json!(request.cloud_type.as_str()));
    if !request.data_center_ids.is_empty() {
        body.insert("dataCenterIds".to_owned(), json!(request.data_center_ids));
    }
    body.insert("interruptible".to_owned(), json!(request.interruptible));
    if !request.docker_entrypoint.is_empty() {
        body.insert(
            "dockerStartCmd".to_owned(),
            json!(request.docker_entrypoint),
        );
    }
    if let Some(volume) = &request.network_volume_id {
        body.insert("networkVolumeId".to_owned(), json!(volume));
    }
    body.retain(|key, _| POD_CREATE_BODY_KEYS.contains(&key.as_str()));
    json!({ "body": body })
}

/// Runpod's `lastStartedAt` (`2024-07-12T19:14:40.144Z`, UTC) in milliseconds since the Unix
/// epoch, or `None` for any other shape.
pub fn started_at_ms(stamp: &str) -> Option<u64> {
    let stamp = stamp.strip_suffix('Z')?;
    let (date, time) = stamp.split_once('T')?;
    let mut date = date.splitn(3, '-');
    let year: i64 = digits(date.next()?, 4)?;
    let month: i64 = digits(date.next()?, 2)?;
    let day: i64 = digits(date.next()?, 2)?;
    let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut clock = clock.splitn(3, ':');
    let hour: i64 = digits(clock.next()?, 2)?;
    let minute: i64 = digits(clock.next()?, 2)?;
    let second: i64 = digits(clock.next()?, 2)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || (time.contains('.') && fraction.is_empty())
    {
        return None;
    }
    let millis: i64 = format!("{fraction:0<3}").get(..3)?.parse().ok()?;
    // Days from the civil date (Howard Hinnant's algorithm).
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let year_of_era = shifted - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(seconds * 1_000 + millis).ok()
}

fn digits(text: &str, width: usize) -> Option<i64> {
    (text.len() == width && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

impl RunpodTransport for ConnectorsRunpod {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        let answer = self.read(&json!({}));
        let Some(entries) = answer.body().and_then(Value::as_array) else {
            return Ok(PodListing {
                pods: Vec::new(),
                complete: false,
            });
        };
        let parsed: Vec<Option<Pod>> = entries.iter().map(pod_of).collect();
        let complete = parsed.iter().all(Option::is_some);
        Ok(PodListing {
            pods: parsed.into_iter().flatten().collect(),
            complete,
        })
    }

    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer {
        let request_tag = request
            .env
            .get(crate::TAG_REQUEST)
            .map_or(request.name.as_str(), String::as_str);
        let key = format!("{POD_CREATE}:{request_tag}:{}", request.gpu_type);
        let Some(answer) = self.write(POD_CREATE, &create_input(request), &key) else {
            return CreateAnswer::Lost;
        };
        match answer.classification() {
            Some("applied") => answer
                .body()
                .and_then(pod_of)
                .map_or(CreateAnswer::Lost, CreateAnswer::Created),
            Some("refused") => CreateAnswer::Refused,
            // The pod may exist and may be billed: one read by its unique name, never a resend.
            Some("unknown") => match self.listing(&json!({ "name": request.name })) {
                Some(pods) => {
                    let mut named = pods.into_iter().filter(|pod| pod.name == request.name);
                    match (named.next(), named.next()) {
                        (Some(pod), None) => CreateAnswer::Created(pod),
                        _ => CreateAnswer::Lost,
                    }
                }
                None => CreateAnswer::Lost,
            },
            _ => CreateAnswer::Lost,
        }
    }

    fn terminate_pod(&mut self, pod_id: &str) -> TerminateAnswer {
        let key = format!("{POD_TERMINATE}:{pod_id}:{}", file_stem("attempt"));
        let Some(answer) = self.write(POD_TERMINATE, &json!({ "podId": pod_id }), &key) else {
            return TerminateAnswer::Lost;
        };
        match answer.classification() {
            Some("applied") => TerminateAnswer::Terminated,
            // Including `404` / `not_found`: nothing was terminated by this call, and under
            // another account's key the pod may still run (connectors docs/catalog-runpod.md).
            Some("refused") => TerminateAnswer::Refused,
            _ => TerminateAnswer::Lost,
        }
    }

    fn probe_ready(&mut self, target: &ProbeTarget<'_>) -> Probe {
        let Some(endpoint) = target.endpoint else {
            return Probe::Unreachable;
        };
        let key = target.model.and_then(|model| self.keys.get(model));
        probe::models(endpoint, key, PROBE_TIMEOUT)
    }

    fn container_started_at(&mut self, pod_id: &str) -> Option<u64> {
        self.get(pod_id)?
            .get("lastStartedAt")
            .and_then(Value::as_str)
            .and_then(started_at_ms)
    }
}
