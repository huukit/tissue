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

/// #211 (decision 15): the workflow states. `closed` is a read-time alias
/// for `done` (migration on read; written back as `done` on next mutation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueStatus {
    Open,
    InProgress,
    Review,
    Done,
    Rejected,
}

impl IssueStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            IssueStatus::Open => "open",
            IssueStatus::InProgress => "in-progress",
            IssueStatus::Review => "review",
            IssueStatus::Done => "done",
            IssueStatus::Rejected => "rejected",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "open" => Ok(IssueStatus::Open),
            "in-progress" => Ok(IssueStatus::InProgress),
            "review" => Ok(IssueStatus::Review),
            "done" | "closed" => Ok(IssueStatus::Done),
            "rejected" => Ok(IssueStatus::Rejected),
            other => Err(Error::Validation(format!(
                "invalid status `{other}` (expected open, in-progress, review, done, or rejected)"
            ))),
        }
    }

    /// #211 workflow transitions: open→in-progress→review→done,
    /// review→in-progress (changes requested), any→rejected, rejected→open
    /// (reopen). Identity (same→same) is a no-op and always allowed.
    pub fn can_transition(from: IssueStatus, to: IssueStatus) -> bool {
        if from == to {
            return true;
        }
        match from {
            IssueStatus::Open => matches!(to, IssueStatus::InProgress | IssueStatus::Rejected),
            IssueStatus::InProgress => matches!(to, IssueStatus::Review | IssueStatus::Rejected),
            IssueStatus::Review => matches!(
                to,
                IssueStatus::InProgress | IssueStatus::Done | IssueStatus::Rejected
            ),
            IssueStatus::Done => matches!(to, IssueStatus::Rejected),
            IssueStatus::Rejected => matches!(to, IssueStatus::Open),
        }
    }

    pub fn transition_error(from: IssueStatus, to: IssueStatus) -> String {
        format!(
            "illegal status transition {} -> {} (workflow: open→in-progress→review→done; review→in-progress; any→rejected; rejected→open)",
            from.as_str(),
            to.as_str()
        )
    }
}

/// #211 (decision 15): issue type. Missing `type` in front matter reads as
/// `task` (backwards compatible with all pre-#211 issues).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueType {
    Feature,
    Task,
    Bug,
}

