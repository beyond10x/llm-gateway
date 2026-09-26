//! The ownership state machine: the resolved hosting lifecycle.
//!
//! Every transition a hosting controller may make is in [`TRANSITIONS`]. Any pair outside it is
//! refused by [`Phase::may_become`], which is what "no UNMAPPED transition" means in code rather
//! than in a sentence.

use crate::closed::closed_enum;

closed_enum! {
    /// Where one owned deployment is in its lifecycle.
    ///
    /// The words are `llm-cost`'s where the meaning is `llm-cost`'s: `Uncertain`,
    /// `StopRequired`, `Stopped` and `Cancelled` are the budget ledger's phases and mean the
    /// same things here, so a hosting obligation and the ledger obligation it feeds are not two
    /// vocabularies for one fact. `Requested`, `Active` and `Disowned` are the hosting-specific
    /// additions: a submitted create, an observed live resource, and a resource a newer epoch
    /// has taken over.
    Phase {
        /// Recorded and authorized. Nothing has been submitted; nothing can be billed.
        Declared => "declared", "declared";
        /// A create mutation was accepted. Not yet confirmed against an inventory.
        Requested => "requested", "requested";
        /// Observed present under this record's exact resource key.
        Active => "active", "active";
        /// The resource may or may not exist. Carries an unresolved stop obligation.
        Uncertain => "uncertain", "uncertain";
        /// Known or suspected to exist and must be stopped, with the stop confirmed.
        StopRequired => "stop-required", "stop required";
        /// Stopped, with evidence. Terminal.
        Stopped => "stopped", "stopped";
        /// Never submitted and now withdrawn. Terminal, and carries no obligation.
        Cancelled => "cancelled", "cancelled";
        /// A strictly newer epoch owns the resource. Terminal for this controller, which must
        /// issue no further mutation against it. The obligation moved with the ownership; it
        /// was not discharged.
        Disowned => "disowned", "disowned";
    }
}

/// Every permitted transition of the ownership state machine.
///
/// The refusals are the point of the table, so the ones that are easy to get wrong are named
/// here rather than left to be read off by subtraction:
///
/// * `Uncertain -> Cancelled` and `StopRequired -> Cancelled` are absent. Cancellation releases
///   an obligation that was never incurred; it cannot discharge one that was.
/// * `StopRequired -> Active` is absent. A later healthy observation does not withdraw a stop
///   obligation — only evidence that the resource stopped does.
/// * `Stopped`, `Cancelled` and `Disowned` have no outgoing edge at all.
/// * `Declared -> Active`, `-> StopRequired` and `-> Stopped` are absent. Nothing can be
///   observed, obliged or discharged before a mutation was submitted.
pub const TRANSITIONS: &[(Phase, Phase)] = &[
    (Phase::Declared, Phase::Requested),
    (Phase::Declared, Phase::Uncertain),
    (Phase::Declared, Phase::Cancelled),
    (Phase::Requested, Phase::Requested),
    (Phase::Requested, Phase::Active),
    (Phase::Requested, Phase::StopRequired),
    (Phase::Requested, Phase::Stopped),
    (Phase::Requested, Phase::Disowned),
    (Phase::Active, Phase::Active),
    (Phase::Active, Phase::StopRequired),
    (Phase::Active, Phase::Stopped),
    (Phase::Active, Phase::Disowned),
    (Phase::Uncertain, Phase::Uncertain),
    (Phase::Uncertain, Phase::Requested),
    (Phase::Uncertain, Phase::Active),
    (Phase::Uncertain, Phase::StopRequired),
    (Phase::Uncertain, Phase::Stopped),
    (Phase::Uncertain, Phase::Disowned),
    (Phase::StopRequired, Phase::StopRequired),
    (Phase::StopRequired, Phase::Stopped),
    (Phase::StopRequired, Phase::Disowned),
];

impl Phase {
    /// Whether this phase may become `next`.
    pub fn may_become(self, next: Self) -> bool {
        TRANSITIONS.contains(&(self, next))
    }

    /// Whether the lifecycle is over for this controller.
    pub fn terminal(self) -> bool {
        !TRANSITIONS.iter().any(|(from, _)| *from == self)
    }

    /// Whether the record still carries an unresolved obligation to stop and confirm.
    ///
    /// An expired lease, a client disconnect, a controller restart and an incomplete inventory
    /// all leave this true. Only evidence does not.
    pub const fn owes_a_stop(self) -> bool {
        matches!(self, Self::Uncertain | Self::StopRequired)
    }

    /// Whether the record occupies a slot against the resource ceiling.
    ///
    /// A resource whose existence is merely uncertain still occupies one: the ceiling exists to
    /// bound what can be billed, and an unknown resource can be billed.
    pub const fn occupies_a_slot(self) -> bool {
        matches!(
            self,
            Self::Requested | Self::Active | Self::Uncertain | Self::StopRequired
        )
    }
}

