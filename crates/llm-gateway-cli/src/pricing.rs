//! A model's token price and what one call costs by it (story:cost-per-call;
//! `spec/domains/deployment.yaml` `TokenPriceDeclaration` and `RoutePriceDeclaration`,
//! `spec/domains/telemetry.yaml` `UsageRecord.cost_usd`).
//!
//! The gateway does no arithmetic on money: a declared price becomes one model of an llm
//! `PriceBook`, and a call's four counters are quoted by `PriceBook::quote`, llm's own rule.

use crate::refusal::{Refusal, StartupRefusal};
use llm_core::{BillingKind, Dispatch, Id, Protocol, Provenance};
use llm_cost::{
    AccountingInput, Amount, AttemptUsage, Currency, ModelPrice, Observation, PriceBook,
    PriceDocument, Rate,
};
use llm_gateway::{CallPricing, Usage, Wire};
use std::{collections::BTreeMap, sync::Arc};

/// The tokens a declared rate is per.
const PER_MILLION: u64 = 1_000_000;

/// What a model's tokens cost in USD per 1000000 tokens, per class: `[<table>.<alias>.token_price]`
/// of the deployment document, or a route's `price` in the routes document. A class without a
/// rate has none: a call reporting a non-zero count of it has no cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenPrice {
    input: Option<Amount>,
    output: Option<Amount>,
    cache_read: Option<Amount>,
    cache_creation: Option<Amount>,
}

