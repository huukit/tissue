//! Sync loop: human->AI reconciliation and AI->human proposal flow.
//!
//! Contract: `operations/product-owner/STATE.md` core loop rules and
//! `operations/product-owner/SPEC-ai-format.md` (proposal / approval /
//! rejection / human-sync records).
//!
//! Loop rules implemented here:
//! 1. The human format is only mutated by approval of a proposal (or direct
//!    human edit outside the core).
//! 3. Creating a proposal tags the issue `needs-review`; approval/rejection
//!    clears it.
//! 4. Rejections record a reason (data, not error).
//!
//! Stale proposals fail safe: applying is refused when `current` no longer
//! matches the human format.
//!
//! The JSONL envelope handling here is intentionally minimal (append +
//! seq integrity). Issue #11 owns the full typed record model; when it
//! lands, this module keeps its public API and delegates to it.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::git::{Author, Repo};
use crate::human::{self, IssueFile, IssueStatus, NEEDS_REVIEW};
use crate::state::{Decision, IllegalTransition, ProposalState};

#[derive(Debug)]
pub enum Error {
    Git(crate::git::Error),
    Human(human::Error),
    /// agent.jsonl is malformed: a line does not parse.
    MalformedStream(String),
    /// Proposal lifecycle violation (already decided, unknown id, ...).
    Proposal(String),
    /// Proposal's `current` no longer matches the human format.
    Stale {
        proposal_id: String,
        field: String,
        current: String,
        actual: String,
    },
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Git(e) => write!(f, "git error: {e}"),
            Error::Human(e) => write!(f, "human format error: {e}"),
            Error::MalformedStream(m) => write!(f, "malformed agent stream: {m}"),
            Error::Proposal(m) => write!(f, "proposal error: {m}"),
            Error::Stale {
                proposal_id,
                field,
                current,
                actual,
            } => write!(
                f,
                "stale proposal {proposal_id}: {field} is now `{actual}`, proposal expects `{current}` — refusing to apply"
            ),
            Error::Io(m) => write!(f, "io error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<crate::git::Error> for Error {
    fn from(e: crate::git::Error) -> Self {
        Error::Git(e)
    }
}

impl From<human::Error> for Error {
    fn from(e: human::Error) -> Self {
        Error::Human(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Fields a proposal may target in the PoC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Status,
    Release,
}

impl Field {
    pub fn as_str(&self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Status => "status",
            Field::Release => "release",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "title" => Some(Field::Title),
            "status" => Some(Field::Status),
            "release" => Some(Field::Release),
            _ => None,
        }
    }
}

/// Who is acting: an agent proposes; a human syncs/approves/rejects.
/// The actor identity becomes the git commit author (issue #12).
#[derive(Debug, Clone)]
pub struct Actor {
    pub author: Author,
    /// e.g. "agent:dev-1" or "human:tuomas" — recorded in each record.
    pub id: String,
}

/// A proposed human-format edit, supplied by the proposing agent.
pub struct ProposalDraft<'a> {
    pub now: &'a str,
    pub field: Field,
    pub current: &'a str,
    pub proposed: &'a str,
    pub rationale: &'a str,
}

/// The core loop over one issue repo.
pub struct Core {
    repo: Repo,
}

/// A proposal record as stored in agent.jsonl.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub proposal_id: String,
    pub field: Field,
    pub current: String,
    pub proposed: String,
    pub rationale: String,
    pub state: ProposalState,
}

impl Core {
    pub fn new(repo: Repo) -> Self {
        Core { repo }
    }

    pub fn repo(&self) -> &Repo {
        &self.repo
    }

    // -- paths -------------------------------------------------------------

    fn issue_dir(&self, issue_id: &str) -> Result<PathBuf> {
        if !human::valid_issue_id(issue_id) {
            return Err(Error::Proposal(format!("invalid issue id `{issue_id}`")));
        }
        Ok(self.repo.resolve(&[".tissue", "issues", issue_id])?)
    }

    fn issue_path(&self, issue_id: &str) -> Result<PathBuf> {
        Ok(self.issue_dir(issue_id)?.join("issue.md"))
    }

    fn stream_path(&self, issue_id: &str) -> Result<PathBuf> {
        Ok(self.issue_dir(issue_id)?.join("agent.jsonl"))
    }

