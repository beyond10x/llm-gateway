//! The translation port (story:gateway-translation): how the relay hands a request on a wire
//! its model's upstream does not speak to a translator the embedding composed, and how that
//! translator writes the answer back.
//!
//! The crate translates nothing itself and links nothing that could: a [`Translator`] is
//! injected, as the target source is. The relay calls [`Translator::prepare`] before it asks
//! for a target, so a request the translator refuses never wakes a pod; it then acquires the
//! target, counting a cold start as on the byte relay, and calls [`Translation::run`] with that
//! target and a [`TranslatedAnswer`] over the client's connection. The target is released after
//! the last byte. The specification is step 5 of the `Relay` command of
//! `spec/domains/gateway.yaml`.

use crate::{
    error::RefusalCode,
    relay::{RelayModel, RelayTarget, Wire},
};

/// One request the relay hands a translator: the wire the client spoke, the wire the model's
/// upstream speaks (its first declared wire), the model, the client's body exactly as read, and
/// the value of the client's `anthropic-beta` header line as the head carried it, unchecked. The
/// head refuses a repeated header (`request-malformed`), so there is at most one line, which may
/// name several betas separated by commas. No header of the client's is forwarded verbatim: the
/// value reaches the translator, which decides what, if anything, of it the upstream is sent.
#[derive(Debug, Clone, Copy)]
pub struct TranslationRequest<'a> {
    pub ingress: Wire,
    pub upstream: Wire,
    pub model: &'a RelayModel,
    pub body: &'a [u8],
    pub anthropic_beta: Option<&'a str>,
}

/// Translates a request from the client's wire to the upstream's, and its answer back.
pub trait Translator: Send + Sync {
    /// Decodes and checks `request` before any target is asked for.
    ///
    /// # Errors
    /// The refusal the client is answered with, nothing having been asked of the target
    /// source: `translation-unsupported`, `translation-invalid` or `translation-opaque-state`.
    fn prepare(
        &self,
        request: &TranslationRequest<'_>,
    ) -> Result<Box<dyn Translation>, RefusalCode>;
}

/// A prepared request, run once against the target the relay acquired for it.
pub trait Translation: Send {
    /// Sends the request to `target` and writes the answer through `answer`. A translation
    /// that has written the head answers `Ok` whatever happened after it: what the client saw
    /// is the answer.
    ///
    /// # Errors
    /// Only before the head is written: [`TranslationFailure::UpstreamFailed`] when the target
    /// could not be reached or failed, which the relay reports to the target source and answers
    /// `upstream-failed`, or [`TranslationFailure::Refused`] with the code the client is
    /// answered with (`upstream-refused`, `translation-unrepresentable`).
    fn run(
        self: Box<Self>,
        target: &dyn RelayTarget,
        answer: &mut dyn TranslatedAnswer,
    ) -> Result<(), TranslationFailure>;
}

/// Why a translation wrote no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslationFailure {
    /// The target could not be reached, timed out or was unavailable before any output: the
    /// relay reports it to the target source by its authority, as on the byte relay.
    UpstreamFailed,
    /// Any other failure before output, answered with this code; the target is kept.
    Refused(RefusalCode),
}

/// How a translated answer's body is framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerFraming {
    /// A stream of pieces of unknown total length: chunked for an HTTP/1.1 client, ended by
    /// closing the connection for an HTTP/1.0 one.
    Stream,
    /// Exactly this many bytes, declared as `content-length`.
    Length(u64),
}

/// The client's side of a translated answer, written by the relay. Every write has its own
/// deadline, the gateway's `read_timeout`, so a client that stops reading cannot hold the
/// connection. It is `Send`, so a translator may hand it to a sink its runtime requires to be.
pub trait TranslatedAnswer: Send {
    /// Writes the head once: `status`, `content-type`, `cache-control: no-store`,
    /// `connection: close` and the framing. `false` when the client is gone, or a head was
    /// already written; nothing more reaches the client then.
    fn head(&mut self, status: u16, content_type: &str, framing: AnswerFraming) -> bool;

    /// Writes one piece of the body after the head. `false` when the client is gone, no head
    /// was written, or the piece would pass a declared length.
    fn body(&mut self, bytes: &[u8]) -> bool;

    /// Whether the client has closed its connection, or its sending side, while it waits: asked
    /// between two events of an answer nothing has been written of yet, so a translation can
    /// cancel its turn. It writes nothing. `false`, the default, for a writer that cannot tell.
    fn gone(&mut self) -> bool {
        false
    }

    /// Tells the relay the status the target's answer head carried, whatever it was, once it
    /// arrived; the last one told counts. A translation that writes no answer, refused or
    /// failed, is recorded with it as its target status, as the byte relay records the status
    /// it read (story:translated-refusal-target-status). Ignored by default.
    fn target_status(&mut self, _status: u16) {}
}
