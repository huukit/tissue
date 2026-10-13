//! End-to-end release-command tests (#81): the release lifecycle over
//! the compiled `tissue` binary — create → assign → cut → show — plus
//! the derived-list invariant (issue front matter is the only source
//! of truth) and the mutation-per-commit audit trail.

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
            "tissue-release-e2e-{}-{}-{}",
            label,
            std::process::id(),
            n
        ));
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

    fn commit_count(&self) -> usize {
        self.git(&["rev-list", "--count", "HEAD"])
            .trim()
            .parse()
            .expect("commit count")
    }
}

fn init(repo: &Repo) {
    let out = repo.tissue(&["init"]);
    Repo::assert_success(&out, "tissue init");
}

fn create(repo: &Repo, id: &str, title: &str) {
    let out = repo.tissue(&["create", id, title, "--body", "release e2e body"]);
    Repo::assert_success(&out, "tissue create");
}

#[test]
fn release_lifecycle_create_assign_cut_show() {
    let repo = Repo::new("lifecycle");
    init(&repo);
    create(&repo, "IAM-1", "First issue");
    create(&repo, "IAM-2", "Second issue");

    // create: draft release, exactly one commit
    let before = repo.commit_count();
    let out = repo.tissue(&[
        "release",
        "create",
        "v1.0.0",
        "First cut",
        "--body",
        "Ship it",
    ]);
    Repo::assert_success(&out, "release create");
    assert_eq!(repo.commit_count(), before + 1, "create = one commit");
    let release_md = std::fs::read_to_string(repo.root.join(".tissue/releases/v1.0.0/release.md"))
        .expect("release.md exists");
    assert!(release_md.contains("id: v1.0.0"));
    assert!(release_md.contains("status: draft"));

    // list shows it
    let list = Repo::stdout(&repo.tissue(&["release", "list"]));
    assert!(
        list.contains("v1.0.0") && list.contains("draft"),
        "list: {list}"
    );

    // assign: writes the front-matter link on the issue, one commit
    let out = repo.tissue(&["release", "assign", "IAM-1", "v1.0.0"]);
    Repo::assert_success(&out, "release assign");
    assert_eq!(repo.commit_count(), before + 2, "assign = one commit");
    let issue_md =
        std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-1/issue.md")).unwrap();
    assert!(
        issue_md.contains("release: v1.0.0"),
        "front-matter link: {issue_md}"
    );

    // assign a second issue; re-assigning to another release is refused
    let out = repo.tissue(&["release", "assign", "IAM-2", "v1.0.0"]);
    Repo::assert_success(&out, "release assign 2");
    let out = repo.tissue(&["release", "create", "v2.0.0", "Second cut"]);
    Repo::assert_success(&out, "release create 2");
    let out = repo.tissue(&["release", "assign", "IAM-1", "v2.0.0"]);
    Repo::assert_failure(&out, "reassign must fail");
    assert!(
        Repo::stderr(&out).contains("already assigned"),
        "reassign error: {}",
        Repo::stderr(&out)
    );

    // show: assigned issues derived from front matter (IAM-1, IAM-2, not IAM-999)
    let show = Repo::stdout(&repo.tissue(&["release", "show", "v1.0.0"]));
    assert!(
        show.contains("v1.0.0") && show.contains("draft"),
        "show header: {show}"
    );
    assert!(
        show.contains("IAM-1") && show.contains("IAM-2"),
        "derived list: {show}"
    );
    assert!(!show.contains("IAM-999"), "no phantom issues: {show}");

    // cut: status -> cut, close-out commit, tag v1.0.0
    let before_cut = repo.commit_count();
    let out = repo.tissue(&["release", "cut", "v1.0.0"]);
    Repo::assert_success(&out, "release cut");
    assert_eq!(repo.commit_count(), before_cut + 1, "cut = one commit");
    let tags = repo.git(&["tag", "--list"]);
    assert!(
        tags.lines().any(|t| t == "v1.0.0"),
        "git tag must be exactly v1.0.0, got: {tags}"
    );
    let release_md =
        std::fs::read_to_string(repo.root.join(".tissue/releases/v1.0.0/release.md")).unwrap();
    assert!(
        release_md.contains("status: cut"),
        "finalized: {release_md}"
    );
    let show = Repo::stdout(&repo.tissue(&["release", "show", "v1.0.0"]));
    assert!(show.contains("cut"), "show reflects cut state: {show}");

    // cutting twice is refused
    let out = repo.tissue(&["release", "cut", "v1.0.0"]);
    Repo::assert_failure(&out, "double cut must fail");
    assert!(
        Repo::stderr(&out).contains("already cut"),
        "double cut error: {}",
        Repo::stderr(&out)
    );

    // audit trail is legible
    let log = repo.git(&["log", "--oneline"]);
    for needle in [
        "release create v1.0.0",
        "release assign IAM-1",
        "release cut v1.0.0",
    ] {
        assert!(log.contains(needle), "log missing {needle}: {log}");
    }
}

#[test]
fn release_edge_cases_error_cleanly() {
    let repo = Repo::new("edge");
    init(&repo);
    create(&repo, "IAM-1", "First issue");

    // invalid release ids: not v+semver, and traversal
    for bad in ["1.0.0", "v1.0", "../escape", "v1.0.0.0"] {
        let out = repo.tissue(&["release", "create", bad, "T"]);
        Repo::assert_failure(&out, "invalid release id create");
        let out = repo.tissue(&["release", "show", bad]);
        Repo::assert_failure(&out, "invalid release id show");
    }
    assert!(
        !repo.root.join(".tissue/releases/escape").exists(),
        "no traversal escape"
    );

    // duplicate create refused
    let out = repo.tissue(&["release", "create", "v1.0.0", "First"]);
    Repo::assert_success(&out, "create");
    let out = repo.tissue(&["release", "create", "v1.0.0", "Again"]);
    Repo::assert_failure(&out, "duplicate create");
    assert!(
        Repo::stderr(&out).contains("already exists"),
        "duplicate error: {}",
        Repo::stderr(&out)
    );

    // assign to a nonexistent release / nonexistent issue refused
    let out = repo.tissue(&["release", "assign", "IAM-1", "v9.9.9"]);
    Repo::assert_failure(&out, "assign to missing release");
    let out = repo.tissue(&["release", "assign", "IAM-999", "v1.0.0"]);
    Repo::assert_failure(&out, "assign missing issue");

    // show a nonexistent release refused
    let out = repo.tissue(&["release", "show", "v9.9.9"]);
    Repo::assert_failure(&out, "show missing release");

    // cut a nonexistent release refused
    let out = repo.tissue(&["release", "cut", "v9.9.9"]);
    Repo::assert_failure(&out, "cut missing release");
}