closed_enum! {
    /// Why a stop obligation is open. Never a claim that the resource actually stopped.
    StopReason {
        /// A create mutation's outcome was never learned.
        AmbiguousMutation => "ambiguous-mutation", "the create outcome was never learned";
        /// The resource outlived the effective time ceiling.
        LifetimeExceeded => "lifetime-exceeded", "the resource outlived its time ceiling";
        /// The ownership lease ran out. This is not evidence that anything stopped.
        LeaseExpired => "lease-expired", "the ownership lease expired";
        /// An operator or a controller asked for the resource to be stopped.
        OperatorRequest => "operator-request", "a stop was requested";
        /// The resource is tagged for an owner this controller does not recognize.
        OwnershipLost => "ownership-lost", "the resource carries a foreign owner tag";
    }
}

#[cfg(test)]
mod tests {
    use super::{Phase, TRANSITIONS};

    /// Every phase change a **conditional** writer asks for — one only some phases reach —
    /// and whether the table permits it.
    ///
    /// A `false` row is a **deliberate** no-op and the comment says why. This table exists
    /// because a phase write guarded on `may_become` and nothing else is silent when the
    /// table forbids it: a foreign owner tag once asked for `Active -> Uncertain`, which is
    /// not an edge, and the record stayed live and unobliged with no diagnostic anywhere.
    /// A row that flips without this table being edited is that defect returning.
    ///
    /// The three writers any phase can reach are in [`OBLIGATION_INTENTS`] instead, because a
    /// hand-written narrative cannot be checked for completeness and this one was not complete.
    const INTENTS: &[(&str, Phase, Phase, bool)] = &[
        ("create accepted", Phase::Declared, Phase::Requested, true),
        (
            "create refused or unsent",
            Phase::Declared,
            Phase::Cancelled,
            true,
        ),
        (
            "create answer lost",
            Phase::Declared,
            Phase::Uncertain,
            true,
        ),
        (
            "idempotent retry answered",
            Phase::Uncertain,
            Phase::Requested,
            true,
        ),
        // A later refusal is not evidence about an earlier ambiguous create, so a lost
        // create is deliberately not withdrawn by one.
        (
            "retry refused or unsent",
            Phase::Uncertain,
            Phase::Cancelled,
            false,
        ),
        (
            "observed under our key",
            Phase::Requested,
            Phase::Active,
            true,
        ),
        (
            "observed under our key",
            Phase::Uncertain,
            Phase::Active,
            true,
        ),
        // A later healthy observation does not withdraw a stop obligation.
        (
            "observed under our key",
            Phase::StopRequired,
            Phase::Active,
            false,
        ),
        (
            "taken over at a newer epoch",
            Phase::Requested,
            Phase::Disowned,
            true,
        ),
        (
            "taken over at a newer epoch",
            Phase::Active,
            Phase::Disowned,
            true,
        ),
        (
            "taken over at a newer epoch",
            Phase::Uncertain,
            Phase::Disowned,
            true,
        ),
        (
            "taken over at a newer epoch",
            Phase::StopRequired,
            Phase::Disowned,
            true,
        ),
    ];

    /// The three writers that take no phase precondition of their own, asked from every phase.
    ///
    /// `require_stop`, `discharge` and `cancel` are reached with whatever phase the record
    /// happens to be in — `RequestStop`, `ConfirmStopped` and `Cancel` each hand their record
    /// straight to one of them — so the honest enumeration of what the controller asks for is a
    /// grid, not a narrative. It is written as a grid because that is what makes it *checkable*:
    /// [`the_three_unconditional_writers_are_enumerated_from_every_phase`] derives
    /// `Phase::ALL × [StopRequired, Stopped, Cancelled]` from the vocabulary itself and requires
    /// exactly one row per cell, so no cell can go missing the way `Declared -> StopRequired`
    /// did — and a phase added to `Phase` fails here until its three rows are decided.
    ///
    /// That missing cell was not cosmetic. `RequestStop` on a `Declared` record asked for it,
    /// the write was a silent no-op, and the command answered `Ok` having recorded no
    /// obligation at all.
    const OBLIGATION_INTENTS: &[(Phase, Phase, bool)] = &[
        // `require_stop` — nothing can be owed before a mutation was submitted, and a
        // terminal record's obligation is settled, transferred or was never incurred.
        (Phase::Declared, Phase::StopRequired, false),
        (Phase::Requested, Phase::StopRequired, true),
        (Phase::Active, Phase::StopRequired, true),
        (Phase::Uncertain, Phase::StopRequired, true),
        (Phase::StopRequired, Phase::StopRequired, true),
        (Phase::Stopped, Phase::StopRequired, false),
        (Phase::Cancelled, Phase::StopRequired, false),
        (Phase::Disowned, Phase::StopRequired, false),
        // `discharge` — evidence closes an obligation that exists. There is none to close
        // before a create was submitted, and none left to close once the record is terminal.
        (Phase::Declared, Phase::Stopped, false),
        (Phase::Requested, Phase::Stopped, true),
        (Phase::Active, Phase::Stopped, true),
        (Phase::Uncertain, Phase::Stopped, true),
        (Phase::StopRequired, Phase::Stopped, true),
        (Phase::Stopped, Phase::Stopped, false),
        (Phase::Cancelled, Phase::Stopped, false),
        (Phase::Disowned, Phase::Stopped, false),
        // `cancel` — releases an obligation that was never incurred, so it is permitted from
        // the one phase where nothing was ever submitted and from nowhere else.
        (Phase::Declared, Phase::Cancelled, true),
        (Phase::Requested, Phase::Cancelled, false),
        (Phase::Active, Phase::Cancelled, false),
        (Phase::Uncertain, Phase::Cancelled, false),
        (Phase::StopRequired, Phase::Cancelled, false),
        (Phase::Stopped, Phase::Cancelled, false),
        (Phase::Cancelled, Phase::Cancelled, false),
        (Phase::Disowned, Phase::Cancelled, false),
    ];

