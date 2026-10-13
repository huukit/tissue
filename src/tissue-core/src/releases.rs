//! Release commands over `.tissue/releases/<ver>/` (STATE.md decision 6,
//! SPEC-human-format.md §releases): create/list/show/assign/cut.
//!
//! Invariants:
//! - Every mutation is exactly one commit.
//! - The issue→release link lives in the issue's `release` front-matter
//!   field (single source of truth); per-release lists are derived, never
//!   stored (SPEC-human-format.md).
//! - `release cut` = set `status: cut` (close-out commit) + git tag
//!   `v<id>`; cutting an already-cut release is refused.
//! - Releases carry the dual format like issues: `release.md` plus an
//!   append-only `agent.jsonl` stream.

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::git::{Author, Repo};
use crate::human::{self, IssueFile, ReleaseFile, ReleaseStatus};

#[derive(Debug)]
pub enum Error {
    Git(crate::git::Error),
    Human(human::Error),
    /// Release id is malformed or fails validation.
    Validation(String),
    /// Release does not exist (or an issue id does not).
    NotFound(String),
    /// Lifecycle violation: duplicate create, cutting a cut release, ...
    Conflict(String),
    Io(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Git(e) => write!(f, "git error: {e}"),
            Error::Human(e) => write!(f, "human format error: {e}"),
            Error::Validation(m) => write!(f, "invalid release: {m}"),
            Error::NotFound(m) => write!(f, "not found: {m}"),
            Error::Conflict(m) => write!(f, "conflict: {m}"),
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

pub struct Releases {
    repo: Repo,
}

impl Releases {
    pub fn new(repo: Repo) -> Self {
        Releases { repo }
    }

    pub fn repo(&self) -> &Repo {
        &self.repo
    }

    fn release_dir(&self, id: &str) -> Result<PathBuf> {
        if !human::valid_release_id(id) {
            return Err(Error::Validation(format!(
                "release id `{id}` is not v+semver (e.g. v1.2.0)"
            )));
        }
        Ok(self.repo.resolve(&[".tissue", "releases", id])?)
    }

    fn release_path(&self, id: &str) -> Result<PathBuf> {
        Ok(self.release_dir(id)?.join("release.md"))
    }

    fn stream_path(&self, id: &str) -> Result<PathBuf> {
        Ok(self.release_dir(id)?.join("agent.jsonl"))
    }

    fn issue_dir(&self, issue_id: &str) -> Result<PathBuf> {
        if !human::valid_issue_id(issue_id) {
            return Err(Error::Validation(format!(
                "issue id `{issue_id}` must match [A-Z]+-<number>"
            )));
        }
        let dir = self.repo.resolve(&[".tissue", "issues", issue_id])?;
        if !dir.is_dir() {
            return Err(Error::NotFound(format!("issue {issue_id}")));
        }
        Ok(dir)
    }

    fn read_stream(&self, release_id: &str) -> Result<Vec<Value>> {
        let path = self.stream_path(release_id)?;
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
                .map_err(|e| Error::Validation(format!("agent stream line {}: {e}", i + 1)))?;
            let seq = record
                .get("seq")
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::Validation(format!("line {}: missing seq", i + 1)))?;
            if seq != expected_seq {
                return Err(Error::Validation(format!(
                    "line {i}: seq is {seq}, expected {expected_seq}"
                )));
            }
            expected_seq += 1;
            records.push(record);
        }
        Ok(records)
    }

    fn append_record(&self, release_id: &str, record: Value) -> Result<()> {
        let mut records = self.read_stream(release_id)?;
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
        let dir = self.release_dir(release_id)?;
        fs::create_dir_all(&dir)?;
        let tmp = dir.join(".agent.jsonl.tmp");
        fs::write(&tmp, &content)?;
        fs::rename(&tmp, self.stream_path(release_id)?)?;
        Ok(())
    }

    /// Load a release; errors if it does not exist.
    pub fn load(&self, id: &str) -> Result<ReleaseFile> {
        let path = self.release_path(id)?;
        if !path.is_file() {
            return Err(Error::NotFound(format!("release {id}")));
        }
        let release = ReleaseFile::from_path(&path)?;
        release.validate_against_dir(id)?;
        Ok(release)
    }

    /// `tissue release create <ver> <title>`: write `release.md` (draft)
    /// and commit. Refuses duplicates.
    pub fn create(
        &self,
        id: &str,
        title: &str,
        body: &str,
        now: &str,
        author: &Author,
    ) -> Result<()> {
        if self.release_dir(id)?.exists() {
            return Err(Error::Conflict(format!("release {id} already exists")));
        }
        if title.contains('\n') {
            return Err(Error::Validation("title must be one line".into()));
        }
        let release = ReleaseFile::new(id, title, body, now)?;
        let dir = self.release_dir(id)?;
        fs::create_dir_all(&dir)?;
        release.write(&dir.join("release.md"))?;
        self.repo
            .commit_all(author, &format!("release create {id}: {title}"))?;
        Ok(())
    }

