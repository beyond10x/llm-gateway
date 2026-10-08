//! Fixed, safe diagnostics.
//!
//! A refusal names its own stable code and a message this crate wrote. No caller byte, no
//! credential and no upstream response text ever reaches a diagnostic.
//!
//! Every error type here is generated with its own `ALL` constant by a macro, so `ALL` cannot
//! fall behind the variants; the crate's own tests then iterate `ALL` through an exhaustive
//! match, so a variant cannot be added without a case that reaches it. Both halves are needed:
//! with a hand-written `ALL`, an exhaustive match forces an *arm* for a new variant but the loop
//! never reaches it, and the claim above was false for these four types until the macro landed.

use std::fmt;

/// Declares the refusal set once.
///
/// This macro is the check, not a convenience. A variant cannot exist without a wire code, an
/// HTTP status, a fixed message and an entry in [`RefusalCode::ALL`], because all four are
/// generated from the same list. The previous hand-written enum needed a test that parsed this
/// file to notice a variant missing from `ALL`, and that parser could be fooled by a brace
/// inside a doc comment.
/// Declares a closed error set: the enum, its `ALL`, its `Display` message, and `Error`.
///
/// Hand-writing `ALL` beside an enum is the defect this removes. An exhaustive match over a
/// variant is not the same as `ALL` containing it, so a variant added without an `ALL` entry
/// compiles and is never provoked by a test that iterates `ALL`.
macro_rules! closed_enum {
    (
        $(#[$outer:meta])*
        $name:ident { $( $(#[$meta:meta])* $variant:ident => $message:literal; )+ }
    ) => {
        $(#[$outer])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $( $(#[$meta])* $variant, )+
        }

        impl $name {
            /// Every variant, complete by construction.
            pub const ALL: &'static [Self] = &[ $( Self::$variant, )+ ];
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(match self { $( Self::$variant => $message, )+ })
            }
        }

        impl std::error::Error for $name {}
    };
}

macro_rules! refusal_codes {
    ($( $(#[$meta:meta])* $variant:ident => $wire:literal, $status:literal, $reason:literal; )+) => {
        /// Every way the gateway refuses a request. The set is closed, and it is published in
        /// `docs/gateway.md` with the same code, status and message this type returns.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum RefusalCode {
            $( $(#[$meta])* $variant, )+
        }

        impl RefusalCode {
            /// Every refusal this gateway can produce. Complete by construction.
            pub const ALL: &'static [Self] = &[ $( Self::$variant, )+ ];

            /// The stable identifier a client reads. It never changes for a given meaning.
            pub const fn wire(self) -> &'static str {
                match self { $( Self::$variant => $wire, )+ }
            }

            /// The HTTP status carrying this refusal.
            pub const fn status(self) -> u16 {
                match self { $( Self::$variant => $status, )+ }
            }

            /// A fixed diagnostic written by this crate. It quotes nothing the caller sent.
            pub const fn reason(self) -> &'static str {
                match self { $( Self::$variant => $reason, )+ }
            }
        }
    };
}

refusal_codes! {
    /// A read-only surface was sent a request body.
    BodyNotAllowed => "body-not-allowed", 400,
        "the inspection surface accepts no request body";
    /// No credential was presented at all.
    CredentialAbsent => "credential-absent", 401,
        "no owner credential was presented";
    /// Something was presented, but it is not a bearer token.
    CredentialMalformed => "credential-malformed", 401,
        "the owner credential is not a bearer token";
    /// A well-formed bearer token that is not the owner's.
    CredentialRejected => "credential-rejected", 401,
        "the presented owner credential was rejected";
    /// A method the path does not take: a write on the read-only inspection surface, or
    /// anything but `POST` on a relayed wire. The `allow` header names what it takes.
    MethodNotAllowed => "method-not-allowed", 405,
        "this path does not accept that method";
    /// A relayed request body over its byte bound, declared or decoded.
    BodyTooLarge => "body-too-large", 413,
        "the request body exceeds its byte bound";
    /// A relayed request body that ended, or stalled past the read timeout, before it was
    /// complete.
    BodyIncomplete => "body-incomplete", 400,
        "the request body ended or stalled before it was complete";
    /// A relayed request body that is not one JSON object.
    BodyNotJson => "body-not-json", 400,
        "the request body is not one JSON object";
    /// A relayed request body with no top-level `model` string.
    ModelAbsent => "model-absent", 400,
        "the request body names no model";
    /// No relayed model has the alias the body names.
    ModelUnknown => "model-unknown", 404,
        "no such model";
    /// The model does not declare the wire of the path it was sent to.
    WireNotServed => "wire-not-served", 400,
        "the model is not served on this wire";
    /// The body offers tools (a non-empty top-level `tools` array) to a model whose tool calling
    /// is [`crate::ToolCalling::Absent`].
    ToolsNotServed => "tools-not-served", 400,
        "the model does not serve tool calls";
    /// The embedding handed out no target for the model.
    TargetUnavailable => "target-unavailable", 503,
        "no model target is available";
    /// The embedding reports the model's target still starting when the request's hold budget
    /// passed (row W6). The only refusal with a `retry-after`.
    ModelColdStart => "model-cold-start", 503,
        "the model is still starting; ask again after the retry-after delay";
    /// The target could not be reached, closed without an answer, or answered 502, 503 or 504.
    UpstreamFailed => "upstream-failed", 502,
        "the model target could not be reached or failed; the next request asks for a replacement";
    /// More work arrived at once than the gateway admits.
    Overloaded => "overloaded", 503,
        "the gateway is already serving its maximum concurrent requests";
    /// No such resource on this gateway.
    PathUnknown => "path-unknown", 404,
        "no such gateway resource";
    /// The request is not HTTP this gateway parses: its head, or the chunked framing of a relayed
    /// body.
    RequestMalformed => "request-malformed", 400,
        "the request is not well-formed HTTP";
    /// The request head exceeds its configured bound.
    RequestTooLarge => "request-too-large", 431,
        "the request head exceeds its byte bound";
    /// No route declares that alias.
    RouteUnknown => "route-unknown", 404,
        "no such route alias";
    /// The gateway has not been marked ready, or has stopped.
    Unavailable => "unavailable", 503,
        "the gateway is not ready to serve inspection";
}

impl fmt::Display for RefusalCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.wire())
    }
}

impl std::error::Error for RefusalCode {}

closed_enum! {
    /// Why an [`crate::OwnerToken`] cannot be built. The rejected material is never named.
    TokenError {
        /// A credential of zero bytes.
        Empty => "an owner credential cannot be empty";
        /// Longer than 4096 bytes.
        TooLarge => "an owner credential cannot exceed 4096 bytes";
        /// Holds a byte that is not printable US-ASCII.
        NotPrintableAscii => "an owner credential must be printable US-ASCII";
    }
}

closed_enum! {
    /// Why a verifier cannot be composed.
    VerifierError {
        /// A shared secret shorter than 32 bytes is refused rather than accepted and warned about.
        SecretTooShort => "a shared owner secret must be at least 32 bytes";
    }
}

closed_enum! {
    /// Why a label is not a usable identifier. Mirrors the catalog's own identifier rule.
    LabelError {
        /// Empty, or longer than 256 bytes.
        Length => "a label must be 1..=256 bytes";
        /// Holds a byte that is not printable US-ASCII.
        NotPrintableAscii => "a label must be printable US-ASCII";
    }
}

closed_enum! {
    /// Why a route inventory snapshot is not internally consistent.
    InventoryError {
        /// A route with no target, which no catalog can produce.
        RouteWithoutTarget => "a route declares no target";
        /// More than 64 targets on one route.
        TooManyTargets => "a route declares more than 64 targets";
        /// More than 4096 routes in one snapshot.
        TooManyRoutes => "the snapshot declares more than 4096 routes";
        /// Target positions are not the contiguous range starting at zero.
        NonContiguousPositions => "route target positions are not contiguous from zero";
        /// The same target identifier twice on one route.
        DuplicateTarget => "a route declares one target identifier twice";
        /// The same alias on two routes.
        DuplicateAlias => "two routes declare the same alias";
        /// A configuration digest that is not 64 lowercase hexadecimal characters.
        MalformedDigest => "a configuration digest must be 64 lowercase hex characters";
    }
}

closed_enum! {
    /// Why relayed models cannot be composed (row K11).
    RelayError {
        /// A model that declares no wire.
        NoWire => "a relayed model must declare at least one wire";
        /// A model that names one wire twice.
        RepeatedWire => "a relayed model names one wire twice";
        /// Two models with one alias.
        DuplicateModel => "two relayed models declare the same alias";
    }
}

closed_enum! {
    /// Why [`crate::RelayTargets::acquire`] handed out no target. Each is answered with its own
    /// refusal: `target-unavailable` or `model-cold-start`.
    TargetRefusal {
        /// No target can be had: the model is down, failed to start, or the embedding is
        /// stopping.
        Unavailable => "no model target is available";
        /// The model's target is still starting and the request's hold budget has passed
        /// (row W6). The client is told to ask again later.
        ColdStart => "the model is still starting";
    }
}

impl TargetRefusal {
    /// The refusal the client is answered with.
    pub const fn refusal(self) -> RefusalCode {
        match self {
            Self::Unavailable => RefusalCode::TargetUnavailable,
            Self::ColdStart => RefusalCode::ModelColdStart,
        }
    }
}
