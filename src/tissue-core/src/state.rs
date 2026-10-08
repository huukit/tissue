//! Proposal state machine: `proposed -> approved | rejected`.
//!
//! Illegal states are unrepresentable; transitions are validated at write
//! time per SPEC-ai-format.md invariant 3. Implemented in #12.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalState {
    Proposed,
    Approved,
    Rejected,
}
