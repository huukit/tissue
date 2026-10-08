//! AI format (`agent.jsonl`): append-only JSONL records.
//!
//! Contract: `operations/product-owner/SPEC-ai-format.md`. Implemented in #11.

#[derive(Debug, Clone, PartialEq)]
pub struct AgentStream {
    pub records: Vec<serde_json::Value>,
}
