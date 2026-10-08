//! Proposal state machine: `proposed -> approved | rejected`.
//!
//! Illegal states are unrepresentable; transitions are validated at write
//! time per SPEC-ai-format.md invariant 3.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalState {
    Proposed,
    Approved,
    Rejected,
}

impl ProposalState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProposalState::Proposed => "proposed",
            ProposalState::Approved => "approved",
            ProposalState::Rejected => "rejected",
        }
    }

    /// The only legal transition out of `proposed` is a human decision.
    /// Everything else (e.g. approved -> rejected) is rejected here.
    pub fn transition(self, decision: Decision) -> Result<ProposalState, IllegalTransition> {
        match (self, decision) {
            (ProposalState::Proposed, Decision::Approve) => Ok(ProposalState::Approved),
            (ProposalState::Proposed, Decision::Reject) => Ok(ProposalState::Rejected),
            (from, _) => Err(IllegalTransition { from, decision }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IllegalTransition {
    pub from: ProposalState,
    pub decision: Decision,
}

impl fmt::Display for IllegalTransition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "illegal proposal transition: {} with {:?} (only proposed -> approved|rejected is legal)",
            self.from.as_str(),
            self.decision
        )
    }
}

impl std::error::Error for IllegalTransition {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposed_reaches_both_terminal_states() {
        assert_eq!(
            ProposalState::Proposed
                .transition(Decision::Approve)
                .unwrap(),
            ProposalState::Approved
        );
        assert_eq!(
            ProposalState::Proposed
                .transition(Decision::Reject)
                .unwrap(),
            ProposalState::Rejected
        );
    }

    #[test]
    fn terminal_states_are_frozen() {
        for from in [ProposalState::Approved, ProposalState::Rejected] {
            for decision in [Decision::Approve, Decision::Reject] {
                assert!(from.transition(decision).is_err());
            }
        }
    }
}
