//! Adapter-local Runpod settings: GPU choices, mounted caches, startup deadlines and vLLM
//! settings.
//!
//! Ported from the `ModelConfig` part of llmgw `src/config.rs`. None of this is part of
//! `llm_provision::DeploymentSpec`: the hosting contract describes what any provider is asked
//! for, and these are the settings only a Runpod vLLM pod has.

/// Where a pod is placed. Runpod's own two words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudType {
    Secure,
    Community,
}

impl CloudType {
    /// The value Runpod's `cloudType` field takes.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Secure => "SECURE",
            Self::Community => "COMMUNITY",
        }
    }
}

/// Whether the model's chat template thinks by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thinking {
    On,
    Off,
}

/// A persistent weight cache.
///
/// A fresh pod otherwise downloads the checkpoint again on every cold start. A network volume
/// lives in one data center, so a model that mounts one should also pin
/// [`RunpodModel::data_center_ids`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkVolume {
    pub volume_id: String,
    pub mount_path: String,
}

/// The vLLM serve settings for one model.
#[derive(Debug, Clone, PartialEq)]
pub struct VllmSettings {
    pub max_model_len: u32,
    pub max_num_seqs: u32,
    /// Fraction of GPU memory vLLM may use, in `(0, 1]`.
    pub gpu_memory_utilization: f64,
    pub thinking: Thinking,
    pub reasoning_effort: Option<String>,
    /// A JSON document for `--override-generation-config`, passed through verbatim. When absent
    /// and thinking is off, the model card's non-thinking sampling defaults are applied.
    pub sampling: Option<String>,
    /// Model-specific arguments, appended after every argument this adapter writes.
    pub extra_args: Vec<String>,
}

/// One fallback profile of a model: the GPU types it is tried on and the vLLM settings that fit
/// them. A model's own [`RunpodModel::gpu_types`] and [`RunpodModel::vllm`] are its first
/// profile, named [`RunpodModel::profile`]; these follow it, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct RunpodProfile {
    /// `1..=32` characters of `A-Za-z0-9-_`, unique within the model.
    pub name: String,
    /// Tried one at a time, in this order. A GPU type belongs to at most one profile of a model.
    pub gpu_types: Vec<String>,
    pub vllm: VllmSettings,
}

/// Everything this adapter needs to run one model on Runpod.
#[derive(Debug, Clone, PartialEq)]
pub struct RunpodModel {
    /// The checkpoint `vllm serve` loads.
    pub hf_model: String,
    pub image: llm_provision::Identifier,
    /// Tried one at a time, in this order: Runpod placement fails per host.
    pub gpu_types: Vec<String>,
    pub cloud_type: CloudType,
    pub container_disk_gb: u32,
    pub cache: Option<NetworkVolume>,
    pub data_center_ids: Vec<String>,
    pub vllm: VllmSettings,
    /// The **name** of a Runpod secret holding the vLLM API key. Never the key itself: the pod
    /// receives it through Runpod's `{{ RUNPOD_SECRET_<name> }}` template, so no credential value
    /// passes through this process.
    pub api_key_secret: String,
    /// How long a pod may take to serve before it is terminated. Inclusive.
    pub startup_deadline_ms: u64,
    /// How long a ready pod may sit without traffic before it is terminated.
    pub idle_timeout_ms: u64,
    /// Container restarts observed during polling that make a pod crash-looping.
    pub crash_restart_limit: u32,
    /// The window restarts are counted in. A restart older than this no longer counts, so a
    /// pod that restarted twice over a long life is not a crash loop.
    pub crash_window_ms: u64,
    /// The name of the first profile, the one `gpu_types` and `vllm` declare. A model declared
    /// without profiles is the one profile `default`.
    pub profile: String,
    /// The profiles tried after the first, in order, when every create of the one before was
    /// refused for capacity.
    pub fallbacks: Vec<RunpodProfile>,
}

/// Why a model declaration was refused. The message never quotes the refused value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    /// No GPU type is declared, so nothing could ever be placed.
    NoGpu,
    /// The GPU memory fraction is not a finite value in `(0, 1]`.
    GpuUtilization,
    /// A count, a limit or a deadline is zero.
    ZeroSetting,
    /// The secret name is not usable inside a Runpod secret template.
    SecretReference,
    /// A value that reaches the pod's command line or environment is empty or not printable.
    UnsafeText,
    /// Two profiles of one model share a name or a GPU type.
    ProfileConflict,
}

impl ConfigError {
    /// Every refusal this type can carry.
    pub const ALL: &'static [Self] = &[
        Self::NoGpu,
        Self::GpuUtilization,
        Self::ZeroSetting,
        Self::SecretReference,
        Self::UnsafeText,
        Self::ProfileConflict,
    ];

    /// The stable kebab-case code.
    pub const fn code(self) -> &'static str {
        match self {
            Self::NoGpu => "no-gpu",
            Self::GpuUtilization => "gpu-utilization",
            Self::ZeroSetting => "zero-setting",
            Self::SecretReference => "secret-reference",
            Self::UnsafeText => "unsafe-text",
            Self::ProfileConflict => "profile-conflict",
        }
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NoGpu => "a Runpod model declares no GPU type",
            Self::GpuUtilization => "GPU memory utilization must be in (0, 1]",
            Self::ZeroSetting => "a Runpod model setting must not be zero",
            Self::SecretReference => "the vLLM key secret name must be [A-Za-z0-9_]+",
            Self::UnsafeText => "a Runpod model value is empty or not printable",
            Self::ProfileConflict => "two profiles of a Runpod model share a name or a GPU type",
        })
    }
}

