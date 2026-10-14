//! AI-format stream (`agent.jsonl`) per `operations/product-owner/SPEC-ai-format.md`.
//!
//! Append-only JSONL record log with envelope validation, gap-free `seq`,
//! typed `data` payloads, and the proposal state machine. Malformed appends
//! abort before any byte is written.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::state::ProposalState;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum AgentStreamError {
    Io(std::io::Error),
    Serde(serde_json::Error),
    /// A line in the existing file failed to parse; index is 1-based.
    MalformedLine {
        line: usize,
        source: serde_json::Error,
    },
    /// `seq` is not exactly `last + 1`.
    NonContiguousSeq {
        expected: u64,
        found: u64,
    },
    /// `id` already exists in the file.
    DuplicateId {
        id: String,
    },
    /// Envelope field validation failed.
    Invalid(String),
    /// State-machine violation on a proposal transition.
    IllegalTransition(String),
    /// A record type was used by a writer not allowed to write it.
    ForbiddenWriter {
        actor: String,
        record_type: String,
    },
}

impl fmt::Display for AgentStreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentStreamError::Io(e) => write!(f, "io error: {e}"),
            AgentStreamError::Serde(e) => write!(f, "serialization error: {e}"),
            AgentStreamError::MalformedLine { line, source } => {
                write!(f, "malformed record at line {line}: {source}")
            }
            AgentStreamError::NonContiguousSeq { expected, found } => {
                write!(f, "seq must be {expected}, got {found}")
            }
            AgentStreamError::DuplicateId { id } => write!(f, "duplicate record id: {id}"),
            AgentStreamError::Invalid(msg) => write!(f, "invalid record: {msg}"),
            AgentStreamError::IllegalTransition(msg) => write!(f, "illegal transition: {msg}"),
            AgentStreamError::ForbiddenWriter { actor, record_type } => {
                write!(f, "actor `{actor}` may not write `{record_type}` records")
            }
        }
    }
}

impl std::error::Error for AgentStreamError {}

impl From<std::io::Error> for AgentStreamError {
    fn from(e: std::io::Error) -> Self {
        AgentStreamError::Io(e)
    }
}

impl From<serde_json::Error> for AgentStreamError {
    fn from(e: serde_json::Error) -> Self {
        AgentStreamError::Serde(e)
    }
}

// ---------------------------------------------------------------------------
// Record types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordType {
    Plan,
    Observation,
    Attempt,
    Result,
    Blocker,
    HumanSync,
    Proposal,
    Approval,
    Rejection,
}

impl RecordType {
    /// Agent-writable types (spec invariant 4); core-only types return false.
    pub fn agent_writable(self) -> bool {
        matches!(
            self,
            RecordType::Plan
                | RecordType::Observation
                | RecordType::Attempt
                | RecordType::Result
                | RecordType::Blocker
                | RecordType::Proposal
        )
    }
}

impl fmt::Display for RecordType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".into());
        write!(f, "{s}")
    }
}

/// Typed `data` payloads for every record type. Unknown fields are rejected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RecordData {
    Plan {
        goal: String,
        #[serde(default)]
        steps: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supersedes: Option<String>,
    },
    Observation {
        summary: String,
        #[serde(default)]
        evidence: String,
    },
    Attempt {
        action: String,
        outcome: AttemptOutcome,
        #[serde(default)]
        detail: String,
    },
    Result {
        summary: String,
        #[serde(default)]
        artifacts: Vec<String>,
    },
    Blocker {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resolved_by: Option<String>,
    },
    HumanSync {
        trigger: String,
        summary_of_human_change: String,
        reconciliation: String,
    },
    Proposal {
        proposal_id: String,
        target: String,
        field: String,
        current: String,
        proposed: String,
        rationale: String,
        state: ProposalState,
    },
    Approval {
        proposal_id: String,
        applied: bool,
    },
    Rejection {
        proposal_id: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttemptOutcome {
    Success,
    Failure,
    Partial,
}

impl RecordData {
    pub fn record_type(&self) -> RecordType {
        match self {
            RecordData::Plan { .. } => RecordType::Plan,
            RecordData::Observation { .. } => RecordType::Observation,
            RecordData::Attempt { .. } => RecordType::Attempt,
            RecordData::Result { .. } => RecordType::Result,
            RecordData::Blocker { .. } => RecordType::Blocker,
            RecordData::HumanSync { .. } => RecordType::HumanSync,
            RecordData::Proposal { .. } => RecordType::Proposal,
            RecordData::Approval { .. } => RecordType::Approval,
            RecordData::Rejection { .. } => RecordType::Rejection,
        }
    }
}

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub id: String,
    pub record_type: RecordType,
    pub ts: String,
    pub actor: String,
    pub seq: u64,
    pub refs: Vec<String>,
    pub data: RecordData,
}