    // -- agent stream (minimal append with seq integrity) -------------------

    /// Read and integrity-check the stream: every line parses, seq is
    /// strictly increasing and gap-free from 1 (SPEC-ai-format invariant 1).
    /// Read the issue's agent stream as raw JSON values (seq-checked).
    /// Public read path for thin interface layers (serve API, #98): the
    /// raw format is the on-disk truth; typed views are derived from it.
    pub fn read_stream(&self, issue_id: &str) -> Result<Vec<Value>> {
        let path = self.stream_path(issue_id)?;
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e.to_string())),
        };
        let mut records = Vec::new();
        let mut expected_seq = 1u64;
        for (i, line) in content.lines().enumerate() {
            if line.is_empty() {
                continue;
            }
            let record: Value = serde_json::from_str(line)
                .map_err(|e| Error::MalformedStream(format!("line {}: {e}", i + 1)))?;
            let seq = record
                .get("seq")
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::MalformedStream(format!("line {}: missing seq", i + 1)))?;
            if seq != expected_seq {
                return Err(Error::MalformedStream(format!(
                    "line {}: seq is {seq}, expected {expected_seq} (gap-free, strictly increasing)",
                    i + 1
                )));
            }
            expected_seq += 1;
            records.push(record);
        }
        Ok(records)
    }

    /// Atomically append one record: the file is rewritten only after the
    /// full new content validates. A malformed append leaves the file
    /// untouched (issue #11 acceptance; no partial writes).
    fn append_record(&self, issue_id: &str, record: Value) -> Result<()> {
        let mut records = self.read_stream(issue_id)?;
        let seq = records.len() as u64 + 1;
        let mut record = record;
        record["seq"] = json!(seq);
        record["id"] = json!(format!("rec-{seq:04}"));
        records.push(record);
        let mut content = String::new();
        for r in &records {
            content.push_str(&serde_json::to_string(r).map_err(|e| Error::Io(e.to_string()))?);
            content.push('\n');
        }
        let dir = self.issue_dir(issue_id)?;
        fs::create_dir_all(&dir)?;
        let path = self.stream_path(issue_id)?;
        let tmp = dir.join(".agent.jsonl.tmp");
        fs::write(&tmp, &content)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn next_proposal_id(&self, records: &[Value]) -> String {
        let count = records
            .iter()
            .filter(|r| r.get("type").and_then(Value::as_str) == Some("proposal"))
            .count();
        format!("prop-{:02}", count + 1)
    }

    /// Effective proposal state: the record itself is never edited (append-only),
    /// so `proposed` + any later approval/rejection record for the same id
    /// resolves to the terminal state. Illegal transitions are rejected at
    /// write time by checking this before appending the decision record.
    fn effective_state(records: &[Value], proposal_id: &str) -> ProposalState {
        for r in records.iter().rev() {
            let ty = r.get("type").and_then(Value::as_str);
            if ty != Some("approval") && ty != Some("rejection") {
                continue;
            }
            let decided = r
                .get("data")
                .and_then(|d| d.get("proposal_id"))
                .and_then(Value::as_str);
            if decided == Some(proposal_id) {
                return if ty == Some("approval") {
                    ProposalState::Approved
                } else {
                    ProposalState::Rejected
                };
            }
        }
        ProposalState::Proposed
    }

    fn find_proposal(&self, records: &[Value], proposal_id: &str) -> Result<Proposal> {
        for r in records.iter().rev() {
            if r.get("type").and_then(Value::as_str) != Some("proposal") {
                continue;
            }
            let data = r.get("data").cloned().unwrap_or(Value::Null);
            let pid = data.get("proposal_id").and_then(Value::as_str);
            if pid != Some(proposal_id) {
                continue;
            }
            let field = Field::from_str(
                data.get("field")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .ok_or_else(|| Error::Proposal(format!("proposal {proposal_id}: unknown field")))?;
            let state = match data.get("state").and_then(Value::as_str) {
                Some("proposed") => Self::effective_state(records, proposal_id),
                Some("approved") => ProposalState::Approved,
                Some("rejected") => ProposalState::Rejected,
                other => {
                    return Err(Error::Proposal(format!(
                        "proposal {proposal_id}: invalid state {other:?}"
                    )))
                }
            };
            return Ok(Proposal {
                proposal_id: proposal_id.to_string(),
                field,
                current: data
                    .get("current")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                proposed: data
                    .get("proposed")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                rationale: data
                    .get("rationale")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                state,
            });
        }
        Err(Error::Proposal(format!("proposal {proposal_id} not found")))
    }

    // -- human format helpers ----------------------------------------------

    fn field_text(issue: &IssueFile, field: Field) -> String {
        match field {
            Field::Title => issue.title.clone(),
            Field::Status => issue.status.as_str().to_string(),
            Field::Release => issue.release.clone().unwrap_or_default(),
        }
    }

    fn apply_field(
        &self,
        issue: &mut IssueFile,
        field: Field,
        value: &str,
        now: &str,
    ) -> Result<()> {
        match field {
            Field::Title => {
                if value.contains('\n') {
                    return Err(Error::Proposal("title must be one line".into()));
                }
                issue.set_title(value.to_string(), now);
            }
            Field::Status => {
                issue.set_status(IssueStatus::parse(value).map_err(Error::Human)?, now);
            }
            Field::Release => {
                if !value.is_empty() && !human::valid_release_id(value) {
                    return Err(Error::Proposal(format!(
                        "release `{value}` is not v+semver"
                    )));
                }
                let release = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
                issue.set_release(release, now);
            }
        }
        Ok(())
    }

    // -- the loop ------------------------------------------------------------

    /// `tissue sync <id>`: reconcile the human format into the AI stream.
    ///
    /// Decision 4's actual promise (#136): the record carries the exact
    /// field-level changes since the last sync baseline, so agents answer
    /// "what did the human change since I last looked" from data, not from
    /// raw diffs. Nothing changed since the baseline => no record, no
    /// commit ("nothing to sync").
    pub fn sync(&self, issue_id: &str, actor: &Actor, now: &str, summary: &str) -> Result<String> {
        let dir = self.issue_dir(issue_id)?;
        if !dir.is_dir() {
            return Err(Error::Proposal(format!("issue {issue_id} not found")));
        }
        let issue_path = self.issue_path(issue_id)?;
        let current_text = fs::read_to_string(&issue_path)
            .map_err(|e| Error::Io(format!("{}: {e}", issue_path.display())))?;
        let current = IssueFile::parse(&current_text)?;
        let baseline = self.last_sync_baseline(issue_id, &issue_path)?;
        let changes = match &baseline {
            Some(before) => Self::human_format_changes(before, &current),
            None => {
                // No baseline resolvable (no prior sync, nothing at HEAD):
                // record the full current state as the reconciliation.
                Self::human_format_changes(&Self::empty_baseline(&current), &current)
            }
        };
        if changes.is_empty() {
            return Err(Error::Proposal(format!(
                "nothing to sync: {issue_id} unchanged since the last sync baseline"
            )));
        }
        let revision_before = baseline.as_ref().map(|b| b.revision).unwrap_or(0);
        self.append_record(
            issue_id,
            json!({
                "type": "human-sync",
                "ts": now,
                "actor": actor.id,
                "refs": [],
                "data": {
                    "trigger": "command",
                    "summary_of_human_change": summary,
                    "reconciliation": "from sync record onward",
                    "changes": changes,
                    "revision_before": revision_before,
                    "revision_after": current.revision,
                    "snapshot": current_text
                }
            }),
        )?;
        Ok(self
            .repo
            .commit_all(&actor.author, &format!("sync {issue_id}: {summary}"))?)
    }

    /// The issue as of the last sync baseline. The last `human-sync`
    /// record carries the full human-format snapshot at its sync time —
    /// traveling in the record keeps the baseline valid across rebases
    /// (a commit sha would not survive history rewrites). No prior sync:
    /// the file as committed at HEAD (the pre-sync state); `None` if not
    /// even that resolvable.
    fn last_sync_baseline(&self, issue_id: &str, issue_path: &Path) -> Result<Option<IssueFile>> {
        let mut baseline: Option<IssueFile> = None;
        for record in self.read_stream(issue_id)? {
            if record.get("type").and_then(Value::as_str) != Some("human-sync") {
                continue;
            }
            let Some(data) = record.get("data") else {
                continue;
            };
            let Some(snapshot) = data.get("snapshot").and_then(Value::as_str) else {
                continue;
            };
            if let Ok(issue) = IssueFile::parse(snapshot) {
                baseline = Some(issue);
            }
        }
        if baseline.is_none() {
            if let Some(rel) = self.repo_relative(issue_path) {
                if let Ok(text) = self.repo.show_file_at_head(&rel) {
                    if let Ok(issue) = IssueFile::parse(&text) {
                        baseline = Some(issue);
                    }
                }
            }
        }
        Ok(baseline)
    }

    /// A synthetic pre-history baseline: everything differs from it, so
    /// the first sync (or a sync with no resolvable baseline) records the
    /// full current state as changes.
    fn empty_baseline(like: &IssueFile) -> IssueFile {
        let synthetic = format!(
            "---\nid: {}\ntitle: \"\"\nstatus: {}\ncreated: \"\"\nupdated: \"\"\nrevision: 0\n---\n\n",
            like.id,
            like.status.as_str()
        );
        IssueFile::parse(&synthetic).unwrap_or_else(|_| like.clone())
    }

    /// Path of an issue file relative to the repo root (for `git show`).
    fn repo_relative(&self, issue_path: &Path) -> Option<String> {
        let root = self.repo.root();
        issue_path
            .strip_prefix(root)
            .ok()
            .and_then(|rel| rel.to_str())
            .map(|s| s.to_string())
    }

    /// Field-level human-format diff (title/status/release/tags/body).
    /// Front-matter bookkeeping fields (created/updated/revision) are
    /// excluded: they move on every write and carry no reconciliation
    /// meaning.
    fn human_format_changes(before: &IssueFile, after: &IssueFile) -> Vec<Value> {
        let mut changes = Vec::new();
        let mut push = |field: &str, from: String, to: String| {
            if from != to {
                changes.push(json!({
                    "field": field,
                    "from": from,
                    "to": to
                }));
            }
        };
        push("title", before.title.clone(), after.title.clone());
        push(
            "status",
            before.status.as_str().to_string(),
            after.status.as_str().to_string(),
        );
        push(
            "release",
            before.release.clone().unwrap_or_default(),
            after.release.clone().unwrap_or_default(),
        );
        push("tags", before.tags.join(" "), after.tags.join(" "));
        push("body", before.body.clone(), after.body.clone());
        changes
    }

    /// `tissue propose <id>`: create a proposal record, set `needs-review`,
    /// exactly one commit. Returns the proposal id.
    pub fn propose(
        &self,
        issue_id: &str,
        actor: &Actor,
        change: &ProposalDraft<'_>,
    ) -> Result<String> {
        let records = self.read_stream(issue_id)?;
        let issue_path = self.issue_path(issue_id)?;
        let expected = fs::read_to_string(&issue_path)
            .map_err(|e| Error::Io(format!("{}: {e}", issue_path.display())))?;
        let mut issue = IssueFile::from_path(&issue_path)?;
        let actual = Self::field_text(&issue, change.field);
        if actual != change.current {
            return Err(Error::Stale {
                proposal_id: "new".into(),
                field: change.field.as_str().to_string(),
                current: change.current.to_string(),
                actual,
            });
        }
        let proposal_id = self.next_proposal_id(&records);
        self.append_record(
            issue_id,
            json!({
                "type": "proposal",
                "ts": change.now,
                "actor": actor.id,
                "refs": [],
                "data": {
                    "proposal_id": proposal_id,
                    "target": "issue.md",
                    "field": change.field.as_str(),
                    "current": change.current,
                    "proposed": change.proposed,
                    "rationale": change.rationale,
                    "state": "proposed"
                }
            }),
        )?;
        issue.add_tag(NEEDS_REVIEW, change.now);
        issue.write_if_unchanged(&issue_path, &expected)?;
        self.repo.commit_all(
            &actor.author,
            &format!(
                "propose {issue_id}/{}: {} {} -> {}",
                proposal_id,
                change.field.as_str(),
                change.current,
                change.proposed
            ),
        )?;
        Ok(proposal_id)
    }

    /// `tissue approve <id>` / `tissue reject <id>`: write the decision record,
    /// apply the edit on approval, clear `needs-review`, one commit.
    /// Human decisions are authored by the human identity.
    pub fn decide(
        &self,
        issue_id: &str,
        actor: &Actor,
        now: &str,
        proposal_id: &str,
        decision: Decision,
        reason: Option<&str>,
    ) -> Result<String> {
        let records = self.read_stream(issue_id)?;
        let proposal = self.find_proposal(&records, proposal_id)?;
        let next = proposal.state.transition(decision).map_err(
            |IllegalTransition { from, decision }| {
                Error::Proposal(format!(
                    "proposal {proposal_id} is {} — {:?} is not legal",
                    from.as_str(),
                    decision
                ))
            },
        )?;

        let issue_path = self.issue_path(issue_id)?;
        let expected = fs::read_to_string(&issue_path)
            .map_err(|e| Error::Io(format!("{}: {e}", issue_path.display())))?;
        let mut issue = IssueFile::from_path(&issue_path)?;
        if decision == Decision::Approve {
            let actual = Self::field_text(&issue, proposal.field);
            if actual != proposal.current {
                return Err(Error::Stale {
                    proposal_id: proposal_id.to_string(),
                    field: proposal.field.as_str().to_string(),
                    current: proposal.current.clone(),
                    actual,
                });
            }
            self.apply_field(&mut issue, proposal.field, &proposal.proposed, now)?;
        }

        // Decision record (approval or rejection; rejection carries a reason —
        // rejections are data agents learn from, not errors).
        let (record_type, data) = match decision {
            Decision::Approve => (
                "approval",
                json!({ "proposal_id": proposal_id, "applied": true }),
            ),
            Decision::Reject => (
                "rejection",
                json!({
                    "proposal_id": proposal_id,
                    "reason": reason.unwrap_or("")
                }),
            ),
        };
        self.append_record(
            issue_id,
            json!({
                "type": record_type,
                "ts": now,
                "actor": actor.id,
                "refs": [format!("rec-{}", 1.max(records.len()))],
                "data": data
            }),
        )?;

        // Proposal state advances only via the decision record; the proposal
        // record itself is never edited. We record the transition by
        // appending a state note inside the decision record (above). The
        // stream is append-only; re-reading the proposal after the decision
        // yields `proposed` + a terminal decision record, which the state
        // machine resolves to `next`.
        debug_assert!(matches!(
            next,
            ProposalState::Approved | ProposalState::Rejected
        ));

        issue.remove_tag(NEEDS_REVIEW, now);
        issue.write_if_unchanged(&issue_path, &expected)?;

        let message = match decision {
            Decision::Approve => format!(
                "approve {issue_id}/{proposal_id}: {} -> {}",
                proposal.current, proposal.proposed
            ),
            Decision::Reject => format!(
                "reject {issue_id}/{proposal_id}: {}",
                reason.unwrap_or("no reason given")
            ),
        };
        Ok(self.repo.commit_all(&actor.author, &message)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Repo as GitRepo;

    fn temp_core() -> (PathBuf, Core) {
        let dir = std::env::temp_dir().join(format!(
            "tissue-loop-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repo = GitRepo::init(&dir).unwrap();
        (dir, Core::new(repo))
    }

    fn agent() -> Actor {
        Actor {
            author: Author {
                name: "agent:dev-1".into(),
                email: "agent-dev-1@tissue.local".into(),
            },
            id: "agent:dev-1".into(),
        }
    }

    fn human_actor() -> Actor {
        Actor {
            author: Author {
                name: "Tuomas Huuki".into(),
                email: "tuomas@example.com".into(),
            },
            id: "human:tuomas".into(),
        }
    }

    fn seed_issue(core: &Core, id: &str) {
        let dir = core.issue_dir(id).unwrap();
        fs::create_dir_all(&dir).unwrap();
        let content = format!(
            "---\nid: {id}\ntitle: Seed issue\nstatus: open\ncreated: 2026-10-08T09:00:00Z\nupdated: 2026-10-08T09:00:00Z\n---\n\n## Summary\n\nBody\n"
        );
        fs::write(dir.join("issue.md"), content).unwrap();
        core.repo()
            .commit_all(&agent().author, &format!("create {id}"))
            .unwrap();
    }

    #[test]
    fn full_loop_propose_tag_approve_apply_clear() {
        let (dir, core) = temp_core();
        seed_issue(&core, "IAM-1");

        let pid = core
            .propose(
                "IAM-1",
                &agent(),
                &ProposalDraft {
                    now: "2026-10-08T10:00:00Z",
                    field: Field::Status,
                    current: "open",
                    proposed: "in-progress",
                    rationale: "work started",
                },
            )
            .unwrap();
        assert_eq!(pid, "prop-01");

        // tag visible after propose
        let issue = IssueFile::from_path(&core.issue_path("IAM-1").unwrap()).unwrap();
        assert!(issue.needs_review());

        // approve: human identity authors the commit
        core.decide(
            "IAM-1",
            &human_actor(),
            "2026-10-08T11:00:00Z",
            &pid,
            Decision::Approve,
            None,
        )
        .unwrap();

        let issue = IssueFile::from_path(&core.issue_path("IAM-1").unwrap()).unwrap();
        assert_eq!(issue.status, IssueStatus::InProgress);
        assert!(!issue.needs_review());

        // proposal state machine: approving again is illegal
        let err = core
            .decide(
                "IAM-1",
                &human_actor(),
                "2026-10-08T11:30:00Z",
                &pid,
                Decision::Approve,
                None,
            )
            .unwrap_err();
        assert!(err.to_string().contains("not legal"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_edit_of_issue_md_refuses_approve_loudly() {
        let (dir, core) = temp_core();
        seed_issue(&core, "IAM-1");
        core.propose(
            "IAM-1",
            &agent(),
            &ProposalDraft {
                now: "2026-10-08T10:00:00Z",
                field: Field::Status,
                current: "open",
                proposed: "in-progress",
                rationale: "work started",
            },
        )
        .unwrap();
        // Simulate the race the guard exists for: the file changes on disk
        // between the command's read and its write. `decide` captures the
        // expected content at read time; a mid-command edit makes the
        // on-disk bytes differ and the guarded write must refuse.
        let path = core.issue_path("IAM-1").unwrap();
        let expected_at_read = fs::read_to_string(&path).unwrap();
        let mut issue = IssueFile::from_path(&path).unwrap();
        issue.remove_tag(NEEDS_REVIEW, "2026-10-08T11:00:00Z");
        // concurrent edit lands first:
        let concurrent = expected_at_read.replace("## Summary", "## Summary (edited concurrently)");
        fs::write(&path, concurrent).unwrap();
        let err = issue
            .write_if_unchanged(&path, &expected_at_read)
            .unwrap_err();
        assert!(
            err.to_string().contains("conflict"),
            "must fail with a conflict, got: {err}"
        );
        // The concurrent edit survives: no silent clobber.
        let after = IssueFile::from_path(&path).unwrap();
        assert!(after.body.contains("edited concurrently"));
        assert_eq!(after.status, IssueStatus::Open);
        assert_eq!(after.revision, issue.revision);
        // and the guarded write succeeds when nothing changed:
        let mut fresh = IssueFile::from_path(&path).unwrap();
        let now_expected = fs::read_to_string(&path).unwrap();
        fresh.remove_tag(NEEDS_REVIEW, "2026-10-08T11:00:00Z");
        fresh.write_if_unchanged(&path, &now_expected).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn approve_bumps_revision_and_rewrite_is_guarded() {
        let (dir, core) = temp_core();
        seed_issue(&core, "IAM-1");
        let before = IssueFile::from_path(&core.issue_path("IAM-1").unwrap()).unwrap();
        assert_eq!(before.revision, 1);
        let pid = core
            .propose(
                "IAM-1",
                &agent(),
                &ProposalDraft {
                    now: "2026-10-08T10:00:00Z",
                    field: Field::Status,
                    current: "open",
                    proposed: "in-progress",
                    rationale: "work started",
                },
            )
            .unwrap();
        let mid = IssueFile::from_path(&core.issue_path("IAM-1").unwrap()).unwrap();
        assert_eq!(mid.revision, 2, "propose (tag mutation) must bump revision");
        core.decide(
            "IAM-1",
            &human_actor(),
            "2026-10-08T11:00:00Z",
            &pid,
            Decision::Approve,
            None,
        )
        .unwrap();
        let after = IssueFile::from_path(&core.issue_path("IAM-1").unwrap()).unwrap();
        assert_eq!(after.revision, 3, "approve must bump revision");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_proposal_is_refused_and_reported() {
        let (dir, core) = temp_core();
        seed_issue(&core, "IAM-1");

        let pid = core
            .propose(
                "IAM-1",
                &agent(),
                &ProposalDraft {
                    now: "2026-10-08T10:00:00Z",
                    field: Field::Status,
                    current: "open",
                    proposed: "closed",
                    rationale: "done",
                },
            )
            .unwrap();

        // human edits the field directly between propose and approve
        let path = core.issue_path("IAM-1").unwrap();
        let mut issue = IssueFile::from_path(&path).unwrap();
        issue.set_status(IssueStatus::InProgress, "2026-10-08T10:30:00Z");
        issue.write(&path).unwrap();
        core.repo()
            .commit_all(&human_actor().author, "human edit")
            .unwrap();

        let err = core
            .decide(
                "IAM-1",
                &human_actor(),
                "2026-10-08T11:00:00Z",
                &pid,
                Decision::Approve,
                None,
            )
            .unwrap_err();
        assert!(matches!(err, Error::Stale { .. }), "{err}");
        // human edit was not clobbered
        let issue = IssueFile::from_path(&path).unwrap();
        assert_eq!(issue.status, IssueStatus::InProgress);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reject_records_reason_and_clears_tag() {
        let (dir, core) = temp_core();
        seed_issue(&core, "IAM-1");
        let pid = core
            .propose(
                "IAM-1",
                &agent(),
                &ProposalDraft {
                    now: "2026-10-08T10:00:00Z",
                    field: Field::Title,
                    current: "Seed issue",
                    proposed: "Better title",
                    rationale: "clearer",
                },
            )
            .unwrap();
        core.decide(
            "IAM-1",
            &human_actor(),
            "2026-10-08T11:00:00Z",
            &pid,
            Decision::Reject,
            Some("title is fine"),
        )
        .unwrap();

        let issue = IssueFile::from_path(&core.issue_path("IAM-1").unwrap()).unwrap();
        assert_eq!(issue.title, "Seed issue");
        assert!(!issue.needs_review());

        // rejection is data: reason is in the stream
        let records = core.read_stream("IAM-1").unwrap();
        let rej = records
            .iter()
            .find(|r| r.get("type").and_then(Value::as_str) == Some("rejection"))
            .unwrap();
        assert_eq!(rej["data"]["reason"].as_str(), Some("title is fine"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn propose_with_wrong_current_text_fails_without_side_effects() {
        let (dir, core) = temp_core();
        seed_issue(&core, "IAM-1");
        let before_commits = core.repo().commit_count().unwrap();
        let err = core
            .propose(
                "IAM-1",
                &agent(),
                &ProposalDraft {
                    now: "2026-10-08T10:00:00Z",
                    field: Field::Status,
                    current: "closed", // wrong: actual is open
                    proposed: "in-progress",
                    rationale: "x",
                },
            )
            .unwrap_err();
        assert!(matches!(err, Error::Stale { .. }));
        assert_eq!(core.repo().commit_count().unwrap(), before_commits);
        assert!(core.read_stream("IAM-1").unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_mutation_is_exactly_one_commit_and_log_reads_well() {
        let (dir, core) = temp_core();
        seed_issue(&core, "IAM-1"); // commit 2 (after init)
        assert_eq!(core.repo().commit_count().unwrap(), 2);

        let pid = core
            .propose(
                "IAM-1",
                &agent(),
                &ProposalDraft {
                    now: "2026-10-08T10:00:00Z",
                    field: Field::Status,
                    current: "open",
                    proposed: "in-progress",
                    rationale: "work started",
                },
            )
            .unwrap();
        assert_eq!(core.repo().commit_count().unwrap(), 3);

        core.decide(
            "IAM-1",
            &human_actor(),
            "2026-10-08T11:00:00Z",
            &pid,
            Decision::Approve,
            None,
        )
        .unwrap();
        assert_eq!(core.repo().commit_count().unwrap(), 4);

        let log = std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["log", "--format=%s"])
            .output()
            .unwrap();
        let subjects = String::from_utf8_lossy(&log.stdout);
        assert!(subjects.contains("propose IAM-1/prop-01: status open -> in-progress"));
        assert!(subjects.contains("approve IAM-1/prop-01: open -> in-progress"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_on_nonexistent_issue_fails_without_side_effects() {
        let (dir, core) = temp_core();
        let err = core
            .sync("IAM-999", &human_actor(), "2026-10-08T12:00:00Z", "s")
            .unwrap_err();
        assert!(err.to_string().contains("IAM-999 not found"));
        assert!(core.read_stream("IAM-999").unwrap().is_empty());
        assert!(!core.issue_dir("IAM-999").unwrap().exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Edit the human format directly (decision 4: humans own it).
    fn human_edit(core: &Core, id: &str, from: &str, to: &str) {
        let path = core.issue_path(id).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(from), "edit precondition: {from} in {text}");
        fs::write(&path, text.replace(from, to)).unwrap();
    }

    #[test]
    fn sync_records_field_level_changes_and_commits() {
        let (dir, core) = temp_core();
        seed_issue(&core, "IAM-1");
        human_edit(
            &core,
            "IAM-1",
            "title: Seed issue",
            "title: Renamed by human",
        );
        core.sync(
            "IAM-1",
            &human_actor(),
            "2026-10-08T12:00:00Z",
            "human edited summary",
        )
        .unwrap();
        let records = core.read_stream("IAM-1").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["type"], "human-sync");
        assert_eq!(records[0]["seq"], 1);
        assert!(records[0]["id"].as_str().unwrap().starts_with("rec-"));
        let data = &records[0]["data"];
        // AC1: the record carries the exact field-level change.
        let changes = data["changes"].as_array().unwrap();
        assert_eq!(changes.len(), 1, "only the title moved: {changes:?}");
        assert_eq!(changes[0]["field"], "title");
        assert_eq!(changes[0]["from"], "Seed issue");
        assert_eq!(changes[0]["to"], "Renamed by human");
        // Baseline snapshot travels in the record (rebase-proof).
        let snapshot = data["snapshot"].as_str().unwrap();
        assert!(snapshot.contains("Renamed by human"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_with_no_changes_since_baseline_is_a_noop() {
        let (dir, core) = temp_core();
        seed_issue(&core, "IAM-1");
        // AC2: unchanged since baseline -> "nothing to sync", no record,
        // no commit.
        let before = core.repo().commit_count().unwrap();
        let err = core
            .sync("IAM-1", &human_actor(), "2026-10-08T12:00:00Z", "x")
            .unwrap_err();
        assert!(err.to_string().contains("nothing to sync"), "{err}");
        assert!(core.read_stream("IAM-1").unwrap().is_empty());
        assert_eq!(core.repo().commit_count().unwrap(), before);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn second_sync_diffs_against_the_last_sync_baseline() {
        let (dir, core) = temp_core();
        seed_issue(&core, "IAM-1");
        human_edit(&core, "IAM-1", "title: Seed issue", "title: First edit");
        core.sync("IAM-1", &human_actor(), "2026-10-08T12:00:00Z", "one")
            .unwrap();
        // Second sync with no further edits: nothing to sync.
        let err = core
            .sync("IAM-1", &human_actor(), "2026-10-08T12:30:00Z", "two")
            .unwrap_err();
        assert!(err.to_string().contains("nothing to sync"), "{err}");
        // A second edit syncs only the new delta against the snapshot.
        human_edit(&core, "IAM-1", "## Summary", "## Summary (edited)");
        core.sync("IAM-1", &human_actor(), "2026-10-08T13:00:00Z", "three")
            .unwrap();
        let records = core.read_stream("IAM-1").unwrap();
        let second = &records[1]["data"];
        let changes = second["changes"].as_array().unwrap();
        assert_eq!(
            changes.len(),
            1,
            "only the body moved this time: {changes:?}"
        );
        assert_eq!(changes[0]["field"], "body");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_issue_id_rejected_before_any_io() {
        let (dir, core) = temp_core();
        let err = core.sync("../etc", &human_actor(), "now", "x").unwrap_err();
        assert!(matches!(err, Error::Proposal(_)));
        let _ = fs::remove_dir_all(&dir);
    }
}
