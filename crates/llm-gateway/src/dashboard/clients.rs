//! Which clients a relayed model serves, read from its last relayed calls
//! (story:hosted-status-probe; docs/gateway.md "The status list" and "Bounds").
//!
//! Each call a target answered is kept as who sent it (whether its `user-agent` names Claude
//! Code) and how it was answered, the last [`SERVED_CLIENTS_CALLS`] of them per model. The record
//! says something only when it holds at least [`SERVED_CLIENTS_MIN_CALLS`] calls of Claude Code
//! and as many of other clients, and every one of them keeps the rule: Claude Code answered 2xx,
//! every other client answered `429` with an error of type `rate_limit_error`. It reads what the
//! gateway relayed and changes nothing a call carries.

use super::{SERVED_CLIENTS_CALLS, SERVED_CLIENTS_MIN_CALLS};
use std::collections::VecDeque;

/// The reason a model whose record shows that it answers Claude Code and rate-limits every other
/// client is listed with (`GET /v1/status`, the dashboard's route row).
pub const SERVES_CLAUDE_CODE_CLIENTS_ONLY: &str = "serves-claude-code-clients-only";

/// How a target answered one call, as far as the rule cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answered {
    /// A 2xx status.
    Success,
    /// `429` with an error of type `rate_limit_error`.
    RateLimited,
    /// Anything else.
    Other,
}

/// One relayed call: whether Claude Code sent it, and how it was answered.
#[derive(Debug, Clone, Copy)]
struct Seen {
    claude_code: bool,
    answered: Answered,
}

/// One model's last relayed calls.
#[derive(Debug, Clone, Default)]
pub(super) struct ServedClients {
    calls: VecDeque<Seen>,
}

impl ServedClients {
    /// Keeps a call the target answered `status`, from Claude Code when `claude_code`, whose
    /// error was a `rate_limit_error` when `rate_limit_error`; the oldest call goes once more
    /// than [`SERVED_CLIENTS_CALLS`] are kept.
    pub(super) fn note(&mut self, claude_code: bool, status: u16, rate_limit_error: bool) {
        let answered = match status {
            200..=299 => Answered::Success,
            429 if rate_limit_error => Answered::RateLimited,
            _ => Answered::Other,
        };
        self.calls.push_back(Seen {
            claude_code,
            answered,
        });
        while self.calls.len() > SERVED_CLIENTS_CALLS {
            self.calls.pop_front();
        }
    }

    /// [`SERVES_CLAUDE_CODE_CLIENTS_ONLY`] when every kept call keeps the rule and each side
    /// has at least [`SERVED_CLIENTS_MIN_CALLS`] of them; `None` otherwise.
    pub(super) fn reason(&self) -> Option<&'static str> {
        let (mut claude_code, mut others) = (0, 0);
        for call in &self.calls {
            match (call.claude_code, call.answered) {
                (true, Answered::Success) => claude_code += 1,
                (false, Answered::RateLimited) => others += 1,
                _ => return None,
            }
        }
        (claude_code >= SERVED_CLIENTS_MIN_CALLS && others >= SERVED_CLIENTS_MIN_CALLS)
            .then_some(SERVES_CLAUDE_CODE_CLIENTS_ONLY)
    }
}

#[cfg(test)]
mod tests {
    use super::{SERVES_CLAUDE_CODE_CLIENTS_ONLY, ServedClients};

    fn record(calls: &[(bool, u16, bool)]) -> ServedClients {
        let mut record = ServedClients::default();
        for (claude_code, status, rate_limit_error) in calls {
            record.note(*claude_code, *status, *rate_limit_error);
        }
        record
    }

    #[test]
    fn the_rule_holds_only_with_enough_calls_of_each_side_all_keeping_it() {
        let holds = [
            (true, 200, false),
            (false, 429, true),
            (true, 204, false),
            (false, 429, true),
        ];
        assert_eq!(
            record(&holds).reason(),
            Some(SERVES_CLAUDE_CODE_CLIENTS_ONLY)
        );
        for broken in [
            (false, 200, false),
            (false, 429, false),
            (true, 429, true),
            (true, 500, false),
            (false, 503, true),
        ] {
            let mut calls = holds.to_vec();
            calls.push(broken);
            assert_eq!(record(&calls).reason(), None, "{broken:?}");
        }
        assert_eq!(record(&holds[..3]).reason(), None);
        assert_eq!(record(&[]).reason(), None);
    }

    #[test]
    fn a_broken_call_counts_until_twenty_calls_came_after_it() {
        let mut calls = vec![(false, 200, false)];
        for index in 0..19 {
            calls.push(if index % 2 == 0 {
                (true, 200, false)
            } else {
                (false, 429, true)
            });
        }
        assert_eq!(record(&calls).reason(), None);
        calls.push((true, 200, false));
        assert_eq!(
            record(&calls).reason(),
            Some(SERVES_CLAUDE_CODE_CLIENTS_ONLY)
        );
    }
}
