//! The read-only route snapshot the owner may inspect.
//!
//! An inventory is a credential-free set of facts about routes, handed to the gateway once at
//! composition. It is built by whatever composed the deployment — typically from
//! `llm_routing::Catalog::routes` and the provenance `llm_routing::Catalog::explain` already
//! computes without resolving anything. The snapshot holds no callback, so inspecting it cannot
//! provision anything.
//!
//! There is no *field* for a base URL, a secret reference or credential material. That is not
//! the same as "a URL cannot appear": a [`Label`] is any 1..=256 printable US-ASCII bytes, which
//! is what a catalog identifier is and also what a URL or an API key is. What holds is that an
//! inspection renders exactly the identifier bytes the composer supplied and nothing else, and
//! `provenance_renders_exactly_the_bytes_the_composer_supplied` pins that. The adapter that
//! builds an inventory is responsible for passing identifiers.

use crate::{
    auth::Authenticated,
    error::{InventoryError, LabelError},
    json::Writer,
};
use std::collections::BTreeMap;

const MAX_LABEL_BYTES: usize = 256;
const MAX_TARGETS_PER_ROUTE: usize = 64;
const MAX_ROUTES: usize = 4096;
const DIGEST_CHARACTERS: usize = 64;

/// Counts are facts about this snapshot, never an estimate and never a substituted default.
fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// An operator-defined identifier, validated by the same rule the catalog uses for its own.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Label(String);

impl Label {
    /// Accepts 1..=256 printable US-ASCII bytes.
    ///
    /// # Errors
    /// Returns [`LabelError`] for an empty, oversized or non-printable value.
    pub fn new(value: impl Into<String>) -> Result<Self, LabelError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_LABEL_BYTES {
            return Err(LabelError::Length);
        }
        if !value.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(LabelError::NotPrintableAscii);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How a target authenticates upstream. The *kind* is inspectable; the material is not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    Anonymous,
    Bearer,
    ApiKey,
}

impl AuthKind {
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Anonymous => "anonymous",
            Self::Bearer => "bearer",
            Self::ApiKey => "api-key",
        }
    }
}

/// How a target is billed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingKind {
    Metered,
    Subscription,
    SelfHosted,
}

impl BillingKind {
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Metered => "metered",
            Self::Subscription => "subscription",
            Self::SelfHosted => "self-hosted",
        }
    }
}

/// The identity of a binding, by operator-defined name. There is no field for an endpoint URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetProvenance {
    pub protocol: Label,
    pub provider: Label,
    pub account: Label,
    pub endpoint: Label,
    pub model: Label,
    pub binding_revision: Label,
}

/// Declared limits. A limit the snapshot's source did not report stays `None`: it is never
/// rendered, and never becomes zero or a substituted default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TargetLimits {
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
}

/// One ordered alternative of a route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetSummary {
    pub target_id: Label,
    pub position: usize,
    pub provenance: TargetProvenance,
    pub auth_kind: AuthKind,
    pub billing_kind: BillingKind,
    pub limits: TargetLimits,
}

/// One route and its explicitly ordered alternatives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSummary {
    route_id: Label,
    alias: Label,
    fallback_enabled: bool,
    targets: Vec<TargetSummary>,
}

impl RouteSummary {
    /// Builds a route snapshot, applying the catalog's own structural rules.
    ///
    /// # Errors
    /// Returns [`InventoryError`] when there is no target, more than 64, a repeated target
    /// identifier, or positions that are not the contiguous range from zero.
    pub fn new(
        route_id: Label,
        alias: Label,
        fallback_enabled: bool,
        mut targets: Vec<TargetSummary>,
    ) -> Result<Self, InventoryError> {
        if targets.is_empty() {
            return Err(InventoryError::RouteWithoutTarget);
        }
        if targets.len() > MAX_TARGETS_PER_ROUTE {
            return Err(InventoryError::TooManyTargets);
        }
        targets.sort_by_key(|target| target.position);
        let mut seen = BTreeMap::new();
        for (index, target) in targets.iter().enumerate() {
            if target.position != index {
                return Err(InventoryError::NonContiguousPositions);
            }
            if seen.insert(target.target_id.clone(), index).is_some() {
                return Err(InventoryError::DuplicateTarget);
            }
        }
        Ok(Self {
            route_id,
            alias,
            fallback_enabled,
            targets,
        })
    }

    pub fn route_id(&self) -> &Label {
        &self.route_id
    }

