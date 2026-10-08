//! Proposal state machine: `proposed -> approved | rejected`.
//!
//! Illegal states are unrepresentable; transitions are validated at write
//! time per SPEC-ai-format.md invariant 3. Implemented in #12.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalState {
    Proposed,
    Approved,
    Rejected,
}
