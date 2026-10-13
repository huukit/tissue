//! End-to-end tests: drive the compiled `tissue` binary against a scratch
//! git repository, exactly the way a human or an AI agent would (#22).
//!
//! These are the PoC (#3) acceptance evidence in automated form: the full
//! propose → tag → approve loop, the stale-proposal fail-safe, the
//! rejection path, and the commit-per-mutation audit trail.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Repo {
    root: PathBuf,
}

impl Repo {
    fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("tissue-e2e-{}-{}-{}", label, std::process::id(), n));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let repo = Self { root: root.clone() };
        repo.git(&["init", "--quiet"]);
        repo.git(&["config", "user.email", "e2e@tissue.test"]);
        repo.git(&["config", "user.name", "e2e"]);
        repo
    }

    fn tissue(&self, args: &[&str]) -> Output {
        let exe: PathBuf = env!("CARGO_BIN_EXE_tissue").into();
        let mut cmd = Command::new(exe);
        cmd.args(args).current_dir(&self.root).env_remove("EDITOR");
        cmd.output().expect("run tissue binary")
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?} failed: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn commit_count(&self) -> usize {
        let s = self.git(&["rev-list", "--count", "HEAD"]);
        s.trim().parse().expect("commit count")
    }

    fn stdout(out: &Output) -> String {
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn stderr(out: &Output) -> String {
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    fn assert_success(out: &Output, ctx: &str) {
        assert!(
            out.status.success(),
            "{ctx} failed: {}\nstdout: {}",
            Repo::stderr(out),
            Repo::stdout(out)
        );
    }

    fn assert_failure(out: &Output, ctx: &str) {
        assert!(
            !out.status.success(),
            "{ctx} unexpectedly succeeded: {}",
            Repo::stdout(out)
        );
    }

    fn read_issue(&self, id: &str) -> String {
        let p = self.root.join(".tissue/issues").join(id).join("issue.md");
        std::fs::read_to_string(p).expect("read issue.md")
    }

    fn agent_stream(&self, id: &str) -> String {
        let p = self
            .root
            .join(".tissue/issues")
            .join(id)
            .join("agent.jsonl");
        std::fs::read_to_string(p).unwrap_or_default()
    }
}

fn init(repo: &Repo) {
    let out = repo.tissue(&["init"]);
    Repo::assert_success(&out, "tissue init");
}

fn create(repo: &Repo, id: &str, title: &str) {
    let out = repo.tissue(&["create", id, title, "--body", "e2e body"]);
    Repo::assert_success(&out, "tissue create");
}

/// Extract the proposal id (`prop-XX`) from `tissue propose` output.
fn proposal_id_from(out: &Output) -> String {
    Repo::stdout(out)
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|t| t.starts_with("prop-"))
        .expect("extract proposal id")
        .to_string()
}

#[test]
fn version_reports_workspace_version() {
    let repo = Repo::new("version");
    let out = repo.tissue(&["--version"]);
    Repo::assert_success(&out, "tissue --version");
    let expected = concat!(env!("CARGO_PKG_VERSION"));
    assert!(
        Repo::stdout(&out).contains(expected),
        "--version must contain the workspace version {expected}: {}",
        Repo::stdout(&out)
    );
}

