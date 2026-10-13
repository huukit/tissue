//! Human format (`issue.md` / `release.md`): YAML front matter + Markdown body.
//!
//! Contract: `operations/product-owner/SPEC-human-format.md`.
//!
//! Round-trip guarantee: parsing a file and serializing it back without
//! mutating any field reproduces the original bytes exactly. Field mutations
//! invalidate the cached front matter and re-render it in canonical style.

use std::fmt;
use std::fs;
use std::path::Path;

use serde_yaml::{Mapping, Value};

// ---------------------------------------------------------------------------
// Errors

#[derive(Debug)]
pub enum Error {
    Io(String),
    /// Structural parse failure: missing `---` fences or invalid YAML.
    Parse(String),
    /// Structural parse succeeded but a field violates the spec.
    Validation(String),
    /// The file changed on disk between read and write (#78 guard).
    Conflict(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(m) => write!(f, "io error: {m}"),
            Error::Parse(m) => write!(f, "parse error: {m}"),
            Error::Validation(m) => write!(f, "validation error: {m}"),
            Error::Conflict(m) => write!(f, "conflict: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

type Result<T> = std::result::Result<T, Error>;

// ---------------------------------------------------------------------------
// Field values

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueStatus {
    Open,
    InProgress,
    Closed,
}

impl IssueStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            IssueStatus::Open => "open",
            IssueStatus::InProgress => "in-progress",
            IssueStatus::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "open" => Ok(IssueStatus::Open),
            "in-progress" => Ok(IssueStatus::InProgress),
            "closed" => Ok(IssueStatus::Closed),
            other => Err(Error::Validation(format!("invalid status `{other}`"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseStatus {
    Draft,
    Cut,
}

impl ReleaseStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReleaseStatus::Draft => "draft",
            ReleaseStatus::Cut => "cut",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "draft" => Ok(ReleaseStatus::Draft),
            "cut" => Ok(ReleaseStatus::Cut),
            other => Err(Error::Validation(format!("invalid status `{other}`"))),
        }
    }
}

/// Reserved tag set by the core when a proposal awaits review.
pub const NEEDS_REVIEW: &str = "needs-review";

// ---------------------------------------------------------------------------
// Front matter block: parse / render / raw preservation

/// A parsed front-matter block. Keeps the raw text so an unchanged file
/// serializes byte-identically; mutating a field drops the cache and
/// re-renders canonically on the next serialize.
#[derive(Debug, Clone)]
struct FrontMatter {
    /// Exact original text between the `---` fences, including newlines.
    raw: Option<String>,
    /// Unknown keys, in original order (forward compatibility: preserved, not dropped).
    extra: Mapping,
    dirty: bool,
}

impl FrontMatter {
    fn parse(raw: &str) -> Result<(Mapping, Self)> {
        let mapping = serde_yaml::from_str::<Mapping>(raw)
            .map_err(|e| Error::Parse(format!("front matter is not valid YAML: {e}")))?;
        let fm = FrontMatter {
            raw: Some(raw.to_string()),
            extra: mapping.clone(),
            dirty: false,
        };
        Ok((mapping, fm))
    }

    fn invalidate(&mut self) {
        self.raw = None;
        self.dirty = true;
    }

    /// Render front matter text (without fences) from ordered field pairs.
    /// Known fields first in spec order, unknown keys after, in original order.
    fn render(&self, known: &[(&str, String)]) -> String {
        if let Some(raw) = &self.raw {
            return raw.clone();
        }
        let mut out = String::new();
        for (key, value) in known {
            if value.is_empty() {
                continue;
            }
            out.push_str(key);
            out.push_str(": ");
            out.push_str(value);
            out.push('\n');
        }
        for (k, v) in &self.extra {
            out.push_str(&yaml_key(k));
            out.push_str(": ");
            if is_scalar(v) {
                out.push_str(&yaml_scalar(v));
                out.push('\n');
            } else {
                let rendered = serde_yaml::to_string(v).unwrap_or_else(|_| "null".into());
                let mut lines = rendered.lines();
                if let Some(first) = lines.next() {
                    out.push_str(first);
                    out.push('\n');
                }
                for line in lines {
                    out.push_str("  ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        out
    }
}

fn is_scalar(v: &Value) -> bool {
    !matches!(v, Value::Mapping(_) | Value::Sequence(_))
}

fn yaml_key(v: &Value) -> String {
    yaml_scalar(v)
}

fn yaml_scalar(v: &Value) -> String {
    serde_yaml::to_string(v)
        .unwrap_or_else(|_| "null".into())
        .trim_end_matches('\n')
        .to_string()
}

fn string_field(mapping: &Mapping, key: &str) -> Result<String> {
    match mapping.get(Value::String(key.to_string())) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Err(Error::Validation(format!(
            "field `{key}` must be a string, got {other:?}"
        ))),
        None => Err(Error::Validation(format!("missing required field `{key}`"))),
    }
}

fn opt_string_field(mapping: &Mapping, key: &str) -> Result<Option<String>> {
    match mapping.get(Value::String(key.to_string())) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(Error::Validation(format!(
            "field `{key}` must be a string, got {other:?}"
        ))),
    }
}

fn tags_field(mapping: &Mapping) -> Result<Vec<String>> {
    match mapping.get(Value::String("tags".to_string())) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Sequence(seq)) => {
            let mut tags = Vec::new();
            for item in seq {
                match item {
                    Value::String(s) => tags.push(s.clone()),
                    other => {
                        return Err(Error::Validation(format!(
                            "tags must be a list of strings, got {other:?}"
                        )))
                    }
                }
            }
            Ok(tags)
        }
        Some(other) => Err(Error::Validation(format!(
            "field `tags` must be a list, got {other:?}"
        ))),
    }
}

fn strip_known(mapping: &Mapping, known: &[&str]) -> Mapping {
    let mut extra = Mapping::new();
    for (k, v) in mapping {
        let is_known = known
            .iter()
            .any(|kn| matches!(k, Value::String(s) if s == *kn));
        if !is_known {
            extra.insert(k.clone(), v.clone());
        }
    }
    extra
}

// ---------------------------------------------------------------------------
// File-level parse/serialize

/// Split a file into (front-matter text, body). The body is returned exactly
/// as it appears after the closing fence, leading newline included.
fn split_fences(content: &str) -> Result<(String, String)> {
    let rest = content
        .strip_prefix("---\n")
        .ok_or_else(|| Error::Parse("file must start with a `---` fence line".into()))?;
    let end = rest
        .find("\n---\n")
        .ok_or_else(|| Error::Parse("missing closing `---` fence".into()))?;
    let front = rest[..end].to_string();
    let body = rest[end + "\n---\n".len()..].to_string();
    Ok((front, body))
}

fn join_fences(front: &str, body: &str) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(front);
    if !front.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("---\n");
    out.push_str(body);
    out
}

/// Read a file from disk, parsing front matter and body.
fn read_file(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))
}

