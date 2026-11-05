//! #213: the clap CLI definition, shared between the `tissue` binary and
//! the dev-only man-page generator (`examples/gen-man.rs`), so generated docs
//! can never drift from the CLI.

use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};

/// Tissue — issue tracker for development with AI in the loop.
///
/// Every unit of work lives in a plain git repo as files: a human format
/// (`issue.md`) owned by humans, and an AI format (`agent.jsonl`) owned by
/// agents. This CLI is the local interface to both.
#[derive(Parser)]
#[command(name = "tissue", version, about, long_about)]
pub struct Cli {
    /// Path to the tissue repository (directory containing `.tissue/`).
    /// Defaults to the current directory.
    #[arg(long, global = true, value_name = "DIR")]
    pub repo: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a tissue repository: create the `.tissue/` structure,
    /// optionally configuring a remote and importing GitHub issues (#37).
    Init {
        /// Project name written to `.tissue/config.yml` (defaults to the
        /// directory name).
        #[arg(long, value_name = "NAME")]
        project: Option<String>,
        /// Import existing issues from a GitHub repository (OWNER/NAME)
        /// as part of setup.
        #[arg(long, value_name = "OWNER/NAME")]
        from_github: Option<String>,
        /// Configure a filesystem git remote at this path (fully offline);
        /// created if it does not exist.
        #[arg(long, value_name = "PATH")]
        remote: Option<PathBuf>,
        /// Issue repository location (default: <project>-issues/ under
        /// the current directory when a remote or import source is given,
        /// otherwise the current directory).
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
        /// Tissue id prefix for imported issues (default: IAM).
        #[arg(long, value_name = "PREFIX", default_value = "IAM")]
        id_prefix: String,
        /// Show what init would do, write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Re-import over issue ids that already exist in the target
        /// repo (default: skip them — re-running an import is a no-op,
        /// #162).
        #[arg(long)]
        force: bool,
    },
    /// Create a new issue.
    Create {
        /// Issue id, pattern `[A-Z]+-<number>` (e.g. `IAM-42`).
        id: String,
        /// One-line issue title (positional; or pass --title).
        title: Option<String>,
        /// Issue title as a flag — alias for the positional form (#145).
        #[arg(long = "title", value_name = "TEXT")]
        title_flag: Option<String>,
        /// Issue body text; created as-is instead of opening the editor.
        #[arg(long, value_name = "TEXT")]
        body: Option<String>,
        /// Project to sign the issue to (#172): the id prefix must match
        /// the project's prefix in `.tissue/config.yml`. Default: the
        /// configured default project — never a hardcoded prefix (#176).
        #[arg(long, value_name = "NAME")]
        project: Option<String>,
    },
    /// List issues; issues awaiting a human decision are flagged `[awaiting decision]`
    /// (stored tag: `needs-review`).
    List {
        /// Show only issues with these statuses (open, in-progress, closed).
        #[arg(long, value_name = "STATUS")]
        status: Vec<String>,
        /// Show only issues of one project (name mapped to an id prefix
        /// in `.tissue/config.yml`, e.g. `iam` -> `IAM`).
        #[arg(long, value_name = "NAME", conflicts_with = "all")]
        project: Option<String>,
        /// Explicitly list every issue in the repo, all projects.
        #[arg(long)]
        all: bool,
    },
    /// Print one issue: front matter fields and body.
    Show {
        /// Issue id, e.g. `IAM-42`.
        id: String,
    },
    /// Record a human edit in the AI stream: agents reconcile from this
    /// record onward, never from raw diffs. Run after editing issue.md.
    Sync {
        /// Issue id, e.g. `IAM-42`.
        id: String,
        /// One-line summary of what you changed and why.
        #[arg(long, value_name = "TEXT")]
        summary: String,
        /// Human identity recorded in the stream and commit
        /// (e.g. `human:tuomas`).
        #[arg(long, value_name = "ID", default_value = "human:local")]
        actor: String,
    },
    /// Propose a change to an issue's human format for review.
    /// Fails safe if the current value no longer matches.
    Propose {
        /// Issue id, e.g. `IAM-42`.
        id: String,
        /// Field to change: title, status, or release. Status values: open, in-progress, closed.
        #[arg(long, value_name = "FIELD")]
        field: String,
        /// The value the field currently has (stale proposals are refused).
        #[arg(long, value_name = "VALUE")]
        current: String,
        /// The value the field should have.
        #[arg(long, value_name = "VALUE")]
        proposed: String,
        /// Why the change is right, recorded for the human reviewer.
        #[arg(long, value_name = "TEXT", default_value = "")]
        rationale: String,
        /// Agent identity recorded in the stream and commit
        /// (e.g. `agent:dev-1`).
        #[arg(long, value_name = "ID", default_value = "agent:local")]
        actor: String,
    },
    /// Approve a pending proposal: applies the edit and clears
    /// `needs-review`.
    Approve {
        /// Issue id, e.g. `IAM-42`.
        id: String,
        /// Proposal id printed by `tissue propose`.
        proposal_id: String,
        /// Human identity recorded in the stream and commit.
        #[arg(long, value_name = "ID", default_value = "human:local")]
        actor: String,
    },
    /// Serve the read-only local viewer (GUI) on 127.0.0.1.
    /// Loopback-only; no remote exposure.
    Serve {
        /// Port to listen on (default 7323).
        #[arg(long, value_name = "N")]
        port: Option<u16>,
        /// Show only issues of one project (name mapped to an id prefix
        /// in `.tissue/config.yml`, e.g. `iam` -> `IAM`).
        #[arg(long, value_name = "NAME")]
        project: Option<String>,
    },
    /// Fetch the remote and rebase onto it — never a merge commit
    /// (#110). On conflict: aborts, names the affected issue ids.
    Pull {},
    /// Push to the remote: fast-forward only, or `--force-with-lease`
    /// after a pull-rebase (#110). Refuses anything else.
    Push {
        /// Allow `--force-with-lease` (needed once after `tissue pull`
        /// rebased your local history).
        #[arg(long)]
        force_with_lease: bool,
    },
    /// Reject a pending proposal: records the reason so agents can learn
    /// from it, and clears `needs-review`.
    Reject {
        /// Issue id, e.g. `IAM-42`.
        id: String,
        /// Proposal id printed by `tissue propose`.
        proposal_id: String,
        /// Why it is rejected (required: rejections are data).
        #[arg(long, value_name = "TEXT")]
        reason: String,
        /// Human identity recorded in the stream and commit.
        #[arg(long, value_name = "ID", default_value = "human:local")]
        actor: String,
    },
    /// Release subcommands: create / list / show / assign / cut.
    Release {
        #[command(subcommand)]
        command: ReleaseCommands,
    },
    /// Project subcommands: add / list / remove (#119).
    Project {
        #[command(subcommand)]
        command: ProjectCommands,
    },
    /// API token subcommands: issue / list / revoke (#159). Secrets are
    /// stored as SHA-256 hashes only — the plaintext is printed once.
    Token {
        #[command(subcommand)]
        command: TokenCommands,
    },
    /// Generate a dev fixture issue whose human/AI data is deliberately out
    /// of sync (#171). Hidden from --help; requires `dev_fixtures: true` in
    /// .tissue/config.yml so it can never run against a real repo.
    #[command(hide = true)]
    DevFixture {
        /// Issue id, e.g. `IAM-42`.
        id: String,
        /// Scenario: human-edited, agent-proposed, both-changed,
        /// agent-created-pending, synced-clean.
        #[arg(long, value_name = "NAME")]
        scenario: String,
        /// Deterministic seed for fixture actors/timestamps.
        #[arg(long, default_value = "1")]
        seed: u64,
    },
}

