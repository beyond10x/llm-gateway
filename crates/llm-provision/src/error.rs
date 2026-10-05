//! Fixed, safe refusals. Every variant names its own code and carries no provider text.

use crate::closed::closed_enum;

closed_enum! {
    /// Why a hosting command was refused.
    ///
    /// Each variant has one stable kebab-case code and a fixed message. No upstream response
    /// body, URL, credential or account value ever reaches one of these, and
    /// [`HostingError::ALL`] is generated from the variant list so that a reader of the
    /// contract is never shown a partial inventory of what this crate can refuse.
    HostingError {
        /// The controller's scope declares an unusable ceiling.
        InvalidPolicy => "invalid-policy", "invalid hosting policy";
        /// The specification names another scope or asks for more than the ceiling allows.
        InvalidSpec => "invalid-spec", "deployment specification is outside this scope";
        /// The authorization names another budget ledger than the controller's scope.
        Unauthorized => "unauthorized", "compute authorization names a different budget ledger";
        /// A deployment with this identity is already declared.
        Duplicate => "duplicate", "deployment identifier already exists";
        /// No such deployment is declared in this controller.
        Missing => "missing", "deployment does not exist";
        /// The deployment's current phase does not permit this command.
        WrongPhase => "wrong-phase", "operation is not valid in this phase";
        /// The supplied clock moved backwards.
        ClockReversed => "clock-reversed", "hosting clock moved backwards";
        /// The resource ceiling is already reached; nothing was submitted.
        CapacityExceeded => "capacity-exceeded", "resource ceiling would be exceeded";
        /// Another controller currently holds the ownership lease.
        LeaseHeld => "lease-held", "another controller holds the ownership lease";
        /// This controller holds no current lease for the deployment.
        LeaseLost => "lease-lost", "this controller holds no current lease";
        /// A newer epoch owns the resource; this controller must not mutate it.
        StaleEpoch => "stale-epoch", "a newer epoch owns this resource";
        /// The resource under that name is a different incarnation or a different owner.
        ForeignResource => "foreign-resource", "the observed resource is not the one owned here";
        /// The outcome of a submitted mutation is unknown; the obligation stays open.
        AmbiguousMutation => "ambiguous-mutation", "the mutation outcome is unknown";
        /// The provider does not guarantee that a repeated request id allocates only once.
        IdempotencyUnsupported =>
            "idempotency-unsupported", "the provider does not honour a repeated request id";
    }
}

impl std::error::Error for HostingError {}

#[cfg(test)]
mod tests {
    use super::HostingError;
    use std::collections::BTreeSet;

    #[test]
    fn every_refusal_has_a_distinct_code_and_a_safe_fixed_message() {
        let codes: BTreeSet<_> = HostingError::ALL.iter().map(|error| error.code()).collect();
        assert_eq!(codes.len(), HostingError::ALL.len());
        for error in HostingError::ALL {
            assert!(error.code().is_ascii() && !error.code().is_empty());
            let rendered = error.to_string();
            assert!(rendered.is_ascii() && !rendered.is_empty());
        }
    }
}