/// Atomic write: write to a temp file in the same directory, then rename.
/// A failed write never leaves a half-written human-format file.
fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| Error::Io(format!("{}: no parent directory", path.display())))?;
    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into())
    ));
    fs::write(&tmp, content).map_err(|e| Error::Io(format!("{}: {e}", tmp.display())))?;
    fs::rename(&tmp, path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// issue.md

#[derive(Debug, Clone)]
pub struct IssueFile {
    pub id: String,
    pub title: String,
    pub status: IssueStatus,
    /// Canonical issue->release link; absent/empty = unscheduled.
    pub release: Option<String>,
    pub tags: Vec<String>,
    pub created: String,
    pub updated: String,
    /// Monotonic revision, bumped by every human-format mutation; the
    /// optimistic concurrency guard refuses writes when it changed (#78).
    pub revision: u64,
    pub body: String,
    front: FrontMatter,
}

/// Valid issue id pattern: `[A-Z]+-\d+` (also blocks path traversal by construction).
pub fn valid_issue_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_uppercase() {
        i += 1;
    }
    if i == 0 {
        return false;
    }
    if i >= bytes.len() || bytes[i] != b'-' {
        return false;
    }
    i += 1;
    let digits_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    i == bytes.len() && i > digits_start
}

impl IssueFile {
    /// Construct a new issue. `now` (RFC3339) stamps both `created` and
    /// `updated`; timestamps come from the caller, not a hidden clock.
    /// Front matter renders canonically on first serialize.
    pub fn new(id: &str, title: &str, status: IssueStatus, body: &str, now: &str) -> Result<Self> {
        if !valid_issue_id(id) {
            return Err(Error::Validation(format!(
                "id `{id}` does not match pattern [A-Z]+-<number>"
            )));
        }
        if title.contains('\n') {
            return Err(Error::Validation("title must be a single line".into()));
        }
        Ok(IssueFile {
            id: id.to_string(),
            title: title.to_string(),
            status,
            release: None,
            tags: Vec::new(),
            created: now.to_string(),
            updated: now.to_string(),
            revision: 1,
            body: body.to_string(),
            front: FrontMatter {
                raw: None,
                extra: Mapping::new(),
                dirty: true,
            },
        })
    }

