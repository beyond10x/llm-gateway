//! Row K29 through the loader the binary uses: the closed document's defaults, every range
//! boundary on both sides, and every key outside the closed document. The defaults and ranges
//! are llmgw's (`src/config.rs:138-160` and `src/config.rs:221-343` at 048ebd8).
//!
//! The loaded settings map onto the types llm-gateway already has: `listen` onto
//! `GatewayConfig::bind` (K1), `cloud_type` onto `llm_runpod::CloudType` (K8), the vLLM keys onto
//! `llm_runpod::VllmSettings` (K16-K22) and the volume keys onto `llm_runpod::NetworkVolume`
//! (K24-K25).

mod support;

use llm_gateway::ToolCalling;
use llm_gateway_cli::{load, vllm_keys};
use llm_runpod::{CloudType, NetworkVolume, Thinking};
use std::net::SocketAddr;
use support::{Fixture, VLLM_KEY, document, mutate, with_vllm_key};

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
