//! Tissue CLI: thin layer over `tissue-core`.
//!
//! Every command is a thin shell over the core library — no business logic
//! lives here. Commands that need the sync/proposal loop (#12) report a
//! clear "not yet available" error until that lands.

mod api_write;
mod bootstrap;
mod serve;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use tissue_core::git;
use tissue_core::human::{valid_issue_id, IssueFile, IssueStatus};
use tissue_core::releases::Releases;
use tissue_core::state::Decision;
use tissue_core::sync::{Actor, Core, Field, ProposalDraft};

/// Tissue — issue tracker for development with AI in the loop.
///
/// Every unit of work lives in a plain git repo as files: a human format
/// (`issue.md`) owned by humans, and an AI format (`agent.jsonl`) owned by
/// agents. This CLI is the local interface to both.
#[derive(Parser)]
#[command(name = "tissue", version, about, long_about)]
struct Cli {
    /// Path to the tissue repository (directory containing `.tissue/`).
    /// Defaults to the current directory.
    #[arg(long, global = true, value_name = "DIR")]
    repo: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
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
    },
    /// List issues; issues awaiting review are flagged `NEEDS REVIEW`.
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
}

#[derive(Subcommand)]
enum ProjectCommands {
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
enum TokenCommands {
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
enum ReleaseCommands {
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// Repo layout
// ---------------------------------------------------------------------------

fn repo_dir(cli_repo: Option<&PathBuf>) -> PathBuf {
    match cli_repo {
        Some(p) => p.clone(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

fn tissue_dir(repo: &Path) -> PathBuf {
    repo.join(".tissue")
}

fn issues_dir(repo: &Path) -> PathBuf {
    tissue_dir(repo).join("issues")
}

fn issue_dir(repo: &Path, id: &str) -> PathBuf {
    issues_dir(repo).join(id)
}

/// Validate a user-supplied issue id against the spec pattern. Rejecting
/// invalid ids here also blocks path traversal into sibling directories.
fn check_issue_id(id: &str) -> Result<(), String> {
    if valid_issue_id(id) {
        Ok(())
    } else {
        Err(format!(
            "invalid issue id `{id}`: must match [A-Z]+-<number>"
        ))
    }
}

fn require_repo(repo: &Path) -> Result<(), String> {
    if tissue_dir(repo).is_dir() {
        Ok(())
    } else {
        Err(format!(
            "not a tissue repository (no `{}` in {})",
            tissue_dir(repo).display(),
            repo.display()
        ))
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn run(cli: Cli) -> Result<(), String> {
    let repo = repo_dir(cli.repo.as_ref());
    match cli.command {
        Commands::Init {
            project,
            from_github,
            remote,
            dir,
            id_prefix,
            dry_run,
        } => bootstrap::run(
            &repo,
            &bootstrap::InitOptions {
                project,
                from_github,
                remote,
                dir,
                id_prefix,
                dry_run,
            },
        ),
        Commands::Create {
            id,
            title,
            title_flag,
            body,
        } => {
            // #145: accept both `tissue create <ID> <TITLE>` and
            // `tissue create <ID> --title <TITLE>`; exactly one required.
            let title = match (title, title_flag) {
                (Some(t), None) | (None, Some(t)) => t,
                (Some(_), Some(_)) => {
                    return Err(
                        "pass the title either positionally or with --title, not both".into(),
                    )
                }
                (None, None) => {
                    return Err("missing title: pass it positionally or with --title <TEXT>".into())
                }
            };
            create(&repo, &id, &title, body)
        }
        Commands::List {
            status,
            project,
            all,
        } => list(&repo, &status, project.as_deref(), all),
        Commands::Show { id } => show(&repo, &id),
        Commands::Sync { id, summary, actor } => sync_cmd(&repo, &id, &summary, &actor),
        Commands::Propose {
            id,
            field,
            current,
            proposed,
            rationale,
            actor,
        } => propose_cmd(&repo, &id, &field, &current, &proposed, &rationale, &actor),
        Commands::Approve {
            id,
            proposal_id,
            actor,
        } => decide_cmd(&repo, &id, &proposal_id, Decision::Approve, None, &actor),
        Commands::Reject {
            id,
            proposal_id,
            reason,
            actor,
        } => decide_cmd(
            &repo,
            &id,
            &proposal_id,
            Decision::Reject,
            Some(&reason),
            &actor,
        ),
        Commands::Serve { port, project } => serve_cmd(&repo, port, project.as_deref()),
        Commands::Pull {} => pull_cmd(&repo),
        Commands::Push { force_with_lease } => push_cmd(&repo, force_with_lease),
        Commands::Release { command } => release_cmd(&repo, command),
        Commands::Project { command } => project_cmd(&repo, command),
        Commands::Token { command } => token_cmd(&repo, command),
    }
}

// ---------------------------------------------------------------------------
// Projects (#119): manage the name -> prefix mapping in config.yml
// ---------------------------------------------------------------------------

/// Shared project-config logic (tissue-core #135): CLI and the write API
/// go through the same module so validation can't drift (#118's lesson).
fn projects_for(repo: &Path) -> Result<tissue_core::projects::Projects, String> {
    let grepo = git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    Ok(tissue_core::projects::Projects::new(grepo))
}

fn tissue_author() -> git::Author {
    git::Author {
        name: "tissue".to_string(),
        email: "tissue@localhost".to_string(),
    }
}

fn project_cmd(repo: &Path, command: ProjectCommands) -> Result<(), String> {
    require_repo(repo)?;
    let projects = projects_for(repo)?;
    match command {
        ProjectCommands::List => {
            let list = projects.list()?;
            let default = projects.default_name()?;
            if list.is_empty() {
                println!("no projects configured");
                return Ok(());
            }
            for (name, prefix) in &list {
                let marker = if *name == default { " (default)" } else { "" };
                println!("{name}\t{prefix}{marker}");
            }
            Ok(())
        }
        ProjectCommands::Add { name, prefix } => {
            projects
                .add(&name, &prefix, &tissue_author())
                .map_err(|e| e.to_string())?;
            println!("added project {name} -> {prefix}");
            Ok(())
        }
        ProjectCommands::Remove { name, force } => {
            projects
                .remove(&name, force, &tissue_author())
                .map_err(|e| e.to_string())?;
            let list = projects.list()?;
            let prefix = list
                .iter()
                .find(|(n, _)| n == &name)
                .map(|(_, p)| p.clone())
                .unwrap_or_default();
            println!("removed project {name} (prefix {prefix})");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Tokens (#159): issue/list/revoke, hash-only storage in config.yml
// ---------------------------------------------------------------------------

fn token_cmd(repo: &Path, command: TokenCommands) -> Result<(), String> {
    require_repo(repo)?;
    let grepo = git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let store = tissue_core::tokens::TokenStore::new(grepo);
    match command {
        TokenCommands::Issue { label } => {
            let (secret, id) = store
                .issue(&label, &tissue_author(), &now_utc())
                .map_err(|e| e.to_string())?;
            println!("issued {id} ({label})");
            println!("token: {secret}");
            println!("stored as sha256 only in .tissue/config.yml — this is the only time the secret is shown");
            Ok(())
        }
        TokenCommands::List => {
            let entries = store.list()?;
            if store.has_legacy_plaintext_token() {
                println!("note: legacy plaintext `api_token:` line in config.yml — migrate with `tissue token issue` + remove the line (a plaintext token lands in git history)");
            }
            if entries.is_empty() {
                println!("no managed tokens");
            }
            for t in &entries {
                println!("{}\t{}\tcreated {}", t.id, t.label, t.created);
            }
            if std::env::var("TISSUE_API_TOKEN")
                .ok()
                .filter(|t| !t.trim().is_empty())
                .is_some()
            {
                println!("(env override TISSUE_API_TOKEN is set and takes precedence for writes)");
            }
            Ok(())
        }
        TokenCommands::Revoke { id } => {
            let removed = store
                .revoke(&id, &tissue_author())
                .map_err(|e| e.to_string())?;
            println!(
                "revoked {} ({}) — it now fails closed",
                removed.id, removed.label
            );
            Ok(())
        }
    }
}

fn release_cmd(repo: &Path, command: ReleaseCommands) -> Result<(), String> {
    require_repo(repo)?;
    let grepo = git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let releases = Releases::new(grepo);
    match command {
        ReleaseCommands::Create {
            id,
            title,
            body,
            actor,
        } => {
            let author = actor_from(&actor).author;
            releases
                .create(&id, &title, &body, &now_utc(), &author)
                .map_err(|e| format!("release create: {e}"))?;
            println!("created release {id}: {title}");
        }
        ReleaseCommands::List => {
            for rel in releases.list().map_err(|e| format!("release list: {e}"))? {
                println!("{}\t{}\t{}", rel.id, rel.status.as_str(), rel.title);
            }
        }
        ReleaseCommands::Show { id } => {
            let rel = releases
                .load(&id)
                .map_err(|e| format!("release show: {e}"))?;
            println!("id:      {}", rel.id);
            println!("title:   {}", rel.title);
            println!("status:  {}", rel.status.as_str());
            if let Some(target) = &rel.target {
                println!("target:  {target}");
            }
            println!("created: {}", rel.created);
            println!("updated: {}", rel.updated);
            let issues = releases
                .assigned_issues(&id)
                .map_err(|e| format!("release show: {e}"))?;
            println!("issues:");
            if issues.is_empty() {
                println!("  (none assigned)");
            }
            for issue in issues {
                println!("  {}\t{}\t{}", issue.id, issue.status.as_str(), issue.title);
            }
            println!("\n{}", rel.body);
        }
        ReleaseCommands::Assign {
            issue_id,
            release_id,
            actor,
        } => {
            let actor = actor_from(&actor);
            releases
                .assign(&issue_id, &release_id, &now_utc(), &actor.author, &actor.id)
                .map_err(|e| format!("release assign: {e}"))?;
            println!("assigned {issue_id} -> {release_id}");
        }
        ReleaseCommands::Cut { id, actor } => {
            let actor = actor_from(&actor);
            releases
                .cut(&id, &now_utc(), &actor.author, &actor.id)
                .map_err(|e| format!("release cut: {e}"))?;
            println!("cut {id}: status=cut, tag {id} created");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Sync / proposal loop (thin wrappers over tissue-core::sync)
// ---------------------------------------------------------------------------

fn actor_from(id: &str) -> Actor {
    let author = git::Author {
        name: id.to_string(),
        email: format!("{}@tissue.local", id.replace(':', "-")),
    };
    Actor {
        author,
        id: id.to_string(),
    }
}

fn core_for(repo: &Path) -> Result<Core, String> {
    require_repo(repo)?;
    let grepo = git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    Ok(Core::new(grepo))
}

fn sync_cmd(repo: &Path, id: &str, summary: &str, actor_id: &str) -> Result<(), String> {
    check_issue_id(id)?;
    let core = core_for(repo)?;
    let actor = actor_from(actor_id);
    // #137: when an `agent:` block is configured, sync also invokes the
    // agent and records the outcome; without it, record-only (unchanged).
    let (commit, agent) = core
        .sync_and_run_agent(id, &actor, &now_utc(), summary)
        .map_err(|e| format!("sync: {e}"))?;
    println!("synced {id} ({commit})");
    match agent {
        None => {
            println!("no agent configured — sync recorded the change note only");
        }
        Some(outcome) if outcome.success() => {
            println!(
                "agent run ok ({} ms) — outcome recorded",
                outcome.duration_ms
            );
        }
        Some(outcome) if outcome.timed_out => {
            println!(
                "agent run timed out after {} ms — recorded as a sync failure note",
                outcome.duration_ms
            );
        }
        Some(outcome) => {
            println!(
                "agent run failed: exit {:?} ({} ms) — recorded as a sync failure note",
                outcome.exit_code, outcome.duration_ms
            );
        }
    }
    Ok(())
}

fn propose_cmd(
    repo: &Path,
    id: &str,
    field: &str,
    current: &str,
    proposed: &str,
    rationale: &str,
    actor_id: &str,
) -> Result<(), String> {
    check_issue_id(id)?;
    let field = match field {
        "title" => Field::Title,
        "status" => Field::Status,
        "release" => Field::Release,
        other => {
            return Err(format!(
                "invalid field `{other}`: expected title, status, or release"
            ))
        }
    };
    let core = core_for(repo)?;
    let actor = actor_from(actor_id);
    let now = now_utc();
    let draft = ProposalDraft {
        now: &now,
        field,
        current,
        proposed,
        rationale,
    };
    let proposal_id = core
        .propose(id, &actor, &draft)
        .map_err(|e| format!("propose: {e}"))?;
    println!("proposed {id}/{proposal_id}: tagged needs-review, awaiting human decision");
    Ok(())
}

fn decide_cmd(
    repo: &Path,
    id: &str,
    proposal_id: &str,
    decision: Decision,
    reason: Option<&str>,
    actor_id: &str,
) -> Result<(), String> {
    check_issue_id(id)?;
    let core = core_for(repo)?;
    let actor = actor_from(actor_id);
    let commit = core
        .decide(id, &actor, &now_utc(), proposal_id, decision, reason)
        .map_err(|e| format!("decide: {e}"))?;
    let verb = match decision {
        Decision::Approve => "approved",
        Decision::Reject => "rejected",
    };
    println!("{verb} {id}/{proposal_id} ({commit})");
    Ok(())
}

/// Current time as RFC3339 UTC, no external deps.
fn now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = civil_from_unix(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Days-to-civil algorithm (Howard Hinnant).
fn civil_from_unix(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m as u32,
        d as u32,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

fn create(repo: &Path, id: &str, title: &str, body: Option<String>) -> Result<(), String> {
    require_repo(repo)?;
    check_issue_id(id)?;
    if issue_dir(repo, id).exists() {
        return Err(format!("issue `{id}` already exists"));
    }
    let body_text = match body {
        Some(b) => b,
        None => {
            let template = "## Summary\n\nDescribe the issue.\n\n## Acceptance criteria\n\n- [ ] First criterion\n";
            edit_body(template)?
        }
    };
    let now = now_utc();
    let issue = IssueFile::new(id, title, IssueStatus::Open, &body_text, &now)
        .map_err(|e| format!("building issue: {e}"))?;
    let dir = issue_dir(repo, id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    issue
        .write(&dir.join("issue.md"))
        .map_err(|e| format!("writing issue.md: {e}"))?;
    let grepo = git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let author = git::Author {
        name: "tissue".to_string(),
        email: "tissue@localhost".to_string(),
    };
    grepo
        .commit_all(&author, &format!("create {id}: {title}"))
        .map_err(|e| format!("committing issue: {e}"))?;
    println!("created {id}: {title}");
    Ok(())
}

fn edit_body(template: &str) -> Result<String, String> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let tmp = std::env::temp_dir().join(format!("tissue-{}.md", std::process::id()));
    std::fs::write(&tmp, template).map_err(|e| format!("writing template: {e}"))?;
    let status = std::process::Command::new(&editor)
        .arg(&tmp)
        .status()
        .map_err(|e| format!("running editor `{editor}`: {e}"))?;
    if !status.success() {
        return Err(format!("editor `{editor}` exited with failure"));
    }
    let text = std::fs::read_to_string(&tmp).map_err(|e| format!("reading edited body: {e}"))?;
    let _ = std::fs::remove_file(&tmp);
    Ok(text)
}

/// Resolve a project name to its issue-id prefix via `.tissue/config.yml`.
/// Returns the prefix (e.g. `IAM`) or a clear error.
fn project_prefix(repo: &Path, name: &str) -> Result<String, String> {
    tissue_core::projects::prefix_for(repo, name)
}

fn list(
    repo: &Path,
    status_filter: &[String],
    project: Option<&str>,
    all: bool,
) -> Result<(), String> {
    require_repo(repo)?;
    for f in status_filter {
        parse_status(f)?;
    }
    let prefix = match project {
        Some(name) => Some(project_prefix(repo, name)?),
        None => None,
    };
    let _ = all; // listing every project is the default; --all makes it explicit
    let dir = issues_dir(repo);
    let mut entries: Vec<_> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(e) => return Err(format!("reading {}: {e}", dir.display())),
    };
    entries.sort();
    let mut any = false;
    for dir_path in entries {
        let Some(name) = dir_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
        else {
            continue;
        };
        // #154: a pending create-proposal has a directory + stream but no
        // issue.md yet — list it marked (like the API/GUI do) instead of
        // warning "skipping unreadable issue directory".
        let issue = match IssueFile::from_path(&dir_path.join("issue.md")) {
            Ok(issue) => issue,
            Err(_) => {
                let draft = core_for(repo)
                    .and_then(|core| core.creation_draft(&name).map_err(|e| e.to_string()))
                    .ok()
                    .flatten();
                match draft {
                    Some((_pid, title, _body)) => {
                        if let Some(p) = &prefix {
                            if !name.starts_with(&format!("{p}-")) {
                                continue;
                            }
                        }
                        if !status_filter.is_empty()
                            && !status_filter.iter().any(|s| s.eq_ignore_ascii_case("open"))
                        {
                            continue;
                        }
                        any = true;
                        println!(
                            "{name}  (pending proposal)  {title}  [ai-proposal] [NEEDS REVIEW]"
                        );
                        continue;
                    }
                    None => {
                        eprintln!("warning: skipping unreadable issue directory {name}");
                        continue;
                    }
                }
            }
        };
        if let Some(p) = &prefix {
            if !name.starts_with(&format!("{p}-")) {
                continue;
            }
        }
        if !status_filter.is_empty()
            && !status_filter
                .iter()
                .any(|s| s.eq_ignore_ascii_case(issue.status.as_str()))
        {
            continue;
        }
        any = true;
        let flag = if issue.needs_review() {
            " [NEEDS REVIEW]"
        } else {
            ""
        };
        // #138: derived last-touch — who last touched the issue.
        let last_touch = core_for(repo)
            .and_then(|core| core.last_touch(&name).map_err(|e| e.to_string()))
            .unwrap_or("human");
        println!(
            "{}  {}  {}  [{}]{}",
            issue.id,
            issue.status.as_str(),
            issue.title,
            last_touch,
            flag
        );
    }
    if !any {
        println!("no issues");
    }
    Ok(())
}

fn parse_status(s: &str) -> Result<IssueStatus, String> {
    match s.to_ascii_lowercase().as_str() {
        "open" => Ok(IssueStatus::Open),
        "in-progress" => Ok(IssueStatus::InProgress),
        "closed" => Ok(IssueStatus::Closed),
        other => Err(format!(
            "invalid status `{other}`: expected open, in-progress, or closed"
        )),
    }
}

fn show(repo: &Path, id: &str) -> Result<(), String> {
    require_repo(repo)?;
    check_issue_id(id)?;
    let path = issue_dir(repo, id).join("issue.md");
    let issue = match IssueFile::from_path(&path) {
        Ok(issue) => issue,
        Err(_) => {
            // #154: a pending create-proposal has no issue.md yet — say so
            // clearly instead of a bare read error.
            let draft = core_for(repo)
                .and_then(|core| core.creation_draft(id).map_err(|e| e.to_string()))
                .ok()
                .flatten();
            if let Some((pid, title, body)) = draft {
                println!("id:      {id}");
                println!("title:   {title} (pending proposal {pid})");
                println!("status:  (pending approval — no issue.md yet)");
                println!("review:  NEEDS REVIEW — approve the proposal to create issue.md");
                println!();
                println!("{}", body.trim_end());
                return Ok(());
            }
            return Err(format!("reading {}: {e}", path.display(), e = "not found"));
        }
    };
    println!("id:      {}", issue.id);
    println!("title:   {}", issue.title);
    println!("status:  {}", issue.status.as_str());
    match &issue.release {
        Some(r) => println!("release: {r}"),
        None => println!("release: (unscheduled)"),
    }
    println!(
        "tags:    {}",
        if issue.tags.is_empty() {
            "(none)".to_string()
        } else {
            issue.tags.join(", ")
        }
    );
    println!("created: {}", issue.created);
    println!("updated: {}", issue.updated);
    if issue.needs_review() {
        println!("review:  NEEDS REVIEW — a proposal awaits your decision");
    }
    println!();
    println!("{}", issue.body.trim_end());
    Ok(())
}

/// `tissue serve`: read-only loopback viewer wrapping the core scan (#30).
fn pull_cmd(repo: &Path) -> Result<(), String> {
    require_repo(repo)?;
    let grepo = git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let report = grepo.pull_rebase().map_err(|e| format!("pull: {e}"))?;
    if report.rebased {
        println!(
            "pulled: rebased onto origin ({} commit{} behind); history stays linear",
            report.commits,
            if report.commits == 1 { "" } else { "s" }
        );
        println!("next: `tissue push --force-with-lease` to update the remote");
    } else {
        println!("pulled: {}", report.note);
    }
    Ok(())
}

fn push_cmd(repo: &Path, force_with_lease: bool) -> Result<(), String> {
    require_repo(repo)?;
    let grepo = git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    grepo
        .push(force_with_lease)
        .map_err(|e| format!("push: {e}"))?;
    if force_with_lease {
        println!("pushed: fast-forwarded the remote to the rebased history (force-with-lease)");
    } else {
        println!("pushed: fast-forward");
    }
    Ok(())
}

fn serve_cmd(repo: &Path, port: Option<u16>, project: Option<&str>) -> Result<(), String> {
    let port = port.unwrap_or(serve::DEFAULT_PORT);
    let prefix = match project {
        Some(name) => Some(project_prefix(repo, name)?),
        None => None,
    };
    let gui_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../gui");
    serve::serve(repo.to_path_buf(), port, gui_root, prefix.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_unix_epoch() {
        // 2026-10-08T00:00:00Z = 1791436800 (a known Wednesday).
        let (y, m, d, h, mi, s) = civil_from_unix(1_791_417_600);
        assert_eq!((y, m, d, h, mi, s), (2026, 10, 8, 0, 0, 0));
    }

    #[test]
    fn parse_status_accepts_known_values() {
        assert!(parse_status("open").is_ok());
        assert!(parse_status("IN-PROGRESS").is_ok());
        assert!(parse_status("closed").is_ok());
        assert!(parse_status("nope").is_err());
    }
}