    /// `tissue release list`: every release in the repo, sorted by id.
    pub fn list(&self) -> Result<Vec<ReleaseFile>> {
        let dir = self.repo.resolve(&[".tissue", "releases"])?;
        let mut entries: Vec<PathBuf> = match fs::read_dir(&dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e.to_string())),
        };
        entries.sort();
        let mut releases = Vec::new();
        for path in entries {
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
                continue;
            };
            let Ok(rel) = self.load(&name) else {
                continue;
            };
            releases.push(rel);
        }
        Ok(releases)
    }

    /// Issues assigned to a release, derived from each issue's
    /// `release` front-matter field (never stored on the release).
    pub fn assigned_issues(&self, release_id: &str) -> Result<Vec<IssueFile>> {
        self.load(release_id)?;
        let issues_dir = self.repo.resolve(&[".tissue", "issues"])?;
        let mut entries: Vec<PathBuf> = match fs::read_dir(&issues_dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e.to_string())),
        };
        entries.sort();
        let mut assigned = Vec::new();
        for dir in entries {
            if !dir.is_dir() {
                continue;
            }
            let Ok(issue) = IssueFile::from_path(&dir.join("issue.md")) else {
                continue;
            };
            if issue.release.as_deref() == Some(release_id) {
                assigned.push(issue);
            }
        }
        Ok(assigned)
    }

    /// `tissue release assign <issue-id> <ver>`: write the `release`
    /// front-matter link on the issue and commit. The release must exist;
    /// the issue must not already be assigned (unassign first).
    pub fn assign(
        &self,
        issue_id: &str,
        release_id: &str,
        now: &str,
        author: &Author,
        actor_id: &str,
    ) -> Result<()> {
        self.load(release_id)?;
        let issue_path = self.issue_dir(issue_id)?.join("issue.md");
        let mut issue = IssueFile::from_path(&issue_path)?;
        if let Some(existing) = &issue.release {
            if existing != release_id {
                return Err(Error::Conflict(format!(
                    "issue {issue_id} is already assigned to {existing} (unassign with an empty release first)"
                )));
            }
        }
        issue.set_release(Some(release_id.to_string()), now);
        issue.write(&issue_path)?;
        self.repo.commit_all(
            author,
            &format!("release assign {issue_id} -> {release_id}"),
        )?;
        self.append_record(
            release_id,
            json!({
                "type": "assignment",
                "ts": now,
                "actor": actor_id,
                "refs": [issue_id],
                "data": { "issue": issue_id, "release": release_id }
            }),
        )?;
        Ok(())
    }

    /// `tissue release cut <ver>`: set `status: cut` (close-out commit),
    /// then create the git tag — the release id *is* the tag name
    /// (`v1.0.0` id → `v1.0.0` tag; ids are `v`-prefixed by validation).
    /// Refuses a second cut.
    pub fn cut(&self, id: &str, now: &str, author: &Author, actor_id: &str) -> Result<()> {
        let release = self.load(id)?;
        if release.status == ReleaseStatus::Cut {
            return Err(Error::Conflict(format!(
                "release {id} is already cut (tag {id} exists)"
            )));
        }
        let path = self.release_path(id)?;
        let mut release = release;
        release.set_status(ReleaseStatus::Cut, now);
        release.write(&path)?;
        self.repo.commit_all(author, &format!("release cut {id}"))?;
        self.append_record(
            id,
            json!({
                "type": "release-cut",
                "ts": now,
                "actor": actor_id,
                "refs": [],
                "data": { "release": id, "tag": id }
            }),
        )?;
        self.repo
            .create_tag(id, &format!("tissue release cut {id}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Repo as GitRepo;

    fn author() -> Author {
        Author {
            name: "Tuomas Huuki".into(),
            email: "tuomas@example.com".into(),
        }
    }

    #[test]
    fn release_lifecycle_unit() {
        let dir = std::env::temp_dir().join(format!(
            "tissue-release-unit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repo = GitRepo::init(&dir).unwrap();
        let releases = Releases::new(repo);
        let now = "2026-10-13T19:00:00Z";

        releases
            .create("v1.0.0", "First cut", "Ship it", now, &author())
            .unwrap();
        let rel = releases.load("v1.0.0").unwrap();
        assert_eq!(rel.status.as_str(), "draft");

        // duplicate create refused
        let err = releases
            .create("v1.0.0", "Again", "", now, &author())
            .unwrap_err();
        assert!(err.to_string().contains("already exists"));

        // invalid id rejected at the boundary (path traversal blocked)
        let err = releases
            .create("../escape", "x", "", now, &author())
            .unwrap_err();
        assert!(matches!(err, Error::Validation(_)));

        releases
            .cut("v1.0.0", now, &author(), "human:tuomas")
            .unwrap();
        let rel = releases.load("v1.0.0").unwrap();
        assert_eq!(rel.status.as_str(), "cut");

        // second cut refused
        let err = releases
            .cut("v1.0.0", now, &author(), "human:tuomas")
            .unwrap_err();
        assert!(err.to_string().contains("already cut"));

        // the cut record is in the release stream
        let records = releases.read_stream("v1.0.0").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["type"], "release-cut");

        let _ = fs::remove_dir_all(&dir);
    }
}
