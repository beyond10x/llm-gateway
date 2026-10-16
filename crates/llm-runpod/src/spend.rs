//! The spend check: a pod start that would pass a spend ceiling of the pool's account is
//! refused before any create (`spec/domains/runpod.yaml`, `SpendLedger`).
//!
//! Every amount is an integer number of micro-USD, so a ceiling compares exactly. The check
//! adds three things and compares the sum with each declared ceiling, the daily one first:
//!
//! * the account's reported spend, read afresh from the transport for each check
//!   ([`crate::RunpodTransport::account_spend`]);
//! * the running estimate of the pool's live pods: each one's declared price per hour times
//!   its time since submission;
//! * the estimate of the pod about to start: its declared price per hour times its startup
//!   deadline.
//!
//! An estimate is rounded up to the next micro-USD. A projection equal to its ceiling is
//! admitted; one micro-USD past it is refused. A read that fails or answers nothing, or a pod
//! with no declared price, is [`SpendRefusal::Unknown`]: no ceiling is ever assumed met.

use std::collections::BTreeMap;

use llm_provision::Identifier;

/// Milliseconds in one hour, the unit a price is declared per.
const HOUR_MS: u128 = 3_600_000;

/// Milliseconds in one UTC day, which the daily ceiling counts from.
pub(crate) const DAY_MS: u64 = 86_400_000;

/// The budget ledger of one pool's account: its spend ceilings and the price each declared
/// model's pods are estimated by. Amounts are micro-USD.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpendPolicy {
    /// The most the account may spend in one UTC day; `None` is no daily ceiling.
    pub daily_cap_micro_usd: Option<u64>,
    /// The most the account may spend in all; `None` is no total ceiling.
    pub total_cap_micro_usd: Option<u64>,
    /// The most each model's pod costs per hour, by model alias.
    pub price_per_hour_micro_usd: BTreeMap<Identifier, u64>,
}

impl SpendPolicy {
    /// Whether any ceiling is declared. Without one the check reads nothing and admits.
    pub const fn is_metered(&self) -> bool {
        self.daily_cap_micro_usd.is_some() || self.total_cap_micro_usd.is_some()
    }
}

/// The account's spend as the control plane reports it, in micro-USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountSpend {
    /// Since the start of the current UTC day.
    pub daily_micro_usd: u64,
    /// In all.
    pub total_micro_usd: u64,
}

/// Which spend ceiling a start would pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpendCeiling {
    Daily,
    Total,
}

impl SpendCeiling {
    /// The stable code: `daily` or `total`.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Total => "total",
        }
    }
}

/// Why the spend check refused a create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpendRefusal {
    /// The projected spend would pass this ceiling by this many micro-USD.
    Over {
        ceiling: SpendCeiling,
        over_micro_usd: u64,
    },
    /// The spend read failed or answered nothing, or a pod has no declared price.
    Unknown,
}

impl SpendRefusal {
    /// The stable code: `daily`, `total` or `unknown`.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Over { ceiling, .. } => ceiling.code(),
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for SpendRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Over {
                ceiling,
                over_micro_usd,
            } => write!(
                formatter,
                "the {} spend ceiling would be passed by {} USD",
                ceiling.code(),
                usd(*over_micro_usd)
            ),
            Self::Unknown => {
                formatter.write_str("the account's spend is unknown, so no ceiling is assumed met")
            }
        }
    }
}

impl std::error::Error for SpendRefusal {}

/// A micro-USD amount as USD with six decimals, `0.500000`.
pub fn usd(micro_usd: u64) -> String {
    format!("{}.{:06}", micro_usd / 1_000_000, micro_usd % 1_000_000)
}

/// A declared USD amount in micro-USD, rounded to the nearest: `None` unless it is finite,
/// above 0 and at least one micro-USD.
pub fn micro_usd(usd: f64) -> Option<u64> {
    if !usd.is_finite() || usd <= 0.0 {
        return None;
    }
    from_scaled((usd * 1e6).round()).filter(|&micro| micro >= 1)
}

/// A reported USD amount in micro-USD, rounded up so that spend is never understated: `None`
/// unless it is finite and not negative.
pub(crate) fn reported_micro_usd(usd: f64) -> Option<u64> {
    if !usd.is_finite() || usd < 0.0 {
        return None;
    }
    from_scaled((usd * 1e6).ceil())
}

/// A whole, non-negative `f64` that fits a `u64`.
fn from_scaled(scaled: f64) -> Option<u64> {
    // 2^64 is exactly representable; anything at or past it does not fit.
    if !(0.0..18_446_744_073_709_551_616.0).contains(&scaled) {
        return None;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "whole, non-negative and below 2^64, checked above"
    )]
    Some(scaled as u64)
}

/// What a pod at `price_per_hour` costs over `ms`, rounded up to the next micro-USD.
pub(crate) fn estimate(price_per_hour: u64, ms: u64) -> u64 {
    let product = u128::from(price_per_hour) * u128::from(ms);
    u64::try_from(product.div_ceil(HOUR_MS)).unwrap_or(u64::MAX)
}

/// The decision: `reported` plus `estimated` against each declared ceiling, the daily one first.
pub(crate) fn admit(
    policy: &SpendPolicy,
    reported: AccountSpend,
    estimated: u64,
) -> Result<(), SpendRefusal> {
    let checks = [
        (
            SpendCeiling::Daily,
            policy.daily_cap_micro_usd,
            reported.daily_micro_usd,
        ),
        (
            SpendCeiling::Total,
            policy.total_cap_micro_usd,
            reported.total_micro_usd,
        ),
    ];
    for (ceiling, cap, spent) in checks {
        let Some(cap) = cap else { continue };
        let projected = spent.saturating_add(estimated);
        if projected > cap {
            return Err(SpendRefusal::Over {
                ceiling,
                over_micro_usd: projected - cap,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{estimate, reported_micro_usd, usd};

    #[test]
    fn an_estimate_rounds_up_and_a_reported_amount_is_never_understated() {
        assert_eq!(estimate(2_000_000, 600_000), 333_334);
        assert_eq!(estimate(2_000_000, 1_800_000), 1_000_000);
        assert_eq!(estimate(u64::MAX, u64::MAX), u64::MAX);
        assert_eq!(reported_micro_usd(0.000_000_4), Some(1));
        assert_eq!(reported_micro_usd(0.0), Some(0));
        assert_eq!(reported_micro_usd(-0.5), None);
        assert_eq!(reported_micro_usd(1e300), None);
        assert_eq!(usd(333_889), "0.333889");
    }
}