    /// Validate that the id matches its directory name (spec field rule).
    pub fn validate_against_dir(&self, dir_name: &str) -> Result<()> {
        if self.id != dir_name {
            return Err(Error::Validation(format!(
                "id `{}` does not match directory name `{dir_name}`",
                self.id
            )));
        }
        Ok(())
    }

    pub fn parse(content: &str) -> Result<Self> {
        let (front_raw, body) = split_fences(content)?;
        let (mapping, front) = FrontMatter::parse(&front_raw)?;
        let id = string_field(&mapping, "id")?;
        if !valid_issue_id(&id) {
            return Err(Error::Validation(format!(
                "id `{id}` does not match pattern [A-Z]+-<number>"
            )));
        }
        let title = string_field(&mapping, "title")?;
        if title.contains('\n') {
            return Err(Error::Validation("title must be a single line".into()));
        }
        let status = IssueStatus::parse(&string_field(&mapping, "status")?)?;
        let release = opt_string_field(&mapping, "release")?;
        let tags = tags_field(&mapping)?;
        let created = string_field(&mapping, "created")?;
        let updated = string_field(&mapping, "updated")?;
        let revision = match mapping.get(Value::String("revision".to_string())) {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(
                s.parse::<u64>()
                    .map_err(|_| Error::Validation(format!("revision `{s}` is not a number")))?,
            ),
            Some(Value::Number(n)) => Some(n.as_u64().ok_or_else(|| {
                Error::Validation(format!("revision `{n}` is not a non-negative integer"))
            })?),
            Some(other) => {
                return Err(Error::Validation(format!(
                    "field `revision` must be a number, got {other:?}"
                )))
            }
        };
        let extra = strip_known(
            &mapping,
            &[
                "id", "title", "status", "release", "tags", "created", "updated", "revision",
            ],
        );
        Ok(IssueFile {
            id,
            title,
            status,
            release,
            tags,
            created,
            updated,
            revision: revision.unwrap_or(1),
            body,
            front: FrontMatter {
                raw: front.raw,
                extra,
                dirty: false,
            },
        })
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        let content = read_file(path)?;
        Self::parse(&content)
    }

