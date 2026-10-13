//! Tissue CLI: thin layer over `tissue-core`.
//!
//! Every command is a thin shell over the core library — no business logic
//! lives here. Commands that need the sync/proposal loop (#12) report a
//! clear "not yet available" error until that lands.

mod bootstrap;
mod serve;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use tissue_core::git;
use tissue_core::human::{valid_issue_id, IssueFile, IssueStatus};
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
        /// One-line issue title.
        title: String,
        /// Issue body text; created as-is instead of opening the editor.
        #[arg(long, value_name = "TEXT")]
        body: Option<String>,
    },
    /// List issues; issues awaiting review are flagged `NEEDS REVIEW`.
    List {
        /// Show only issues with these statuses (open, in-progress, closed).
        #[arg(long, value_name = "STATUS")]
        status: Vec<String>,
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
        Commands::Create { id, title, body } => create(&repo, &id, &title, body),
        Commands::List { status } => list(&repo, &status),
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
        Commands::Serve { port } => serve_cmd(&repo, port),
    }
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
    let commit = core
        .sync(id, &actor, &now_utc(), summary)
        .map_err(|e| format!("sync: {e}"))?;
    println!("synced {id} ({commit})");
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

fn list(repo: &Path, status_filter: &[String]) -> Result<(), String> {
    require_repo(repo)?;
    for f in status_filter {
        parse_status(f)?;
    }
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
        let Ok(issue) = IssueFile::from_path(&dir_path.join("issue.md")) else {
            eprintln!("warning: skipping unreadable issue directory {name}");
            continue;
        };
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
        println!(
            "{}  {}  {}{}",
            issue.id,
            issue.status.as_str(),
            issue.title,
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
    let issue =
        IssueFile::from_path(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
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
fn serve_cmd(repo: &Path, port: Option<u16>) -> Result<(), String> {
    let port = port.unwrap_or(serve::DEFAULT_PORT);
    let gui_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../gui");
    serve::serve(repo.to_path_buf(), port, gui_root)
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
