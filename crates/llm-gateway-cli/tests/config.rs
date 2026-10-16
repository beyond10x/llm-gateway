//! Row K29 through the loader the binary uses: the closed document's defaults, every range
//! boundary on both sides, and every key outside the closed document. The defaults and ranges
//! are llmgw's (`src/config.rs:138-160` and `src/config.rs:221-343` at 048ebd8).
//!
//! The loaded settings map onto the types llm-gateway already has: `listen` onto
//! `GatewayConfig::bind` (K1), `cloud_type` onto `llm_runpod::CloudType` (K8), the vLLM keys onto
//! `llm_runpod::VllmSettings` (K16-K22) and the volume keys onto `llm_runpod::NetworkVolume`
//! (K24-K25).

mod support;

use llm_gateway::{AuthKind, BillingKind, CredentialHeader, ReasoningEffort, ToolCalling};
use llm_gateway_cli::{Wire, load, vllm_keys};
use llm_runpod::{CloudType, ConnectorsBinding, NetworkVolume, Thinking};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};
use support::{
    ENDPOINT_CREDENTIAL, Fixture, VLLM_KEY, api_key_keys, bearer_keys, connectors_table, document,
    hosted_document, hosted_table, mutate, with_connectors, with_vllm_key,
};

fn base(fixture: &Fixture) -> String {
    document("127.0.0.1:0", &fixture.owner_secret())
}

/// Appends `lines` to the `[models.small]` table, which is the document's last table.
fn with_model_keys(text: &str, lines: &str) -> String {
    mutate(
        text,
        "max_model_len = 1024\n",
        &format!("max_model_len = 1024\n{lines}"),
    )
}

#[test]
fn k29_the_base_document_is_accepted() {
    // The control every refusal below is measured against.
    let fixture = Fixture::new("k29-base");
    let config = fixture.config(&base(&fixture));
    let deployment = load(&config).unwrap();
    assert_eq!(
        deployment.gateway.bind,
        "127.0.0.1:0".parse::<SocketAddr>().unwrap()
    );
    assert_eq!(deployment.owner_secret_file, fixture.path("owner-secret"));
    assert_eq!(deployment.models.len(), 1);
    assert_eq!(deployment.models["small"].provider, "runpod");
    assert_eq!(deployment.models["small"].context_window, 65536);
    assert_eq!(deployment.models["small"].vllm.max_model_len, 1024);
}

#[test]
fn k29_the_llmgw_defaults_apply() {
    let fixture = Fixture::new("k29-defaults");
    let deployment = load(&fixture.config(&base(&fixture))).unwrap();
    assert_eq!(deployment.providers["runpod"].cloud_type, CloudType::Secure);
    let model = &deployment.models["small"];
    assert_eq!(model.vllm.max_num_seqs, 8);
    assert!((model.vllm.gpu_memory_utilization - 0.90).abs() < f64::EPSILON);
    assert_eq!(model.vllm.thinking, Thinking::Off);
    assert_eq!(model.vllm.reasoning_effort, None);
    assert_eq!(model.vllm.sampling, None);
    assert_eq!(model.vllm.extra_args, Vec::<String>::new());
    assert_eq!(model.disk_gb, 80);
    assert_eq!(model.cache, None);
    assert_eq!(model.data_center_ids, Vec::<String>::new());
    assert_eq!(model.idle_timeout_minutes, 30);
    assert_eq!(model.start_wait_seconds, 600);
}

#[test]
fn k29_explicit_values_replace_the_defaults() {
    // Proves the defaults are defaults, not constants: every defaulted key set to another value.
    let fixture = Fixture::new("k29-explicit");
    let text = mutate(
        &base(&fixture),
        "kind = \"runpod-vllm\"\n",
        "kind = \"runpod-vllm\"\ncloud_type = \"COMMUNITY\"\n",
    );
    let text = with_model_keys(
        &text,
        "max_num_seqs = 16\ngpu_util = 0.5\ndisk_gb = 100\nthinking = \"on\"\n\
         reasoning_effort = \"xhigh\"\nsampling = \"{\\\"temperature\\\": 0.6}\"\n\
         extra_vllm_args = [\"--kv-cache-dtype\", \"fp8\"]\nnetwork_volume_id = \"vol123\"\n\
         volume_mount_path = \"/models\"\ndata_center_ids = [\"EU-RO-1\"]\n\
         idle_timeout_minutes = 0\nstart_wait_seconds = 10\n",
    );
    let deployment = load(&fixture.config(&text)).unwrap();
    assert_eq!(
        deployment.providers["runpod"].cloud_type,
        CloudType::Community
    );
    let model = &deployment.models["small"];
    assert_eq!(model.vllm.max_num_seqs, 16);
    assert!((model.vllm.gpu_memory_utilization - 0.5).abs() < f64::EPSILON);
    assert_eq!(model.vllm.thinking, Thinking::On);
    assert_eq!(model.vllm.reasoning_effort.as_deref(), Some("xhigh"));
    assert_eq!(
        model.vllm.sampling.as_deref(),
        Some("{\"temperature\": 0.6}")
    );
    assert_eq!(model.vllm.extra_args, ["--kv-cache-dtype", "fp8"]);
    assert_eq!(model.disk_gb, 100);
    assert_eq!(
        model.cache,
        Some(NetworkVolume {
            volume_id: "vol123".to_string(),
            mount_path: "/models".to_string(),
        })
    );
    assert_eq!(model.data_center_ids, ["EU-RO-1"]);
    assert_eq!(model.idle_timeout_minutes, 0);
    assert_eq!(model.start_wait_seconds, 10);
}

#[test]
fn k29_a_network_volume_mounts_at_workspace_by_default() {
    let fixture = Fixture::new("k29-volume");
    let text = with_model_keys(
        &base(&fixture),
        "network_volume_id = \"vol123\"\ndata_center_ids = [\"EU-RO-1\"]\n",
    );
    let deployment = load(&fixture.config(&text)).unwrap();
    assert_eq!(
        deployment.models["small"].cache,
        Some(NetworkVolume {
            volume_id: "vol123".to_string(),
            mount_path: "/workspace".to_string(),
        })
    );
}