impl std::error::Error for ConfigError {}

/// Printable ASCII, no whitespace: safe as one argv element or one environment value.
fn token(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_graphic())
}

/// Printable ASCII, spaces allowed, no control characters.
fn text(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte == b' ' || byte.is_ascii_graphic())
}

/// `1..=32` characters of `A-Za-z0-9-_`.
fn profile_name(value: &str) -> bool {
    (1..=32).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// The checks one profile's GPU types and vLLM settings answer, in [`RunpodModel::validate`]'s
/// order.
fn check_profile(gpu_types: &[String], vllm: &VllmSettings) -> Result<(), ConfigError> {
    if gpu_types.is_empty() {
        return Err(ConfigError::NoGpu);
    }
    let utilization = vllm.gpu_memory_utilization;
    if !utilization.is_finite() || utilization <= 0.0 || utilization > 1.0 {
        return Err(ConfigError::GpuUtilization);
    }
    if vllm.max_model_len == 0 || vllm.max_num_seqs == 0 {
        return Err(ConfigError::ZeroSetting);
    }
    Ok(())
}

fn profile_text_ok(gpu_types: &[String], vllm: &VllmSettings) -> bool {
    gpu_types.iter().all(|gpu| text(gpu))
        && vllm.extra_args.iter().all(|arg| text(arg))
        && vllm.sampling.as_deref().is_none_or(text)
        && vllm.reasoning_effort.as_deref().is_none_or(|effort| {
            effort.bytes().all(|byte| byte.is_ascii_alphanumeric()) && !effort.is_empty()
        })
}

impl RunpodModel {
    /// This model with the profile at `index` in place of its first: index 0 is the model
    /// itself, index `n` its `n`th fallback. `None` past the last profile.
    #[must_use]
    pub fn with_profile(&self, index: usize) -> Option<(String, Self)> {
        if index == 0 {
            return Some((self.profile.clone(), self.clone()));
        }
        let fallback = self.fallbacks.get(index - 1)?;
        let mut model = self.clone();
        model.gpu_types.clone_from(&fallback.gpu_types);
        model.vllm = fallback.vllm.clone();
        Some((fallback.name.clone(), model))
    }

    /// The name of the profile that declares `gpu_type`, if any does.
    #[must_use]
    pub fn profile_of_gpu(&self, gpu_type: &str) -> Option<&str> {
        if self.gpu_types.iter().any(|gpu| gpu == gpu_type) {
            return Some(&self.profile);
        }
        self.fallbacks
            .iter()
            .find(|fallback| fallback.gpu_types.iter().any(|gpu| gpu == gpu_type))
            .map(|fallback| fallback.name.as_str())
    }

    /// Checks every setting a pod could not be started or bounded without.
    ///
    /// # Errors
    ///
    /// Returns the first [`ConfigError`] that applies.
    pub fn validate(&self) -> Result<(), ConfigError> {
        check_profile(&self.gpu_types, &self.vllm)?;
        for fallback in &self.fallbacks {
            check_profile(&fallback.gpu_types, &fallback.vllm)?;
        }
        if self.container_disk_gb == 0
            || self.startup_deadline_ms == 0
            || self.idle_timeout_ms == 0
            || self.crash_restart_limit == 0
            || self.crash_window_ms == 0
        {
            return Err(ConfigError::ZeroSetting);
        }
        if self.api_key_secret.is_empty()
            || !self
                .api_key_secret
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(ConfigError::SecretReference);
        }
        let tokens_ok = token(&self.hf_model)
            && profile_name(&self.profile)
            && profile_text_ok(&self.gpu_types, &self.vllm)
            && self.fallbacks.iter().all(|fallback| {
                profile_name(&fallback.name) && profile_text_ok(&fallback.gpu_types, &fallback.vllm)
            })
            && self.data_center_ids.iter().all(|dc| token(dc))
            && self
                .cache
                .as_ref()
                .is_none_or(|cache| token(&cache.volume_id) && token(&cache.mount_path));
        if !tokens_ok {
            return Err(ConfigError::UnsafeText);
        }
        let mut names = std::collections::BTreeSet::from([self.profile.as_str()]);
        let mut gpus: std::collections::BTreeSet<&str> =
            self.gpu_types.iter().map(String::as_str).collect();
        for fallback in &self.fallbacks {
            if !names.insert(fallback.name.as_str()) {
                return Err(ConfigError::ProfileConflict);
            }
            let own: std::collections::BTreeSet<&str> =
                fallback.gpu_types.iter().map(String::as_str).collect();
            if !gpus.is_disjoint(&own) {
                return Err(ConfigError::ProfileConflict);
            }
            gpus.extend(own);
        }
        Ok(())
    }
}