    pub fn alias(&self) -> &Label {
        &self.alias
    }

    pub fn fallback_enabled(&self) -> bool {
        self.fallback_enabled
    }

    pub fn targets(&self) -> &[TargetSummary] {
        &self.targets
    }

    fn write(&self, writer: &mut Writer) {
        writer.begin_object();
        writer.key("route_id");
        writer.string(self.route_id.as_str());
        writer.key("alias");
        writer.string(self.alias.as_str());
        writer.key("fallback_enabled");
        writer.boolean(self.fallback_enabled);
        writer.key("target_count");
        writer.number(count(self.targets.len()));
        writer.key("targets");
        writer.begin_array();
        for target in &self.targets {
            write_target(target, writer);
        }
        writer.end_array();
        writer.end_object();
    }
}

fn write_target(target: &TargetSummary, writer: &mut Writer) {
    writer.begin_object();
    writer.key("target_id");
    writer.string(target.target_id.as_str());
    writer.key("position");
    writer.number(count(target.position));
    writer.key("protocol");
    writer.string(target.provenance.protocol.as_str());
    writer.key("provider");
    writer.string(target.provenance.provider.as_str());
    writer.key("account");
    writer.string(target.provenance.account.as_str());
    writer.key("endpoint");
    writer.string(target.provenance.endpoint.as_str());
    writer.key("model");
    writer.string(target.provenance.model.as_str());
    writer.key("binding_revision");
    writer.string(target.provenance.binding_revision.as_str());
    writer.key("auth_kind");
    writer.string(target.auth_kind.wire());
    writer.key("billing_kind");
    writer.string(target.billing_kind.wire());
    // An unreported limit is absent. It is never zero and never a substituted configured value.
    if let Some(context_window) = target.limits.context_window {
        writer.key("context_window");
        writer.number(context_window);
    }
    if let Some(max_output_tokens) = target.limits.max_output_tokens {
        writer.key("max_output_tokens");
        writer.number(max_output_tokens);
    }
    writer.end_object();
}

/// An immutable snapshot of every route the owner may inspect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteInventory {
    config_digest: Option<String>,
    routes: BTreeMap<String, RouteSummary>,
}

impl RouteInventory {
    /// Builds a snapshot.
    ///
    /// # Errors
    /// Returns [`InventoryError`] for more than 4096 routes or a repeated alias.
    pub fn new(routes: Vec<RouteSummary>) -> Result<Self, InventoryError> {
        if routes.len() > MAX_ROUTES {
            return Err(InventoryError::TooManyRoutes);
        }
        let mut indexed = BTreeMap::new();
        for route in routes {
            if indexed
                .insert(route.alias.as_str().to_string(), route)
                .is_some()
            {
                return Err(InventoryError::DuplicateAlias);
            }
        }
        Ok(Self {
            config_digest: None,
            routes: indexed,
        })
    }

