//! Adversary pass on story:hosted-base-path (wave 2026-10-16-w01): the authority/path split of
//! `HostedEndpoint::check` at its edges.

use llm_gateway::{AuthKind, BasePath};
use llm_gateway_cli::{BaseUrlError, HostedEndpoint, Scheme};

fn checked(url: &str, auth_kind: AuthKind) -> Result<(Scheme, String, Option<String>), BaseUrlError> {
    HostedEndpoint::check(url, auth_kind).map(|(scheme, authority, base)| {
        (
            scheme,
            authority.to_owned(),
            base.as_ref().map(BasePath::as_str).map(str::to_owned),
        )
    })
}

/// An IPv6 literal with a port and a path splits at the first `/` after the bracket, and a
/// credential over `http` to `[::1]` stays admitted.
#[test]
fn an_ipv6_literal_with_a_path_keeps_its_brackets_in_the_authority() {
    for (url, auth_kind, scheme, authority, base) in [
        ("http://[::1]:8000/api/v1", AuthKind::Bearer, Scheme::Http, "[::1]:8000", Some("/api/v1")),
        ("https://[::1]/v1/", AuthKind::ApiKey, Scheme::Https, "[::1]", Some("/v1")),
        ("http://[::1]/", AuthKind::Bearer, Scheme::Http, "[::1]", None),
        ("http://localhost:9/openai", AuthKind::ApiKey, Scheme::Http, "localhost:9", Some("/openai")),
    ] {
        assert_eq!(
            checked(url, auth_kind),
            Ok((scheme, authority.to_owned(), base.map(str::to_owned))),
            "{url}"
        );
    }
}

/// A `@`, `:` or `[` after the first `/` is a path byte, never user information, a port or a
/// host, and a path byte outside the rule refuses the URL; a bracket left open across the `/`
/// refuses it too.
#[test]
fn delimiters_after_the_first_slash_are_refused_as_path_bytes() {
    for url in [
        "https://api.example.test/@evil.test",
        "https://api.example.test/v1@evil.test",
        "https://api.example.test/a:b",
        "https://api.example.test:8443/x:1",
        "https://api.example.test/v1;a=1",
        "https://api.example.test/[::1]",
        "https://[::1/v1]",
        "https://[::1]x/v1",
        "https://api.example.test:/v1",
        "https://api.example.test/v1/.",
        "https://api.example.test/v1/..",
        "https://api.example.test/./",
    ] {
        assert_eq!(
            checked(url, AuthKind::Anonymous),
            Err(BaseUrlError::Malformed),
            "{url}"
        );
    }
}

/// A path changes nothing about the cleartext rule: a credential over `http` to a remote host
/// is `CleartextCredential` with or without one, and an anonymous endpoint there is admitted.
#[test]
fn a_path_neither_opens_nor_closes_the_cleartext_rule() {
    for auth_kind in [AuthKind::Bearer, AuthKind::ApiKey] {
        assert_eq!(
            checked("http://api.example.test/api/v1", auth_kind),
            Err(BaseUrlError::CleartextCredential)
        );
        assert_eq!(
            checked("http://127.0.0.1.example.test/v1", auth_kind),
            Err(BaseUrlError::CleartextCredential)
        );
    }
    assert_eq!(
        checked("http://api.example.test/api/v1", AuthKind::Anonymous),
        Ok((
            Scheme::Http,
            "api.example.test".to_owned(),
            Some("/api/v1".to_owned())
        ))
    );
}
