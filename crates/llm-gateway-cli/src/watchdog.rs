//! `b10x-llm-gateway watchdog`: a separate process that terminates the pods the gateway owns
//! when the gateway cannot (story:pod-watchdog).

use crate::{
    cli::WatchdogCli,
    refusal::{Refusal, StartupRefusal},
};

/// The `watchdog` command: load the document, then watch.
pub(crate) fn main(cli: &WatchdogCli, _pods: &crate::ProxyConnector) -> std::process::ExitCode {
    let outcome = crate::serve::document_path(cli.config.clone())
        .and_then(|path| crate::load(&path))
        .and_then(|_| {
            Err(Refusal::new(
                StartupRefusal::ConfigValue,
                "watchdog: the document has no [watchdog] table".to_owned(),
            ))
        });
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(refusal) => {
            crate::serve::report(format_args!("refused {refusal}"));
            std::process::ExitCode::FAILURE
        }
    }
}