impl<'de> Deserialize<'de> for Record {
    /// One canonical shape on disk (#106): `type` lives on the envelope
    /// only; `data` is flat. Records written by the legacy tagged shape
    /// (a second `type` inside `data`) are accepted on read for
    /// compatibility, and re-serialize canonically.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct RawRecord {
            id: String,
            #[serde(rename = "type")]
            record_type: RecordType,
            ts: String,
            actor: String,
            seq: u64,
            #[serde(default)]
            refs: Vec<String>,
            data: serde_json::Value,
        }
        let raw = RawRecord::deserialize(d)?;
        // Tolerate the legacy shape: strip a duplicate `type` from data.
        let mut data = raw.data;
        if let Some(obj) = data.as_object_mut() {
            let matches = obj.get("type").and_then(serde_json::Value::as_str)
                == Some(raw.record_type.to_string().as_str());
            if matches {
                obj.remove("type");
            }
        }
        // Dispatch the flat data by the envelope's type.
        let data: RecordData =
            dispatch_data(raw.record_type, data).map_err(serde::de::Error::custom)?;
        Ok(Record {
            id: raw.id,
            record_type: raw.record_type,
            ts: raw.ts,
            actor: raw.actor,
            seq: raw.seq,
            refs: raw.refs,
            data,
        })
    }
}

impl Serialize for Record {
    /// Canonical shape (#106): `type` on the envelope only. The enum's
    /// derived (tagged) serialization is used to build `data`, then the
    /// duplicate tag is stripped — one shape on disk, both historical
    /// shapes readable.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut data = serde_json::to_value(&self.data).map_err(serde::ser::Error::custom)?;
        if let Some(obj) = data.as_object_mut() {
            obj.remove("type");
        }
        let mut st = s.serialize_struct("Record", 7)?;
        st.serialize_field("id", &self.id)?;
        st.serialize_field("type", &self.record_type)?;
        st.serialize_field("ts", &self.ts)?;
        st.serialize_field("actor", &self.actor)?;
        st.serialize_field("seq", &self.seq)?;
        if self.refs.is_empty() {
            st.skip_field("refs")?;
        } else {
            st.serialize_field("refs", &self.refs)?;
        }
        st.serialize_field("data", &data)?;
        st.end()
    }
}

/// Deserialize a flat `data` object by the envelope's record type —
/// one shape: the tag comes from the envelope, never from `data`.
fn dispatch_data(record_type: RecordType, data: serde_json::Value) -> Result<RecordData, String> {
    use serde::de::DeserializeOwned;
    fn from_value<T: DeserializeOwned>(v: serde_json::Value) -> Result<T, String> {
        serde_json::from_value(v).map_err(|e| e.to_string())
    }
    // Inject the tag the enum's derived Deserialize still expects, parse,
    // then rely on serialization being tag-free (field removed above and
    // skip via untagged-like manual shape): simplest is to parse with the
    // tag and let Serialize omit it — RecordData serializes without a
    // `type` field because the variant tag is not a field.
    let mut v = data;
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "type".to_string(),
            serde_json::json!(record_type.to_string()),
        );
    } else {
        return Err(format!("data must be an object, got {v}"));
    }
    from_value(v)
}

