//! Dev fixtures (#171): generate issues whose human format and AI stream are
//! deliberately out of sync, for smoke/conflict testing of the sync loop
//! (#136), last-touch states (#138), and the propose/approve loop (#139).
//!
//! Safety: refuses to run unless `dev_fixtures: true` is set in
//! `.tissue/config.yml` — fixtures must never run against a real repo.
//! Deterministic: fixed actors and timestamps derived from `--seed`; every
//! mutation is still a proper git commit so the audit trail stays realistic.

use std::path::Path;

use crate::agent::{AgentStream, RecordData};
use crate::git::{Author, Repo};
use crate::human::{self, IssueFile, IssueStatus};
use crate::sync::{Actor, Core, Field, ProposalDraft};

/// The five fixture scenarios (CLI `--scenario` values).
pub const SCENARIOS: &[&str] = &[
    "human-edited",
    "agent-proposed",
    "both-changed",
    "agent-created-pending",
    "synced-clean",
    "typed-feature",
];

/// The opt-in switch: `dev_fixtures: true` in `.tissue/config.yml`.
pub fn dev_fixtures_enabled(repo_root: &Path) -> bool {
    let path = repo_root.join(".tissue").join("config.yml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    serde_yaml::from_str::<serde_yaml::Value>(&text)
        .ok()
        .and_then(|d| d.get("dev_fixtures").and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

/// Deterministic timestamp: 2026-01-01T00:00:00Z + seed days + step minutes.
fn ts(seed: u64, step: u64) -> String {
    let secs = 1_767_225_600u64 + seed.saturating_mul(86_400) + step * 60;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Days-from-civil (Howard Hinnant).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn actor(id: &str) -> Actor {
    Actor {
        author: Author {
            name: id.to_string(),
            email: format!("{}@tissue.local", id.replace(':', "-")),
        },
        id: id.to_string(),
    }
}

/// Append one agent observation record (one commit) — used to move the
/// derived last-touch off `synced` after an unsynced human edit.
fn append_observation(
    repo: &Repo,
    dir: &Path,
    id: &str,
    agent: &Actor,
    now: &str,
) -> Result<(), String> {
    let data = RecordData::Observation {
        summary: "fixture: agent observed the unsynced human edit".to_string(),
        evidence: "dev-fixture".to_string(),
    };
    let mut stream = AgentStream::open(dir.join("agent.jsonl"))
        .map_err(|e| format!("opening agent stream: {e}"))?;
    let rec = stream
        .append_agent(data, &agent.id, now)
        .map_err(|e| format!("append: {e}"))?;
    let seq = rec.seq;
    repo.commit_all(
        &agent.author,
        &format!("record {id}: observation (fixture, seq {seq})"),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Generate one fixture issue. Returns a one-line description of the state.
pub fn generate(repo: &Repo, id: &str, scenario: &str, seed: u64) -> Result<String, String> {
    if !SCENARIOS.contains(&scenario) {
        return Err(format!(
            "unknown scenario `{scenario}` (one of: {})",
            SCENARIOS.join(", ")
        ));
    }
    if !human::valid_issue_id(id) {
        return Err(format!(
            "invalid issue id `{id}`: must match [A-Z]+-<number>"
        ));
    }
    if !dev_fixtures_enabled(repo.root()) {
        return Err(
            "refusing to generate a fixture: `dev_fixtures: true` is not set in .tissue/config.yml \
             (dev fixtures must never run against a real repo — enable the flag only in a throwaway test repo)"
                .to_string(),
        );
    }
    let core = Core::new(repo.clone());
    let human = actor("human:fixture");
    let agent = actor("agent:fixture");
    let dir = repo
        .resolve(&[".tissue", "issues", id])
        .map_err(|e| e.to_string())?;

    // #139/#154: agent-created-pending has no issue.md at all — only a
    // creation proposal in the stream.
    if scenario == "agent-created-pending" {
        core.propose_create(
            id,
            &agent,
            &ts(seed, 0),
            "Fixture creation (agent)",
            "## Summary\n\nagent-created fixture body\n",
            "agent-created fixture awaiting review",
        )
        .map_err(|e| format!("propose create: {e}"))?;
        return Ok(format!(
            "{id}: agent-created-pending — creation proposal in the stream, no issue.md yet, needs-review"
        ));
    }

    if dir.exists() {
        return Err(format!("issue {id} already exists"));
    }
    // Baseline: create (one commit) + baseline sync (one commit).
    let issue_path = dir.join("issue.md");
    let issue = IssueFile::new(
        id,
        "Fixture issue",
        IssueStatus::Open,
        "## Summary\n\nfixture body\n",
        &ts(seed, 0),
    )
    .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    issue.write(&issue_path).map_err(|e| e.to_string())?;
    repo.commit_all(
        &human.author,
        &format!("create {id}: Fixture issue (fixture)"),
    )
    .map_err(|e| e.to_string())?;
    core.sync(id, &human, &ts(seed, 1), "baseline sync (fixture)")
        .map_err(|e| format!("baseline sync: {e}"))?;

    match scenario {
        "synced-clean" => Ok(format!(
            "{id}: synced-clean — baseline sync only; a re-sync is a no-op"
        )),
        "human-edited" => {
            // Human edits front matter directly (editor) and commits — no
            // sync — then the agent notes it (last stream record is the
            // observation, so last-touch derives to `human`).
            let mut issue = IssueFile::from_path(&issue_path).map_err(|e| e.to_string())?;
            issue.set_title("Fixture issue (human edit)".to_string(), &ts(seed, 2));
            issue.write(&issue_path).map_err(|e| e.to_string())?;
            repo.commit_all(
                &human.author,
                &format!("edit {id}: title (human, unsynced) (fixture)"),
            )
            .map_err(|e| e.to_string())?;
            append_observation(repo, &dir, id, &agent, &ts(seed, 3))?;
            Ok(format!(
                "{id}: human-edited — last-touch=human; `tissue sync` yields the title field diff"
            ))
        }
        "agent-proposed" => {
            core.propose(
                id,
                &agent,
                &ProposalDraft {
                    now: &ts(seed, 2),
                    field: Field::Title,
                    current: "Fixture issue",
                    proposed: "Fixture issue (agent title)",
                    rationale: "agent-proposed fixture",
                },
            )
            .map_err(|e| format!("propose: {e}"))?;
            Ok(format!(
                "{id}: agent-proposed — pending proposal, needs-review, last-touch=ai-proposal"
            ))
        }
        "both-changed" => {
            // Agent proposes first, then the human changes the same field:
            // approving afterwards hits the stale-current conflict path.
            core.propose(
                id,
                &agent,
                &ProposalDraft {
                    now: &ts(seed, 2),
                    field: Field::Title,
                    current: "Fixture issue",
                    proposed: "Fixture issue (agent title)",
                    rationale: "both-changed fixture",
                },
            )
            .map_err(|e| format!("propose: {e}"))?;
            let mut issue = IssueFile::from_path(&issue_path).map_err(|e| e.to_string())?;
            issue.set_title("Fixture issue (human edit)".to_string(), &ts(seed, 3));
            issue.write(&issue_path).map_err(|e| e.to_string())?;
            repo.commit_all(
                &human.author,
                &format!("edit {id}: title (human, after proposal) (fixture)"),
            )
            .map_err(|e| e.to_string())?;
            Ok(format!(
                "{id}: both-changed — pending proposal + unsynced human edit; approving now conflicts (stale)"
            ))
        }
        "typed-feature" => {
            // #211: a feature issue plus this issue as its child task
            // (linked via `parent`) — exercises the type/parent fields.
            let num: u64 = id
                .rsplit('-')
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            let prefix = id.rsplit('-').nth(1).unwrap_or("IAM");
            let feat_id = format!("{prefix}-{}", num + 900);
            let feat_dir = repo
                .resolve(&[".tissue", "issues", &feat_id])
                .map_err(|e| e.to_string())?;
            let mut feat = IssueFile::new(
                &feat_id,
                "Fixture feature",
                IssueStatus::Open,
                "## Summary\n\nfixture feature body\n",
                &ts(seed, 2),
            )
            .map_err(|e| e.to_string())?;
            feat.set_issue_type(crate::human::IssueType::Feature, &ts(seed, 2));
            std::fs::create_dir_all(&feat_dir)
                .map_err(|e| format!("creating {}: {e}", feat_dir.display()))?;
            feat.write(&feat_dir.join("issue.md"))
                .map_err(|e| e.to_string())?;
            repo.commit_all(
                &human.author,
                &format!("create {feat_id}: Fixture feature (fixture)"),
            )
            .map_err(|e| e.to_string())?;
            let mut issue = IssueFile::from_path(&issue_path).map_err(|e| e.to_string())?;
            issue
                .set_parent(Some(feat_id.clone()), &ts(seed, 3))
                .map_err(|e| e.to_string())?;
            issue.write(&issue_path).map_err(|e| e.to_string())?;
            repo.commit_all(
                &human.author,
                &format!("edit {id}: parent -> {feat_id} (fixture)"),
            )
            .map_err(|e| e.to_string())?;
            Ok(format!(
                "{id}: typed-feature — task with parent {feat_id} (a feature); the feature view lists it as a child"
            ))
        }
        _ => unreachable!("scenario checked above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Repo;

    fn temp_repo(fixtures: bool) -> (std::path::PathBuf, Repo) {
        let dir = std::env::temp_dir().join(format!(
            "tissue-fixture-test-{}-{}-{}",
            fixtures,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join(".tissue")).unwrap();
        let repo = Repo::init(&dir).unwrap();
        let flag = if fixtures { "dev_fixtures: true\n" } else { "" };
        std::fs::write(
            dir.join(".tissue/config.yml"),
            format!("project: demo\n{flag}"),
        )
        .unwrap();
        repo.commit_all(
            &Author {
                name: "t".into(),
                email: "t@t".into(),
            },
            "test config",
        )
        .unwrap();
        (dir, repo)
    }

    #[test]
    fn refuses_without_opt_in() {
        let (dir, repo) = temp_repo(false);
        let err = generate(&repo, "IAM-1", "synced-clean", 1).unwrap_err();
        assert!(err.contains("dev_fixtures"), "{err}");
        assert!(!dir.join(".tissue/issues/IAM-1").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_scenario_and_bad_id_rejected() {
        let (dir, repo) = temp_repo(true);
        assert!(generate(&repo, "IAM-1", "nope", 1)
            .unwrap_err()
            .contains("unknown scenario"));
        assert!(generate(&repo, "../x", "synced-clean", 1).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deterministic_timestamps() {
        assert_eq!(ts(1, 0), "2026-01-02T00:00:00Z");
        assert_eq!(ts(0, 5), "2026-01-01T00:05:00Z");
        assert_eq!(ts(2, 90), ts(2, 90));
    }

    #[test]
    fn all_scenarios_generate_and_carry_expected_state() {
        let (dir, repo) = temp_repo(true);
        let core = Core::new(repo.clone());
        for (i, scenario) in SCENARIOS.iter().enumerate() {
            let id = format!("IAM-{}", i + 1);
            generate(&repo, &id, scenario, 3).unwrap();
        }
        // synced-clean: last-touch synced, no needs-review
        assert_eq!(core.last_touch("IAM-5").unwrap(), "synced");
        // human-edited: last-touch human; sync yields the title diff
        assert_eq!(core.last_touch("IAM-1").unwrap(), "human");
        let human = actor("human:fixture");
        core.sync("IAM-1", &human, &ts(9, 0), "sync the human edit")
            .unwrap();
        let stream = core.read_stream("IAM-1").unwrap();
        let last = stream.last().unwrap();
        assert_eq!(last["type"], "human-sync");
        assert!(
            last["data"]["changes"]
                .to_string()
                .contains("Fixture issue (human edit)"),
            "field diff: {last}"
        );
        // agent-proposed: pending proposal -> ai-proposal
        assert_eq!(core.last_touch("IAM-2").unwrap(), "ai-proposal");
        let issue = IssueFile::from_path(&dir.join(".tissue/issues/IAM-2/issue.md")).unwrap();
        assert!(issue.needs_review());
        // both-changed: pending proposal; approve is stale
        assert_eq!(core.last_touch("IAM-3").unwrap(), "ai-proposal");
        let err = core
            .decide(
                "IAM-3",
                &human,
                &ts(9, 1),
                "prop-01",
                crate::state::Decision::Approve,
                None,
            )
            .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("stale"), "{err}");
        // agent-created-pending: no issue.md, draft in stream
        assert!(!dir.join(".tissue/issues/IAM-4/issue.md").exists());
        assert!(core.creation_draft("IAM-4").unwrap().is_some());
        // synced-clean re-sync is a no-op
        let err = core
            .sync("IAM-5", &human, &ts(9, 2), "nothing new")
            .unwrap_err();
        assert!(err.to_string().contains("nothing to sync"), "{err}");
        // #211 typed-feature: the issue is a task with a feature parent
        let typed = IssueFile::from_path(&dir.join(".tissue/issues/IAM-6/issue.md")).unwrap();
        assert_eq!(typed.issue_type, crate::human::IssueType::Task);
        assert_eq!(typed.parent.as_deref(), Some("IAM-906"));
        let feat = IssueFile::from_path(&dir.join(".tissue/issues/IAM-906/issue.md")).unwrap();
        assert_eq!(feat.issue_type, crate::human::IssueType::Feature);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
