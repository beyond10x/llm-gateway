//! Adversarial pass over `story:model-tool-calling`: the deployment document's `tool_calling`
//! key. `spec/domains/deployment.yaml` says it is `parsed` or `absent` and that any other value is
//! `config:schema`.

mod support;

use llm_gateway_cli::load;
use support::{Fixture, document, mutate};

/// TOML spells an enum as a string; a one-key inline table naming a variant is another value,
/// and the closed document must refuse it rather than read it as that variant.
#[test]
fn adversary_w05_tool_calling_as_an_inline_table_is_refused_as_schema() {
    let fixture = Fixture::new("adversary-w05-tools-table");
    for value in ["{ parsed = {} }", "{ absent = {} }"] {
        let text = mutate(
            &document("127.0.0.1:0", &fixture.owner_secret()),
            "max_model_len = 1024\n",
            &format!("max_model_len = 1024\ntool_calling = {value}\n"),
        );
        match load(&fixture.config(&text)) {
            Ok(deployment) => panic!(
                "tool_calling = {value}: accepted as {:?}",
                deployment.models["small"].tool_calling
            ),
            Err(refusal) => assert_eq!(refusal.code(), "config:schema", "{value}"),
        }
    }
}
