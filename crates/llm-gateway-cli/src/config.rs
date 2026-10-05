//! The closed TOML deployment document (row K29; llmgw `src/config.rs` at 048ebd8), mapped onto
//! the settings llm-gateway already has: `listen` onto [`GatewayConfig`], `cloud_type` onto
//! [`CloudType`], the vLLM keys onto [`VllmSettings`] and the volume keys onto
//! [`NetworkVolume`].

use crate::{
    refusal::{Refusal, StartupRefusal},
    trusted,
};
use llm_gateway::GatewayConfig;
use llm_runpod::{CloudType, NetworkVolume, Thinking, VllmSettings};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    ops::RangeInclusive,
    path::{Path, PathBuf},
};

const REASONING_EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];
const HEX: &[u8; 16] = b"0123456789abcdef";

// --- The document, exactly as written --------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    listen: SocketAddr,
    owner_secret_file: PathBuf,
    #[serde(default)]
    providers: BTreeMap<String, ProviderDocument>,
    #[serde(default)]
    models: BTreeMap<String, ModelDocument>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderDocument {
    kind: ProviderKind,
    #[serde(default)]
    cloud_type: CloudTypeDocument,
}

#[derive(Deserialize, Default)]
enum CloudTypeDocument {
    #[default]
    #[serde(rename = "SECURE")]
    Secure,
    #[serde(rename = "COMMUNITY")]
    Community,
}

#[derive(Deserialize, Default)]
enum ThinkingDocument {
    #[serde(rename = "on")]
    On,
    #[default]
    #[serde(rename = "off")]
    Off,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelDocument {
    provider: String,
    wires: Vec<Wire>,
    context_window: u32,
    hf_model: String,
    image: String,
    gpu_types: Vec<String>,
    max_model_len: u32,
    #[serde(default = "default_max_num_seqs")]
    max_num_seqs: u32,
    #[serde(default = "default_gpu_util")]
    gpu_util: f64,
    #[serde(default = "default_disk_gb")]
    disk_gb: u32,
    #[serde(default)]
    thinking: ThinkingDocument,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    sampling: Option<String>,
    #[serde(default)]
    extra_vllm_args: Vec<String>,
    #[serde(default)]
    network_volume_id: Option<String>,
    #[serde(default = "default_volume_mount_path")]
    volume_mount_path: String,
    #[serde(default)]
    data_center_ids: Vec<String>,
    #[serde(default = "default_idle_timeout_minutes")]
    idle_timeout_minutes: u64,
    #[serde(default = "default_start_wait_seconds")]
    start_wait_seconds: u64,
}

fn default_max_num_seqs() -> u32 {
    8
}

fn default_gpu_util() -> f64 {
    0.90
}

fn default_disk_gb() -> u32 {
    80
}

fn default_volume_mount_path() -> String {
    "/workspace".to_string()
}

fn default_idle_timeout_minutes() -> u64 {
    30
}

fn default_start_wait_seconds() -> u64 {
    600
}

// --- The loaded deployment -------------------------------------------------------------------

/// `providers.<name>.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "runpod-vllm")]
    RunpodVllm,
}

/// A wire a model is served on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum Wire {
    #[serde(rename = "chat")]
    Chat,
    #[serde(rename = "responses")]
    Responses,
    #[serde(rename = "messages")]
    Messages,
}

impl Wire {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }
}

/// `[providers.<name>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub kind: ProviderKind,
    pub cloud_type: CloudType,
}

/// `[models.<alias>]`, with every default applied and every rule checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub provider: String,
    pub wires: Vec<Wire>,
    pub context_window: u32,
    pub hf_model: String,
    pub image: String,
    pub gpu_types: Vec<String>,
    pub disk_gb: u32,
    pub vllm: VllmSettings,
    pub cache: Option<NetworkVolume>,
    pub data_center_ids: Vec<String>,
    pub idle_timeout_minutes: u64,
    pub start_wait_seconds: u64,
}

/// A loaded and validated deployment document.
#[derive(Debug, Clone)]
pub struct Deployment {
    pub gateway: GatewayConfig,
    pub owner_secret_file: PathBuf,
    pub providers: BTreeMap<String, Provider>,
    pub models: BTreeMap<String, Model>,
    /// SHA-256 of the exact document bytes read, as 64 lowercase hex characters.
    pub digest: String,
}

