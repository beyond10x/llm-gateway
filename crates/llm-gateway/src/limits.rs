//! The owner's spend ceilings, `GET /v1/limits` (`llm-gateway.gateway.LimitsRoute`,
//! story:limits-read): each ceiling the embedding's spend check enforces, with its window, what
//! the check counts as spent in it, what is left and the last start it refused. The gateway
//! computes nothing of it but `left`: the embedding reads the rest from its own check
//! ([`crate::RelayTargets::spend_limits`]).

/// The path of the read.
pub(crate) const PATH: &str = "/v1/limits";

/// The read's format.
const FORMAT: &str = "llm-gateway.limits/1";

/// Which spend ceiling an entry is (`llm-gateway.gateway.LimitCeiling`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitCeiling {
    /// The current UTC day's.
    Daily,
    /// The 30 days up to now.
    Total,
}

impl LimitCeiling {
    /// The stable code: `daily` or `total`.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Total => "total",
        }
    }
}

/// The last start a ceiling refused: when, in Unix milliseconds, and for which model alias.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitRefusal {
    pub at_ms: u64,
    pub model: String,
}

/// One spend ceiling as the embedding's spend check counts it at the instant of the read
/// (`llm-gateway.gateway.LimitEntry`). Amounts are micro-USD; instants Unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendLimit {
    /// The provider that declares the ceiling.
    pub provider: String,
    pub ceiling: LimitCeiling,
    pub cap_micro_usd: u64,
    pub window_start_ms: u64,
    pub window_end_ms: u64,
    /// What the check counts against the ceiling in the window; `None` when it cannot count,
    /// and refuses every start.
    pub spent_micro_usd: Option<u64>,
    /// The last start this ceiling refused; `None` when it refused none.
    pub last_refusal: Option<LimitRefusal>,
}

impl SpendLimit {
    /// The cap less what is spent, never below 0; `None` when the spend is unknown.
    pub fn left_micro_usd(&self) -> Option<u64> {
        self.spent_micro_usd
            .map(|spent| self.cap_micro_usd.saturating_sub(spent))
    }
}

/// `{"format":"llm-gateway.limits/1","limits":[…]}`, `limits` in the order given.
pub(crate) fn body(limits: &[SpendLimit]) -> String {
    let mut writer = crate::json::Writer::new();
    writer.begin_object();
    writer.key("format");
    writer.string(FORMAT);
    writer.key("limits");
    writer.begin_array();
    for limit in limits {
        writer.begin_object();
        writer.key("provider");
        writer.string(&limit.provider);
        writer.key("ceiling");
        writer.string(limit.ceiling.code());
        writer.key("cap_usd");
        writer.string(&usd(limit.cap_micro_usd));
        writer.key("window_start_ms");
        writer.number(limit.window_start_ms);
        writer.key("window_end_ms");
        writer.number(limit.window_end_ms);
        for (key, amount) in [
            ("spent_usd", limit.spent_micro_usd),
            ("left_usd", limit.left_micro_usd()),
        ] {
            writer.key(key);
            match amount {
                Some(amount) => writer.string(&usd(amount)),
                None => writer.null(),
            }
        }
        writer.key("last_refusal");
        match &limit.last_refusal {
            Some(refusal) => {
                writer.begin_object();
                writer.key("at_ms");
                writer.number(refusal.at_ms);
                writer.key("model");
                writer.string(&refusal.model);
                writer.end_object();
            }
            None => writer.null(),
        }
        writer.end_object();
    }
    writer.end_array();
    writer.end_object();
    writer.finish()
}

/// A micro-USD amount as USD with six decimals, `0.500000`.
fn usd(micro_usd: u64) -> String {
    format!("{}.{:06}", micro_usd / 1_000_000, micro_usd % 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::{LimitCeiling, LimitRefusal, SpendLimit, body};

    fn limit(ceiling: LimitCeiling, spent: Option<u64>) -> SpendLimit {
        SpendLimit {
            provider: "runpod".to_owned(),
            ceiling,
            cap_micro_usd: 10_000_000,
            window_start_ms: 0,
            window_end_ms: 86_400_000,
            spent_micro_usd: spent,
            last_refusal: None,
        }
    }

    /// Every key in order; left is the cap less spent, 0 past the cap, and `null` with spent
    /// when the spend is unknown.
    #[test]
    fn an_entry_writes_every_key_in_order_with_left_clamped_and_unknown_null() {
        let mut over = limit(LimitCeiling::Daily, Some(12_000_001));
        over.last_refusal = Some(LimitRefusal {
            at_ms: 7,
            model: "small".to_owned(),
        });
        let unknown = limit(LimitCeiling::Total, None);
        assert_eq!(
            body(&[over, unknown]),
            "{\"format\":\"llm-gateway.limits/1\",\"limits\":[\
             {\"provider\":\"runpod\",\"ceiling\":\"daily\",\"cap_usd\":\"10.000000\",\
             \"window_start_ms\":0,\"window_end_ms\":86400000,\"spent_usd\":\"12.000001\",\
             \"left_usd\":\"0.000000\",\"last_refusal\":{\"at_ms\":7,\"model\":\"small\"}},\
             {\"provider\":\"runpod\",\"ceiling\":\"total\",\"cap_usd\":\"10.000000\",\
             \"window_start_ms\":0,\"window_end_ms\":86400000,\"spent_usd\":null,\
             \"left_usd\":null,\"last_refusal\":null}]}"
        );
        assert_eq!(
            body(&[]),
            "{\"format\":\"llm-gateway.limits/1\",\"limits\":[]}"
        );
    }
}