impl TokenPrice {
    /// The keys of a `token_price` table.
    pub const KEYS: [&'static str; 4] = ["input", "output", "cache_read", "cache_creation"];

    /// Reads a `token_price` table whose values are strings; `at` is the table's own key
    /// (`hosted.<alias>.token_price`).
    ///
    /// # Errors
    /// `config:value` naming `<at>.<key>`, quoting no value: for a key that is none of
    /// [`Self::KEYS`], a value llm's `Amount::parse` refuses (a sign, an exponent, a leading
    /// zero, a space, more than nine decimal places) and a missing `input` or `output`.
    pub fn from_table(at: &str, table: &BTreeMap<String, String>) -> Result<Self, Refusal> {
        if let Some(key) = table.keys().find(|key| !Self::KEYS.contains(&key.as_str())) {
            return Err(unknown(at, key));
        }
        let rate = |key: &str| {
            table
                .get(key)
                .map(|value| Amount::parse(value).map_err(|_| not_an_amount(at, key)))
                .transpose()
        };
        let required = |key: &str| {
            rate(key)?.map_or_else(
                || Err(config_value(format!("{at}.{key} is required"))),
                |amount| Ok(Some(amount)),
            )
        };
        Ok(Self {
            input: required("input")?,
            output: required("output")?,
            cache_read: rate("cache_read")?,
            cache_creation: rate("cache_creation")?,
        })
    }

    /// Reads a `token_price` table of the deployment document: every value a string.
    pub(crate) fn from_toml(at: &str, table: toml::Table) -> Result<Self, Refusal> {
        let mut strings = BTreeMap::new();
        for (key, value) in table {
            if !Self::KEYS.contains(&key.as_str()) {
                return Err(unknown(at, &key));
            }
            let toml::Value::String(value) = value else {
                return Err(not_an_amount(at, &key));
            };
            strings.insert(key, value);
        }
        Self::from_table(at, &strings)
    }

    /// A route's `price` as llm reads it: `input` onto `input`, `cached_input` onto
    /// `cache_read`, `cache_creation_input` onto `cache_creation`, `output` onto `output`.
    ///
    /// # Errors
    /// `config:value` naming `routes.<route>.price.currency` for a currency other than USD,
    /// quoting no value.
    pub(crate) fn from_route(
        route: &str,
        price: &llm_models::routes::RoutePrice,
    ) -> Result<Self, Refusal> {
        if price.currency().as_str() != "USD" {
            return Err(config_value(format!(
                "routes.{route}.price.currency must be USD: a call's cost is recorded in USD"
            )));
        }
        Ok(Self {
            input: price.input(),
            output: price.output(),
            cache_read: price.cached_input(),
            cache_creation: price.cache_creation_input(),
        })
    }

    /// The pricing a relayed model declaring this price is composed with
    /// (`llm_gateway::RelayModel::with_pricing`): this price as the one model of an llm
    /// `PriceBook`, which quotes each call.
    ///
    /// # Panics
    /// Never: the book's identifiers are constants llm accepts and every rate's denominator is
    /// 1000000.
    pub fn pricing(&self) -> Arc<dyn CallPricing> {
        let rate = |amount: Option<Amount>| {
            amount.map(|amount| Rate {
                amount,
                per_units: PER_MILLION,
            })
        };
        let document = PriceDocument::new(
            id(),
            "llm-gateway token_price",
            0,
            Currency::new("USD").expect("USD is a currency"),
            vec![ModelPrice {
                id: id(),
                serving_model_id: id(),
                binding_revision: id(),
                upstream_model: id(),
                input: rate(self.input),
                cached_input: rate(self.cache_read),
                cache_creation_input: rate(self.cache_creation),
                output: rate(self.output),
            }],
        );
        let book = PriceBook::from_document(document).expect("one model, every rate per 1000000");
        Arc::new(Priced(book))
    }
}

/// The one identifier of a price book holding one model's price.
fn id() -> Id {
    Id::new("token-price").expect("a printable identifier")
}

fn config_value(message: String) -> Refusal {
    Refusal::new(StartupRefusal::ConfigValue, message)
}

fn unknown(at: &str, key: &str) -> Refusal {
    config_value(format!(
        "{at}.{key} is not a token price key: input, output, cache_read and cache_creation are"
    ))
}

fn not_an_amount(at: &str, key: &str) -> Refusal {
    config_value(format!(
        "{at}.{key} must be a non-negative decimal amount of USD per 1000000 tokens, written \
         as a string, with at most nine decimal places"
    ))
}

/// A declared price as llm's price book.
struct Priced(PriceBook);

impl CallPricing for Priced {
    fn cost_usd(&self, wire: Wire, usage: Usage) -> Option<String> {
        let attempt = AttemptUsage {
            id: id(),
            usage_record_id: id(),
            request_id: id(),
            route_target_id: id(),
            serving_model_id: id(),
            price_id: id(),
            binding: Provenance {
                protocol: Protocol::ChatCompletions,
                provider: id(),
                account: id(),
                endpoint: id(),
                model: id(),
                binding_revision: id(),
            },
            upstream_model: Some(id()),
            billing: BillingKind::Metered,
            dispatch: Dispatch::Accepted,
            failed: false,
            final_usage: true,
            usage: reported(wire, usage)?,
        };
        let input = AccountingInput::new(vec![Observation::Attempt(attempt)]);
        let report = self.0.quote(&input).ok()?;
        let total = report.records.first()?.total.complete_total?;
        Some(total.to_string())
    }
}

/// A call's counters as llm counts them: its prompt tokens with the cache reads and writes
/// inside them. The messages wire reports `input_tokens` beside the cache classes, chat and
/// responses inside them. A cache class the answer did not report counts no tokens; input and
/// output stay unknown when unreported, and llm prices no unknown count.
fn reported(wire: Wire, usage: Usage) -> Option<llm_core::Usage> {
    let read = usage.cache_read_tokens.unwrap_or(0);
    let created = usage.cache_creation_tokens.unwrap_or(0);
    let input = match (wire, usage.input_tokens) {
        (Wire::Messages, Some(input)) => Some(input.checked_add(read)?.checked_add(created)?),
        (_, input) => input,
    };
    Some(llm_core::Usage {
        input_tokens: input,
        output_tokens: usage.output_tokens,
        cached_input_tokens: Some(read),
        cache_creation_input_tokens: Some(created),
        reasoning_output_tokens: None,
    })
}

#[cfg(test)]
mod tests {
    use super::TokenPrice;
    use llm_gateway::{Usage, Wire};

    fn price(pairs: &[(&str, &str)]) -> TokenPrice {
        let table = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        TokenPrice::from_table("hosted.m.token_price", &table).unwrap()
    }