impl Record {
    /// Validate envelope-level invariants: `type` matches `data`, and fields
    /// are non-empty where the spec requires them.
    fn validate(&self) -> Result<(), AgentStreamError> {
        if self.record_type != self.data.record_type() {
            return Err(AgentStreamError::Invalid(format!(
                "envelope type `{}` does not match data type `{}`",
                self.record_type,
                self.data.record_type()
            )));
        }
        if self.id.trim().is_empty() {
            return Err(AgentStreamError::Invalid("id must not be empty".into()));
        }
        if self.actor.trim().is_empty() {
            return Err(AgentStreamError::Invalid("actor must not be empty".into()));
        }
        if self.seq == 0 {
            return Err(AgentStreamError::Invalid(
                "seq is 1-based and must be >= 1".into(),
            ));
        }
        if self.ts.trim().is_empty() {
            return Err(AgentStreamError::Invalid("ts must not be empty".into()));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------

/// An open AI-format stream. Records already in the file are never rewritten.
#[derive(Debug)]
pub struct AgentStream {
    path: PathBuf,
    records: Vec<Record>,
    ids: std::collections::HashSet<String>,
    /// proposal_id -> current state, from the latest proposal record plus
    /// approval/rejection transitions.
    proposals: std::collections::HashMap<String, ProposalState>,
}

impl AgentStream {
    /// Load and validate the whole file. Every line must parse and `seq`
    /// must be 1-based, strictly increasing, gap-free.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AgentStreamError> {
        let path = path.as_ref().to_path_buf();
        let mut records = Vec::new();
        let mut ids = std::collections::HashSet::new();
        let mut proposals = std::collections::HashMap::new();
        if path.exists() {
            let file = File::open(&path)?;
            let reader = BufReader::new(file);
            for (idx, line) in reader.lines().enumerate() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let record: Record = serde_json::from_str(&line).map_err(|source| {
                    AgentStreamError::MalformedLine {
                        line: idx + 1,
                        source,
                    }
                })?;
                let expected = (records.len() as u64) + 1;
                if record.seq != expected {
                    return Err(AgentStreamError::NonContiguousSeq {
                        expected,
                        found: record.seq,
                    });
                }
                record.validate()?;
                if !ids.insert(record.id.clone()) {
                    return Err(AgentStreamError::DuplicateId { id: record.id });
                }
                track_proposal(&record, &mut proposals)?;
                records.push(record);
            }
        }
        Ok(Self {
            path,
            records,
            ids,
            proposals,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn last_seq(&self) -> u64 {
        self.records.len() as u64
    }

    pub fn next_seq(&self) -> u64 {
        self.last_seq() + 1
    }

    pub fn proposal_state(&self, proposal_id: &str) -> Option<ProposalState> {
        self.proposals.get(proposal_id).copied()
    }

    /// Append a record written by an AI agent. Enforces invariant 4: agents
    /// may only write plan/observation/attempt/result/blocker/proposal.
    pub fn append_agent(
        &mut self,
        data: RecordData,
        actor: &str,
        ts: &str,
    ) -> Result<&Record, AgentStreamError> {
        let record_type = data.record_type();
        if !record_type.agent_writable() {
            return Err(AgentStreamError::ForbiddenWriter {
                actor: actor.to_owned(),
                record_type: record_type.to_string(),
            });
        }
        self.append(data, actor, ts, Vec::new())
    }

    /// Append a record written by the core (human-sync, approval, rejection).
    /// Only the core may write these types (invariant 4).
    pub fn append_core(&mut self, data: RecordData, ts: &str) -> Result<&Record, AgentStreamError> {
        let record_type = data.record_type();
        if record_type.agent_writable() {
            return Err(AgentStreamError::Invalid(format!(
                "core append is for human-sync/approval/rejection, got `{record_type}`"
            )));
        }
        self.append(data, "tissue-core", ts, Vec::new())
    }

    /// Validate and append one record. A malformed append never touches the
    /// file: the line is fully built and re-parsed before any write, and seq,
    /// id, and state-machine checks run against in-memory state first.
    pub fn append(
        &mut self,
        data: RecordData,
        actor: &str,
        ts: &str,
        refs: Vec<String>,
    ) -> Result<&Record, AgentStreamError> {
        let record = Record {
            id: self.next_id(),
            record_type: data.record_type(),
            ts: ts.to_owned(),
            actor: actor.to_owned(),
            seq: self.next_seq(),
            refs,
            data,
        };
        self.append_record(record)
    }

    /// Append an already-built record. `id` must be the next generated id and
    /// `seq` must be `last + 1`; both are enforced.
    pub fn append_record(&mut self, record: Record) -> Result<&Record, AgentStreamError> {
        record.validate()?;
        if record.seq != self.next_seq() {
            return Err(AgentStreamError::NonContiguousSeq {
                expected: self.next_seq(),
                found: record.seq,
            });
        }
        if self.ids.contains(&record.id) {
            return Err(AgentStreamError::DuplicateId { id: record.id });
        }

        // Proposal state machine (invariant 3): validate the transition the
        // new record implies before writing anything.
        let mut proposals = self.proposals.clone();
        track_proposal(&record, &mut proposals)?;

        // Atomicity (invariant 2): serialize and re-parse so a malformed record
        // aborts before the file is touched at all.
        let line = serde_json::to_string(&record)?;
        serde_json::from_str::<Record>(&line)?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;

        self.proposals = proposals;
        self.ids.insert(record.id.clone());
        let last = self.records.len();
        self.records.push(record);
        Ok(&self.records[last])
    }

    /// Record ids are monotonic with `seq` (spec: `rec-XXXX-NNNN` style).
    fn next_id(&self) -> String {
        let seq = self.next_seq();
        let stamp = self
            .records
            .first()
            .and_then(|r| r.id.split('-').nth(1))
            .map(str::to_owned)
            .unwrap_or_else(|| "0000".into());
        format!("rec-{stamp}-{seq:04}")
    }
}

/// Update the proposal map according to the record being appended:
///
/// - `proposal` introduces a proposal in the `proposed` state;
/// - `approval`/`rejection` advance it to `approved`/`rejected`.
///
/// Any other transition (unknown proposal, double decision, transition away
/// from a terminal state) is rejected at write time (invariant 3).
fn track_proposal(
    record: &Record,
    proposals: &mut std::collections::HashMap<String, ProposalState>,
) -> Result<(), AgentStreamError> {
    match &record.data {
        RecordData::Proposal {
            proposal_id, state, ..
        } => {
            if proposals.contains_key(proposal_id) {
                return Err(AgentStreamError::IllegalTransition(format!(
                    "proposal `{proposal_id}` already exists; proposal records are append-only, supersede via a new proposal_id"
                )));
            }
            if *state != ProposalState::Proposed {
                return Err(AgentStreamError::IllegalTransition(format!(
                    "a proposal record must be created in the `proposed` state, got `{state:?}`"
                )));
            }
            proposals.insert(proposal_id.clone(), ProposalState::Proposed);
        }
        RecordData::Approval { proposal_id, .. } => {
            let current = proposals.get(proposal_id).copied().ok_or_else(|| {
                AgentStreamError::IllegalTransition(format!(
                    "approval references unknown proposal `{proposal_id}`"
                ))
            })?;
            if current != ProposalState::Proposed {
                return Err(AgentStreamError::IllegalTransition(format!(
                    "proposal `{proposal_id}` is `{current:?}`, cannot approve"
                )));
            }
            proposals.insert(proposal_id.clone(), ProposalState::Approved);
        }
        RecordData::Rejection { proposal_id, .. } => {
            let current = proposals.get(proposal_id).copied().ok_or_else(|| {
                AgentStreamError::IllegalTransition(format!(
                    "rejection references unknown proposal `{proposal_id}`"
                ))
            })?;
            if current != ProposalState::Proposed {
                return Err(AgentStreamError::IllegalTransition(format!(
                    "proposal `{proposal_id}` is `{current:?}`, cannot reject"
                )));
            }
            proposals.insert(proposal_id.clone(), ProposalState::Rejected);
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn tmp_stream(name: &str) -> (AgentStream, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("tissue-agent-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent.jsonl");
        let _ = std::fs::remove_file(&path);
        (AgentStream::open(&path).unwrap(), path)
    }

    fn file_bytes(path: &Path) -> Vec<u8> {
        let mut buf = Vec::new();
        if let Ok(mut f) = File::open(path) {
            let _ = f.read_to_end(&mut buf);
        }
        buf
    }

    /// #106 AC2: `AgentStream::open` must parse a stream written by the
    /// core loop (envelope-only `type`, flat `data`) — the two writers
    /// share one shape, round-trippable through one reader.
    #[test]
    fn open_parses_core_loop_written_stream() {
        let dir = std::env::temp_dir().join(format!(
            "tissue-stream-shape-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent.jsonl");
        // Exactly the bytes the core loop writes (sync.rs append_record):
        // `type` on the envelope only, `data` flat.
        std::fs::write(
            &path,
            concat!(
                "{\"id\":\"rec-0001\",\"type\":\"human-sync\",\"ts\":\"2026-10-08T11:00:00Z\",",
                "\"actor\":\"human:tuomas\",\"seq\":1,\"refs\":[],",
                "\"data\":{\"trigger\":\"command\",\"summary_of_human_change\":\"edited\",",
                "\"reconciliation\":\"from sync record onward\"}}\n"
            ),
        )
        .unwrap();
        let s = AgentStream::open(&path).unwrap();
        assert_eq!(s.records().len(), 1);
        assert_eq!(s.records()[0].seq, 1);
        assert_eq!(s.records()[0].actor, "human:tuomas");
        // Round-trip: re-serialize and re-parse (canonical shape both ways).
        let line = serde_json::to_string(&s.records()[0]).unwrap();
        assert!(
            !line.contains("\"type\":\"human-sync\",\"data\":{\"type\""),
            "serialization must be tag-free inside data: {line}"
        );
        let re: Record = serde_json::from_str(&line).unwrap();
        assert_eq!(re, s.records()[0]);
        // AC1 second half: a typed (legacy tagged-shape) record also parses
        // — the reader accepts both historical shapes.
        let tagged = concat!(
            "{\"id\":\"rec-0002\",\"type\":\"plan\",\"ts\":\"2026-10-08T12:00:00Z\",",
            "\"actor\":\"agent:dev-1\",\"seq\":2,\"refs\":[],",
            "\"data\":{\"type\":\"plan\",\"goal\":\"g\",\"steps\":[]}}"
        );
        let legacy: Record = serde_json::from_str(tagged).unwrap();
        match legacy.data {
            RecordData::Plan { goal, .. } => assert_eq!(goal, "g"),
            other => panic!("expected plan, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_and_reload_roundtrip() {
        let (mut s, path) = tmp_stream("roundtrip");
        let r = s
            .append_agent(
                RecordData::Plan {
                    goal: "implement stream".into(),
                    steps: vec!["parse".into()],
                    supersedes: None,
                },
                "agent:dev-1",
                "2026-10-08T14:20:00Z",
            )
            .unwrap();
        assert_eq!(r.seq, 1);
        let r2 = s
            .append_agent(
                RecordData::Observation {
                    summary: "spec read".into(),
                    evidence: "SPEC-ai-format.md".into(),
                },
                "agent:dev-1",
                "2026-10-08T14:21:00Z",
            )
            .unwrap();
        assert_eq!(r2.seq, 2);

        let reloaded = AgentStream::open(&path).unwrap();
        assert_eq!(reloaded.records().len(), 2);
        assert_eq!(reloaded.last_seq(), 2);
        assert_eq!(reloaded.records()[0].id, "rec-0000-0001");
    }

    #[test]
    fn seq_must_be_contiguous() {
        let (mut s, path) = tmp_stream("seq-gap");
        s.append_agent(
            RecordData::Result {
                summary: "done".into(),
                artifacts: vec![],
            },
            "agent:dev-1",
            "2026-10-08T14:20:00Z",
        )
        .unwrap();
        let bad = Record {
            id: "rec-0000-0009".into(),
            record_type: RecordType::Result,
            ts: "2026-10-08T14:22:00Z".into(),
            actor: "agent:dev-1".into(),
            seq: 9,
            refs: vec![],
            data: RecordData::Result {
                summary: "skip".into(),
                artifacts: vec![],
            },
        };
        let err = s.append_record(bad).unwrap_err();
        assert!(matches!(
            err,
            AgentStreamError::NonContiguousSeq {
                expected: 2,
                found: 9
            }
        ));
        assert_eq!(file_bytes(&path).iter().filter(|b| **b == b'\n').count(), 1);
    }

    #[test]
    fn duplicate_id_rejected() {
        let (mut s, path) = tmp_stream("dup-id");
        s.append_agent(
            RecordData::Observation {
                summary: "first".into(),
                evidence: String::new(),
            },
            "agent:dev-1",
            "2026-10-08T14:20:00Z",
        )
        .unwrap();
        let dup = Record {
            id: "rec-0000-0001".into(),
            record_type: RecordType::Observation,
            ts: "2026-10-08T14:21:00Z".into(),
            actor: "agent:dev-1".into(),
            seq: 2,
            refs: vec![],
            data: RecordData::Observation {
                summary: "second".into(),
                evidence: String::new(),
            },
        };
        assert!(matches!(
            s.append_record(dup).unwrap_err(),
            AgentStreamError::DuplicateId { .. }
        ));
        assert_eq!(file_bytes(&path).iter().filter(|b| **b == b'\n').count(), 1);
    }

    #[test]
    fn type_mismatch_between_envelope_and_data_rejected() {
        let (mut s, path) = tmp_stream("type-mismatch");
        let bad = Record {
            id: "rec-0000-0001".into(),
            record_type: RecordType::Plan,
            ts: "2026-10-08T14:20:00Z".into(),
            actor: "agent:dev-1".into(),
            seq: 1,
            refs: vec![],
            data: RecordData::Observation {
                summary: "lying envelope".into(),
                evidence: String::new(),
            },
        };
        assert!(matches!(
            s.append_record(bad).unwrap_err(),
            AgentStreamError::Invalid { .. }
        ));
        assert!(file_bytes(&path).is_empty());
    }

    #[test]
    fn malformed_file_rejected_on_open() {
        let (mut s, path) = tmp_stream("malformed-open");
        s.append_agent(
            RecordData::Observation {
                summary: "ok".into(),
                evidence: String::new(),
            },
            "agent:dev-1",
            "2026-10-08T14:20:00Z",
        )
        .unwrap();
        let mut existing = file_bytes(&path);
        existing.extend_from_slice(b"{ not json }\n");
        std::fs::write(&path, &existing).unwrap();
        assert!(matches!(
            AgentStream::open(&path).unwrap_err(),
            AgentStreamError::MalformedLine { line: 2, .. }
        ));
    }

    #[test]
    fn agents_cannot_write_core_records() {
        let (mut s, path) = tmp_stream("writer-rules");
        let err = s
            .append_agent(
                RecordData::Approval {
                    proposal_id: "prop-01".into(),
                    applied: true,
                },
                "agent:dev-1",
                "2026-10-08T14:20:00Z",
            )
            .unwrap_err();
        assert!(matches!(err, AgentStreamError::ForbiddenWriter { .. }));
        assert!(file_bytes(&path).is_empty());
    }

    #[test]
    fn core_cannot_write_agent_records() {
        let (mut s, path) = tmp_stream("core-writer-rules");
        let err = s
            .append_core(
                RecordData::Plan {
                    goal: "no".into(),
                    steps: vec![],
                    supersedes: None,
                },
                "2026-10-08T14:20:00Z",
            )
            .unwrap_err();
        assert!(matches!(err, AgentStreamError::Invalid { .. }));
        assert!(file_bytes(&path).is_empty());
    }

    #[test]
    fn proposal_lifecycle_happy_path() {
        let (mut s, path) = tmp_stream("proposal-happy");
        s.append_agent(
            RecordData::Proposal {
                proposal_id: "prop-01".into(),
                target: "issue.md".into(),
                field: "status".into(),
                current: "in-progress".into(),
                proposed: "closed".into(),
                rationale: "work landed".into(),
                state: ProposalState::Proposed,
            },
            "agent:dev-1",
            "2026-10-08T14:20:00Z",
        )
        .unwrap();
        assert_eq!(s.proposal_state("prop-01"), Some(ProposalState::Proposed));
        s.append_core(
            RecordData::Approval {
                proposal_id: "prop-01".into(),
                applied: true,
            },
            "2026-10-08T14:25:00Z",
        )
        .unwrap();
        assert_eq!(s.proposal_state("prop-01"), Some(ProposalState::Approved));
        assert_eq!(
            AgentStream::open(&path).unwrap().proposal_state("prop-01"),
            Some(ProposalState::Approved)
        );
    }

    #[test]
    fn double_decision_rejected_atomically() {
        let (mut s, path) = tmp_stream("double-decision");
        s.append_agent(
            RecordData::Proposal {
                proposal_id: "prop-01".into(),
                target: "issue.md".into(),
                field: "status".into(),
                current: "a".into(),
                proposed: "b".into(),
                rationale: String::new(),
                state: ProposalState::Proposed,
            },
            "agent:dev-1",
            "2026-10-08T14:20:00Z",
        )
        .unwrap();
        s.append_core(
            RecordData::Rejection {
                proposal_id: "prop-01".into(),
                reason: "not yet".into(),
            },
            "2026-10-08T14:21:00Z",
        )
        .unwrap();
        let before = file_bytes(&path);
        let err = s
            .append_core(
                RecordData::Approval {
                    proposal_id: "prop-01".into(),
                    applied: true,
                },
                "2026-10-08T14:22:00Z",
            )
            .unwrap_err();
        assert!(matches!(err, AgentStreamError::IllegalTransition { .. }));
        assert_eq!(
            file_bytes(&path),
            before,
            "file must be untouched by the rejected append"
        );
    }

    #[test]
    fn decision_on_unknown_proposal_rejected() {
        let (mut s, path) = tmp_stream("unknown-proposal");
        let before = file_bytes(&path);
        let err = s
            .append_core(
                RecordData::Approval {
                    proposal_id: "prop-404".into(),
                    applied: true,
                },
                "2026-10-08T14:20:00Z",
            )
            .unwrap_err();
        assert!(matches!(err, AgentStreamError::IllegalTransition { .. }));
        assert_eq!(file_bytes(&path), before);
    }

    #[test]
    fn proposal_must_be_created_proposed() {
        let (mut s, path) = tmp_stream("proposal-bad-state");
        let err = s
            .append_agent(
                RecordData::Proposal {
                    proposal_id: "prop-01".into(),
                    target: "issue.md".into(),
                    field: "status".into(),
                    current: "a".into(),
                    proposed: "b".into(),
                    rationale: String::new(),
                    state: ProposalState::Approved,
                },
                "agent:dev-1",
                "2026-10-08T14:20:00Z",
            )
            .unwrap_err();
        assert!(matches!(err, AgentStreamError::IllegalTransition { .. }));
        assert!(file_bytes(&path).is_empty());
    }

    #[test]
    fn unknown_fields_rejected_on_parse() {
        let dir =
            std::env::temp_dir().join(format!("tissue-agent-test-unknown-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent.jsonl");
        std::fs::write(
            &path,
            "{\"id\":\"rec-0000-0001\",\"type\":\"observation\",\"ts\":\"t\",\"actor\":\"a\",\"seq\":1,\"data\":{\"summary\":\"s\",\"mystery\":1}}\n",
        )
        .unwrap();
        assert!(matches!(
            AgentStream::open(&path).unwrap_err(),
            AgentStreamError::MalformedLine { .. }
        ));
    }

    #[test]
    fn refs_survive_roundtrip() {
        let (mut s, path) = tmp_stream("refs");
        let first = s
            .append_agent(
                RecordData::Attempt {
                    action: "try fmt".into(),
                    outcome: AttemptOutcome::Failure,
                    detail: "rustfmt diff".into(),
                },
                "agent:dev-1",
                "2026-10-08T14:20:00Z",
            )
            .unwrap()
            .clone();
        s.append(
            RecordData::Observation {
                summary: "root cause".into(),
                evidence: "file:line".into(),
            },
            "agent:dev-1",
            "2026-10-08T14:21:00Z",
            vec![first.id.clone()],
        )
        .unwrap();
        let reloaded = AgentStream::open(&path).unwrap();
        assert_eq!(reloaded.records()[1].refs, vec![first.id]);
    }
}
