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

/// #145: `tissue create <ID> --title <TEXT>` equals the positional form;
/// both/neither errors cleanly.
#[test]
fn create_accepts_title_flag() {
    let repo = Repo::new("create-title");
    init(&repo);
    let out = repo.tissue(&["create", "IAM-1", "--title", "Flag title", "--body", "b"]);
    Repo::assert_success(&out, "create --title");
    let issue = repo.read_issue("IAM-1");
    assert!(
        issue.contains("title: Flag title"),
        "title written: {issue}"
    );
    // positional form unchanged
    let out = repo.tissue(&["create", "IAM-2", "Positional title", "--body", "b"]);
    Repo::assert_success(&out, "create positional");
    // both -> clear error
    let out = repo.tissue(&["create", "IAM-3", "--title", "X", "Y"]);
    Repo::assert_failure(&out, "both titles");
    // neither -> clear error
    let out = repo.tissue(&["create", "IAM-4"]);
    Repo::assert_failure(&out, "no title");
}

/// #138: derived last-touch — human / synced / ai-proposal across the
/// three transitions, visible in `tissue list`.
#[test]
fn last_touch_transitions_e2e() {
    let repo = Repo::new("last-touch");
    init(&repo);

    // Case 2: human creates — no stream — last touch is "human".
    create(&repo, "IAM-1", "Fresh issue");
    let list = Repo::stdout(&repo.tissue(&["list"]));
    assert!(list.contains("[human]"), "human-created: {list}");

    // Human syncs (establishes the baseline) -> "synced".
    let out = repo.tissue(&["sync", "IAM-1", "--summary", "baseline"]);
    Repo::assert_success(&out, "sync baseline");
    let list = Repo::stdout(&repo.tissue(&["list"]));
    assert!(list.contains("[synced]"), "after sync: {list}");

    // Agent proposes -> "ai-proposal" (+ NEEDS REVIEW flag as today).
    let out = repo.tissue(&[
        "propose",
        "IAM-1",
        "--field",
        "status",
        "--current",
        "open",
        "--proposed",
        "in-progress",
        "--rationale",
        "work started",
    ]);
    Repo::assert_success(&out, "propose");
    let list = Repo::stdout(&repo.tissue(&["list"]));
    assert!(list.contains("[ai-proposal]"), "pending proposal: {list}");
    assert!(list.contains("NEEDS REVIEW"), "flag still shown: {list}");

    // Human approves -> back to "human".
    let out = repo.tissue(&["approve", "IAM-1", "prop-01"]);
    Repo::assert_success(&out, "approve");
    let list = Repo::stdout(&repo.tissue(&["list"]));
    assert!(list.contains("[human]"), "after approval: {list}");
    assert!(!list.contains("[synced]"), "no stale state: {list}");
}

/// #119: `tissue project add/list/remove` — manage the multi-project
/// mapping without hand-editing config.yml. Every mutation is one commit;
/// list/serve project filters work immediately after add.
#[test]
fn project_add_list_remove_e2e() {
    let repo = Repo::new("project");
    init(&repo);

    // list: the init project is there, marked default
    let out = repo.tissue(&["project", "list"]);
    Repo::assert_success(&out, "project list");
    let list = Repo::stdout(&out);
    let config = std::fs::read_to_string(repo.root.join(".tissue/config.yml")).unwrap();
    let default_name = config
        .lines()
        .find_map(|l| l.trim().strip_prefix("project:"))
        .map(|v| v.trim().to_string())
        .expect("default project in config");
    assert!(
        list.contains(&format!("{default_name}\tIAM (default)")),
        "default listed: {list}"
    );

    // add: one commit, mapping written
    let before = repo.commit_count();
    let out = repo.tissue(&["project", "add", "second", "VER"]);
    Repo::assert_success(&out, "project add");
    assert_eq!(repo.commit_count(), before + 1, "add = one commit");
    let config = std::fs::read_to_string(repo.root.join(".tissue/config.yml")).unwrap();
    assert!(config.contains("second: VER"), "mapping written: {config}");

    // AC2: list --project and serve --project resolve immediately
    create(&repo, "VER-1", "Second project issue");
    let out = repo.tissue(&["list", "--project", "second"]);
    Repo::assert_success(&out, "list --project second");
    let list = Repo::stdout(&out);
    assert!(list.contains("VER-1"), "new project issue listed: {list}");
    let out = repo.tissue(&["project", "list"]);
    let list = Repo::stdout(&out);
    assert!(list.contains("second\tVER"), "second listed: {list}");

    // AC3: duplicate prefix under a different name is refused
    let out = repo.tissue(&["project", "add", "other", "VER"]);
    Repo::assert_failure(&out, "duplicate prefix");
    assert!(
        Repo::stderr(&out).contains("already mapped"),
        "duplicate prefix error: {}",
        Repo::stderr(&out)
    );
    // AC3: invalid prefix and name are refused
    let out = repo.tissue(&["project", "add", "bad", "ver"]);
    Repo::assert_failure(&out, "lowercase prefix");
    let out = repo.tissue(&["project", "add", "has space", "VER"]);
    Repo::assert_failure(&out, "invalid name");
    // unknown name on remove
    let out = repo.tissue(&["project", "remove", "nope"]);
    Repo::assert_failure(&out, "unknown remove");
    assert!(
        Repo::stderr(&out).contains("unknown project"),
        "unknown remove error: {}",
        Repo::stderr(&out)
    );

    // remove: refused while issues exist, --force removes (one commit)
    let before = repo.commit_count();
    let out = repo.tissue(&["project", "remove", "second"]);
    Repo::assert_failure(&out, "remove with issues");
    assert_eq!(repo.commit_count(), before, "refused remove = no commit");
    let out = repo.tissue(&["project", "remove", "second", "--force"]);
    Repo::assert_success(&out, "remove --force");
    assert_eq!(repo.commit_count(), before + 1, "remove = one commit");
    let config = std::fs::read_to_string(repo.root.join(".tissue/config.yml")).unwrap();
    assert!(!config.contains("second:"), "mapping removed: {config}");
    // the default project cannot be removed
    let out = repo.tissue(&["project", "remove", &default_name]);
    Repo::assert_failure(&out, "remove default");
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

    // sync: one commit, human-sync record in the stream. #136: the sync
    // records the field-level diff, so make a real human edit first
    // (uncommitted working tree — the human owns the format).
    let issue_md = repo.root.join(".tissue/issues/IAM-1/issue.md");
    let text = std::fs::read_to_string(&issue_md).unwrap();
    std::fs::write(
        &issue_md,
        text.replace("title: First issue", "title: First issue (edited)"),
    )
    .unwrap();
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
    assert!(
        stream.contains("\"changes\":[{\"field\":\"title\""),
        "sync record carries the field-level change (#136): {stream}"
    );

    // propose: needs-review flag surfaces in list
    let out = repo.tissue(&[
        "propose",
        "IAM-1",
        "--field",
        "title",
        "--current",
        "First issue (edited)",
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