/// Reads `path` through the trusted-file reader and loads the closed document.
///
/// # Errors
/// A `config:*` [`Refusal`]: a trusted-file rule, `config:schema` for anything outside the
/// closed document, or `config:value` for a value outside its rule.
pub fn load(path: &Path) -> Result<Deployment, Refusal> {
    let text = trusted::read(path, &trusted::CONFIG)?;
    parse(&text)
}

fn schema_refusal(text: &str, error: &toml::de::Error) -> Refusal {
    let message = error.message();
    match error.span() {
        Some(span) => {
            // Counted over bytes: the span is a byte offset, which need not fall on a character
            // boundary of multi-byte text.
            let end = span.start.min(text.len());
            // n newlines split the bytes into n + 1 pieces, which is the 1-based line number.
            let line = text.as_bytes()[..end].split(|byte| *byte == b'\n').count();
            Refusal::new(
                StartupRefusal::ConfigSchema,
                format!("line {line}: {message}"),
            )
        }
        None => Refusal::new(StartupRefusal::ConfigSchema, message),
    }
}

fn parse(text: &str) -> Result<Deployment, Refusal> {
    let document: Document = toml::from_str(text).map_err(|error| schema_refusal(text, &error))?;
    let mut digest = String::with_capacity(64);
    for byte in Sha256::digest(text.as_bytes()) {
        digest.push(char::from(HEX[usize::from(byte >> 4)]));
        digest.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    if document.models.is_empty() {
        return Err(value("models", "at least one model is required"));
    }
    let mut providers = BTreeMap::new();
    for (name, provider) in document.providers {
        closed_name(&format!("providers.{name}"), &name)?;
        let cloud_type = match provider.cloud_type {
            CloudTypeDocument::Secure => CloudType::Secure,
            CloudTypeDocument::Community => CloudType::Community,
        };
        providers.insert(
            name,
            Provider {
                kind: provider.kind,
                cloud_type,
            },
        );
    }
    let mut models = BTreeMap::new();
    for (alias, model) in document.models {
        let at = format!("models.{alias}");
        closed_name(&at, &alias)?;
        if !providers.contains_key(&model.provider) {
            return Err(value(
                &format!("{at}.provider"),
                "must name a declared provider",
            ));
        }
        models.insert(alias, validate_model(&at, model)?);
    }
    Ok(Deployment {
        gateway: GatewayConfig::new(document.listen),
        owner_secret_file: document.owner_secret_file,
        providers,
        models,
        digest,
    })
}

fn value(key: &str, rule: &str) -> Refusal {
    Refusal::new(StartupRefusal::ConfigValue, format!("{key} {rule}"))
}

fn ensure(holds: bool, key: &str, rule: &str) -> Result<(), Refusal> {
    if holds { Ok(()) } else { Err(value(key, rule)) }
}

fn within<T: PartialOrd + std::fmt::Display>(
    at: &str,
    key: &str,
    actual: &T,
    range: &RangeInclusive<T>,
) -> Result<(), Refusal> {
    ensure(
        range.contains(actual),
        &format!("{at}.{key}"),
        &format!("must be within {}..={}", range.start(), range.end()),
    )
}

/// A provider name or model alias: 1..=64 of `a-z 0-9 - _ .` without a leading dash.
fn closed_name(key: &str, name: &str) -> Result<(), Refusal> {
    ensure(
        (1..=64).contains(&name.len())
            && !name.starts_with('-')
            && name.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.')
            }),
        key,
        "must be 1..=64 lowercase URL-safe ASCII characters without a leading dash",
    )
}

fn bounded_text(key: &str, value: &str, limit: usize) -> Result<(), Refusal> {
    ensure(
        (1..=limit).contains(&value.len())
            && value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)),
        key,
        &format!("must be 1..={limit} printable ASCII characters"),
    )
}

fn validate_model(at: &str, model: ModelDocument) -> Result<Model, Refusal> {
    check_serving(at, &model)?;
    check_vllm(at, &model)?;
    check_placement(at, &model)?;
    Ok(into_model(model))
}

/// What the gateway serves and how long it holds a request.
fn check_serving(at: &str, model: &ModelDocument) -> Result<(), Refusal> {
    let key = |name: &str| format!("{at}.{name}");
    ensure(!model.wires.is_empty(), &key("wires"), "must not be empty")?;
    let unique: BTreeSet<Wire> = model.wires.iter().copied().collect();
    ensure(
        unique.len() == model.wires.len(),
        &key("wires"),
        "must not repeat a wire",
    )?;
    within(
        at,
        "context_window",
        &model.context_window,
        &(4_096..=2_000_000),
    )?;
    within(
        at,
        "idle_timeout_minutes",
        &model.idle_timeout_minutes,
        &(0..=1_440),
    )?;
    within(
        at,
        "start_wait_seconds",
        &model.start_wait_seconds,
        &(10..=3_600),
    )
}