    /// Records the digest of the configuration this snapshot was taken from. A snapshot whose
    /// source reported no digest keeps `None`, and renders no digest at all.
    ///
    /// # Errors
    /// Returns [`InventoryError::MalformedDigest`] for anything but 64 lowercase hex characters.
    pub fn with_config_digest(mut self, digest: &str) -> Result<Self, InventoryError> {
        if digest.len() != DIGEST_CHARACTERS
            || !digest
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(InventoryError::MalformedDigest);
        }
        self.config_digest = Some(digest.to_string());
        Ok(self)
    }

    pub fn config_digest(&self) -> Option<&str> {
        self.config_digest.as_deref()
    }

    pub fn len(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    pub fn route(&self, alias: &str) -> Option<&RouteSummary> {
        self.routes.get(alias)
    }

    /// Renders every route. The proof argument is why the gateway's own request path cannot
    /// reach this before authenticating; it is not a capability, and an embedding building its
    /// own surface mints one from its own [`crate::Verdict`]. See [`crate::Authenticated`].
    pub fn inspect(&self, _owner: &Authenticated) -> String {
        let mut writer = Writer::new();
        writer.begin_object();
        self.write_digest(&mut writer);
        writer.key("route_count");
        writer.number(count(self.routes.len()));
        writer.key("routes");
        writer.begin_array();
        for route in self.routes.values() {
            route.write(&mut writer);
        }
        writer.end_array();
        writer.end_object();
        writer.finish()
    }

    /// Renders one route by alias, or nothing when the alias is not declared.
    pub fn inspect_alias(&self, _owner: &Authenticated, alias: &str) -> Option<String> {
        let route = self.routes.get(alias)?;
        let mut writer = Writer::new();
        writer.begin_object();
        self.write_digest(&mut writer);
        writer.key("route");
        route.write(&mut writer);
        writer.end_object();
        Some(writer.finish())
    }

    fn write_digest(&self, writer: &mut Writer) {
        if let Some(digest) = &self.config_digest {
            writer.key("config_digest");
            writer.string(digest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthKind, BillingKind, DIGEST_CHARACTERS, InventoryError, Label, LabelError,
        MAX_LABEL_BYTES, MAX_ROUTES, MAX_TARGETS_PER_ROUTE, RouteInventory, RouteSummary,
        TargetLimits, TargetProvenance, TargetSummary,
    };
    use crate::auth::Verdict;

    fn label(value: &str) -> Label {
        Label::new(value).unwrap()
    }

    fn target(id: &str, position: usize) -> TargetSummary {
        TargetSummary {
            target_id: label(id),
            position,
            provenance: TargetProvenance {
                protocol: label("responses"),
                provider: label("my-lab"),
                account: label("remote"),
                endpoint: label("remote-models"),
                model: label("large"),
                binding_revision: label("rev-1"),
            },
            auth_kind: AuthKind::Bearer,
            billing_kind: BillingKind::Metered,
            limits: TargetLimits::default(),
        }
    }

    fn route(id: &str, alias: &str) -> RouteSummary {
        RouteSummary::new(label(id), label(alias), false, vec![target("t0", 0)]).unwrap()
    }

    #[test]
    fn labels_follow_the_catalog_identifier_rule() {
        assert_eq!(Label::new("").unwrap_err(), LabelError::Length);
        assert_eq!(Label::new("x".repeat(257)).unwrap_err(), LabelError::Length);
        assert_eq!(
            Label::new("a b").unwrap_err(),
            LabelError::NotPrintableAscii
        );
        assert_eq!(label("coding-primary").as_str(), "coding-primary");
    }

    #[test]
    fn a_snapshot_applies_the_catalogs_structural_rules() {
        assert_eq!(
            RouteSummary::new(label("r"), label("a"), false, Vec::new()).unwrap_err(),
            InventoryError::RouteWithoutTarget
        );
        assert_eq!(
            RouteSummary::new(label("r"), label("a"), false, vec![target("t0", 1)]).unwrap_err(),
            InventoryError::NonContiguousPositions
        );
        assert_eq!(
            RouteSummary::new(
                label("r"),
                label("a"),
                false,
                vec![target("t0", 0), target("t0", 1)]
            )
            .unwrap_err(),
            InventoryError::DuplicateTarget
        );
        assert_eq!(
            RouteInventory::new(vec![route("r1", "a"), route("r2", "a")]).unwrap_err(),
            InventoryError::DuplicateAlias
        );
    }

    #[test]
    fn a_digest_is_either_a_real_digest_or_absent() {
        let inventory = RouteInventory::new(vec![route("r", "a")]).unwrap();
        assert_eq!(inventory.config_digest(), None);
        assert_eq!(
            inventory.clone().with_config_digest("").unwrap_err(),
            InventoryError::MalformedDigest
        );
        assert_eq!(
            inventory
                .clone()
                .with_config_digest(&"A".repeat(64))
                .unwrap_err(),
            InventoryError::MalformedDigest
        );
        assert_eq!(
            inventory
                .clone()
                .with_config_digest(&"0".repeat(63))
                .unwrap_err(),
            InventoryError::MalformedDigest
        );
        assert_eq!(
            inventory
                .clone()
                .with_config_digest(&"0".repeat(65))
                .unwrap_err(),
            InventoryError::MalformedDigest
        );
        assert_eq!(DIGEST_CHARACTERS, 64);
        let digest = "0".repeat(63) + "f";
        assert_eq!(
            inventory
                .with_config_digest(&digest)
                .unwrap()
                .config_digest(),
            Some(digest.as_str())
        );
    }

    #[test]
    fn every_label_error_has_a_value_that_provokes_it() {
        for expected in LabelError::ALL {
            let provocation = match expected {
                LabelError::Length => String::new(),
                LabelError::NotPrintableAscii => "a b".to_string(),
            };
            assert_eq!(Label::new(provocation).unwrap_err(), *expected);
        }
        assert!(Label::new("x".repeat(256)).is_ok());
        assert_eq!(Label::new("x".repeat(257)).unwrap_err(), LabelError::Length);
        assert_eq!(
            MAX_LABEL_BYTES, 256,
            "docs/gateway.md publishes this number"
        );
    }

    #[test]
    fn every_inventory_error_has_a_construction_that_provokes_it() {
        let many = |count: usize| {
            (0..count)
                .map(|position| target(&format!("t{position}"), position))
                .collect::<Vec<_>>()
        };
        for expected in InventoryError::ALL {
            let observed = match expected {
                InventoryError::RouteWithoutTarget => {
                    RouteSummary::new(label("r"), label("a"), false, Vec::new()).unwrap_err()
                }
                // Literals throughout: a bound compared with itself cannot detect the
                // bound being changed.
                InventoryError::TooManyTargets => {
                    RouteSummary::new(label("r"), label("a"), false, many(65)).unwrap_err()
                }
                InventoryError::TooManyRoutes => RouteInventory::new(
                    (0..=4096)
                        .map(|index| route(&format!("r{index}"), &format!("a{index}")))
                        .collect(),
                )
                .unwrap_err(),
                InventoryError::NonContiguousPositions => {
                    RouteSummary::new(label("r"), label("a"), false, vec![target("t0", 1)])
                        .unwrap_err()
                }
                InventoryError::DuplicateTarget => RouteSummary::new(
                    label("r"),
                    label("a"),
                    false,
                    vec![target("t0", 0), target("t0", 1)],
                )
                .unwrap_err(),
                InventoryError::DuplicateAlias => {
                    RouteInventory::new(vec![route("r1", "a"), route("r2", "a")]).unwrap_err()
                }
                InventoryError::MalformedDigest => RouteInventory::new(vec![route("r", "a")])
                    .unwrap()
                    .with_config_digest("")
                    .unwrap_err(),
            };
            assert_eq!(observed, *expected);
        }
    }

    #[test]
    fn the_admitted_side_of_every_structural_bound_is_measured_too() {
        let many = |count: usize| {
            (0..count)
                .map(|position| target(&format!("t{position}"), position))
                .collect::<Vec<_>>()
        };
        assert!(
            RouteSummary::new(label("r"), label("a"), false, many(64)).is_ok(),
            "the bound must admit exactly its maximum"
        );
        let routes = (0..4096)
            .map(|index| route(&format!("r{index}"), &format!("a{index}")))
            .collect::<Vec<_>>();
        assert_eq!(RouteInventory::new(routes).unwrap().len(), 4096);
        assert_eq!(
            MAX_TARGETS_PER_ROUTE, 64,
            "docs/gateway.md publishes this number"
        );
        assert_eq!(MAX_ROUTES, 4096, "docs/gateway.md publishes this number");
    }

    /// The crate cannot tell an identifier from a URL or a credential: it renders the bytes the
    /// composer supplied. This pins that boundary rather than leaving it to a fixture that
    /// happens to choose opaque names.
    #[test]
    fn provenance_renders_exactly_the_bytes_the_composer_supplied() {
        let owner = Verdict::Owner.into_authenticated().unwrap();
        let mut leaking = target("t0", 0);
        leaking.provenance.endpoint = label("https://models.example.invalid/v1");
        leaking.provenance.account = label("lab-llm-token");
        let route =
            RouteSummary::new(label("coding"), label("code"), false, vec![leaking]).unwrap();
        let rendered = RouteInventory::new(vec![route]).unwrap().inspect(&owner);
        assert!(
            rendered.contains("https://models.example.invalid/v1"),
            "the guarantee is that no *field* names a URL, not that a URL cannot be placed in \
             an identifier field; if this ever stops holding, docs/gateway.md must change: \
             {rendered}"
        );
        assert!(rendered.contains("lab-llm-token"), "{rendered}");
    }

    #[test]
    fn rendering_needs_proof_of_authentication_and_omits_unknown_limits() {
        let owner = Verdict::Owner.into_authenticated().unwrap();
        let inventory = RouteInventory::new(vec![route("coding", "code")]).unwrap();
        let rendered = inventory.inspect(&owner);
        assert!(rendered.contains("\"route_count\":1"), "{rendered}");
        assert!(rendered.contains("\"target_count\":1"), "{rendered}");
        assert!(!rendered.contains("context_window"), "{rendered}");
        assert!(!rendered.contains("config_digest"), "{rendered}");
        assert!(inventory.inspect_alias(&owner, "code").is_some());
        assert!(inventory.inspect_alias(&owner, "missing").is_none());
    }
}