#[test]
fn full_dogfood_loop_e2e() {
    let repo = Repo::new("full-loop");
    init(&repo);
    let after_init = repo.commit_count();
    create(&repo, "IAM-1", "First issue");
    create(&repo, "IAM-2", "Second issue");
    assert_eq!(
        repo.commit_count(),
        after_init + 2,
        "create = exactly one commit per issue"
    );
    let log = repo.git(&["log", "--oneline"]);
    assert!(log.contains("create IAM-1: First issue"), "log: {log}");
    assert!(log.contains("create IAM-2: Second issue"), "log: {log}");
    let status = repo.git(&["status", "--porcelain"]);
    assert!(
        status.trim().is_empty(),
        "clean tree after create: {status}"
    );

    // list: both issues visible, no review flags
    let list = Repo::stdout(&repo.tissue(&["list"]));
    assert!(list.contains("IAM-1") && list.contains("IAM-2"));
    assert!(!list.contains("NEEDS REVIEW"), "no flags expected: {list}");

    // sync: one commit, human-sync record in the stream
    let before = repo.commit_count();
    let out = repo.tissue(&["sync", "IAM-1", "--summary", "human edit"]);
    Repo::assert_success(&out, "tissue sync");
    assert_eq!(repo.commit_count(), before + 1, "sync = exactly one commit");
    let sync_log = repo.git(&["log", "-1", "--oneline"]);
    assert!(
        sync_log.starts_with(|c: char| c.is_ascii_hexdigit()) && sync_log.contains("sync IAM-1"),
        "sync commit is its own commit, not absorbing creation: {sync_log}"
    );
    let stream = repo.agent_stream("IAM-1");
    assert!(stream.contains("\"type\":\"human-sync\""));
    assert!(stream.contains("human edit"));

    // propose: needs-review flag surfaces in list
    let out = repo.tissue(&[
        "propose",
        "IAM-1",
        "--field",
        "title",
        "--current",
        "First issue",
        "--proposed",
        "Renamed issue",
        "--rationale",
        "clearer",
    ]);
    Repo::assert_success(&out, "tissue propose");
    let proposal_out = Repo::stdout(&out);
    assert!(
        proposal_out.contains("prop-"),
        "proposal id printed: {proposal_out}"
    );
    assert_eq!(
        repo.commit_count(),
        before + 2,
        "propose = exactly one commit"
    );
    let list = Repo::stdout(&repo.tissue(&["list"]));
    assert!(
        list.contains("IAM-1") && list.contains("NEEDS REVIEW"),
        "flag expected: {list}"
    );

    // approve: human format updated, tag cleared, one commit
    let prop_id = proposal_id_from(&out);
    let out = repo.tissue(&["approve", "IAM-1", &prop_id]);
    Repo::assert_success(&out, "tissue approve");
    assert_eq!(
        repo.commit_count(),
        before + 3,
        "approve = exactly one commit"
    );
    let issue = repo.read_issue("IAM-1");
    assert!(issue.contains("Renamed issue"), "title applied: {issue}");
    let list = Repo::stdout(&repo.tissue(&["list"]));
    assert!(!list.contains("NEEDS REVIEW"), "tag cleared: {list}");
    let stream = repo.agent_stream("IAM-1");
    assert!(stream.contains("\"type\":\"approval\""));

    // git log is a legible audit trail of the loop
    let log = repo.git(&["log", "--oneline"]);
    for needle in ["sync IAM-1", "propose IAM-1", "approve IAM-1"] {
        assert!(log.contains(needle), "legible log missing {needle}: {log}");
    }
}

#[test]
fn stale_proposal_is_refused_and_never_clobbers() {
    let repo = Repo::new("stale");
    init(&repo);
    create(&repo, "IAM-1", "First issue");

    let out = repo.tissue(&[
        "propose",
        "IAM-1",
        "--field",
        "title",
        "--current",
        "Wrong current value",
        "--proposed",
        "X",
    ]);
    Repo::assert_failure(&out, "stale propose must fail");
    assert!(
        Repo::stderr(&out).to_lowercase().contains("stale"),
        "error should name staleness: {}",
        Repo::stderr(&out)
    );

    // no proposal was created: no tag, no stream record, human format untouched
    let list = Repo::stdout(&repo.tissue(&["list"]));
    assert!(!list.contains("NEEDS REVIEW"));
    assert!(repo.agent_stream("IAM-1").is_empty(), "no stream records");
    let issue = repo.read_issue("IAM-1");
    assert!(issue.contains("First issue"), "human format untouched");
}

#[test]
fn rejection_records_reason_and_clears_tag() {
    let repo = Repo::new("reject");
    init(&repo);
    create(&repo, "IAM-1", "First issue");

    let out = repo.tissue(&[
        "propose",
        "IAM-1",
        "--field",
        "status",
        "--current",
        "open",
        "--proposed",
        "closed",
    ]);
    Repo::assert_success(&out, "tissue propose");
    let list = Repo::stdout(&repo.tissue(&["list"]));
    assert!(list.contains("NEEDS REVIEW"));

    let prop_id = proposal_id_from(&out);
    let out = repo.tissue(&["reject", "IAM-1", &prop_id, "--reason", "not done"]);
    Repo::assert_success(&out, "tissue reject");

    let list = Repo::stdout(&repo.tissue(&["list"]));
    assert!(!list.contains("NEEDS REVIEW"), "tag cleared after reject");
    let issue = repo.read_issue("IAM-1");
    assert!(issue.contains("open"), "human format untouched by reject");
    let stream = repo.agent_stream("IAM-1");
    assert!(stream.contains("\"type\":\"rejection\""));
    assert!(stream.contains("not done"), "reason recorded as data");

    // the loop is legible in git log
    let log = repo.git(&["log", "--oneline"]);
    assert!(log.contains("reject IAM-1"));
}