    /// The four counters a call's answer reported, in the scanner's order: input, output,
    /// cache read, cache creation.
    fn usage(
        input: Option<u64>,
        output: Option<u64>,
        cache_read: Option<u64>,
        cache_creation: Option<u64>,
    ) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_creation_tokens: cache_creation,
        }
    }

    fn cost(price: &TokenPrice, wire: Wire, usage: Usage) -> Option<String> {
        price.pricing().cost_usd(wire, usage)
    }

    const ALL: [(&str, &str); 4] = [
        ("input", "3"),
        ("output", "15"),
        ("cache_read", "0.3"),
        ("cache_creation", "3.75"),
    ];

    #[test]
    fn cost_input_tokens_are_priced_at_the_input_rate() {
        let price = price(&ALL);
        let read = usage(Some(1_000_000), Some(0), None, None);
        assert_eq!(cost(&price, Wire::Chat, read).as_deref(), Some("3"));
        let one = usage(Some(1), Some(0), None, None);
        assert_eq!(
            cost(&price, Wire::Messages, one).as_deref(),
            Some("0.000003")
        );
    }

    #[test]
    fn cost_output_tokens_are_priced_at_the_output_rate() {
        let price = price(&ALL);
        let written = usage(Some(0), Some(2000), None, None);
        assert_eq!(
            cost(&price, Wire::Responses, written).as_deref(),
            Some("0.03")
        );
    }

    /// On the messages wire the cache reads stand beside `input_tokens`; on chat and responses
    /// they are part of it. The same reads cost the same on every wire.
    #[test]
    fn cost_cache_read_tokens_are_priced_at_the_cache_read_rate_on_every_wire() {
        let price = price(&ALL);
        let messages = usage(Some(0), Some(0), Some(10_000), None);
        assert_eq!(
            cost(&price, Wire::Messages, messages).as_deref(),
            Some("0.003")
        );
        let chat = usage(Some(10_000), Some(0), Some(10_000), None);
        assert_eq!(cost(&price, Wire::Chat, chat).as_deref(), Some("0.003"));
        assert_eq!(
            cost(&price, Wire::Responses, chat).as_deref(),
            Some("0.003")
        );
    }

    #[test]
    fn cost_cache_creation_tokens_are_priced_at_the_cache_creation_rate() {
        let price = price(&ALL);
        let created = usage(Some(0), Some(0), None, Some(2000));
        assert_eq!(
            cost(&price, Wire::Messages, created).as_deref(),
            Some("0.0075")
        );
    }

    /// The four classes of one messages answer, each priced at its rate and summed: 100
    /// uncached input tokens, 10000 read from the cache, 2000 written to it, 50 output tokens.
    #[test]
    fn cost_every_class_of_one_answer_is_summed() {
        let price = price(&ALL);
        let answer = usage(Some(100), Some(50), Some(10_000), Some(2000));
        assert_eq!(
            cost(&price, Wire::Messages, answer).as_deref(),
            Some("0.01155")
        );
        // The same answer on chat: 12100 prompt tokens, 10000 of them cached.
        let chat = usage(Some(12_100), Some(50), Some(10_000), Some(2000));
        assert_eq!(cost(&price, Wire::Chat, chat).as_deref(), Some("0.01155"));
    }

    /// llm rounds each line up once, to a nanodollar: one token at 0.000001 USD per million is
    /// one nanodollar, not zero.
    #[test]
    fn cost_each_line_is_rounded_up_once_by_llms_rule() {
        let price = price(&[("input", "0.000001"), ("output", "0")]);
        let one = usage(Some(1), Some(7), None, None);
        assert_eq!(
            cost(&price, Wire::Chat, one).as_deref(),
            Some("0.000000001")
        );
    }

    #[test]
    fn cost_a_price_and_no_reported_tokens_has_none() {
        let price = price(&ALL);
        for wire in Wire::ALL {
            assert_eq!(cost(&price, wire, Usage::default()), None, "{wire:?}");
        }
        // Input reported without output, and output without input, is no complete count.
        let input_only = usage(Some(100), None, None, None);
        let output_only = usage(None, Some(100), None, None);
        assert_eq!(cost(&price, Wire::Chat, input_only), None);
        assert_eq!(cost(&price, Wire::Chat, output_only), None);
    }

    #[test]
    fn cost_reported_cache_tokens_without_a_cache_rate_have_none() {
        let price = price(&[("input", "3"), ("output", "15")]);
        let read = usage(Some(100), Some(50), Some(500), None);
        assert_eq!(cost(&price, Wire::Messages, read), None);
        let created = usage(Some(100), Some(50), None, Some(500));
        assert_eq!(cost(&price, Wire::Messages, created), None);
        let cached = usage(Some(600), Some(50), Some(500), None);
        assert_eq!(cost(&price, Wire::Chat, cached), None);
    }

    /// A class reported as 0, or not reported, costs nothing whether or not it has a rate.
    #[test]
    fn cost_a_count_of_zero_in_a_class_without_a_rate_keeps_the_cost() {
        let price = price(&[("input", "3"), ("output", "15")]);
        let zero = usage(Some(100), Some(50), Some(0), Some(0));
        assert_eq!(
            cost(&price, Wire::Messages, zero).as_deref(),
            Some("0.00105")
        );
        let unreported = usage(Some(100), Some(50), None, None);
        assert_eq!(
            cost(&price, Wire::Chat, unreported).as_deref(),
            Some("0.00105")
        );
    }

    /// Counters that contradict each other (more cached than prompt tokens) have no cost.
    #[test]
    fn cost_contradicting_counters_have_none() {
        let price = price(&ALL);
        let contradicting = usage(Some(100), Some(50), Some(500), None);
        assert_eq!(cost(&price, Wire::Chat, contradicting), None);
    }
}