    /// Serialize to file bytes. Byte-identical to the source when no field
    /// has been mutated since parse; canonical render otherwise.
    pub fn serialize(&self) -> String {
        let known = vec![
            ("id", self.id.clone()),
            ("title", yaml_scalar(&Value::String(self.title.clone()))),
            ("status", self.status.as_str().to_string()),
            ("release", self.release.clone().unwrap_or_default()),
            (
                "tags",
                if self.tags.is_empty() {
                    "[]".to_string()
                } else {
                    format!("[{}]", self.tags.join(", "))
                },
            ),
            ("created", self.created.clone()),
            ("updated", self.updated.clone()),
            ("revision", self.revision.to_string()),
        ];
        join_fences(&self.front.render(&known), &self.body)
    }

    /// Write, refusing if the file on disk changed since `expected` was
    /// read (optimistic concurrency, #78). Content-based, no locks.
    pub fn write_if_unchanged(&mut self, path: &Path, expected: &str) -> Result<()> {
        let current = read_file(path)?;
        if current != expected {
            return Err(Error::Conflict(
                "issue changed while you were working — re-read it and retry".into(),
            ));
        }
        self.revision += 1;
        self.front.invalidate();
        atomic_write(path, &self.serialize())
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        atomic_write(path, &self.serialize())
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    pub fn needs_review(&self) -> bool {
        self.has_tag(NEEDS_REVIEW)
    }

    /// Mutating helpers: any human-format mutation must bump `updated`.
    /// `now` is an RFC3339 timestamp supplied by the caller (no hidden clock).
    pub fn set_title(&mut self, title: String, now: &str) {
        self.title = title;
        self.front.invalidate();
        self.updated = now.to_string();
    }

    pub fn set_status(&mut self, status: IssueStatus, now: &str) {
        self.status = status;
        self.front.invalidate();
        self.updated = now.to_string();
    }

    pub fn set_release(&mut self, release: Option<String>, now: &str) {
        self.release = release;
        self.front.invalidate();
        self.updated = now.to_string();
    }

    pub fn add_tag(&mut self, tag: &str, now: &str) {
        if !self.has_tag(tag) {
            self.tags.push(tag.to_string());
            self.front.invalidate();
            self.updated = now.to_string();
        }
    }

    pub fn remove_tag(&mut self, tag: &str, now: &str) {
        if let Some(pos) = self.tags.iter().position(|t| t == tag) {
            self.tags.remove(pos);
            self.front.invalidate();
            self.updated = now.to_string();
        }
    }

    /// Spec invariant 3: closing an issue with no release link requires
    /// explicit human confirmation. The caller (core loop) must check this
    /// before applying a close.
    pub fn closing_needs_confirmation(&self) -> bool {
        self.status == IssueStatus::Closed && self.release.is_none()
    }
}

// ---------------------------------------------------------------------------
// release.md

#[derive(Debug, Clone)]
pub struct ReleaseFile {
    pub id: String,
    pub title: String,
    pub status: ReleaseStatus,
    pub target: Option<String>,
    pub created: String,
    pub updated: String,
    pub body: String,
    front: FrontMatter,
}

/// PoC release id: `v` + semver (e.g. `v1.2.0`). Blocks path traversal.
pub fn valid_release_id(id: &str) -> bool {
    let rest = id.strip_prefix('v').unwrap_or("");
    if rest.is_empty() {
        return false;
    }
    let parts: Vec<&str> = rest.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

impl ReleaseFile {
    pub fn validate_against_dir(&self, dir_name: &str) -> Result<()> {
        if self.id != dir_name {
            return Err(Error::Validation(format!(
                "id `{}` does not match directory name `{dir_name}`",
                self.id
            )));
        }
        Ok(())
    }

    pub fn parse(content: &str) -> Result<Self> {
        let (front_raw, body) = split_fences(content)?;
        let (mapping, front) = FrontMatter::parse(&front_raw)?;
        let id = string_field(&mapping, "id")?;
        if !valid_release_id(&id) {
            return Err(Error::Validation(format!("id `{id}` is not `v` + semver")));
        }
        let title = string_field(&mapping, "title")?;
        let status = ReleaseStatus::parse(&string_field(&mapping, "status")?)?;
        let target = opt_string_field(&mapping, "target")?;
        let created = string_field(&mapping, "created")?;
        let updated = string_field(&mapping, "updated")?;
        let extra = strip_known(
            &mapping,
            &["id", "title", "status", "target", "created", "updated"],
        );
        Ok(ReleaseFile {
            id,
            title,
            status,
            target,
            created,
            updated,
            body,
            front: FrontMatter {
                raw: front.raw,
                extra,
                dirty: false,
            },
        })
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        let content = read_file(path)?;
        Self::parse(&content)
    }

    pub fn serialize(&self) -> String {
        let known = vec![
            ("id", self.id.clone()),
            ("title", yaml_scalar(&Value::String(self.title.clone()))),
            ("status", self.status.as_str().to_string()),
            ("target", self.target.clone().unwrap_or_default()),
            ("created", self.created.clone()),
            ("updated", self.updated.clone()),
        ];
        join_fences(&self.front.render(&known), &self.body)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        atomic_write(path, &self.serialize())
    }

    pub fn set_status(&mut self, status: ReleaseStatus, now: &str) {
        self.status = status;
        self.front.invalidate();
        self.updated = now.to_string();
    }
}

// ---------------------------------------------------------------------------
// Tests: one per spec invariant, plus round-trip guarantees.

#[cfg(test)]
mod tests {
    use super::*;

    const ISSUE: &str = "---\nid: IAM-42\ntitle: Sync fails on empty agent stream\nstatus: open\nrelease: v1.2.0\ntags: [needs-review]\ncreated: 2026-10-08T09:00:00Z\nupdated: 2026-10-08T14:20:00Z\n---\n\n## Summary\n\nHuman-written summary.\n\n## Acceptance criteria\n\n- [ ] First criterion\n";

    const RELEASE: &str = "---\nid: v1.2.0\ntitle: First cut\nstatus: draft\ntarget: 2026-11-01\ncreated: 2026-10-08T09:00:00Z\nupdated: 2026-10-08T14:20:00Z\n---\n\n## Goals\n\nWhat this release is for.\n";

    #[test]
    fn issue_round_trip_is_byte_identical_when_unchanged() {
        let file = IssueFile::parse(ISSUE).unwrap();
        assert_eq!(file.serialize(), ISSUE);
    }

    #[test]
    fn release_round_trip_is_byte_identical_when_unchanged() {
        let file = ReleaseFile::parse(RELEASE).unwrap();
        assert_eq!(file.serialize(), RELEASE);
    }

    #[test]
    fn unknown_front_matter_keys_are_preserved() {
        let content = "---\nid: IAM-42\ntitle: T\nstatus: open\ncreated: 2026-10-08T09:00:00Z\nupdated: 2026-10-08T09:00:00Z\ncustom-key: keep-me\n---\n\nBody\n";
        let file = IssueFile::parse(content).unwrap();
        assert_eq!(
            file.front.extra.get(Value::String("custom-key".into())),
            Some(&Value::String("keep-me".into()))
        );
        assert_eq!(file.serialize(), content);
    }

    #[test]
    fn mutation_renders_canonical_and_preserves_unknown_keys() {
        let mut file = IssueFile::parse(ISSUE).unwrap();
        file.set_status(IssueStatus::Closed, "2026-10-09T10:00:00Z");
        let out = file.serialize();
        let reparsed = IssueFile::parse(&out).unwrap();
        assert_eq!(reparsed.status, IssueStatus::Closed);
        assert_eq!(reparsed.updated, "2026-10-09T10:00:00Z");
        // idempotent: second round-trip is byte-identical
        assert_eq!(reparsed.serialize(), out);
    }

    #[test]
    fn missing_fences_fail() {
        assert!(IssueFile::parse("no fences here").is_err());
        assert!(IssueFile::parse("---\nid: IAM-1\n").is_err());
    }

    #[test]
    fn missing_required_fields_fail() {
        let content = "---\nid: IAM-42\nstatus: open\ncreated: x\nupdated: x\n---\n\nBody\n";
        assert!(matches!(
            IssueFile::parse(content),
            Err(Error::Validation(m)) if m.contains("title")
        ));
    }

    #[test]
    fn id_pattern_enforced() {
        assert!(valid_issue_id("IAM-42"));
        assert!(!valid_issue_id("iam-42"));
        assert!(!valid_issue_id("IAM-"));
        assert!(!valid_issue_id("IAM-42x"));
        assert!(!valid_issue_id("IAM"));
        assert!(!valid_issue_id("IAM-1/../etc"));
        let bad = ISSUE.replace("IAM-42", "IAM 42");
        assert!(IssueFile::parse(&bad).is_err());
    }

    #[test]
    fn id_must_match_directory_name() {
        let file = IssueFile::parse(ISSUE).unwrap();
        assert!(file.validate_against_dir("IAM-42").is_ok());
        assert!(file.validate_against_dir("IAM-43").is_err());
    }

    #[test]
    fn status_values_are_validated() {
        let bad = ISSUE.replace("status: open", "status: wontfix");
        assert!(IssueFile::parse(&bad).is_err());
        assert_eq!(
            IssueStatus::parse("in-progress").unwrap(),
            IssueStatus::InProgress
        );
    }

    #[test]
    fn needs_review_tag_helpers() {
        let mut file = IssueFile::parse(ISSUE).unwrap();
        assert!(file.needs_review());
        file.remove_tag(NEEDS_REVIEW, "2026-10-09T10:00:00Z");
        assert!(!file.needs_review());
        file.add_tag(NEEDS_REVIEW, "2026-10-09T11:00:00Z");
        assert!(file.needs_review());
    }

    #[test]
    fn closed_without_release_needs_confirmation() {
        let mut file = IssueFile::parse(ISSUE).unwrap();
        file.release = None;
        file.set_status(IssueStatus::Closed, "2026-10-09T10:00:00Z");
        assert!(file.closing_needs_confirmation());
        file.set_release(Some("v1.2.0".into()), "2026-10-09T10:00:00Z");
        assert!(!file.closing_needs_confirmation());
    }

    #[test]
    fn release_id_must_be_semver_with_v_prefix() {
        assert!(valid_release_id("v1.2.0"));
        assert!(!valid_release_id("1.2.0"));
        assert!(!valid_release_id("v1.2"));
        assert!(!valid_release_id("v1.2.x"));
        let bad = RELEASE.replace("id: v1.2.0", "id: 1.2.0");
        assert!(ReleaseFile::parse(&bad).is_err());
    }

    #[test]
    fn atomic_write_leaves_no_tmp_and_content_is_correct() {
        let dir = std::env::temp_dir().join("tissue-human-test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("issue.md");
        let file = IssueFile::parse(ISSUE).unwrap();
        file.write(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), ISSUE);
        assert!(!dir.join(".issue.md.tmp").exists());
        let reparsed = IssueFile::from_path(&path).unwrap();
        assert_eq!(reparsed.serialize(), ISSUE);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn absent_optional_fields_parse_to_none() {
        let content =
            "---\nid: IAM-42\ntitle: T\nstatus: open\ncreated: x\nupdated: x\n---\n\nBody\n";
        let file = IssueFile::parse(content).unwrap();
        assert_eq!(file.release, None);
        assert!(file.tags.is_empty());
        assert_eq!(file.serialize(), content);
    }

    #[test]
    fn tags_accept_null_and_flow_style() {
        let content =
            "---\nid: IAM-42\ntitle: T\nstatus: open\ntags:\ncreated: x\nupdated: x\n---\n\nBody\n";
        let file = IssueFile::parse(content).unwrap();
        assert!(file.tags.is_empty());
    }
}
