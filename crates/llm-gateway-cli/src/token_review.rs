//! The binary's caller review (story:gateway-sa-token-ingress): a Kubernetes service-account
//! token checked with the cluster's TokenReview API.

use crate::{config::Deployment, refusal::Refusal};
use llm_gateway::{CallerReview, CallerToken, Review};
use std::{fmt, sync::Arc, time::Duration};

/// The path of the TokenReview API, `authentication.k8s.io/v1`.
pub const TOKEN_REVIEW_PATH: &str = "/apis/authentication.k8s.io/v1/tokenreviews";

/// The bounds of the review. [`ReviewBounds::default`] is what the binary serves with and what
/// `docs/gateway.md` publishes; a test may shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewBounds {
    /// One review, from opening the connection to the last byte of its answer.
    pub timeout: Duration,
    /// Reviews under way at once; one more is refused at once.
    pub concurrent: usize,
    /// Reviews kept, admissions and rejections together.
    pub entries: usize,
    /// The longest an admission is kept, never past the token's own expiry.
    pub admission: Duration,
    /// The longest a rejection is kept.
    pub rejection: Duration,
}

impl Default for ReviewBounds {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            concurrent: 8,
            entries: 1024,
            admission: Duration::from_secs(60),
            rejection: Duration::from_secs(3),
        }
    }
}

/// The clock the review reads, in milliseconds since the Unix epoch.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Reviews a caller's token with the cluster's TokenReview API.
pub struct TokenReviewer {
    bounds: ReviewBounds,
}

impl fmt::Debug for TokenReviewer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenReviewer")
            .field("bounds", &self.bounds)
            .finish_non_exhaustive()
    }
}

impl TokenReviewer {
    /// The same reviewer under `bounds`.
    #[must_use]
    pub fn with_bounds(mut self, bounds: ReviewBounds) -> Self {
        self.bounds = bounds;
        self
    }

    /// The same reviewer reading `clock`.
    #[must_use]
    pub fn with_clock(self, _clock: Clock) -> Self {
        self
    }
}

impl CallerReview for TokenReviewer {
    fn review(&self, _presented: &CallerToken) -> Review {
        Review::Unavailable
    }
}

/// The reviewer a document with `ingress_auth = "service-account-token"` serves with.
///
/// # Errors
/// A `token-review:*` [`Refusal`].
pub fn token_reviewer(_deployment: &Deployment) -> Result<Option<TokenReviewer>, Refusal> {
    Ok(None)
}
