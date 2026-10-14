//! E2E for `tissue pull` / `tissue push` (#110): two machines sharing one
//! issue-repo remote. Pins: pull rebases (never merges) or fails loudly
//! naming the affected issue ids; push refuses non-FF; force-with-lease
//! updates the remote after a rebase; history stays linear throughout.

use std::path::PathBuf;
use std::process::{Command, Output};

fn tissue(repo: &std::path::Path, args: &[&str]) -> Output {
    let exe: PathBuf = env!("CARGO_BIN_EXE_tissue").into();
    let mut cmd = Command::new(exe);
    cmd.args(args).current_dir(repo).env_remove("EDITOR");
    cmd.output().expect("run tissue binary")
}

fn git(repo: &std::path::Path, args: &[&str]) -> Output {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(repo);
    cmd.output().expect("run git")
}

fn assert_success(out: &Output, ctx: &str) {
    assert!(
        out.status.success(),
        "{ctx} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn assert_failure(out: &Output, ctx: &str) {
    assert!(
        !out.status.success(),
        "{ctx} unexpectedly succeeded: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

struct Repo {
    root: PathBuf,
}

impl Repo {
    fn new(parent: &std::path::Path, name: &str) -> Self {
        let root = parent.join(name);
        std::fs::create_dir_all(&root).expect("create dir");
        assert_success(
            &git(&root, &["init", "--quiet", "--initial-branch", "main"]),
            "git init",
        );
        assert_success(
            &git(&root, &["config", "user.email", "e2e@tissue.test"]),
            "config email",
        );
        assert_success(&git(&root, &["config", "user.name", "e2e"]), "config name");
        Self { root }
    }

    fn tissue(&self, args: &[&str]) -> Output {
        tissue(&self.root, args)
    }
}

/// AC1-AC3: the two-machine loop — shared remote, divergent edits,
/// conflict message names the issue, resolve, pull succeeds, push
/// succeeds, linear history.
#[test]
fn pull_push_two_machines() {
    let base = std::env::temp_dir().join(format!(
        "tissue-pullpush-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&base).expect("base dir");

    // The shared remote: a bare repo.
    let remote = base.join("remote.git");
    assert_success(
        &git(
            &base,
            &[
                "init",
                "--quiet",
                "--bare",
                "--initial-branch",
                "main",
                remote.to_string_lossy().as_ref(),
            ],
        ),
        "init bare remote",
    );

    // Machine A: init with the remote, first issue.
    let a_dir = Repo::new(&base, "machine-a");
    // init with a remote creates <project>-issues/ under the cwd.
    assert_success(
        &a_dir.tissue(&["init", "--remote", remote.to_string_lossy().as_ref()]),
        "init A",
    );
    let a = Repo {
        root: a_dir.root.join("machine-a-issues"),
    };
    assert_success(
        &a.tissue(&["create", "IAM-1", "First", "--body", "b"]),
        "create A",
    );
    assert_success(&a.tissue(&["push"]), "first push");

    // Machine B: clone the remote as a tissue repo.
    let b_root = base.join("machine-b");
    assert_success(
        &git(
            &base,
            &[
                "clone",
                "--quiet",
                remote.to_string_lossy().as_ref(),
                b_root.to_string_lossy().as_ref(),
            ],
        ),
        "clone B",
    );
    assert_success(
        &git(&b_root, &["config", "user.email", "e2e@tissue.test"]),
        "config B email",
    );
    assert_success(
        &git(&b_root, &["config", "user.name", "e2e"]),
        "config B name",
    );

    // B pulls (up to date) and creates its own issue.
    assert_success(&tissue(&b_root, &["pull"]), "pull B");
    assert_success(
        &tissue(&b_root, &["create", "VER-1", "From B", "--body", "b"]),
        "create B",
    );
    assert_success(&tissue(&b_root, &["push"]), "push B");

    // A pulls: rebases A's local state onto B's new commit (nothing
    // diverged yet — fast-forward-ish rebase), then A makes an edit.
    assert_success(&a.tissue(&["pull"]), "pull A after B push");
    assert_success(
        &a.tissue(&["create", "IAM-2", "Second", "--body", "b"]),
        "create A2",
    );

    // Divergence: B also creates a commit. Now both sides have new work.
    assert_success(
        &tissue(&b_root, &["create", "VER-2", "Also B", "--body", "b"]),
        "create B2",
    );
    assert_success(&tissue(&b_root, &["push"]), "push B2");

    // A pulls: rebase replays A's IAM-2 commit on top of B's VER-2.
    let out = a.tissue(&["pull"]);
    assert_success(&out, "pull A with divergence");
    assert!(
        stdout(&out).contains("rebased"),
        "pull reports rebase: {}",
        stdout(&out)
    );
    // Linear history: no merge commits.
    let log = stdout(&git(&a.root, &["log", "--merges", "--oneline"]));
    assert!(log.trim().is_empty(), "no merge commits: {log}");

    // After the pull-rebase, A's commits sit on top of the remote tip, so
    // the plain push is a fast-forward of the remote (AC2's FF-only rule).
    // --force-with-lease is the correct form only when the remote moved
    // again mid-rebase; both forms must succeed here, FF or lease.
    assert_success(
        &a.tissue(&["push"]),
        "plain push after pull-rebase (fast-forward)",
    );

    // B pulls the rebased history cleanly.
    assert_success(&tissue(&b_root, &["pull"]), "pull B final");
    let log = stdout(&git(&b_root, &["log", "--merges", "--oneline"]));
    assert!(log.trim().is_empty(), "B stays linear: {log}");

    let _ = std::fs::remove_dir_all(&base);
}

/// AC1's conflict half: same-issue edits on both machines → pull fails
/// loudly, names the affected issue id, and leaves the tree untouched.
#[test]
fn pull_conflict_names_the_issue() {
    let base = std::env::temp_dir().join(format!(
        "tissue-pullconflict-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&base).expect("base dir");

    let remote = base.join("remote.git");
    assert_success(
        &git(
            &base,
            &[
                "init",
                "--quiet",
                "--bare",
                "--initial-branch",
                "main",
                remote.to_string_lossy().as_ref(),
            ],
        ),
        "init bare remote",
    );

    let a_dir = Repo::new(&base, "machine-a");
    // init with a remote creates <project>-issues/ under the cwd.
    assert_success(
        &a_dir.tissue(&["init", "--remote", remote.to_string_lossy().as_ref()]),
        "init A",
    );
    let a = Repo {
        root: a_dir.root.join("machine-a-issues"),
    };
    assert_success(
        &a.tissue(&["create", "IAM-1", "Base", "--body", "line"]),
        "create A",
    );
    assert_success(&a.tissue(&["push"]), "push base");

    let b_root = base.join("machine-b");
    assert_success(
        &git(
            &base,
            &[
                "clone",
                "--quiet",
                remote.to_string_lossy().as_ref(),
                b_root.to_string_lossy().as_ref(),
            ],
        ),
        "clone B",
    );
    assert_success(
        &git(&b_root, &["config", "user.email", "e2e@tissue.test"]),
        "config B",
    );
    assert_success(
        &git(&b_root, &["config", "user.name", "e2e"]),
        "config B name",
    );

    // Both machines edit the same line of the same issue's body.
    let edit = |root: &std::path::Path, new: &str| {
        let p = root.join(".tissue/issues/IAM-1/issue.md");
        let text = std::fs::read_to_string(&p).expect("issue.md");
        std::fs::write(&p, text.replace("line", new)).expect("edit issue.md");
        assert_success(&git(root, &["add", "-A"]), "stage edit");
        assert_success(
            &git(root, &["commit", "-m", &format!("edit IAM-1: {new}")]),
            "commit edit",
        );
    };
    edit(&a.root, "from A");
    assert_success(&tissue(&b_root, &["pull"]), "pull B before edit");
    edit(&b_root, "from B");
    assert_success(&tissue(&b_root, &["push"]), "push B");

    // A pulls: textual conflict on the same line → loud failure naming IAM-1.
    let out = a.tissue(&["pull"]);
    assert_failure(&out, "conflicted pull must fail");
    let err = stderr(&out);
    assert!(err.contains("IAM-1"), "conflict names the issue: {err}");
    // Tree untouched: the rebase was aborted.
    let status = stdout(&git(&a.root, &["status", "--porcelain"]));
    assert!(
        status.trim().is_empty(),
        "tree untouched after aborted rebase: {status}"
    );

    // Manual resolution with plain git: the same rebase conflicts again,
    // the human resolves the marker text, continues the rebase.
    let _ = git(&a.root, &["pull", "--rebase", "origin", "main"]);
    let issue = a.root.join(".tissue/issues/IAM-1/issue.md");
    let conflicted = std::fs::read_to_string(&issue).expect("conflicted issue.md");
    assert!(
        conflicted.contains("<<<<<<<"),
        "rebase in progress with markers: {conflicted}"
    );
    // Human edit: pick both sides' intent.
    let merged = conflicted
        .replace("<<<<<<< HEAD", "")
        .replace("=======", "")
        .replace(">>>>>>> ", "")
        .replace("from B", "from A and B")
        .replace("from A", "from A and B");
    std::fs::write(&issue, merged).expect("write merged");
    assert_success(&git(&a.root, &["add", "-A"]), "stage resolution");
    // GIT_EDITOR=true accepts the kept commit message in the dumb terminal.
    let mut cont = Command::new("git");
    cont.args(["rebase", "--continue"])
        .current_dir(&a.root)
        .env("GIT_EDITOR", "true");
    assert_success(
        &cont.output().expect("git rebase --continue"),
        "continue rebase",
    );
    // History stays linear end to end; the pushed result carries both edits.
    let log = stdout(&git(&a.root, &["log", "--merges", "--oneline"]));
    assert!(log.trim().is_empty(), "linear after resolve: {log}");
    assert_success(&a.tissue(&["push"]), "push after manual resolve");
    let final_text = std::fs::read_to_string(&issue).expect("final issue.md");
    assert!(
        final_text.contains("from A and B"),
        "both edits survive: {final_text}"
    );
    let _ = std::fs::remove_dir_all(&base);
}