/// `(case, find, replace)`: a mutation of the base document.
type Case = (&'static str, &'static str, &'static str);

/// Every range edge llmgw accepts, each on its own.
const BOUNDARIES: &[Case] = &[
    (
        "context_window at 4096",
        "context_window = 65536",
        "context_window = 4096",
    ),
    (
        "context_window at 2000000",
        "context_window = 65536",
        "context_window = 2000000",
    ),
    (
        "max_model_len at context_window",
        "max_model_len = 1024\n",
        "max_model_len = 65536\n",
    ),
    (
        "max_num_seqs at 1",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nmax_num_seqs = 1\n",
    ),
    (
        "max_num_seqs at 1024",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nmax_num_seqs = 1024\n",
    ),
    (
        "gpu_util at 0.1",
        "max_model_len = 1024\n",
        "max_model_len = 1024\ngpu_util = 0.1\n",
    ),
    (
        "gpu_util at 1.0",
        "max_model_len = 1024\n",
        "max_model_len = 1024\ngpu_util = 1.0\n",
    ),
    (
        "disk_gb at 10",
        "max_model_len = 1024\n",
        "max_model_len = 1024\ndisk_gb = 10\n",
    ),
    (
        "disk_gb at 2000",
        "max_model_len = 1024\n",
        "max_model_len = 1024\ndisk_gb = 2000\n",
    ),
    (
        "idle_timeout_minutes at 1440",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nidle_timeout_minutes = 1440\n",
    ),
    (
        "start_wait_seconds at 3600",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nstart_wait_seconds = 3600\n",
    ),
    (
        "a 64-byte alias",
        "[models.small]",
        "[models.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa]",
    ),
    (
        "sixteen gpu_types",
        "gpu_types = [\"NVIDIA L40S\"]",
        "gpu_types = [\"a\",\"b\",\"c\",\"d\",\"e\",\"f\",\"g\",\"h\",\"i\",\"j\",\"k\",\"l\",\"m\",\"n\",\"o\",\"p\"]",
    ),
];

#[test]
fn k29_every_range_boundary_is_accepted() {
    let fixture = Fixture::new("k29-boundaries");
    let text = base(&fixture);
    for (case, find, replace) in BOUNDARIES {
        let config = fixture.config(&mutate(&text, find, replace));
        if let Err(refusal) = load(&config) {
            panic!("{case}: refused as {}", refusal.code());
        }
    }
}

/// Each breaks exactly one llmgw rule; the base document breaks none.
const OUT_OF_RULE: &[Case] = &[
    (
        "context_window under 4096",
        "context_window = 65536",
        "context_window = 4095",
    ),
    (
        "context_window over 2000000",
        "context_window = 65536",
        "context_window = 2000001",
    ),
    (
        "max_model_len under 1024",
        "max_model_len = 1024\n",
        "max_model_len = 1023\n",
    ),
    (
        "max_model_len over context_window",
        "max_model_len = 1024\n",
        "max_model_len = 65537\n",
    ),
    (
        "max_num_seqs 0",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nmax_num_seqs = 0\n",
    ),
    (
        "max_num_seqs over 1024",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nmax_num_seqs = 1025\n",
    ),
    (
        "gpu_util under 0.1",
        "max_model_len = 1024\n",
        "max_model_len = 1024\ngpu_util = 0.09\n",
    ),
    (
        "gpu_util over 1.0",
        "max_model_len = 1024\n",
        "max_model_len = 1024\ngpu_util = 1.01\n",
    ),
    (
        "disk_gb under 10",
        "max_model_len = 1024\n",
        "max_model_len = 1024\ndisk_gb = 9\n",
    ),
    (
        "disk_gb over 2000",
        "max_model_len = 1024\n",
        "max_model_len = 1024\ndisk_gb = 2001\n",
    ),
    (
        "idle_timeout_minutes over 1440",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nidle_timeout_minutes = 1441\n",
    ),
    (
        "start_wait_seconds under 10",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nstart_wait_seconds = 9\n",
    ),
    (
        "start_wait_seconds over 3600",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nstart_wait_seconds = 3601\n",
    ),
    ("no wire", "wires = [\"chat\", \"responses\"]", "wires = []"),
    (
        "a repeated wire",
        "wires = [\"chat\", \"responses\"]",
        "wires = [\"chat\", \"chat\"]",
    ),
    (
        "no gpu type",
        "gpu_types = [\"NVIDIA L40S\"]",
        "gpu_types = []",
    ),
    (
        "seventeen gpu types",
        "gpu_types = [\"NVIDIA L40S\"]",
        "gpu_types = [\"a\",\"b\",\"c\",\"d\",\"e\",\"f\",\"g\",\"h\",\"i\",\"j\",\"k\",\"l\",\"m\",\"n\",\"o\",\"p\",\"q\"]",
    ),
    (
        "an empty hf_model",
        "hf_model = \"example/small-model\"",
        "hf_model = \"\"",
    ),
    (
        "an unprintable image",
        "image = \"vllm/vllm-openai:v0.27.1\"",
        "image = \"vllm\\u0007\"",
    ),
    (
        "an unknown reasoning effort",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nreasoning_effort = \"extreme\"\n",
    ),
    (
        "sampling that is not an object",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nsampling = \"[1,2]\"\n",
    ),
    (
        "sampling that is not JSON",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nsampling = \"{\"\n",
    ),
    (
        "a network volume without a data center",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nnetwork_volume_id = \"vol123\"\n",
    ),
    (
        "a non-alphanumeric network volume",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nnetwork_volume_id = \"vol-123\"\ndata_center_ids = [\"EU-RO-1\"]\n",
    ),
    (
        "a lowercase data center",
        "max_model_len = 1024\n",
        "max_model_len = 1024\ndata_center_ids = [\"eu-ro-1\"]\n",
    ),
    (
        "a relative volume mount path",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nvolume_mount_path = \"workspace\"\n",
    ),
    (
        "a volume mount path with a trailing slash",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nvolume_mount_path = \"/workspace/\"\n",
    ),
    (
        "a model naming an undeclared provider",
        "provider = \"runpod\"",
        "provider = \"missing\"",
    ),
    (
        "a slashed model alias",
        "[models.small]",
        "[models.\"small/one\"]",
    ),
    (
        "a 65-byte model alias",
        "[models.small]",
        "[models.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa]",
    ),
    (
        "an upper-case provider name",
        "[providers.runpod]",
        "[providers.Runpod]",
    ),
];

#[test]
fn k29_every_value_outside_its_rule_is_refused() {
    let fixture = Fixture::new("k29-out-of-rule");
    let text = base(&fixture);
    assert!(
        load(&fixture.config(&text)).is_ok(),
        "the control is refused"
    );
    for (case, find, replace) in OUT_OF_RULE {
        let config = fixture.config(&mutate(&text, find, replace));
        match load(&config) {
            Ok(_) => panic!("{case}: accepted"),
            Err(refusal) => assert_eq!(refusal.code(), "config:value", "{case}"),
        }
    }
}

#[test]
fn k29_a_document_without_models_is_refused_as_a_value() {
    let fixture = Fixture::new("k29-no-models");
    let secret = fixture.owner_secret();
    let text = format!(
        "listen = \"127.0.0.1:0\"\nowner_secret_file = \"{}\"\n",
        secret.display()
    );
    let refusal = load(&fixture.config(&text)).unwrap_err();
    assert_eq!(refusal.code(), "config:value");
}

/// Each leaves the closed document: an unknown key at every level, a section llmgw had and this
/// document does not, a missing required key, a value of the wrong type, and a closed-set value
/// outside its set.
const OUTSIDE_THE_SCHEMA: &[Case] = &[
    (
        "an unknown top-level key",
        "listen = ",
        "listn = \"127.0.0.1:0\"\nlisten = ",
    ),
    (
        "an unknown provider key",
        "kind = \"runpod-vllm\"\n",
        "kind = \"runpod-vllm\"\nregion = \"eu\"\n",
    ),
    (
        "an unknown model key",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nstart_wait_secnds = 600\n",
    ),
    (
        "an [identity] section",
        "[providers.runpod]",
        "[identity]\norigin = \"https://identity.example\"\ntenant = \"t\"\n\n[providers.runpod]",
    ),
    (
        // Row K7 under design choice D1 = A (docs/design/runpod-clients.md § 4): the Runpod API
        // key is held by connectors, and the gateway never holds it.
        "a runpod_api_key_file, which D1 = A leaves to connectors",
        "kind = \"runpod-vllm\"\n",
        "kind = \"runpod-vllm\"\nrunpod_api_key_file = \"/run/key\"\n",
    ),
    ("no listen", "listen = \"127.0.0.1:0\"\n", ""),
    (
        "a listen that is not an address",
        "listen = \"127.0.0.1:0\"",
        "listen = \"localhost\"",
    ),
    (
        "no owner_secret_file",
        "owner_secret_file = ",
        "unused_secret_file = ",
    ),
    (
        "a string context_window",
        "context_window = 65536",
        "context_window = \"big\"",
    ),
    (
        "a negative context_window",
        "context_window = 65536",
        "context_window = -1",
    ),
    (
        "an unknown provider kind",
        "kind = \"runpod-vllm\"",
        "kind = \"modal\"",
    ),
    (
        "an unknown cloud type",
        "kind = \"runpod-vllm\"\n",
        "kind = \"runpod-vllm\"\ncloud_type = \"SPOT\"\n",
    ),
    ("an unknown wire", "\"responses\"", "\"grpc\""),
    (
        "an unknown thinking value",
        "max_model_len = 1024\n",
        "max_model_len = 1024\nthinking = \"maybe\"\n",
    ),
    ("not TOML", "[models.small]", "[models.small"),
];

// --- B9: each model's vLLM key, read from a trusted file -------------------------------------

#[test]
fn b9_the_vllm_api_key_file_is_optional() {
    let fixture = Fixture::new("b9-optional");
    let without = load(&fixture.config(&base(&fixture))).unwrap();
    assert_eq!(without.models["small"].vllm_api_key_file, None);
    assert!(vllm_keys(&without).unwrap().is_empty());

    let key = fixture.vllm_key();
    let with = load(&fixture.config(&with_vllm_key(&base(&fixture), &key))).unwrap();
    assert_eq!(with.models["small"].vllm_api_key_file, Some(key));
}

#[test]
fn b9_the_binary_holds_each_models_vllm_key_and_debug_prints_none() {
    let fixture = Fixture::new("b9-held");
    let small = fixture.vllm_key();
    let large_key = "large-model-test-token-not-a-secret";
    let large = fixture.write(
        "vllm-key-large",
        format!("{large_key}\r\n").as_bytes(),
        0o600,
    );
    let text = with_vllm_key(&base(&fixture), &small);
    let text = format!(
        "{text}\n[models.large]\nprovider = \"runpod\"\nwires = [\"chat\"]\n\
         context_window = 65536\nhf_model = \"example/large-model\"\n\
         image = \"vllm/vllm-openai:v0.27.1\"\ngpu_types = [\"NVIDIA L40S\"]\n\
         max_model_len = 1024\nvllm_api_key_file = \"{}\"\n\
         \n[models.keyless]\nprovider = \"runpod\"\nwires = [\"chat\"]\n\
         context_window = 65536\nhf_model = \"example/keyless-model\"\n\
         image = \"vllm/vllm-openai:v0.27.1\"\ngpu_types = [\"NVIDIA L40S\"]\n\
         max_model_len = 1024\n",
        large.display()
    );
    let deployment = load(&fixture.config(&text)).unwrap();
    let keys = vllm_keys(&deployment).unwrap();

    // The value read, with the trailing newline or CRLF trimmed; a model without a key has none.
    assert_eq!(keys.len(), 2);
    assert_eq!(keys.get("small").unwrap().expose(), VLLM_KEY.as_bytes());
    assert_eq!(keys.get("large").unwrap().expose(), large_key.as_bytes());
    assert!(keys.get("keyless").is_none());

    // Neither the map nor one key prints its value.
    let printed = [
        format!("{keys:?}"),
        format!("{keys:#?}"),
        format!("{:?}", keys.get("small").unwrap()),
    ];
    for text in &printed {
        for value in [VLLM_KEY, large_key] {
            assert!(!text.contains(value), "Debug prints a key: {text}");
        }
    }
    assert!(printed[0].contains("small"), "{}", printed[0]);
    assert!(printed[0].contains("REDACTED"), "{}", printed[0]);
}

#[test]
fn k29_every_key_outside_the_closed_document_is_refused() {
    let fixture = Fixture::new("k29-schema");
    let text = base(&fixture);
    assert!(
        load(&fixture.config(&text)).is_ok(),
        "the control is refused"
    );
    for (case, find, replace) in OUTSIDE_THE_SCHEMA {
        let config = fixture.config(&mutate(&text, find, replace));
        match load(&config) {
            Ok(_) => panic!("{case}: accepted"),
            Err(refusal) => assert_eq!(refusal.code(), "config:schema", "{case}"),
        }
    }
}

// --- Tool calling: each model's own declaration, default `absent` ----------------------------

#[test]
fn tool_calling_a_document_that_omits_it_parses_and_the_model_takes_absent() {
    let fixture = Fixture::new("tools-default");
    let deployment = load(&fixture.config(&base(&fixture))).unwrap();
    assert_eq!(deployment.models["small"].tool_calling, ToolCalling::Absent);
}

#[test]
fn tool_calling_parsed_and_absent_are_read_per_model() {
    let fixture = Fixture::new("tools-declared");
    let text = with_model_keys(&base(&fixture), "tool_calling = \"parsed\"\n");
    let text = format!(
        "{text}\n[models.text]\nprovider = \"runpod\"\nwires = [\"chat\"]\n\
         context_window = 65536\nhf_model = \"example/text-model\"\n\
         image = \"vllm/vllm-openai:v0.27.1\"\ngpu_types = [\"NVIDIA L40S\"]\n\
         max_model_len = 1024\ntool_calling = \"absent\"\n"
    );
    let deployment = load(&fixture.config(&text)).unwrap();
    assert_eq!(deployment.models["small"].tool_calling, ToolCalling::Parsed);
    assert_eq!(deployment.models["text"].tool_calling, ToolCalling::Absent);
}

#[test]
fn tool_calling_any_other_value_is_refused_as_schema() {
    let fixture = Fixture::new("tools-schema");
    for value in ["\"Parsed\"", "\"auto\"", "\"\"", "true"] {
        let text = with_model_keys(&base(&fixture), &format!("tool_calling = {value}\n"));
        match load(&fixture.config(&text)) {
            Ok(_) => panic!("tool_calling = {value}: accepted"),
            Err(refusal) => assert_eq!(refusal.code(), "config:schema", "{value}"),
        }
    }
}

// --- Every closed value is a TOML string, never a one-key inline table naming a variant ------

/// serde reads an externally tagged unit variant from a one-key table as well as from a string,
/// so `{ on = {} }` loaded as `on`. Each enumerated key of the document is refused that way.
#[test]
fn k29_an_enumerated_value_written_as_an_inline_table_is_refused_as_schema() {
    let fixture = Fixture::new("k29-inline-table");
    let text = base(&fixture);
    let mut accepted = Vec::new();
    for (case, find, replace) in [
        (
            "kind",
            "kind = \"runpod-vllm\"",
            "kind = { runpod-vllm = {} }",
        ),
        (
            "cloud_type",
            "kind = \"runpod-vllm\"",
            "kind = \"runpod-vllm\"\ncloud_type = { COMMUNITY = {} }",
        ),
        (
            "wires",
            "wires = [\"chat\", \"responses\"]",
            "wires = [\"chat\", { responses = {} }]",
        ),
        (
            "thinking",
            "max_model_len = 1024\n",
            "max_model_len = 1024\nthinking = { on = {} }\n",
        ),
        (
            "tool_calling",
            "max_model_len = 1024\n",
            "max_model_len = 1024\ntool_calling = { parsed = {} }\n",
        ),
    ] {
        match load(&fixture.config(&mutate(&text, find, replace))) {
            Ok(_) => accepted.push(case),
            Err(refusal) => assert_eq!(refusal.code(), "config:schema", "{case}"),
        }
    }
    assert!(
        accepted.is_empty(),
        "accepted as inline tables: {accepted:?}"
    );
}

// --- The provider's connectors connection (story:live-runpod-wiring) --------------------------

const EXECUTABLE: &str = "/opt/connectors/bin/connectors";
const WORK: &str = "/var/lib/gateway/connectors";

fn connected(fixture: &Fixture, lines: &str) -> String {
    with_connectors(&base(fixture), lines)
}

fn table() -> String {
    connectors_table(Path::new(EXECUTABLE), Path::new(WORK))
}

#[test]
fn connectors_a_provider_without_the_table_names_no_connection() {
    let fixture = Fixture::new("connectors-absent");
    let deployment = load(&fixture.config(&base(&fixture))).unwrap();
    assert_eq!(deployment.providers["runpod"].connectors, None);
}

#[test]
fn connectors_the_table_maps_onto_the_transport_binding_with_a_60_second_default() {
    let fixture = Fixture::new("connectors-binding");
    let deployment = load(&fixture.config(&connected(&fixture, &table()))).unwrap();
    assert_eq!(
        deployment.providers["runpod"].connectors,
        Some(ConnectorsBinding {
            executable: PathBuf::from(EXECUTABLE),
            adapter: "gpu".to_owned(),
            connection: "conn-1".to_owned(),
            work_directory: PathBuf::from(WORK),
            timeout: Duration::from_secs(60),
        })
    );
    let explicit = format!("{}timeout_seconds = 5\n", table());
    let deployment = load(&fixture.config(&connected(&fixture, &explicit))).unwrap();
    assert_eq!(
        deployment.providers["runpod"]
            .connectors
            .as_ref()
            .map(|binding| binding.timeout),
        Some(Duration::from_secs(5))
    );
}

#[test]
fn connectors_every_range_boundary_is_accepted() {
    let fixture = Fixture::new("connectors-bounds");
    let long_path = format!("/{}", "p".repeat(1023));
    let long_name = "a".repeat(128);
    for (case, lines) in [
        ("timeout 1", format!("{}timeout_seconds = 1\n", table())),
        ("timeout 600", format!("{}timeout_seconds = 600\n", table())),
        (
            "a 1024-byte executable",
            connectors_table(Path::new(&long_path), Path::new(WORK)),
        ),
        (
            "a 1024-byte work directory",
            connectors_table(Path::new(EXECUTABLE), Path::new(&long_path)),
        ),
        (
            "a 128-character adapter and connection",
            format!(
                "executable = \"{EXECUTABLE}\"\nadapter = \"{long_name}\"\n\
                 connection = \"{long_name}\"\nwork_directory = \"{WORK}\"\n"
            ),
        ),
        (
            "an adapter and connection of every printable class",
            format!(
                "executable = \"{EXECUTABLE}\"\nadapter = \"a-B_9.x:y\"\n\
                 connection = \"conn/1@2\"\nwork_directory = \"{WORK}\"\n"
            ),
        ),
    ] {
        if let Err(refusal) = load(&fixture.config(&connected(&fixture, &lines))) {
            panic!("{case}: refused {refusal}");
        }
    }
}

#[test]
fn connectors_every_value_outside_its_rule_is_refused() {
    let fixture = Fixture::new("connectors-out-of-rule");
    let long_path = format!("/{}", "p".repeat(1024));
    let long_name = "a".repeat(129);
    let named = |adapter: &str, connection: &str| {
        format!(
            "executable = \"{EXECUTABLE}\"\nadapter = \"{adapter}\"\n\
             connection = \"{connection}\"\nwork_directory = \"{WORK}\"\n"
        )
    };
    let cases = [
        ("timeout 0", format!("{}timeout_seconds = 0\n", table())),
        ("timeout 601", format!("{}timeout_seconds = 601\n", table())),
        (
            "a relative executable",
            connectors_table(Path::new("connectors"), Path::new(WORK)),
        ),
        (
            "a relative work directory",
            connectors_table(Path::new(EXECUTABLE), Path::new("work")),
        ),
        (
            "a 1025-byte executable",
            connectors_table(Path::new(&long_path), Path::new(WORK)),
        ),
        (
            "a 1025-byte work directory",
            connectors_table(Path::new(EXECUTABLE), Path::new(&long_path)),
        ),
        (
            "an executable with a control character",
            connectors_table(Path::new("/opt/conn\\u0007ectors"), Path::new(WORK)),
        ),
        (
            "a work directory with a control character",
            connectors_table(Path::new(EXECUTABLE), Path::new("/var/w\\tork")),
        ),
        ("an empty adapter", named("", "conn-1")),
        ("an adapter read as an option", named("--adapter", "conn-1")),
        ("an adapter with a space", named("g pu", "conn-1")),
        ("a 129-character adapter", named(&long_name, "conn-1")),
        ("a non-ASCII adapter", named("gpü", "conn-1")),
        ("an empty connection", named("gpu", "")),
        ("a connection read as an option", named("gpu", "-c")),
        ("a connection with a space", named("gpu", "conn 1")),
        ("a 129-character connection", named("gpu", &long_name)),
    ];
    for (case, lines) in cases {
        match load(&fixture.config(&connected(&fixture, &lines))) {
            Ok(_) => panic!("{case}: accepted"),
            Err(refusal) => assert_eq!(refusal.code(), "config:value", "{case}"),
        }
    }
}

#[test]
fn connectors_at_most_one_provider_declares_a_connection() {
    let fixture = Fixture::new("connectors-two");
    let text = connected(&fixture, &table());
    let second = format!(
        "{text}\n[providers.other]\nkind = \"runpod-vllm\"\n\n\
         [providers.other.connectors]\n{}",
        table()
    );
    match load(&fixture.config(&second)) {
        Ok(_) => panic!("two connected providers: accepted"),
        Err(refusal) => assert_eq!(refusal.code(), "config:value"),
    }
    // The control: a second provider without a connection is a valid document.
    let unconnected = format!("{text}\n[providers.other]\nkind = \"runpod-vllm\"\n");
    assert!(load(&fixture.config(&unconnected)).is_ok());
}

#[test]
fn connectors_an_unknown_missing_or_mistyped_key_is_refused_as_schema() {
    let fixture = Fixture::new("connectors-schema");
    let without = |key: &str| {
        table()
            .split_inclusive('\n')
            .filter(|line| !line.starts_with(key))
            .collect::<String>()
    };
    let cases = [
        ("an unknown key", format!("{}region = \"eu\"\n", table())),
        ("no executable", without("executable")),
        ("no adapter", without("adapter")),
        ("no connection", without("connection")),
        ("no work_directory", without("work_directory")),
        (
            "a string timeout",
            format!("{}timeout_seconds = \"60\"\n", table()),
        ),
        (
            "a negative timeout",
            format!("{}timeout_seconds = -1\n", table()),
        ),
    ];
    for (case, lines) in cases {
        match load(&fixture.config(&connected(&fixture, &lines))) {
            Ok(_) => panic!("{case}: accepted"),
            Err(refusal) => assert_eq!(refusal.code(), "config:schema", "{case}"),
        }
    }
}

/// Criterion 3: no document key selects the emulator, or any transport but the production one.
/// Every spelling a reader might try, at every level a transport could be named, is outside the
/// closed document.
#[test]
fn connectors_no_document_key_selects_the_emulator() {
    let fixture = Fixture::new("connectors-no-emulator");
    let text = connected(&fixture, &table());
    assert!(
        load(&fixture.config(&text)).is_ok(),
        "the control is refused"
    );
    let mut accepted = Vec::new();
    for key in [
        "emulated",
        "emulator",
        "transport",
        "runpod_transport",
        "fake",
    ] {
        for (level, find) in [
            ("top level", "listen = "),
            ("provider", "kind = \"runpod-vllm\"\n"),
            ("connectors", "adapter = \"gpu\"\n"),
            ("model", "max_model_len = 1024\n"),
        ] {
            for value in ["true", "\"emulated\""] {
                let line = format!("{key} = {value}\n");
                let mutated = mutate(&text, find, &format!("{line}{find}"));
                match load(&fixture.config(&mutated)) {
                    Ok(_) => accepted.push(format!("{level}: {line}")),
                    Err(refusal) => {
                        assert_eq!(refusal.code(), "config:schema", "{level}: {line}");
                    }
                }
            }
        }
    }
    assert!(accepted.is_empty(), "accepted: {accepted:?}");
}

/// story:pod-proxy-tls, criterion 3: no document key adds a trust root, names a certificate, or
/// disables or weakens the pod connection's certificate verification. Every spelling a reader
/// might try, at every level, and as a table of its own, is outside the closed document
/// (`config:schema`).
#[test]
fn tls_no_document_key_adds_a_trust_root_or_disables_verification() {
    let fixture = Fixture::new("tls-no-key");
    let text = connected(&fixture, &table());
    assert!(
        load(&fixture.config(&text)).is_ok(),
        "the control is refused"
    );
    let keys = [
        "ca_file",
        "ca_certificates",
        "ca_bundle",
        "trust_roots",
        "root_certificates",
        "extra_roots",
        "ssl_cert_file",
        "tls_ca_file",
        "certificate",
        "insecure",
        "tls_insecure",
        "insecure_skip_verify",
        "danger_accept_invalid_certs",
        "verify_tls",
        "tls_verify",
        "verify_certificates",
        "pod_ca_file",
        "tls",
    ];
    let mut accepted = Vec::new();
    for key in keys {
        for (level, find) in [
            ("top level", "listen = "),
            ("provider", "kind = \"runpod-vllm\"\n"),
            ("connectors", "adapter = \"gpu\"\n"),
            ("model", "max_model_len = 1024\n"),
        ] {
            for value in ["true", "false", "\"/etc/ssl/certs/ca.pem\""] {
                let line = format!("{key} = {value}\n");
                let mutated = mutate(&text, find, &format!("{line}{find}"));
                match load(&fixture.config(&mutated)) {
                    Ok(_) => accepted.push(format!("{level}: {line}")),
                    Err(refusal) => {
                        assert_eq!(refusal.code(), "config:schema", "{level}: {line}");
                    }
                }
            }
        }
        let table = format!("{text}\n[{key}]\nenabled = false\n");
        match load(&fixture.config(&table)) {
            Ok(_) => accepted.push(format!("table [{key}]")),
            Err(refusal) => assert_eq!(refusal.code(), "config:schema", "table [{key}]"),
        }
    }
    assert!(accepted.is_empty(), "accepted: {accepted:?}");
}

// --- Hosted endpoints (story:hosted-endpoints) ---------------------------------------------

/// A document serving the hosted model `remote` at `https://api.example.test/` with `keys`,
/// and the Runpod model `small` of the base document beside it.
fn hosted(fixture: &Fixture, keys: &str) -> String {
    format!(
        "{}\n{}",
        base(fixture),
        hosted_table("remote", "https://api.example.test/", keys)
    )
}

#[test]
fn hosted_a_declared_endpoint_loads_with_every_default_applied() {
    let fixture = Fixture::new("hosted-defaults");
    let credential = fixture.endpoint_credential();
    let deployment = load(&fixture.config(&hosted(&fixture, &bearer_keys(&credential)))).unwrap();
    assert_eq!(
        deployment.models.len(),
        1,
        "the Runpod model is still served"
    );
    let remote = &deployment.hosted["remote"];
    assert_eq!(remote.wires, vec![Wire::Chat, Wire::Messages]);
    assert_eq!(remote.base_url, "https://api.example.test/");
    assert_eq!(remote.auth_kind, AuthKind::Bearer);
    assert_eq!(
        remote.credential_file.as_deref(),
        Some(credential.as_path())
    );
    assert_eq!(remote.credential_header, None);
    assert_eq!(remote.billing_kind, BillingKind::Metered);
    assert_eq!(remote.upstream_model, "remote");
    assert_eq!(remote.context_window, None);
    assert_eq!(remote.tool_calling, ToolCalling::Absent);
}

#[test]
fn hosted_explicit_values_replace_the_defaults() {
    let fixture = Fixture::new("hosted-explicit");
    let credential = fixture.endpoint_credential();
    let keys = format!(
        "{}billing_kind = \"subscription\"\nupstream_model = \"vendor/model-1\"\ncontext_window = 128000\ntool_calling = \"parsed\"\n",
        api_key_keys(&credential, "X-Api-Key")
    );
    let deployment = load(&fixture.config(&hosted(&fixture, &keys))).unwrap();
    let remote = &deployment.hosted["remote"];
    assert_eq!(remote.auth_kind, AuthKind::ApiKey);
    assert_eq!(
        remote
            .credential_header
            .as_ref()
            .map(CredentialHeader::as_str),
        Some("x-api-key")
    );
    assert_eq!(remote.billing_kind, BillingKind::Subscription);
    assert_eq!(remote.upstream_model, "vendor/model-1");
    assert_eq!(remote.context_window, Some(128_000));
    assert_eq!(remote.tool_calling, ToolCalling::Parsed);
    // An anonymous endpoint names neither a file nor a header, and needs no Runpod model.
    let secret = fixture.owner_secret();
    let only = hosted_document(
        "127.0.0.1:0",
        &secret,
        &hosted_table(
            "open",
            "http://127.0.0.1:8000",
            "auth_kind = \"anonymous\"\nbilling_kind = \"self-hosted\"\n",
        ),
    );
    let deployment = load(&fixture.config(&only)).unwrap();
    assert!(deployment.models.is_empty() && deployment.providers.is_empty());
    let open = &deployment.hosted["open"];
    assert_eq!(open.auth_kind, AuthKind::Anonymous);
    assert_eq!(open.credential_file, None);
    assert_eq!(open.billing_kind, BillingKind::SelfHosted);
}

/// Criterion 6, first half: an endpoint whose `auth_kind` is not `anonymous` but which names no
/// `credential_file` is refused as `config:value`.
#[test]
fn hosted_an_endpoint_that_needs_a_credential_but_names_no_file_is_refused_as_a_value() {
    let fixture = Fixture::new("hosted-no-file");
    let credential = fixture.endpoint_credential();
    for (case, control, keys) in [
        (
            "bearer",
            bearer_keys(&credential),
            "auth_kind = \"bearer\"\n".to_owned(),
        ),
        (
            "api-key",
            api_key_keys(&credential, "x-api-key"),
            "auth_kind = \"api-key\"\ncredential_header = \"x-api-key\"\n".to_owned(),
        ),
    ] {
        assert!(
            load(&fixture.config(&hosted(&fixture, &control))).is_ok(),
            "{case}: the control is refused"
        );
        let refusal = load(&fixture.config(&hosted(&fixture, &keys))).unwrap_err();
        assert_eq!(refusal.code(), "config:value", "{case}");
        assert!(
            refusal.message().contains("hosted.remote.credential_file"),
            "{case}: {}",
            refusal.message()
        );
    }
}

/// Criterion 6, second half: an `api-key` endpoint that names no `credential_header` is
/// refused as `config:value`.
#[test]
fn hosted_an_api_key_endpoint_that_names_no_credential_header_is_refused_as_a_value() {
    let fixture = Fixture::new("hosted-no-header");
    let credential = fixture.endpoint_credential();
    let keys = format!(
        "auth_kind = \"api-key\"\ncredential_file = \"{}\"\n",
        credential.display()
    );
    let refusal = load(&fixture.config(&hosted(&fixture, &keys))).unwrap_err();
    assert_eq!(refusal.code(), "config:value");
    assert!(
        refusal
            .message()
            .contains("hosted.remote.credential_header"),
        "{}",
        refusal.message()
    );
}

/// A credential is never sent in cleartext to a remote host: an `http` endpoint whose
/// `auth_kind` is not `anonymous` and whose host is not a loopback literal or `localhost` is
/// refused as `config:value`, naming the endpoint and quoting no credential.
#[test]
fn hosted_a_credential_over_http_to_a_remote_host_is_refused_as_a_value() {
    let fixture = Fixture::new("hosted-cleartext-refused");
    let credential = fixture.endpoint_credential();
    let secret = fixture.owner_secret();
    let bearer = bearer_keys(&credential);
    let api_key = api_key_keys(&credential, "x-api-key");
    for (case, url, keys) in [
        ("bearer to a public address", "http://203.0.113.10", &bearer),
        (
            "bearer to a public address and port",
            "http://203.0.113.10:8000/",
            &bearer,
        ),
        ("bearer to a DNS name", "http://api.example.test/", &bearer),
        (
            "bearer to a private address",
            "http://10.0.0.7:8000",
            &bearer,
        ),
        (
            "bearer to a non-loopback IPv6 address",
            "http://[2001:db8::1]:8000",
            &bearer,
        ),
        (
            "bearer to a name that only starts with localhost",
            "http://localhost.example.test",
            &bearer,
        ),
        (
            "api-key to a DNS name",
            "http://api.example.test:8080/",
            &api_key,
        ),
        (
            "api-key to a public address",
            "http://203.0.113.10",
            &api_key,
        ),
    ] {
        let document = hosted_document("127.0.0.1:0", &secret, &hosted_table("remote", url, keys));
        let refusal = load(&fixture.config(&document))
            .err()
            .unwrap_or_else(|| panic!("{case}: {url} admitted"));
        assert_eq!(refusal.code(), "config:value", "{case}: {refusal}");
        let message = refusal.message();
        assert!(message.contains("hosted.remote"), "{case}: {message}");
        assert!(!message.contains(ENDPOINT_CREDENTIAL), "{case}: {message}");
    }
}

/// The controls of the cleartext rule: an anonymous endpoint may be anywhere over `http`, a
/// credential may go over `http` to a loopback literal or `localhost`, and over `https` to any
/// host.
#[test]
fn hosted_a_credential_over_http_is_admitted_only_to_loopback() {
    let fixture = Fixture::new("hosted-cleartext-admitted");
    let credential = fixture.endpoint_credential();
    let secret = fixture.owner_secret();
    let anonymous = "auth_kind = \"anonymous\"\n".to_owned();
    let bearer = bearer_keys(&credential);
    let api_key = api_key_keys(&credential, "x-api-key");
    for (case, url, keys) in [
        (
            "anonymous to a public address",
            "http://203.0.113.10",
            &anonymous,
        ),
        (
            "anonymous to a DNS name",
            "http://api.example.test:8080/",
            &anonymous,
        ),
        ("bearer to 127.0.0.1", "http://127.0.0.1:8000", &bearer),
        (
            "bearer to the 127/8 block",
            "http://127.200.3.4:8000/",
            &bearer,
        ),
        ("bearer to ::1", "http://[::1]:8000", &bearer),
        ("bearer to localhost", "http://localhost:8000/", &bearer),
        ("api-key to localhost", "http://localhost", &api_key),
        (
            "bearer over https to a DNS name",
            "https://api.example.test/",
            &bearer,
        ),
        (
            "bearer over https to a public address",
            "https://203.0.113.10",
            &bearer,
        ),
        (
            "api-key over https to a DNS name",
            "https://api.example.test:8443",
            &api_key,
        ),
    ] {
        let document = hosted_document("127.0.0.1:0", &secret, &hosted_table("remote", url, keys));
        if let Err(refusal) = load(&fixture.config(&document)) {
            panic!("{case}: {url} refused: {refusal}");
        }
    }
}

/// The out-of-rule cases of a hosted endpoint's credential keys, each a `(case, find, replace)`
/// on the `bearer` control.
fn credential_cases(credential: &Path, bearer: &str) -> Vec<(&'static str, String, String)> {
    vec![
        (
            "an anonymous endpoint naming a file",
            bearer.to_owned(),
            format!(
                "auth_kind = \"anonymous\"\ncredential_file = \"{}\"\n",
                credential.display()
            ),
        ),
        (
            "a bearer endpoint naming a header",
            bearer.to_owned(),
            format!("{bearer}credential_header = \"x-api-key\"\n"),
        ),
        (
            "an anonymous endpoint naming a header",
            bearer.to_owned(),
            "auth_kind = \"anonymous\"\ncredential_header = \"x-api-key\"\n".to_owned(),
        ),
        (
            "a reserved credential header",
            bearer.to_owned(),
            api_key_keys(credential, "authorization"),
        ),
        (
            "a framing credential header",
            bearer.to_owned(),
            api_key_keys(credential, "content-length"),
        ),
        (
            "a credential header with a space",
            bearer.to_owned(),
            api_key_keys(credential, "x api key"),
        ),
        (
            "a credential header over 64 bytes",
            bearer.to_owned(),
            api_key_keys(credential, &"k".repeat(65)),
        ),
    ]
}

/// story:hosted-base-path: the out-of-rule base paths, each a `(case, find, replace)` on the
/// control's `base_url` line.
fn base_path_cases(base_url: &str) -> Vec<(&'static str, String, String)> {
    vec![
        (
            "a dot-dot segment",
            base_url.to_owned(),
            "base_url = \"https://api.example.test/api/../admin\"".to_owned(),
        ),
        (
            "a dot segment",
            base_url.to_owned(),
            "base_url = \"https://api.example.test/./v1\"".to_owned(),
        ),
        (
            "an empty segment",
            base_url.to_owned(),
            "base_url = \"https://api.example.test/api//v1\"".to_owned(),
        ),
        (
            "two final slashes",
            base_url.to_owned(),
            "base_url = \"https://api.example.test/v1//\"".to_owned(),
        ),
        (
            "a percent-encoded byte in the path",
            base_url.to_owned(),
            "base_url = \"https://api.example.test/api%2F..\"".to_owned(),
        ),
        (
            "a backslash in the path",
            base_url.to_owned(),
            "base_url = \"https://api.example.test/api\\\\v1\"".to_owned(),
        ),
        (
            "a query after a path",
            base_url.to_owned(),
            "base_url = \"https://api.example.test/v1?a=1\"".to_owned(),
        ),
        (
            "a fragment after a path",
            base_url.to_owned(),
            "base_url = \"https://api.example.test/v1#a\"".to_owned(),
        ),
        (
            "user information before a path",
            base_url.to_owned(),
            "base_url = \"https://user@api.example.test/v1\"".to_owned(),
        ),
        (
            "a path of 33 segments",
            base_url.to_owned(),
            format!("base_url = \"https://api.example.test{}\"", "/a".repeat(33)),
        ),
        (
            "a path over 256 bytes",
            base_url.to_owned(),
            format!(
                "base_url = \"https://api.example.test/{}\"",
                "a".repeat(256)
            ),
        ),
    ]
}

/// The out-of-rule cases of a hosted endpoint's `base_url`, each a `(case, find, replace)` on the
/// control's `base_url` line.
fn base_url_cases(base_url: &str) -> Vec<(&'static str, String, String)> {
    vec![
        (
            "another scheme",
            base_url.to_owned(),
            "base_url = \"ftp://api.example.test/\"".to_owned(),
        ),
        (
            "no scheme",
            base_url.to_owned(),
            "base_url = \"api.example.test\"".to_owned(),
        ),
        (
            "a query",
            base_url.to_owned(),
            "base_url = \"https://api.example.test/?a=1\"".to_owned(),
        ),
        (
            "a fragment",
            base_url.to_owned(),
            "base_url = \"https://api.example.test#a\"".to_owned(),
        ),
        (
            "user information",
            base_url.to_owned(),
            "base_url = \"https://user:pw@api.example.test/\"".to_owned(),
        ),
        (
            "no authority",
            base_url.to_owned(),
            "base_url = \"https:///\"".to_owned(),
        ),
        (
            "a port out of range",
            base_url.to_owned(),
            "base_url = \"https://api.example.test:65536\"".to_owned(),
        ),
        (
            "a port that is not a number",
            base_url.to_owned(),
            "base_url = \"https://api.example.test:x\"".to_owned(),
        ),
        (
            "a space",
            base_url.to_owned(),
            "base_url = \"https://api example.test\"".to_owned(),
        ),
        (
            "an https host no certificate can name",
            base_url.to_owned(),
            "base_url = \"https://-bad-.example.test\"".to_owned(),
        ),
        (
            "an authority over 256 bytes",
            base_url.to_owned(),
            format!("base_url = \"http://{}.test\"", "a.".repeat(126)),
        ),
    ]
}

/// The out-of-rule cases of a hosted endpoint's wires, alias, upstream model and context window,
/// each a `(case, find, replace)` on the control.
fn declaration_cases(bearer: &str) -> Vec<(&'static str, String, String)> {
    vec![
        (
            "no wire",
            "wires = [\"chat\", \"messages\"]".to_owned(),
            "wires = []".to_owned(),
        ),
        (
            "a repeated wire",
            "wires = [\"chat\", \"messages\"]".to_owned(),
            "wires = [\"chat\", \"chat\"]".to_owned(),
        ),
        (
            "an alias that is not a closed name",
            "[hosted.remote]".to_owned(),
            "[hosted.Remote]".to_owned(),
        ),
        (
            "an alias a Runpod model already has",
            "[hosted.remote]".to_owned(),
            "[hosted.small]".to_owned(),
        ),
        (
            "an empty upstream model",
            bearer.to_owned(),
            format!("{bearer}upstream_model = \"\"\n"),
        ),
        (
            "an upstream model over 256 bytes",
            bearer.to_owned(),
            format!("{bearer}upstream_model = \"{}\"\n", "m".repeat(257)),
        ),
        (
            "a context window under 4096",
            bearer.to_owned(),
            format!("{bearer}context_window = 4095\n"),
        ),
        (
            "a context window over 2000000",
            bearer.to_owned(),
            format!("{bearer}context_window = 2000001\n"),
        ),
    ]
}

/// Every other rule of `[hosted.<alias>]` (llm-gateway.deployment.HostedDeclaration), each
/// broken on its own against a control that loads.
#[test]
fn hosted_every_value_outside_its_rule_is_refused() {
    let fixture = Fixture::new("hosted-out-of-rule");
    let credential = fixture.endpoint_credential();
    let bearer = bearer_keys(&credential);
    let control = hosted(&fixture, &bearer);
    assert!(
        load(&fixture.config(&control)).is_ok(),
        "the control is refused"
    );
    let base_url = "base_url = \"https://api.example.test/\"";
    let cases = [
        credential_cases(&credential, &bearer),
        base_url_cases(base_url),
        base_path_cases(base_url),
        declaration_cases(&bearer),
    ]
    .concat();
    for (case, find, replace) in cases {
        let text = mutate(&control, &find, &replace);
        match load(&fixture.config(&text)) {
            Ok(_) => panic!("{case}: accepted"),
            Err(refusal) => assert_eq!(refusal.code(), "config:value", "{case}: {refusal}"),
        }
    }
    // Each boundary on its own side is accepted.
    for (case, replace) in [
        (
            "a 64-byte credential header",
            api_key_keys(&credential, &"k".repeat(64)),
        ),
        (
            "a context window of 4096",
            format!("{bearer}context_window = 4096\n"),
        ),
        (
            "a context window of 2000000",
            format!("{bearer}context_window = 2000000\n"),
        ),
        (
            "a 256-byte upstream model",
            format!("{bearer}upstream_model = \"{}\"\n", "m".repeat(256)),
        ),
    ] {
        let text = mutate(&control, &bearer, &replace);
        assert!(load(&fixture.config(&text)).is_ok(), "{case}: refused");
    }
    for url in [
        "http://127.0.0.1:8000",
        "http://localhost",
        "https://api.example.test",
        "https://api.example.test:8443/",
        "https://[::1]:8443",
        "https://127.0.0.1:443",
        "https://api.example.test/v1",
        "https://openrouter.example.test/api/v1/",
        "https://api.example.test/a-b_c.d~e/V2",
    ] {
        let text = mutate(&control, base_url, &format!("base_url = \"{url}\""));
        assert!(load(&fixture.config(&text)).is_ok(), "{url}: refused");
    }
    // A base path of exactly 32 segments and of exactly 256 bytes is accepted; the base URL is
    // kept as written.
    for url in [
        format!("https://api.example.test{}", "/a".repeat(32)),
        format!("https://api.example.test/{}", "a".repeat(255)),
    ] {
        let text = mutate(&control, base_url, &format!("base_url = \"{url}\""));
        let loaded = load(&fixture.config(&text));
        assert!(loaded.is_ok(), "{url}: refused");
        assert_eq!(loaded.unwrap().hosted["remote"].base_url, url);
    }
}

/// story:hosted-static-headers: `[hosted.<alias>.headers]` loads as lowercase names in name
/// order with their values as written.
#[test]
fn hosted_static_headers_load_lowercase_in_name_order() {
    let fixture = Fixture::new("hosted-headers");
    let credential = fixture.endpoint_credential();
    let text = format!(
        "{}[hosted.remote.headers]\nX-Title = \"b10x gateway\"\nanthropic-version = \"2023-06-01\"\n\"HTTP-Referer\" = \"https://example.test/app\"\n",
        hosted(&fixture, &api_key_keys(&credential, "x-api-key"))
    );
    let deployment = load(&fixture.config(&text)).unwrap();
    let headers: Vec<(&str, &str)> = deployment.hosted["remote"]
        .headers
        .iter()
        .map(|header| (header.name(), header.value()))
        .collect();
    assert_eq!(
        headers,
        [
            ("anthropic-version", "2023-06-01"),
            ("http-referer", "https://example.test/app"),
            ("x-title", "b10x gateway"),
        ]
    );
    let none = load(&fixture.config(&hosted(&fixture, &bearer_keys(&credential)))).unwrap();
    assert!(none.hosted["remote"].headers.is_empty());
}

/// story:hosted-static-headers: every static header outside its rule is `config:value` naming
/// `hosted.remote.headers`, and quoting no value; each boundary on its own side loads.
#[test]
fn hosted_static_headers_outside_their_rule_are_refused() {
    let fixture = Fixture::new("hosted-headers-out-of-rule");
    let credential = fixture.endpoint_credential();
    let control = hosted(&fixture, &api_key_keys(&credential, "x-api-key"));
    let table = |lines: &str| format!("{control}[hosted.remote.headers]\n{lines}");
    let many = |count: usize| {
        (0..count).fold(String::new(), |mut lines, index| {
            lines.push_str(&format!("x-h{index} = \"v\"\n"));
            lines
        })
    };
    for (case, lines) in [
        ("host", "Host = \"elsewhere.test\"\n".to_owned()),
        ("authorization", "authorization = \"Bearer leaked-value\"\n".to_owned()),
        ("content-length", "content-length = \"5\"\n".to_owned()),
        ("transfer-encoding", "transfer-encoding = \"chunked\"\n".to_owned()),
        ("connection", "Connection = \"keep-alive\"\n".to_owned()),
        ("proxy-authorization", "proxy-authorization = \"Basic leaked-value\"\n".to_owned()),
        ("the credential header", "X-Api-Key = \"leaked-value\"\n".to_owned()),
        ("a name repeated in another case", "x-title = \"a\"\nX-Title = \"b\"\n".to_owned()),
        ("an empty name", "\"\" = \"v\"\n".to_owned()),
        ("a name of 65 bytes", format!("{} = \"v\"\n", "k".repeat(65))),
        ("a name with a space", "\"x title\" = \"v\"\n".to_owned()),
        ("a name with a colon", "\"x:title\" = \"v\"\n".to_owned()),
        ("an empty value", "x-title = \"\"\n".to_owned()),
        ("a value of 1025 bytes", format!("x-title = \"{}\"\n", "v".repeat(1025))),
        ("a value with CR LF", "x-title = \"a\\r\\nx-injected: b\"\n".to_owned()),
        ("a value with LF", "x-title = \"a\\nb\"\n".to_owned()),
        ("a value with NUL", "x-title = \"a\\u0000b\"\n".to_owned()),
        ("a value with a tab", "x-title = \"a\\tb\"\n".to_owned()),
        ("a value with a leading space", "x-title = \" a\"\n".to_owned()),
        ("a value with a trailing space", "x-title = \"a \"\n".to_owned()),
        ("a value outside ASCII", "x-title = \"caf\u{e9}\"\n".to_owned()),
        ("33 headers", many(33)),
    ] {
        match load(&fixture.config(&table(&lines))) {
            Ok(_) => panic!("{case}: accepted"),
            Err(refusal) => {
                assert_eq!(refusal.code(), "config:value", "{case}: {refusal}");
                let message = refusal.to_string();
                assert!(message.contains("hosted.remote.headers"), "{case}: {message}");
                assert!(!message.contains("leaked-value"), "{case}: {message}");
            }
        }
    }
    for (case, lines) in [
        ("32 headers", many(32)),
        ("a name of 64 bytes", format!("{} = \"v\"\n", "k".repeat(64))),
        ("a value of 1024 bytes", format!("x-title = \"{}\"\n", "v".repeat(1024))),
        ("a value with inner spaces", "x-title = \"a b  c\"\n".to_owned()),
    ] {
        assert!(load(&fixture.config(&table(&lines))).is_ok(), "{case}: refused");
    }
    // A value that is not a string leaves the closed document.
    let mistyped = table("x-title = 5\n");
    assert_eq!(
        load(&fixture.config(&mistyped)).unwrap_err().code(),
        "config:schema"
    );
}

/// An unknown key, a missing required key and a closed value outside its set leave the closed
/// document (`config:schema`).
#[test]
fn hosted_an_unknown_missing_or_mistyped_key_is_refused_as_schema() {
    let fixture = Fixture::new("hosted-schema");
    let credential = fixture.endpoint_credential();
    let bearer = bearer_keys(&credential);
    let control = hosted(&fixture, &bearer);
    for (case, find, replace) in [
        (
            "an unknown key",
            bearer.clone(),
            format!("{bearer}api_base = \"x\"\n"),
        ),
        (
            "a trust root",
            bearer.clone(),
            format!("{bearer}ca_file = \"/etc/ssl/ca.pem\"\n"),
        ),
        (
            "no base_url",
            "base_url = \"https://api.example.test/\"\n".to_owned(),
            String::new(),
        ),
        (
            "no auth_kind",
            "auth_kind = \"bearer\"\n".to_owned(),
            String::new(),
        ),
        (
            "no wires",
            "wires = [\"chat\", \"messages\"]\n".to_owned(),
            String::new(),
        ),
        (
            "an auth_kind outside its set",
            "auth_kind = \"bearer\"".to_owned(),
            "auth_kind = \"token\"".to_owned(),
        ),
        (
            "a billing_kind outside its set",
            bearer.clone(),
            format!("{bearer}billing_kind = \"free\"\n"),
        ),
        (
            "a mistyped context_window",
            bearer.clone(),
            format!("{bearer}context_window = \"large\"\n"),
        ),
    ] {
        let text = mutate(&control, &find, &replace);
        match load(&fixture.config(&text)) {
            Ok(_) => panic!("{case}: accepted"),
            Err(refusal) => assert_eq!(refusal.code(), "config:schema", "{case}: {refusal}"),
        }
    }
}

// --- A hosted model's declared effort (story:effort-default) ---------------------------------

/// `[hosted.<alias>] reasoning_effort` is absent by default and loads as `low`, `medium` or
/// `xhigh`; any other string, `high` and `max` included, is `config:value` naming the key, and a
/// value that is not a string leaves the closed document (`config:schema`). A Runpod model's
/// `reasoning_effort` keeps its own set, `high` included.
#[test]
fn hosted_a_reasoning_effort_of_low_medium_or_xhigh_loads_and_any_other_is_refused() {
    let fixture = Fixture::new("hosted-effort");
    let credential = fixture.endpoint_credential();
    let bearer = bearer_keys(&credential);
    let control = hosted(&fixture, &bearer);
    let deployment = load(&fixture.config(&control)).unwrap();
    assert_eq!(deployment.hosted["remote"].reasoning_effort, None);
    for effort in ReasoningEffort::ALL {
        let text = mutate(
            &control,
            &bearer,
            &format!("{bearer}reasoning_effort = \"{}\"\n", effort.as_str()),
        );
        let deployment = load(&fixture.config(&text)).unwrap();
        assert_eq!(
            deployment.hosted["remote"].reasoning_effort,
            Some(effort),
            "{effort:?}"
        );
    }
    for other in [
        "high", "max", "none", "minimal", "", "LOW", "Xhigh", " low", "x-high",
    ] {
        let text = mutate(
            &control,
            &bearer,
            &format!("{bearer}reasoning_effort = \"{other}\"\n"),
        );
        match load(&fixture.config(&text)) {
            Ok(_) => panic!("{other:?}: accepted"),
            Err(refusal) => {
                assert_eq!(refusal.code(), "config:value", "{other:?}: {refusal}");
                assert!(
                    refusal
                        .to_string()
                        .contains("hosted.remote.reasoning_effort"),
                    "{refusal}"
                );
            }
        }
    }
    let text = mutate(
        &control,
        &bearer,
        &format!("{bearer}reasoning_effort = 2\n"),
    );
    match load(&fixture.config(&text)) {
        Ok(_) => panic!("a mistyped reasoning_effort: accepted"),
        Err(refusal) => assert_eq!(refusal.code(), "config:schema", "{refusal}"),
    }
    let runpod = mutate(
        &control,
        "max_model_len = 1024\n",
        "max_model_len = 1024\nreasoning_effort = \"high\"\n",
    );
    let deployment = load(&fixture.config(&runpod)).unwrap();
    assert_eq!(
        deployment.models["small"].vllm.reasoning_effort.as_deref(),
        Some("high")
    );
}
