//! E2E tests for #171: `tissue dev-fixture <ID> --scenario <name>` generates
//! issues with human/AI data deliberately out of sync, deterministically,
//! one commit per mutation — and refuses to run unless `dev_fixtures: true`
//! is set in .tissue/config.yml (never on a real repo). The command is
//! hidden from --help.

mod common;

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str, fixtures_enabled: bool) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "tissue-devfixture-{}-{}-{}",
            label,
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let s = Self { root: root.clone() };
        s.git(&["init", "--quiet"]);
        s.git(&["config", "user.email", "e2e@tissue.test"]);
        s.git(&["config", "user.name", "e2e"]);
        let out = s.tissue(&["init"]);
        assert!(out.status.success(), "tissue init failed: {out:?}");
        if fixtures_enabled {
            let config = root.join(".tissue/config.yml");
            let content = std::fs::read_to_string(&config).unwrap();
            std::fs::write(&config, format!("{content}dev_fixtures: true\n")).unwrap();
            s.git(&["add", "-A"]);
            s.git(&["commit", "-m", "test: enable dev fixtures"]);
        }
        s
    }

    fn tissue(&self, args: &[&str]) -> Output {
        let exe: PathBuf = env!("CARGO_BIN_EXE_tissue").into();
        let mut cmd = Command::new(exe);
        cmd.args(args).current_dir(&self.root).env_remove("EDITOR");
        common::output(&mut cmd, common::default_timeout()).expect("run tissue binary")
    }

    fn tissue_ok(&self, args: &[&str]) -> String {
        let out = self.tissue(args);
        assert!(
            out.status.success(),
            "tissue {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn git(&self, args: &[&str]) -> String {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(&self.root);
        let out = common::output(&mut cmd, common::default_timeout()).expect("run git");
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn commit_count(&self) -> usize {
        self.git(&["rev-list", "--count", "HEAD"])
            .trim()
            .parse()
            .unwrap()
    }

    fn issue_md(&self, id: &str) -> String {
        std::fs::read_to_string(self.root.join(format!(".tissue/issues/{id}/issue.md")))
            .expect("issue.md")
    }

    fn stream(&self, id: &str) -> String {
        std::fs::read_to_string(self.root.join(format!(".tissue/issues/{id}/agent.jsonl")))
            .expect("agent.jsonl")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The command is hidden from --help but works when called directly.
#[test]
fn dev_fixture_is_hidden_but_usable() {
    let repo = Scratch::new("hidden", true);
    let help = repo.tissue(&["--help"]);
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(
        !stdout.contains("dev-fixture"),
        "hidden from --help: {stdout}"
    );
    let out = repo.tissue(&["dev-fixture", "IAM-1", "--scenario", "synced-clean"]);
    assert!(out.status.success(), "direct call works: {out:?}");
}

/// The opt-in guard: without `dev_fixtures: true` the command refuses and
/// writes nothing.
#[test]
fn refuses_without_dev_fixtures_opt_in() {
    let repo = Scratch::new("guard", false);
    let before = repo.commit_count();
    let out = repo.tissue(&["dev-fixture", "IAM-1", "--scenario", "synced-clean"]);
    assert!(!out.status.success(), "must refuse: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("dev_fixtures"), "clear refusal: {stderr}");
    assert!(!repo.root.join(".tissue/issues/IAM-1").exists());
    assert_eq!(repo.commit_count(), before, "nothing committed");
}

/// All five scenarios generate, and `tissue list` shows the expected
/// last-touch + needs-review states (#138/#154).
#[test]
fn all_scenarios_generate_expected_list_states() {
    let repo = Scratch::new("scenarios", true);
    let cases = [
        ("IAM-1", "human-edited", "[human]"),
        ("IAM-2", "agent-proposed", "[ai-proposal]"),
        ("IAM-3", "both-changed", "[ai-proposal]"),
        ("IAM-4", "agent-created-pending", "[ai-proposal]"),
        ("IAM-5", "synced-clean", "[synced]"),
    ];
    for (id, scenario, _) in cases {
        repo.tissue_ok(&["dev-fixture", id, "--scenario", scenario, "--seed", "7"]);
    }
    let list = repo.tissue_ok(&["list"]);
    for (id, scenario, touch) in cases {
        let line = list
            .lines()
            .find(|l| l.starts_with(id))
            .unwrap_or_else(|| panic!("{id} ({scenario}) in list: {list}"));
        assert!(line.contains(touch), "{id} last-touch {touch}: {line}");
        if scenario != "synced-clean" && scenario != "human-edited" {
            assert!(
                line.contains("[awaiting decision]"),
                "{id} awaiting decision: {line}"
            );
        }
    }
    // agent-created-pending has no issue.md yet, only a draft in the stream.
    assert!(!repo.root.join(".tissue/issues/IAM-4/issue.md").exists());
    assert!(repo.stream("IAM-4").contains("\"field\":\"create\""));
    // human-edited: front matter changed, sync produces the field diff.
    assert!(repo
        .issue_md("IAM-1")
        .contains("Fixture issue (human edit)"));
    let out = repo.tissue(&["sync", "IAM-1", "--summary", "sync the human edit"]);
    assert!(out.status.success(), "sync human-edited: {out:?}");
    let stream = repo.stream("IAM-1");
    assert!(stream.contains("\"type\":\"human-sync\""), "{stream}");
    assert!(
        stream.contains("Fixture issue (human edit)"),
        "field diff recorded: {stream}"
    );
    // synced-clean: re-sync is a no-op.
    let out = repo.tissue(&["sync", "IAM-5", "--summary", "nothing new"]);
    assert!(!out.status.success(), "synced-clean re-sync must no-op");
    assert!(String::from_utf8_lossy(&out.stderr).contains("nothing to sync"));
    // both-changed: approving the pending proposal now conflicts (stale).
    let out = repo.tissue(&["approve", "IAM-3", "prop-01"]);
    assert!(!out.status.success(), "approve must conflict: {out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .to_lowercase()
            .contains("stale"),
        "stale error: {out:?}"
    );
}

/// Determinism + commit-per-mutation: same seed gives identical timestamps;
/// every fixture mutation is its own commit.
#[test]
fn deterministic_seed_and_commit_per_mutation() {
    let repo = Scratch::new("deterministic", true);
    let before = repo.commit_count();
    repo.tissue_ok(&[
        "dev-fixture",
        "IAM-1",
        "--scenario",
        "agent-proposed",
        "--seed",
        "42",
    ]);
    let commits_after = repo.commit_count();
    assert!(
        commits_after > before + 1,
        "create + sync + propose = several commits: {before} -> {commits_after}"
    );
    let log = repo.git(&["log", "--pretty=%s"]);
    assert!(log.contains("(fixture)"), "fixture commits labeled: {log}");
    let stream = repo.stream("IAM-1");
    assert!(
        stream.contains("2026-02-12T"),
        "seed 42 => 2026-02-12 base: {stream}"
    );

    // Same seed, different id: identical timestamps.
    let repo2 = Scratch::new("deterministic2", true);
    repo2.tissue_ok(&[
        "dev-fixture",
        "IAM-9",
        "--scenario",
        "agent-proposed",
        "--seed",
        "42",
    ]);
    let stream2 = repo2.stream("IAM-9");
    let ts_of = |s: &str| -> Vec<String> {
        s.lines()
            .filter_map(|l| {
                l.split("\"ts\":\"")
                    .nth(1)
                    .map(|r| r.split('"').next().unwrap().to_string())
            })
            .collect()
    };
    assert_eq!(
        ts_of(&stream),
        ts_of(&stream2),
        "same seed => same timestamps"
    );
}
