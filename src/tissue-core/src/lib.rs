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
        assert_eq!(super::VERSION, "0.1.0");
    }
}
