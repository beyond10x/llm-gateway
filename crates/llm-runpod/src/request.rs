//! Turning a declared model into a Runpod create request.
//!
//! Ported from `vllm_entrypoint` and `create_body` in llmgw `src/runpod.rs`. Two things changed
//! on purpose: the vLLM key reaches the pod as a Runpod secret reference instead of a derived
//! value on the command line, and the owner, epoch and request id are written into the pod's
//! environment so a listing can tell whose pod it is.

use std::collections::BTreeMap;

use crate::{PodRequest, RunpodModel, Thinking};

/// The prefix of every pod name this adapter creates.
///
/// Deliberately not llmgw's `llmgw-` (llmgw `src/runpod.rs:21`): the two controllers must never
/// select each other's pods, and a shared prefix would make llmgw's orphan sweep terminate pods
/// created here, and this adapter's sweep terminate llmgw's.
pub const POD_NAME_PREFIX: &str = "b10x-llm-";

/// llmgw's pod name prefix. Named only so it can be refused.
pub const LEGACY_POD_NAME_PREFIX: &str = "llmgw-";

/// Environment key carrying the controller identity that created the pod.
pub const TAG_OWNER: &str = "B10X_LLM_OWNER";
/// Environment key carrying the ownership epoch the pod was created at.
pub const TAG_EPOCH: &str = "B10X_LLM_EPOCH";
/// Environment key carrying the idempotency key of the create request.
pub const TAG_REQUEST: &str = "B10X_LLM_REQUEST";

/// The model card's non-thinking sampling defaults, as llmgw applied them.
pub const NON_THINKING_SAMPLING: &str =
    r#"{"temperature":0.7,"top_p":0.8,"top_k":20,"presence_penalty":1.5}"#;

/// The pod name for one resource name.
pub fn pod_name(resource_name: &str) -> String {
    format!("{POD_NAME_PREFIX}{resource_name}")
}

/// Whether a pod name is inside this adapter's namespace. llmgw's never is: the two prefixes are
/// disjoint, which `the_orphan_sweep_never_selects_a_legacy_llmgw_pod` asserts.
pub fn in_namespace(name: &str) -> bool {
    name.starts_with(POD_NAME_PREFIX)
}

/// The vLLM serve arguments: the closed base set, the thinking and sampling knobs, then the
/// model-specific extra arguments verbatim.
pub fn vllm_entrypoint(served_name: &str, model: &RunpodModel) -> Vec<String> {
    let vllm = &model.vllm;
    let mut args: Vec<String> = [
        "vllm",
        "serve",
        &model.hf_model,
        "--host",
        "0.0.0.0",
        "--port",
        "8000",
        "--served-model-name",
        served_name,
        "--max-model-len",
        &vllm.max_model_len.to_string(),
        "--max-num-seqs",
        &vllm.max_num_seqs.to_string(),
        "--gpu-memory-utilization",
        &vllm.gpu_memory_utilization.to_string(),
        "--generation-config",
        "vllm",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    // Both values are validated to be plain tokens, so this is well-formed JSON without an
    // encoder: `enable_thinking` is a literal and `reasoning_effort` is alphanumeric.
    let mut kwargs = Vec::new();
    if vllm.thinking == Thinking::Off {
        kwargs.push(r#""enable_thinking":false"#.to_owned());
    }
    if let Some(effort) = &vllm.reasoning_effort {
        kwargs.push(format!(r#""reasoning_effort":"{effort}""#));
    }
    if !kwargs.is_empty() {
        args.push("--default-chat-template-kwargs".to_owned());
        args.push(format!("{{{}}}", kwargs.join(",")));
    }
    let sampling = vllm
        .sampling
        .clone()
        .or_else(|| (vllm.thinking == Thinking::Off).then(|| NON_THINKING_SAMPLING.to_owned()));
    if let Some(sampling) = sampling {
        args.push("--override-generation-config".to_owned());
        args.push(sampling);
    }
    args.extend(vllm.extra_args.iter().cloned());
    args
}

/// Everything that identifies who created a pod and why.
pub struct Tags<'a> {
    pub owner: &'a str,
    pub epoch: u64,
    pub request_id: &'a str,
}

/// The create request for one GPU choice.
pub fn pod_request(
    resource_name: &str,
    served_name: &str,
    gpu: &str,
    model: &RunpodModel,
    tags: &Tags<'_>,
) -> PodRequest {
    let mut env = BTreeMap::from([
        (
            "PYTORCH_CUDA_ALLOC_CONF".to_owned(),
            "expandable_segments:True".to_owned(),
        ),
        (
            "VLLM_API_KEY".to_owned(),
            format!("{{{{ RUNPOD_SECRET_{} }}}}", model.api_key_secret),
        ),
        (crate::TAG_OWNER.to_owned(), tags.owner.to_owned()),
        (crate::TAG_EPOCH.to_owned(), tags.epoch.to_string()),
        (crate::TAG_REQUEST.to_owned(), tags.request_id.to_owned()),
    ]);
    if let Some(cache) = &model.cache {
        // The checkpoint lands on the mounted volume, so the next cold start finds it there
        // instead of downloading it again.
        env.insert(
            "HF_HOME".to_owned(),
            format!("{}/huggingface", cache.mount_path),
        );
    }
    PodRequest {
        name: pod_name(resource_name),
        image: model.image.as_str().to_owned(),
        gpu_type: gpu.to_owned(),
        gpu_count: 1,
        cloud_type: model.cloud_type,
        container_disk_gb: model.container_disk_gb,
        ports: vec!["8000/http".to_owned()],
        interruptible: false,
        env,
        docker_entrypoint: vllm_entrypoint(served_name, model),
        network_volume_id: model.cache.as_ref().map(|cache| cache.volume_id.clone()),
        volume_mount_path: model.cache.as_ref().map(|cache| cache.mount_path.clone()),
        data_center_ids: model.data_center_ids.clone(),
    }
}
