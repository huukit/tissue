//! Edge and negative-path e2e tests (#66): pin the CLI's behavior on
//! invalid input, missing repos/issues, unknown or already-decided
//! proposals, and corrupted on-disk formats. Every case expects an
//! explicit error exit — never a panic — per AGENTS.md §5.

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
        let root = std::env::temp_dir().join(format!(
            "tissue-edge-{}-{}-{}",
            label,
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let repo = Self { root: root.clone() };
        repo.git(&["init", "--quiet"]);
        repo.git(&["config", "user.email", "edge@tissue.test"]);
        repo.git(&["config", "user.name", "edge"]);
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

    fn stdout(out: &Output) -> String {
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn stderr(out: &Output) -> String {
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    fn assert_failure(out: &Output, ctx: &str) {
        assert!(
            !out.status.success(),
            "{ctx} unexpectedly succeeded: {}",
            Repo::stdout(out)
        );
    }

    fn issue_path(&self, id: &str) -> PathBuf {
        self.root.join(".tissue/issues").join(id).join("issue.md")
    }

    fn stream_path(&self, id: &str) -> PathBuf {
        self.root
            .join(".tissue/issues")
            .join(id)
            .join("agent.jsonl")
    }
}

fn init(repo: &Repo) {
    let out = repo.tissue(&["init"]);
    assert!(
        out.status.success(),
        "tissue init failed: {}",
        Repo::stderr(&out)
    );
}

fn create(repo: &Repo, id: &str, title: &str) {
    let out = repo.tissue(&["create", id, title, "--body", "edge body"]);
    assert!(
        out.status.success(),
        "tissue create failed: {}",
        Repo::stderr(&out)
    );
}

/// Every command must refuse invalid issue ids with a clean error,
/// never a panic or a path escape (AGENTS.md §5).
#[test]
fn invalid_ids_are_rejected_by_every_command() {
    let repo = Repo::new("invalid-ids");
    init(&repo);
    create(&repo, "IAM-1", "Seed");

    for id in ["../escape", "/etc", "iam-1", "IAM-", "IAM-1/../IAM-9", ".."] {
        let cases: Vec<(&str, Vec<&str>)> = vec![
            ("create", vec!["create", id, "t", "--body", "b"]),
            ("show", vec!["show", id]),
            ("sync", vec!["sync", id, "--summary", "s"]),
            (
                "propose",
                vec![
                    "propose",
                    id,
                    "--field",
                    "title",
                    "--current",
                    "a",
                    "--proposed",
                    "b",
                ],
            ),
            ("approve", vec!["approve", id, "prop-1"]),
            ("reject", vec!["reject", id, "prop-1", "--reason", "r"]),
        ];
        for (cmd, args) in cases {
            let out = repo.tissue(&args);
            assert!(
                !out.status.success(),
                "{cmd} with id `{id}` unexpectedly succeeded"
            );
            assert!(
                Repo::stderr(&out).contains("invalid issue id"),
                "{cmd} with id `{id}` should report invalid id, got: {}",
                Repo::stderr(&out)
            );
        }
    }

    // no stray directories were created outside .tissue/issues
    assert!(!repo.root.join("escape").exists());
    assert!(!repo.root.join("IAM-9").exists());
}

/// Commands outside an initialized tissue repo fail with the clear
/// "not a tissue repository" error, no panic.
#[test]
fn commands_outside_a_tissue_repo_fail_cleanly() {
    let repo = Repo::new("outside-repo");
    // a git repo with no .tissue/
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("list", vec!["list"]),
        ("show", vec!["show", "IAM-1"]),
        ("sync", vec!["sync", "IAM-1", "--summary", "s"]),
        (
            "propose",
            vec![
                "propose",
                "IAM-1",
                "--field",
                "title",
                "--current",
                "a",
                "--proposed",
                "b",
            ],
        ),
        ("approve", vec!["approve", "IAM-1", "prop-1"]),
        ("reject", vec!["reject", "IAM-1", "prop-1", "--reason", "r"]),
    ];
    for (cmd, args) in cases {
        let out = repo.tissue(&args);
        assert!(
            !out.status.success(),
            "{cmd} outside a tissue repo unexpectedly succeeded"
        );
        assert!(
            Repo::stderr(&out).contains("not a tissue repository"),
            "{cmd} should report missing repo, got: {}",
            Repo::stderr(&out)
        );
    }
}

/// Operations on a nonexistent issue id error out explicitly.
///
/// `sync` also refuses a nonexistent id since the #68 fix — it used to
/// succeed, creating a phantom stream and commit.
#[test]
fn nonexistent_issue_errors_explicitly() {
    let repo = Repo::new("missing-issue");
    init(&repo);

    let out = repo.tissue(&["show", "IAM-999"]);
    Repo::assert_failure(&out, "show missing");
    let out = repo.tissue(&["sync", "IAM-999", "--summary", "s"]);
    Repo::assert_failure(&out, "sync missing (#68 fix)");
    assert!(
        Repo::stderr(&out).contains("not found"),
        "sync missing should say not found, got: {}",
        Repo::stderr(&out)
    );
    assert!(
        !repo.stream_path("IAM-999").exists(),
        "no phantom stream file (#68)"
    );
    let out = repo.tissue(&[
        "propose",
        "IAM-999",
        "--field",
        "title",
        "--current",
        "a",
        "--proposed",
        "b",
    ]);
    Repo::assert_failure(&out, "propose missing");
    let out = repo.tissue(&["approve", "IAM-999", "prop-1"]);
    Repo::assert_failure(&out, "approve missing");
    let out = repo.tissue(&["reject", "IAM-999", "prop-1", "--reason", "r"]);
    Repo::assert_failure(&out, "reject missing");
}

/// Creating an issue with an id that already exists is refused.
#[test]
fn duplicate_create_is_refused() {
    let repo = Repo::new("dup-create");
    init(&repo);
    create(&repo, "IAM-1", "First");

    let out = repo.tissue(&["create", "IAM-1", "Second", "--body", "b"]);
    Repo::assert_failure(&out, "duplicate create");
    assert!(
        Repo::stderr(&out).contains("already exists"),
        "duplicate create should say already exists, got: {}",
        Repo::stderr(&out)
    );

    // the original issue is untouched
    let issue = std::fs::read_to_string(repo.issue_path("IAM-1")).expect("read issue.md");
    assert!(
        issue.contains("First"),
        "original title must survive: {issue}"
    );
}

/// A proposal can only be decided once: approving twice fails with an
/// explicit illegal-transition error and leaves no second mutation.
#[test]
fn approving_twice_is_an_illegal_transition() {
    let repo = Repo::new("double-approve");
    init(&repo);
    create(&repo, "IAM-1", "First");

    let out = repo.tissue(&[
        "propose",
        "IAM-1",
        "--field",
        "title",
        "--current",
        "First",
        "--proposed",
        "Renamed",
    ]);
    assert!(
        out.status.success(),
        "propose failed: {}",
        Repo::stderr(&out)
    );
    let prop_id = Repo::stdout(&out)
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|t| t.starts_with("prop-"))
        .expect("proposal id")
        .to_string();

    let out = repo.tissue(&["approve", "IAM-1", &prop_id]);
    assert!(
        out.status.success(),
        "first approve failed: {}",
        Repo::stderr(&out)
    );

    let before = repo.git(&["rev-list", "--count", "HEAD"]);
    let out = repo.tissue(&["approve", "IAM-1", &prop_id]);
    Repo::assert_failure(&out, "second approve must fail");
    assert!(
        Repo::stderr(&out).to_lowercase().contains("not legal"),
        "second approve should name the illegal transition: {}",
        Repo::stderr(&out)
    );
    let after = repo.git(&["rev-list", "--count", "HEAD"]);
    assert_eq!(
        before.trim(),
        after.trim(),
        "failed approve must not commit"
    );
}

/// Approving an unknown proposal id fails explicitly.
#[test]
fn approving_an_unknown_proposal_fails() {
    let repo = Repo::new("unknown-proposal");
    init(&repo);
    create(&repo, "IAM-1", "First");

    let out = repo.tissue(&["approve", "IAM-1", "prop-99"]);
    Repo::assert_failure(&out, "approve unknown proposal");
    assert!(
        Repo::stderr(&out).contains("not found"),
        "unknown proposal should say not found, got: {}",
        Repo::stderr(&out)
    );
}

/// A malformed issue.md yields an explicit error for commands that
/// parse it — never a panic. `sync` does not parse issue.md (the human
/// may have just edited it; the summary record is the point), so it
/// succeeds — that is by design, not a bug.
#[test]
fn malformed_issue_md_errors_not_panics() {
    let repo = Repo::new("bad-issue-md");
    init(&repo);
    create(&repo, "IAM-1", "First");
    std::fs::write(repo.issue_path("IAM-1"), "not yaml front matter at all\n").expect("write");

    for args in [
        vec!["show", "IAM-1"],
        vec![
            "propose",
            "IAM-1",
            "--field",
            "title",
            "--current",
            "a",
            "--proposed",
            "b",
        ],
    ] {
        let out = repo.tissue(&args);
        assert!(
            !out.status.success(),
            "{args:?} on malformed issue.md unexpectedly succeeded"
        );
        assert!(
            !Repo::stderr(&out).contains("panicked"),
            "{args:?} must not panic, got: {}",
            Repo::stderr(&out)
        );
    }

    // sync does not parse issue.md — by design
    let out = repo.tissue(&["sync", "IAM-1", "--summary", "s"]);
    assert!(
        out.status.success(),
        "sync does not parse issue.md and must succeed: {}",
        Repo::stderr(&out)
    );

    // list skips unreadable issues with a warning instead of failing
    let out = repo.tissue(&["list"]);
    assert!(
        out.status.success(),
        "list must survive a bad issue: {}",
        Repo::stderr(&out)
    );
    assert!(
        !Repo::stdout(&out).contains("IAM-1"),
        "malformed issue must not be listed: {}",
        Repo::stdout(&out)
    );
}

/// An invalid agent.jsonl line yields an explicit error, not a panic.
#[test]
fn invalid_agent_stream_errors_not_panics() {
    let repo = Repo::new("bad-stream");
    init(&repo);
    create(&repo, "IAM-1", "First");
    std::fs::write(repo.stream_path("IAM-1"), "this is not json\n").expect("write stream");

    for args in [
        vec!["sync", "IAM-1", "--summary", "s"],
        vec![
            "propose",
            "IAM-1",
            "--field",
            "title",
            "--current",
            "First",
            "--proposed",
            "X",
        ],
        vec!["approve", "IAM-1", "prop-1"],
    ] {
        let out = repo.tissue(&args);
        assert!(
            !out.status.success(),
            "{args:?} on invalid agent.jsonl unexpectedly succeeded"
        );
        let err = Repo::stderr(&out);
        assert!(
            !err.contains("panicked"),
            "{args:?} must not panic, got: {err}"
        );
        assert!(
            err.contains("malformed") || err.contains("agent stream"),
            "{args:?} should name the stream problem, got: {err}"
        );
    }
}
