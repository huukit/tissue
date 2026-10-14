//! Agent write path for the serve API (#80): thin wrappers over
//! `tissue-core` so every write goes through the core loop and produces
//! its commit. The API never writes the human format directly
//! (STATE.md decision 4): create writes the initial issue.md (the
//! creation, not a mutation of an existing human record), append adds
//! an agent record, propose goes through the proposal loop.

use std::path::Path;

use tissue_core::agent::{AgentStream, RecordData};
use tissue_core::git;
use tissue_core::human::{valid_issue_id, IssueFile, IssueStatus};
use tissue_core::sync::{Actor, Core, Field, ProposalDraft};

use crate::{issue_dir, now_utc};

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

/// POST /api/issues: create an issue. One commit, same as the CLI path.
pub fn create_issue(repo: &Path, id: &str, title: &str, actor_id: &str) -> Result<(), String> {
    if !valid_issue_id(id) {
        return Err(format!(
            "invalid issue id `{id}`: must match [A-Z]+-<number>"
        ));
    }
    if title.contains('\n') {
        return Err("title must be one line".into());
    }
    let dir = issue_dir(repo, id);
    if dir.exists() {
        return Err(format!("issue `{id}` already exists"));
    }
    let now = now_utc();
    let issue = IssueFile::new(id, title, IssueStatus::Open, "", &now)
        .map_err(|e| format!("building issue: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    issue
        .write(&dir.join("issue.md"))
        .map_err(|e| format!("writing issue.md: {e}"))?;
    let grepo = git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let actor = actor_from(actor_id);
    grepo
        .commit_all(&actor.author, &format!("create {id}: {title} (api)"))
        .map_err(|e| format!("committing issue: {e}"))?;
    Ok(())
}

/// POST /api/issues/<id>/records: append an agent-writable record to the
/// issue's stream (type/actor rules enforced by tissue-core::agent).
/// One commit per append (audit trail invariant).
pub fn append_record(
    repo: &Path,
    id: &str,
    actor_id: &str,
    data: &serde_json::Value,
) -> Result<u64, String> {
    if !valid_issue_id(id) {
        return Err(format!(
            "invalid issue id `{id}`: must match [A-Z]+-<number>"
        ));
    }
    let dir = issue_dir(repo, id);
    if !dir.is_dir() {
        return Err(format!("issue {id} not found"));
    }
    let data: RecordData =
        serde_json::from_value(data.clone()).map_err(|e| format!("invalid record data: {e}"))?;
    let mut stream = AgentStream::open(dir.join("agent.jsonl"))
        .map_err(|e| format!("opening agent stream: {e}"))?;
    let now = now_utc();
    let record = stream
        .append_agent(data, actor_id, &now)
        .map_err(|e| format!("append: {e}"))?;
    let seq = record.seq;
    let grepo = git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let actor = actor_from(actor_id);
    grepo
        .commit_all(
            &actor.author,
            &format!("record {id}: append via api (seq {seq})"),
        )
        .map_err(|e| format!("committing record: {e}"))?;
    Ok(seq)
}

/// POST /api/issues/<id>/propose: create a proposal through the core
/// loop — tags needs-review, exactly one commit, human format never
/// written directly. Stale `current` values are refused (fail-safe).
pub fn propose(
    repo: &Path,
    id: &str,
    field: &str,
    current: &str,
    proposed: &str,
    rationale: &str,
    actor_id: &str,
) -> Result<String, String> {
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
    let grepo = git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let core = Core::new(grepo);
    let actor = actor_from(actor_id);
    let draft = ProposalDraft {
        now: &now_utc(),
        field,
        current,
        proposed,
        rationale,
    };
    core.propose(id, &actor, &draft)
        .map_err(|e| format!("propose: {e}"))
}

/// POST /api/issues/<id>/approve and /reject: decide a proposal through
/// the core loop — one commit, stream records identical to the CLI path.
pub fn decide(
    repo: &Path,
    id: &str,
    proposal_id: &str,
    decision: tissue_core::state::Decision,
    reason: Option<&str>,
    actor_id: &str,
) -> Result<String, String> {
    if !valid_issue_id(id) {
        return Err(format!(
            "invalid issue id `{id}`: must match [A-Z]+-<number>"
        ));
    }
    let grepo = git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let core = Core::new(grepo);
    let actor = actor_from(actor_id);
    core.decide(id, &actor, &now_utc(), proposal_id, decision, reason)
        .map_err(|e| format!("decide: {e}"))
}

/// GET /api/issues/<id>/proposals: pending proposals with enough context
/// to review (field, current, proposed, rationale). Read-only.
pub fn pending_proposals(repo: &Path, id: &str) -> Result<Vec<serde_json::Value>, String> {
    if !valid_issue_id(id) {
        return Err(format!(
            "invalid issue id `{id}`: must match [A-Z]+-<number>"
        ));
    }
    let grepo = git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let core = Core::new(grepo);
    let records = core
        .read_stream(id)
        .map_err(|e| format!("reading stream: {e}"))?;
    let mut pending = Vec::new();
    for r in &records {
        let ty = r.get("type").and_then(|t| t.as_str());
        if ty != Some("proposal") {
            continue;
        }
        let data = r.get("data").cloned().unwrap_or(serde_json::Value::Null);
        let pid = data.get("proposal_id").and_then(|p| p.as_str());
        let Some(pid) = pid else { continue };
        // Effective state: a later approval/rejection record for the same
        // id decides it (append-only stream, records are never edited).
        let decided = records.iter().rev().find_map(|r2| {
            let ty2 = r2.get("type").and_then(|t| t.as_str())?;
            if ty2 != "approval" && ty2 != "rejection" {
                return None;
            }
            let d = r2.get("data")?;
            if d.get("proposal_id").and_then(|p| p.as_str()) == Some(pid) {
                Some(ty2)
            } else {
                None
            }
        });
        if decided.is_some() {
            continue;
        }
        pending.push(serde_json::json!({
            "proposal_id": pid,
            "state": "proposed",
            "field": data.get("field").cloned().unwrap_or(serde_json::Value::Null),
            "current": data.get("current").cloned().unwrap_or(serde_json::Value::Null),
            "proposed": data.get("proposed").cloned().unwrap_or(serde_json::Value::Null),
            "rationale": data.get("rationale").cloned().unwrap_or(serde_json::Value::Null),
            "ts": r.get("ts").cloned().unwrap_or(serde_json::Value::Null),
        }));
    }
    Ok(pending)
}

/// GET /api/issues/<id>/records: the agent stream for the timeline view.
/// Read-only; returns raw records in seq order.
pub fn records(repo: &Path, id: &str) -> Result<Vec<serde_json::Value>, String> {
    if !valid_issue_id(id) {
        return Err(format!(
            "invalid issue id `{id}`: must match [A-Z]+-<number>"
        ));
    }
    let grepo = git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let core = Core::new(grepo);
    core.read_stream(id)
        .map_err(|e| format!("reading stream: {e}"))
}
