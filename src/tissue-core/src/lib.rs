//! Tissue core library.
//!
//! Formats, sync loop, proposal state machine, and git operations.
//! UI-free by design: no interface crate implements business logic here.

pub mod agent;
pub mod config;
pub mod git;
pub mod human;
pub mod projects;
pub mod releases;
pub mod state;
pub mod sync;
pub mod tokens;

/// Crate version, for diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// #143: the serve API's compatibility version. Bumped only on breaking
/// changes to the `/api/*` contract (documented in docs/cli-reference.md).
pub const API_VERSION: &str = "v1";

/// #143: the workspace license, from the workspace `license` field.
pub const LICENSE: &str = env!("CARGO_PKG_LICENSE");

/// #143: build target triple, captured at compile time via build.rs.
pub const BUILD_TARGET: &str = env!("TISSUE_BUILD_TARGET");

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