impl IssueType {
    pub fn as_str(&self) -> &'static str {
        match self {
            IssueType::Feature => "feature",
            IssueType::Task => "task",
            IssueType::Bug => "bug",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "feature" => Ok(IssueType::Feature),
            "task" => Ok(IssueType::Task),
            "bug" => Ok(IssueType::Bug),
            other => Err(Error::Validation(format!(
                "invalid type `{other}` (expected feature, task, or bug)"
            ))),
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

/// #195: YAML double-quoted scalar — safe inside a flow sequence for any
/// string (unlike `yaml_scalar`, which quotes for block context only).
fn yaml_dquote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
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
    /// #211: issue type; defaults to `task` when absent in front matter.
    pub issue_type: IssueType,
    pub status: IssueStatus,
    /// #211: parent feature id (task/bug belonging to a feature); absent =
    /// no parent.
    pub parent: Option<String>,
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
            issue_type: IssueType::Task,
            status,
            parent: None,
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
        // #211: absent `type` reads as `task` (backwards compatible).
        let issue_type = match opt_string_field(&mapping, "type")? {
            None => IssueType::Task,
            Some(t) => IssueType::parse(&t)?,
        };
        let status = IssueStatus::parse(&string_field(&mapping, "status")?)?;
        let parent = match opt_string_field(&mapping, "parent")? {
            None => None,
            Some(p) => {
                if !valid_issue_id(&p) {
                    return Err(Error::Validation(format!(
                        "parent `{p}` does not match pattern [A-Z]+-<number>"
                    )));
                }
                Some(p)
            }
        };
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
                "id", "title", "type", "status", "parent", "release", "tags", "created", "updated",
                "revision",
            ],
        );
        Ok(IssueFile {
            id,
            title,
            issue_type,
            status,
            parent,
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
            ("type", self.issue_type.as_str().to_string()),
            ("status", self.status.as_str().to_string()),
            ("parent", self.parent.clone().unwrap_or_default()),
            ("release", self.release.clone().unwrap_or_default()),
            (
                "tags",
                if self.tags.is_empty() {
                    "[]".to_string()
                } else if self.tags.iter().all(|t| valid_tag(t)) {
                    // Plain flow scalars are safe for the restricted charset
                    // — byte-identical to the historical render.
                    format!("[{}]", self.tags.join(", "))
                } else {
                    // #195: hand-edited/imported special-char tags must stay
                    // safe inside the flow sequence — yaml_scalar quotes for
                    // block context only, so double-quote each element here.
                    format!(
                        "[{}]",
                        self.tags
                            .iter()
                            .map(|t| yaml_dquote(t))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
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

    /// #211: a programmatic status change — enforces the workflow
    /// transitions (open→in-progress→review→done; review→in-progress;
    /// any→rejected; rejected→open) on top of the raw `set_status`.
    pub fn transition_to(&mut self, status: IssueStatus, now: &str) -> Result<()> {
        if !IssueStatus::can_transition(self.status, status) {
            return Err(Error::Validation(IssueStatus::transition_error(
                self.status,
                status,
            )));
        }
        self.set_status(status, now);
        Ok(())
    }

    pub fn set_issue_type(&mut self, issue_type: IssueType, now: &str) {
        self.issue_type = issue_type;
        self.front.invalidate();
        self.updated = now.to_string();
    }

    pub fn set_parent(&mut self, parent: Option<String>, now: &str) -> Result<()> {
        if let Some(p) = &parent {
            if !valid_issue_id(p) {
                return Err(Error::Validation(format!(
                    "parent `{p}` does not match pattern [A-Z]+-<number>"
                )));
            }
            if *p == self.id {
                return Err(Error::Validation(
                    "an issue cannot be its own parent".into(),
                ));
            }
        }
        self.parent = parent;
        self.front.invalidate();
        self.updated = now.to_string();
        Ok(())
    }

    pub fn set_release(&mut self, release: Option<String>, now: &str) {
        self.release = release;
        self.front.invalidate();
        self.updated = now.to_string();
    }

    pub fn add_tag(&mut self, tag: &str, now: &str) -> Result<()> {
        // #195: reject unsafe tag values at the mutation point.
        if !valid_tag(tag) {
            return Err(Error::Validation(format!(
                "tag `{tag}` is not valid (allowed: a-z, 0-9, '-', '_'; must start with a letter or digit)"
            )));
        }
        if !self.has_tag(tag) {
            self.tags.push(tag.to_string());
            self.front.invalidate();
            self.updated = now.to_string();
        }
        Ok(())
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
    /// before applying a close. (#211: `closed` migrated to `done`.)
    pub fn closing_needs_confirmation(&self) -> bool {
        self.status == IssueStatus::Done && self.release.is_none()
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

/// #195: tag charset — kebab-case lowercase (`needs-review` style), safe as
/// an unquoted YAML flow-sequence scalar. Mirrors `projects::valid_project_prefix`.
pub fn valid_tag(tag: &str) -> bool {
    let mut chars = tag.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
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
    /// Build a fresh draft release (created/updated = now).
    pub fn new(id: &str, title: &str, body: &str, now: &str) -> Result<Self> {
        if !valid_release_id(id) {
            return Err(Error::Validation(format!("id `{id}` is not `v` + semver")));
        }
        Ok(ReleaseFile {
            id: id.to_string(),
            title: title.to_string(),
            status: ReleaseStatus::Draft,
            target: None,
            created: now.to_string(),
            updated: now.to_string(),
            body: body.to_string(),
            front: FrontMatter {
                raw: None,
                extra: serde_yaml::Mapping::new(),
                dirty: true,
            },
        })
    }

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
        file.set_status(IssueStatus::Done, "2026-10-09T10:00:00Z");
        let out = file.serialize();
        let reparsed = IssueFile::parse(&out).unwrap();
        assert_eq!(reparsed.status, IssueStatus::Done);
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
        file.add_tag(NEEDS_REVIEW, "2026-10-09T11:00:00Z").unwrap();
        assert!(file.needs_review());
    }

    /// #195 (review Finding 4): unsafe tag values are rejected at the
    /// mutation point, hand-edited special-char tags still round-trip
    /// through serialize, and the current constant tag renders
    /// byte-for-byte unchanged.
    #[test]
    fn tag_charset_enforced_and_serialization_safe() {
        let mut file = IssueFile::new("IAM-1", "T", IssueStatus::Open, "b", "x").unwrap();
        for bad in [
            "bad,tag",
            "x]y",
            "has: colon",
            "a b",
            "",
            "-leading",
            "NEEDS-REVIEW",
            "tag;drop",
        ] {
            assert!(
                file.add_tag(bad, "x").is_err(),
                "unsafe tag must be rejected: {bad:?}"
            );
        }
        assert!(file.tags.is_empty(), "no unsafe tag was stored");
        file.add_tag("needs-review", "x").unwrap();
        file.add_tag("v2_rollout", "x").unwrap();
        assert_eq!(file.tags, vec!["needs-review", "v2_rollout"]);

        // A hand-edited front matter carrying special-char tags must survive
        // the CANONICAL render (a mutation invalidates the raw cache) and
        // re-parse to the same tags — flow-safe quoting, no corruption.
        let src = "---\nid: IAM-9\ntitle: T\nstatus: open\ntags: [\"a, b\", \"x]y\", \"a\\\"b\"]\ncreated: x\nupdated: x\n---\n\nBody\n";
        let mut parsed = IssueFile::parse(src).unwrap();
        assert_eq!(
            parsed.tags,
            vec!["a, b".to_string(), "x]y".to_string(), "a\"b".to_string()]
        );
        parsed.add_tag("v2_rollout", "x").unwrap(); // forces canonical render
        let out = parsed.serialize();
        assert!(
            out.contains("tags: [\"a, b\", \"x]y\", \"a\\\"b\", \"v2_rollout\"]"),
            "flow-safe quoting: {out}"
        );
        let reparsed = IssueFile::parse(&out).unwrap();
        assert_eq!(
            reparsed.tags, parsed.tags,
            "round-trip must be lossless: {out}"
        );

        // Byte-for-byte for the current constant tag.
        let mut f2 = IssueFile::new("IAM-2", "T", IssueStatus::Open, "b", "x").unwrap();
        f2.add_tag(NEEDS_REVIEW, "x").unwrap();
        assert!(
            f2.serialize().contains("tags: [needs-review]"),
            "current render unchanged: {}",
            f2.serialize()
        );
    }

    #[test]
    fn done_without_release_needs_confirmation() {
        let mut file = IssueFile::parse(ISSUE).unwrap();
        file.release = None;
        file.set_status(IssueStatus::Done, "2026-10-09T10:00:00Z");
        assert!(file.closing_needs_confirmation());
        file.set_release(Some("v1.2.0".into()), "2026-10-09T10:00:00Z");
        assert!(!file.closing_needs_confirmation());
    }

    /// #211: `closed` migrates to `done` on read; an unmutated file keeps
    /// its raw bytes, a mutation writes the canonical `done` back.
    #[test]
    fn closed_migrates_to_done_on_read_and_writeback() {
        let legacy = ISSUE.replace("status: open", "status: closed");
        let file = IssueFile::parse(&legacy).unwrap();
        assert_eq!(file.status, IssueStatus::Done);
        assert_eq!(file.status.as_str(), "done");
        // Unmutated: byte-identical round-trip (no data rewrite).
        assert_eq!(file.serialize(), legacy);
        // Mutated: canonical render carries `done`, never `closed`.
        let mut mutated = IssueFile::parse(&legacy).unwrap();
        mutated.set_title("New title".into(), "2026-10-10T00:00:00Z");
        let out = mutated.serialize();
        assert!(out.contains("status: done"), "write-back: {out}");
        assert!(!out.contains("status: closed"), "no legacy value: {out}");
        assert_eq!(IssueFile::parse(&out).unwrap().status, IssueStatus::Done);
    }

    /// #211: workflow transition matrix.
    #[test]
    fn status_transition_rules() {
        use IssueStatus::*;
        let legal = [
            (Open, InProgress),
            (Open, Rejected),
            (InProgress, Review),
            (InProgress, Rejected),
            (Review, InProgress),
            (Review, Done),
            (Review, Rejected),
            (Done, Rejected),
            (Rejected, Open),
        ];
        let all = [Open, InProgress, Review, Done, Rejected];
        for from in all {
            for to in all {
                assert_eq!(
                    IssueStatus::can_transition(from, to),
                    from == to || legal.contains(&(from, to)),
                    "{from:?} -> {to:?}"
                );
            }
        }
        // transition_to enforces; set_status is the raw mutator.
        let mut file = IssueFile::parse(ISSUE).unwrap();
        assert!(
            file.transition_to(Done, "x").is_err(),
            "open -> done illegal"
        );
        assert!(file.transition_to(InProgress, "x").is_ok());
        assert_eq!(file.status, InProgress);
        let err = file.transition_to(Open, "x").unwrap_err();
        assert!(
            err.to_string().contains("illegal status transition"),
            "{err}"
        );
    }

    /// #211: type defaults to task, parent validates, both round-trip.
    #[test]
    fn type_and_parent_round_trip() {
        // Absent type/parent: backwards compatible, raw preserved.
        let file = IssueFile::parse(ISSUE).unwrap();
        assert_eq!(file.issue_type, IssueType::Task);
        assert_eq!(file.parent, None);
        assert_eq!(file.serialize(), ISSUE);

        // Present: parse + byte-identical round-trip.
        let typed = ISSUE.replace("status: open", "type: feature\nstatus: open");
        let file = IssueFile::parse(&typed).unwrap();
        assert_eq!(file.issue_type, IssueType::Feature);
        assert_eq!(file.serialize(), typed);

        let with_parent = ISSUE.replace("status: open", "type: bug\nstatus: open\nparent: IAM-42");
        let file = IssueFile::parse(&with_parent).unwrap();
        assert_eq!(file.issue_type, IssueType::Bug);
        assert_eq!(file.parent.as_deref(), Some("IAM-42"));
        assert_eq!(file.serialize(), with_parent);

        // Canonical render of a mutated file carries type/parent.
        let mut file = IssueFile::parse(ISSUE).unwrap();
        file.set_issue_type(IssueType::Bug, "x");
        file.set_parent(Some("IAM-1".into()), "x").unwrap();
        let out = file.serialize();
        assert!(out.contains("type: bug"), "{out}");
        assert!(out.contains("parent: IAM-1"), "{out}");
        let reparsed = IssueFile::parse(&out).unwrap();
        assert_eq!(reparsed.issue_type, IssueType::Bug);
        assert_eq!(reparsed.parent.as_deref(), Some("IAM-1"));

        // Invalid values are rejected at parse and at the mutation point.
        assert!(
            IssueFile::parse(&ISSUE.replace("status: open", "type: epic\nstatus: open")).is_err()
        );
        assert!(
            IssueFile::parse(&ISSUE.replace("status: open", "parent: ../evil\nstatus: open"))
                .is_err()
        );
        let mut file = IssueFile::parse(ISSUE).unwrap();
        assert!(file.set_parent(Some("not-an-id".into()), "x").is_err());
        assert!(
            file.set_parent(Some("IAM-42".into()), "x").is_err(),
            "self-parent"
        );
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