#[test]
fn show_and_create_validate_input() {
    let repo = Repo::new("validation");
    init(&repo);

    // invalid id: rejected (AGENTS.md §5 path safety)
    let out = repo.tissue(&["create", "../escape", "t", "--body", "b"]);
    Repo::assert_failure(&out, "invalid id must fail");

    let out = repo.tissue(&["create", "IAM-1", "Ok", "--body", "e2e body"]);
    Repo::assert_success(&out, "valid create");

    // show prints front matter and body
    let out = repo.tissue(&["show", "IAM-1"]);
    Repo::assert_success(&out, "tissue show");
    let show = Repo::stdout(&out);
    assert!(
        show.contains("IAM-1") && show.contains("Ok") && show.contains("e2e body"),
        "show output: {show}"
    );

    // unknown issue: clean error, no panic
    let out = repo.tissue(&["show", "IAM-999"]);
    Repo::assert_failure(&out, "unknown issue must fail");
}
#[test]
fn multi_project_filters_and_full_loop() {
    let repo = Repo::new("multi-project");
    let out = repo.tissue(&["init", "--project", "e2e"]);
    Repo::assert_success(&out, "tissue init --project");
    // AC5 graceful default: the projects map exists from init; a second
    // project is added to config.yml the way a user would.
    let cfg_path = repo.root.join(".tissue/config.yml");
    let cfg = std::fs::read_to_string(&cfg_path).expect("read config.yml");
    assert!(cfg.contains("projects:"), "init writes the projects map");
    let cfg2 = format!("{cfg}  second: VER\n");
    std::fs::write(&cfg_path, cfg2).expect("extend config.yml");

    // AC1: two prefixes coexist; the full loop works per-prefix.
    create(&repo, "IAM-1", "identity issue");
    create(&repo, "VER-1", "verification issue");
    for (id, title) in [("IAM-1", "identity issue"), ("VER-1", "verification issue")] {
        let out = repo.tissue(&["show", id]);
        Repo::assert_success(&out, "show per-prefix");
        assert!(Repo::stdout(&out).contains(title), "show {id}");
    }

    // AC2: --project filter and --all.
    let out = repo.tissue(&["list", "--project", "e2e"]);
    Repo::assert_success(&out, "list --project");
    let iam_list = Repo::stdout(&out);
    assert!(
        iam_list.contains("IAM-1") && !iam_list.contains("VER-1"),
        "{iam_list}"
    );
    let out = repo.tissue(&["list", "--project", "second"]);
    Repo::assert_success(&out, "list --project second");
    let ver_list = Repo::stdout(&out);
    assert!(
        ver_list.contains("VER-1") && !ver_list.contains("IAM-1"),
        "{ver_list}"
    );
    let out = repo.tissue(&["list", "--all"]);
    Repo::assert_success(&out, "list --all");
    let all = Repo::stdout(&out);
    assert!(all.contains("IAM-1") && all.contains("VER-1"), "{all}");
    // unknown project name: clear error
    let out = repo.tissue(&["list", "--project", "nope"]);
    Repo::assert_failure(&out, "unknown project must fail");

    // AC4: approve one proposal from each project — full loop on both.
    let mut ids = Vec::new();
    for id in ["IAM-1", "VER-1"] {
        let out = repo.tissue(&[
            "propose",
            id,
            "--field",
            "status",
            "--current",
            "open",
            "--proposed",
            "in-progress",
            "--rationale",
            "e2e",
            "--actor",
            "agent:e2e",
        ]);
        Repo::assert_success(&out, "propose {id}");
        ids.push(proposal_id_from(&out));
    }
    for (id, pid) in [("IAM-1", ids[0].clone()), ("VER-1", ids[1].clone())] {
        let out = repo.tissue(&["approve", id, &pid, "--actor", "human:e2e"]);
        Repo::assert_success(&out, "approve {id}");
        let issue = repo.read_issue(id);
        assert!(issue.contains("in-progress"), "applied to {id}");
    }
}