#[derive(Subcommand)]
pub enum ProjectCommands {
    /// List the configured projects (name -> issue-id prefix).
    List,
    /// Add (or overwrite) a project mapping. Refuses a prefix already
    /// mapped to a different name. Commits the config change.
    Add {
        /// Project name (a valid config key: no whitespace or colon).
        name: String,
        /// Issue-id prefix for the project, e.g. `IAM`.
        prefix: String,
    },
    /// Remove a project mapping. Refuses while issues with that prefix
    /// exist, unless `--force`. Commits the config change.
    Remove {
        /// Project name to remove.
        name: String,
        /// Remove even if issues with the project's prefix exist.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
pub enum TokenCommands {
    /// Issue a new API token. Prints the secret ONCE — store it somewhere
    /// safe; only its SHA-256 hash is written to config.yml (one commit).
    Issue {
        /// Label for the token, e.g. `human:tuomas` or `agent:dev-1`.
        #[arg(long, value_name = "LABEL")]
        label: String,
    },
    /// List tokens: id, label, created — never the secret. A legacy
    /// plaintext `api_token:` line is reported as a migration item.
    List,
    /// Revoke a token by id; it fails closed immediately (one commit).
    Revoke {
        /// Token id printed by `tissue token list` (e.g. `tok-01`).
        id: String,
    },
}

#[derive(Subcommand)]
pub enum ReleaseCommands {
    /// Create a new release (draft).
    Create {
        /// Release id, `v` + semver (e.g. `v1.2.0`).
        id: String,
        /// One-line release title.
        title: String,
        /// Release notes / goals text.
        #[arg(long, value_name = "TEXT", default_value = "")]
        body: String,
        /// Human identity recorded in the commit.
        #[arg(long, value_name = "ID", default_value = "human:local")]
        actor: String,
    },
    /// List all releases.
    List,
    /// Print one release and its assigned issues (derived from front matter).
    Show {
        /// Release id, e.g. `v1.2.0`.
        id: String,
    },
    /// Assign an issue to a release (writes the issue's `release` field).
    Assign {
        /// Issue id, e.g. `IAM-42`.
        issue_id: String,
        /// Release id, e.g. `v1.2.0`.
        release_id: String,
        /// Human identity recorded in the stream and commit.
        #[arg(long, value_name = "ID", default_value = "human:local")]
        actor: String,
    },
    /// Cut a release: status -> cut, close-out commit, git tag `v<id>`.
    Cut {
        /// Release id, e.g. `v1.2.0`.
        id: String,
        /// Human identity recorded in the stream and commit.
        #[arg(long, value_name = "ID", default_value = "human:local")]
        actor: String,
    },
}

/// The clap `Command` for `tissue` (#213: the man page renders from this).
pub fn command() -> clap::Command {
    Cli::command()
}
