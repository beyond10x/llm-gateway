//! Identifiers and the resource identity that ownership is fenced on.

const MAX_IDENTIFIER_BYTES: usize = 256;

/// Refusal of a value that is not a valid [`Identifier`].
///
/// The message is fixed and never quotes the refused value: a hosting identifier can carry an
/// operator's account or deployment naming, and a diagnostic is not a place for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidIdentifier;

impl std::fmt::Display for InvalidIdentifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a hosting identifier must be 1..=256 printable ASCII bytes")
    }
}

impl std::error::Error for InvalidIdentifier {}

/// An operator-defined or provider-assigned identifier.
///
/// The rules mirror `llm_core::Id` deliberately: this crate carries no dependency, so the
/// hosting contract stays usable by a provider adapter that links nothing else. A value that
/// is valid here is valid there.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Identifier(String);

impl Identifier {
    /// Builds a validated identifier.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidIdentifier`] when the value is empty, longer than 256 bytes, or holds
    /// anything other than printable ASCII.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidIdentifier> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_IDENTIFIER_BYTES
            || !value.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(InvalidIdentifier);
        }
        Ok(Self(value))
    }

    /// Builds an identifier from a value this crate generated itself.
    ///
    /// Used only for the documented evidence constants and the fake provider's generated
    /// incarnations, all of which are printable ASCII by construction. Keeping it crate-private
    /// is what lets every public entry point stay panic-free.
    pub(crate) fn generated(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Identifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The identity a hosting resource is owned and fenced by.
///
/// A **name is not an identity**. A provider may hand the same name to a later resource once the
/// previous one is gone, so a controller that matched on name alone would adopt, bill against and
/// eventually stop somebody else's resource. The provider-assigned `incarnation` is what makes two
/// resources that share a name distinguishable across epochs, and it is part of equality.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceKey {
    pub provider: Identifier,
    pub account: Identifier,
    pub name: Identifier,
    pub incarnation: Identifier,
}

impl ResourceKey {
    /// Whether two keys name the same slot at the same provider and account.
    ///
    /// True together with `self != other` is exactly the name-reuse case: the same name, a
    /// different resource.
    pub fn same_name(&self, other: &Self) -> bool {
        self.provider == other.provider && self.account == other.account && self.name == other.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_refuse_empty_oversize_and_non_printable_values() {
        assert!(Identifier::new("pool-slot").is_ok());
        assert_eq!(Identifier::new(""), Err(InvalidIdentifier));
        assert_eq!(Identifier::new("pool slot"), Err(InvalidIdentifier));
        assert_eq!(Identifier::new("pool\nslot"), Err(InvalidIdentifier));
        assert!(Identifier::new("p".repeat(MAX_IDENTIFIER_BYTES)).is_ok());
        assert_eq!(
            Identifier::new("p".repeat(MAX_IDENTIFIER_BYTES + 1)),
            Err(InvalidIdentifier)
        );
    }

    #[test]
    fn a_reused_name_is_a_different_key() {
        let name = Identifier::new("pool-slot").expect("name");
        let provider = Identifier::new("fake").expect("provider");
        let account = Identifier::new("account-1").expect("account");
        let first = ResourceKey {
            provider: provider.clone(),
            account: account.clone(),
            name: name.clone(),
            incarnation: Identifier::new("one").expect("incarnation"),
        };
        let second = ResourceKey {
            incarnation: Identifier::new("two").expect("incarnation"),
            ..first.clone()
        };
        assert!(first.same_name(&second));
        assert_ne!(first, second);
    }
}
