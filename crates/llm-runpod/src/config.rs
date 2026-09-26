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
}

impl ConfigError {
    /// Every refusal this type can carry.
    pub const ALL: &'static [Self] = &[
        Self::NoGpu,
        Self::GpuUtilization,
        Self::ZeroSetting,
        Self::SecretReference,
        Self::UnsafeText,
    ];

    /// The stable kebab-case code.
    pub const fn code(self) -> &'static str {
        match self {
            Self::NoGpu => "no-gpu",
            Self::GpuUtilization => "gpu-utilization",
            Self::ZeroSetting => "zero-setting",
            Self::SecretReference => "secret-reference",
            Self::UnsafeText => "unsafe-text",
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

impl RunpodModel {
    /// Checks every setting a pod could not be started or bounded without.
    ///
    /// # Errors
    ///
    /// Returns the first [`ConfigError`] that applies.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.gpu_types.is_empty() {
            return Err(ConfigError::NoGpu);
        }
        let utilization = self.vllm.gpu_memory_utilization;
        if !utilization.is_finite() || utilization <= 0.0 || utilization > 1.0 {
            return Err(ConfigError::GpuUtilization);
        }
        if self.vllm.max_model_len == 0
            || self.vllm.max_num_seqs == 0
            || self.container_disk_gb == 0
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
        let effort_ok = self.vllm.reasoning_effort.as_deref().is_none_or(|effort| {
            effort.bytes().all(|byte| byte.is_ascii_alphanumeric()) && !effort.is_empty()
        });
        let tokens_ok = token(&self.hf_model)
            && self.gpu_types.iter().all(|gpu| text(gpu))
            && self.data_center_ids.iter().all(|dc| token(dc))
            && self.vllm.extra_args.iter().all(|arg| text(arg))
            && self.vllm.sampling.as_deref().is_none_or(text)
            && self
                .cache
                .as_ref()
                .is_none_or(|cache| token(&cache.volume_id) && token(&cache.mount_path));
        if !effort_ok || !tokens_ok {
            return Err(ConfigError::UnsafeText);
        }
        Ok(())
    }
}
