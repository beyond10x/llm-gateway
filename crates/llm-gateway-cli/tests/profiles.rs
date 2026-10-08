//! Row D3: llmgw's two proven model profiles and its weight-cache block (llmgw
//! `docs/model-profiles.md:8`, `:44` and `:88` at 048ebd8), as written in
//! `docs/model-profiles.md`, each loaded through the loader the binary uses.
//!
//! The blocks are taken out of the document itself, so the document cannot drift from what the
//! loader accepts.

mod support;

use llm_gateway_cli::{Model, load};
use llm_runpod::{NetworkVolume, Thinking};
use std::path::Path;
use support::Fixture;

const DEFAULT: &str = "Default: broad availability, 32k context";
const HIGH_THROUGHPUT: &str =
    "High throughput: H100 NVL with MTP speculative decoding, 64k context";
const WEIGHT_CACHE: &str = "Weight cache (optional, either profile)";
const ALIAS: &str = "qwen3.8-27b";

fn profiles() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/model-profiles.md");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// The first ```` ```toml ```` block under the `## <title>` heading, before the next heading.
fn block(text: &str, title: &str) -> String {
    let heading = format!("## {title}\n");
    let start = text
        .find(&heading)
        .unwrap_or_else(|| panic!("docs/model-profiles.md has no heading {heading:?}"))
        + heading.len();
    let section = &text[start..];
    let section = section.find("\n## ").map_or(section, |end| &section[..end]);
    let open = section
        .find("```toml\n")
        .unwrap_or_else(|| panic!("section {title:?} has no toml block"))
        + "```toml\n".len();
    let length = section[open..]
        .find("```")
        .unwrap_or_else(|| panic!("section {title:?} has an unclosed block"));
    section[open..open + length].to_string()
}

/// The context length a title names: `32k` is 32 × 1024 tokens.
fn titled_context(title: &str) -> u32 {
    let word = title
        .split(|character: char| !character.is_ascii_alphanumeric())
        .find(|word| {
            word.len() > 1
                && word.ends_with('k')
                && word[..word.len() - 1]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit())
        })
        .unwrap_or_else(|| panic!("title {title:?} names no context length"));
    word[..word.len() - 1].parse::<u32>().unwrap() * 1024
}

/// A deployment document holding exactly `models`, loaded through [`load`].
fn load_profile(name: &str, models: &str) -> Model {
    let fixture = Fixture::new(name);
    let secret = fixture.owner_secret();
    let text = format!(
        "listen = \"127.0.0.1:0\"\nowner_secret_file = \"{}\"\n\n[providers.runpod]\nkind = \"runpod-vllm\"\n\n{models}",
        secret.display()
    );
    let deployment = load(&fixture.config(&text))
        .unwrap_or_else(|refusal| panic!("profile {name} refused: {refusal:?}"));
    assert_eq!(
        deployment.models.len(),
        1,
        "profile {name} declares one model"
    );
    deployment.models[ALIAS].clone()
}

fn assert_profile(model: &Model, title: &str) {
    let context = titled_context(title);
    assert_eq!(model.context_window, context, "{title}");
    assert_eq!(model.vllm.max_model_len, context, "{title}");
    assert_eq!(model.provider, "runpod");
    assert_eq!(model.hf_model, "Qwen/Qwen3.8-27B-FP8");
    assert_eq!(model.image, "vllm/vllm-openai:v0.27.1");
    assert_eq!(
        model.gpu_types.len(),
        3,
        "{title}: three cards to fall back across"
    );
}

#[test]
fn d3_the_default_profile_is_a_32k_model_declaration() {
    let model = load_profile("d3-default", &block(&profiles(), DEFAULT));
    assert_eq!(titled_context(DEFAULT), 32_768);
    assert_profile(&model, DEFAULT);
    assert_eq!(model.vllm.max_num_seqs, 8);
    assert_eq!(model.vllm.thinking, Thinking::Off);
    assert!(
        !model
            .vllm
            .extra_args
            .contains(&"--speculative-config".to_string()),
        "the default profile has no speculative decoding"
    );
    assert_eq!(model.cache, None);
}

#[test]
fn d3_the_high_throughput_profile_is_a_64k_model_declaration_with_mtp() {
    let model = load_profile("d3-high", &block(&profiles(), HIGH_THROUGHPUT));
    assert_eq!(titled_context(HIGH_THROUGHPUT), 65_536);
    assert_profile(&model, HIGH_THROUGHPUT);
    assert_eq!(model.vllm.max_num_seqs, 64);
    assert_eq!(model.vllm.thinking, Thinking::On);
    assert!(model.gpu_types.contains(&"NVIDIA H100 NVL".to_string()));
    let args = &model.vllm.extra_args;
    let flag = args
        .iter()
        .position(|argument| argument == "--speculative-config")
        .expect("the high-throughput profile sets --speculative-config");
    let config: serde_json::Value =
        serde_json::from_str(&args[flag + 1]).expect("the speculative config is JSON");
    assert_eq!(
        config,
        serde_json::json!({"method": "mtp", "num_speculative_tokens": 3})
    );
    assert_eq!(model.cache, None);
}

#[test]
fn d3_the_weight_cache_block_is_accepted_with_either_profile() {
    let text = profiles();
    let cache = block(&text, WEIGHT_CACHE);
    for (name, title) in [
        ("d3-cache-default", DEFAULT),
        ("d3-cache-high", HIGH_THROUGHPUT),
    ] {
        let model = load_profile(name, &format!("{}{cache}", block(&text, title)));
        assert_profile(&model, title);
        assert_eq!(
            model.cache,
            Some(NetworkVolume {
                volume_id: "YOURVOLUMEID".to_string(),
                mount_path: "/workspace".to_string(),
            }),
            "{title}"
        );
        assert_eq!(model.data_center_ids, ["US-CA-2"], "{title}");
    }
}