/// What reaches `vllm serve`.
fn check_vllm(at: &str, model: &ModelDocument) -> Result<(), Refusal> {
    let key = |name: &str| format!("{at}.{name}");
    within(
        at,
        "max_model_len",
        &model.max_model_len,
        &(1_024..=model.context_window),
    )?;
    within(at, "max_num_seqs", &model.max_num_seqs, &(1..=1_024))?;
    // A range check that is false for NaN as well.
    within(at, "gpu_util", &model.gpu_util, &(0.1..=1.0))?;
    bounded_text(&key("hf_model"), &model.hf_model, 200)?;
    if let Some(effort) = &model.reasoning_effort {
        ensure(
            REASONING_EFFORTS.contains(&effort.as_str()),
            &key("reasoning_effort"),
            "must be one of low, medium, high, xhigh",
        )?;
    }
    if let Some(sampling) = &model.sampling {
        let object = serde_json::from_str::<serde_json::Value>(sampling)
            .is_ok_and(|parsed| parsed.is_object());
        ensure(object, &key("sampling"), "must be a JSON object")?;
    }
    ensure(
        model.extra_vllm_args.len() <= 64,
        &key("extra_vllm_args"),
        "must hold at most 64 entries",
    )?;
    for argument in &model.extra_vllm_args {
        bounded_text(&key("extra_vllm_args"), argument, 256)?;
    }
    Ok(())
}

/// Where a pod is placed and what it mounts.
fn check_placement(at: &str, model: &ModelDocument) -> Result<(), Refusal> {
    let key = |name: &str| format!("{at}.{name}");
    within(at, "disk_gb", &model.disk_gb, &(10..=2_000))?;
    bounded_text(&key("image"), &model.image, 200)?;
    ensure(
        (1..=16).contains(&model.gpu_types.len()),
        &key("gpu_types"),
        "must name 1..=16 candidates",
    )?;
    for gpu in &model.gpu_types {
        bounded_text(&key("gpu_types"), gpu, 64)?;
    }
    if let Some(volume) = &model.network_volume_id {
        ensure(
            (1..=64).contains(&volume.len())
                && volume.bytes().all(|byte| byte.is_ascii_alphanumeric()),
            &key("network_volume_id"),
            "must be 1..=64 ASCII alphanumeric characters",
        )?;
        ensure(
            !model.data_center_ids.is_empty(),
            &key("network_volume_id"),
            "requires data_center_ids: the volume exists in one data center",
        )?;
    }
    let mount = &model.volume_mount_path;
    ensure(
        mount.starts_with('/')
            && !mount.ends_with('/')
            && mount.len() <= 128
            && mount
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_')),
        &key("volume_mount_path"),
        "must be an absolute path of at most 128 characters without a trailing slash",
    )?;
    ensure(
        model.data_center_ids.len() <= 16,
        &key("data_center_ids"),
        "must name at most 16 data centers",
    )?;
    for data_center in &model.data_center_ids {
        ensure(
            (2..=32).contains(&data_center.len())
                && data_center
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-'),
            &key("data_center_ids"),
            "entries must be 2..=32 of A-Z, 0-9 and -",
        )?;
    }
    Ok(())
}

/// The checked document, mapped onto the settings llm-gateway already has.
fn into_model(model: ModelDocument) -> Model {
    let cache = model.network_volume_id.map(|volume_id| NetworkVolume {
        volume_id,
        mount_path: model.volume_mount_path,
    });
    Model {
        provider: model.provider,
        wires: model.wires,
        context_window: model.context_window,
        hf_model: model.hf_model,
        image: model.image,
        gpu_types: model.gpu_types,
        disk_gb: model.disk_gb,
        vllm: VllmSettings {
            max_model_len: model.max_model_len,
            max_num_seqs: model.max_num_seqs,
            gpu_memory_utilization: model.gpu_util,
            thinking: match model.thinking {
                ThinkingDocument::On => Thinking::On,
                ThinkingDocument::Off => Thinking::Off,
            },
            reasoning_effort: model.reasoning_effort,
            sampling: model.sampling,
            extra_args: model.extra_vllm_args,
        },
        cache,
        data_center_ids: model.data_center_ids,
        idle_timeout_minutes: model.idle_timeout_minutes,
        start_wait_seconds: model.start_wait_seconds,
    }
}