    #[test]
    fn every_phase_change_the_controller_asks_for_is_accounted_for() {
        for (intent, from, to, permitted) in INTENTS {
            assert_eq!(
                from.may_become(*to),
                *permitted,
                "{intent}: {from:?} -> {to:?}"
            );
        }
        for (from, to, permitted) in OBLIGATION_INTENTS {
            assert_eq!(from.may_become(*to), *permitted, "{from:?} -> {to:?}");
        }
    }

    /// The completeness check the narrative table never had.
    ///
    /// The grid comes from `Phase::ALL`, not from the rows below it, so an omitted cell is a
    /// failure here rather than a hole nobody notices until a command reports `Ok` for a change
    /// that never happened.
    #[test]
    fn the_three_unconditional_writers_are_enumerated_from_every_phase() {
        let targets = [Phase::StopRequired, Phase::Stopped, Phase::Cancelled];
        for phase in Phase::ALL {
            for target in targets {
                let rows = OBLIGATION_INTENTS
                    .iter()
                    .filter(|(from, to, _)| from == phase && *to == target)
                    .count();
                assert_eq!(
                    rows, 1,
                    "OBLIGATION_INTENTS must hold exactly one row for {phase:?} -> {target:?}"
                );
            }
        }
        assert_eq!(
            OBLIGATION_INTENTS.len(),
            Phase::ALL.len() * targets.len(),
            "the grid holds a row for every phase and no others"
        );
    }

    /// Whatever else changes, a record that may be holding a billed resource must always be
    /// able to be obliged, discharged and disowned. If one of these stops being expressible,
    /// the code that wants it becomes a silent no-op rather than a refusal.
    #[test]
    fn every_obligation_is_expressible_from_every_phase_that_can_hold_a_resource() {
        for phase in Phase::ALL.iter().filter(|phase| phase.occupies_a_slot()) {
            assert!(phase.may_become(Phase::StopRequired), "oblige {phase:?}");
            assert!(phase.may_become(Phase::Stopped), "discharge {phase:?}");
            assert!(phase.may_become(Phase::Disowned), "disown {phase:?}");
        }
    }

    #[test]
    fn the_terminal_phases_have_no_outgoing_edge() {
        for phase in Phase::ALL {
            let has_edge = TRANSITIONS.iter().any(|(from, _)| from == phase);
            assert_eq!(phase.terminal(), !has_edge, "{phase:?}");
        }
        assert!(Phase::Stopped.terminal());
        assert!(Phase::Cancelled.terminal());
        assert!(Phase::Disowned.terminal());
        assert!(!Phase::Declared.terminal());
    }

    #[test]
    fn no_transition_leaves_a_terminal_phase_and_none_is_listed_twice() {
        for (from, to) in TRANSITIONS {
            assert!(
                TRANSITIONS
                    .iter()
                    .filter(|pair| *pair == &(*from, *to))
                    .count()
                    == 1,
                "{from:?} -> {to:?} listed twice"
            );
        }
        assert!(
            !TRANSITIONS
                .iter()
                .any(|(_, to)| matches!(to, Phase::Declared))
        );
    }

    #[test]
    fn a_stop_obligation_cannot_be_cancelled_away() {
        assert!(!Phase::Uncertain.may_become(Phase::Cancelled));
        assert!(!Phase::StopRequired.may_become(Phase::Cancelled));
        assert!(!Phase::StopRequired.may_become(Phase::Active));
        assert!(Phase::Uncertain.owes_a_stop());
        assert!(Phase::StopRequired.owes_a_stop());
        assert!(!Phase::Stopped.owes_a_stop());
        assert!(!Phase::Disowned.owes_a_stop());
    }
}
