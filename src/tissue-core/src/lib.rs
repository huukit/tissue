//! Tissue core library.
//!
//! Formats, sync loop, proposal state machine, and git operations.
//! UI-free by design: no interface crate implements business logic here.

pub mod agent;
pub mod git;
pub mod human;
pub mod releases;
pub mod state;
pub mod sync;

/// Crate version, for diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn workspace_scaffold_is_alive() {
        // Not pinned to a literal: the version bumps per release (#88), and
        // the release workflow's tag guard enforces tag == workspace version.
        assert_eq!(super::VERSION, env!("CARGO_PKG_VERSION"));
        assert!(!super::VERSION.is_empty());
    }
}
