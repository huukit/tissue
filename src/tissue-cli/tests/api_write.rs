//! End-to-end tests for the agent write API (#80): the full remote
//! agent loop over HTTP — create → append → propose — with the human
//! approving via the CLI. Pins: token auth required and never logged,
//! reads stay loopback-only and tokenless, every write produces a
//! commit, and the human format is never written directly.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "tissue-api-write-{}-{}-{}",
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
        s
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
        self.git(&["rev-list", "--count", "HEAD"])
            .trim()
            .parse()
            .expect("commit count")
    }

    fn set_token(&self, token: &str) {
        let config = self.root.join(".tissue/config.yml");
        let content = std::fs::read_to_string(&config).expect("config.yml exists after init");
        std::fs::write(&config, format!("{content}api_token: \"{token}\"\n"))
            .expect("write config");
        self.git(&["add", "-A"]);
        self.git(&["commit", "-m", "test: set api token"]);
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Server {
    child: Child,
    addr: SocketAddr,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_server(repo: &Scratch) -> Server {
    start_server_with_env(repo, &[])
}

/// Start serve with extra environment pairs (e.g. TISSUE_API_TOKEN).
fn start_server_with_env(repo: &Scratch, env: &[(&str, &str)]) -> Server {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("reserve port");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    let exe: PathBuf = env!("CARGO_BIN_EXE_tissue").into();
    let mut cmd = Command::new(exe);
    cmd.args(["serve", "--port", &port.to_string()])
        .current_dir(&repo.root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn tissue serve");
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    for _ in 0..100 {
        if TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(50)).is_ok() {
            return Server { child, addr };
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("server did not come up on {addr}");
}

fn http(addr: &SocketAddr, request: &str) -> (String, String) {
    let mut last_err = None;
    for attempt in 0..10 {
        match http_once(addr, request) {
            Ok(r) => return r,
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(25 * (attempt + 1)));
            }
        }
    }
    panic!("request to {addr} failed after retries: {last_err:?}");
}

fn http_once(addr: &SocketAddr, request: &str) -> Result<(String, String), String> {
    let mut stream = TcpStream::connect(addr).map_err(|e| format!("connect: {e}"))?;
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write request: {e}"))?;
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader
        .read_line(&mut status)
        .map_err(|e| format!("read status: {e}"))?;
    if status.is_empty() {
        return Err("empty status line".to_string());
    }
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("read header: {e}"))?;
        if line.trim().is_empty() {
            break;
        }
        headers.push_str(&line);
    }
    let mut body = String::new();
    reader
        .read_to_string(&mut body)
        .map_err(|e| format!("read body: {e}"))?;
    Ok((status, body))
}

fn post(
    addr: &SocketAddr,
    path: &str,
    body: &str,
    token: Option<&str>,
) -> (String, String, String) {
    let auth = match token {
        Some(t) => format!("Authorization: Bearer {t}\r\n"),
        None => String::new(),
    };
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (status, resp_body) = http(addr, &req);
    (status, String::new(), resp_body)
}

fn get(addr: &SocketAddr, path: &str) -> (String, String) {
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    http(addr, &req)
}

/// The full remote agent loop (AC1/AC5): create → append → propose over
/// HTTP with a token; then the human approves via the CLI and the
/// human format updates. Every write produces a commit (AC3).
#[test]
fn remote_agent_loop_end_to_end() {
    let repo = Scratch::new("loop");
    repo.set_token("test-token-1");
    let server = start_server(&repo);
    let addr = &server.addr;

    // create via API with an AGENT actor (#139, #192): the actor comes from
    // the TOKEN label, not the body — issue an agent-labeled token; the body
    // `actor` field is ignored. Creation proposal: no issue.md, 201, one
    // commit, draft in the stream.
    let out = repo.tissue(&["token", "issue", "--label", "agent:dev-1"]);
    assert!(out.status.success(), "{out:?}");
    let agent_secret = String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("token: "))
        .expect("agent token")
        .trim()
        .to_string();
    let before = repo.commit_count();
    let (status, _, body) = post(
        addr,
        "/api/issues",
        r#"{"id":"IAM-9","title":"From the API","actor":"human:spoofed"}"#,
        Some(&agent_secret),
    );
    assert!(
        status.contains("201"),
        "create status: {status} body: {body}"
    );
    assert!(
        body.contains("proposed-needs-review"),
        "agent create is a proposal: {body}"
    );
    assert_eq!(repo.commit_count(), before + 1, "create = one commit");
    assert!(
        !repo.root.join(".tissue/issues/IAM-9/issue.md").exists(),
        "agent create does not write issue.md (decision 4)"
    );
    let stream = std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-9/agent.jsonl"))
        .expect("draft stream exists");
    assert!(
        stream.contains("\"field\":\"create\""),
        "create draft: {stream}"
    );
    assert!(
        !repo.root.join(".tissue/issues/IAM-9/issue.md").exists(),
        "still no issue.md before approval"
    );

    // list (read, tokenless) shows the draft as a needs-review item
    let (status, body) = get(addr, "/api/issues");
    assert!(status.contains("200"), "read status: {status}");
    assert!(body.contains("IAM-9"), "draft listed: {body}");
    assert!(body.contains("needs-review"), "flag visible: {body}");

    // the human approves the creation: issue.md materializes, tag clears
    let out = repo.tissue(&["approve", "IAM-9", "prop-01"]);
    assert!(out.status.success(), "approve create failed: {out:?}");
    let issue = std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-9/issue.md"))
        .expect("issue.md materialized on approval");
    assert!(
        issue.contains("From the API"),
        "draft title applied: {issue}"
    );
    assert!(!issue.contains("needs-review"), "tag cleared: {issue}");

    // append an agent record via API: 201, one commit, seq continues
    let (status, _, body) = post(
        addr,
        "/api/issues/IAM-9/records",
        r#"{"actor":"agent:dev-1","data":{"type":"observation","summary":"investigated","evidence":"logs"}}"#,
        Some(&agent_secret),
    );
    assert!(
        status.contains("201"),
        "append status: {status} body: {body}"
    );
    assert!(
        body.contains("\"seq\":3"),
        "seq continues past proposal+approval: {body}"
    );
    assert_eq!(repo.commit_count(), before + 3, "append = one commit");

    // propose a field edit via API: 201, one commit, needs-review tag set
    let (status, _, body) = post(
        addr,
        "/api/issues/IAM-9/propose",
        r#"{"field":"status","current":"open","proposed":"in-progress","rationale":"work started","actor":"agent:dev-1"}"#,
        Some(&agent_secret),
    );
    assert!(
        status.contains("201"),
        "propose status: {status} body: {body}"
    );
    assert!(body.contains("prop-02"), "proposal id: {body}");
    assert_eq!(repo.commit_count(), before + 4, "propose = one commit");

    // human approves via the CLI: human format updates, tag clears
    let out = repo.tissue(&["approve", "IAM-9", "prop-02"]);
    assert!(out.status.success(), "approve failed: {out:?}");
    let issue = std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-9/issue.md"))
        .expect("read issue.md");
    assert!(issue.contains("in-progress"), "status applied: {issue}");
    assert!(!issue.contains("needs-review"), "tag cleared: {issue}");

    // agent record survived as data
    let stream = std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-9/agent.jsonl"))
        .expect("stream exists");
    assert!(
        stream.contains("observation"),
        "agent record in stream: {stream}"
    );
}

/// Auth failures are explicit (AC4): missing token, wrong token, and
/// no token configured at all. Reads stay tokenless (AC2).
#[test]
fn write_api_auth_failures_are_explicit() {
    let repo = Scratch::new("auth");
    let server = start_server(&repo);
    let addr = &server.addr;

    // no api_token configured: write API disabled, explicit error
    let (status, _, body) = post(
        addr,
        "/api/issues",
        r#"{"id":"IAM-1","title":"x"}"#,
        Some("anything"),
    );
    assert!(
        status.contains("503"),
        "disabled status: {status} body: {body}"
    );
    assert!(body.contains("disabled"), "explicit error: {body}");

    // reads are open, loopback-only, tokenless
    let (status, body) = get(addr, "/api/issues");
    assert!(status.contains("200"), "read ok: {status}");
    assert!(body.contains("issues"), "read body: {body}");

    repo.set_token("secret-token");
    // wrong token: 401, token never echoed
    let (status, _, body) = post(
        addr,
        "/api/issues",
        r#"{"id":"IAM-1","title":"x"}"#,
        Some("wrong-token"),
    );
    assert!(status.contains("401"), "wrong token status: {status}");
    assert!(
        !body.contains("secret-token"),
        "token never in response: {body}"
    );

    // missing token: 401
    let (status, _, body) = post(addr, "/api/issues", r#"{"id":"IAM-1","title":"x"}"#, None);
    assert!(status.contains("401"), "missing token status: {status}");
    assert!(
        body.contains("missing Authorization"),
        "explicit error: {body}"
    );

    // no writes happened: zero commits beyond init
    let count = repo
        .git(&["rev-list", "--count", "HEAD"])
        .trim()
        .parse::<usize>()
        .expect("count");
    assert!(count <= 4, "no phantom writes: {count} commits");

    // GET on a POST-only write route is refused (read-only viewer).
    // records is now GET-readable (#98 timeline), so propose is the
    // POST-only representative.
    let (status, _) = get(addr, "/api/issues/IAM-1/propose");
    assert!(status.contains("405"), "GET on write route: {status}");
    // ... and GET on records is a valid read (empty stream, no token).
    let (status, _) = get(addr, "/api/issues/IAM-1/records");
    assert!(status.contains("200"), "GET records read: {status}");
}

/// The write API never writes the human format directly (decision 4):
/// the propose endpoint goes through the core loop, so a stale current
/// is refused and nothing is written.
#[test]
fn propose_via_api_fails_safe_on_stale_current() {
    let repo = Scratch::new("stale");
    repo.set_token("tok");
    let out = repo.tissue(&["create", "IAM-1", "First", "--body", "b"]);
    assert!(out.status.success(), "create failed: {out:?}");
    let server = start_server(&repo);

    let before = repo.commit_count();
    let (status, _, body) = post(
        &server.addr,
        "/api/issues/IAM-1/propose",
        r#"{"field":"title","current":"Wrong current","proposed":"X"}"#,
        Some("tok"),
    );
    assert!(
        status.contains("400"),
        "stale propose must fail: {status} body: {body}"
    );
    assert!(body.contains("stale"), "staleness named: {body}");
    assert_eq!(
        repo.commit_count(),
        before,
        "failed propose must not commit"
    );
    let issue = std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-1/issue.md")).unwrap();
    assert!(issue.contains("First"), "human format untouched: {issue}");
}

/// TISSUE_API_TOKEN env var takes precedence over (and works without)
/// config.yml (#80 token-storage fix): untracked, deployment-friendly.
#[test]
fn env_var_token_authenticates_without_config() {
    let repo = Scratch::new("env-token");
    // No api_token in config.yml at all.
    let server = start_server_with_env(&repo, &[("TISSUE_API_TOKEN", "env-secret")]);

    // Wrong token still 401; token never echoed.
    let (status, _, body) = post(
        &server.addr,
        "/api/issues",
        r#"{"id":"IAM-1","title":"x"}"#,
        Some("wrong"),
    );
    assert!(status.contains("401"), "wrong env token: {status}");
    assert!(
        !body.contains("env-secret"),
        "token never in response: {body}"
    );

    // Correct env token authenticates and commits.
    let before = repo.commit_count();
    let (status, _, body) = post(
        &server.addr,
        "/api/issues",
        r#"{"id":"IAM-1","title":"Env created"}"#,
        Some("env-secret"),
    );
    assert!(
        status.contains("201"),
        "env token create: {status} body: {body}"
    );
    assert_eq!(
        repo.commit_count(),
        before + 1,
        "env-token write = one commit"
    );

    // Reads stay tokenless.
    let (status, body) = get(&server.addr, "/api/issues");
    assert!(
        status.contains("200") && body.contains("IAM-1"),
        "read: {status}"
    );
}

#[test]
fn gui_sync_endpoint_matches_cli_commit() {
    let repo = Scratch::new("gui-sync");
    repo.set_token("test-token-1");
    repo.tissue(&["create", "IAM-1", "Synced issue", "--body", "b"]);
    // #136: a human edit before the sync, so there is a change to record.
    let issue_md = repo.root.join(".tissue/issues/IAM-1/issue.md");
    let text = std::fs::read_to_string(&issue_md).unwrap();
    std::fs::write(
        &issue_md,
        text.replace("title: Synced issue", "title: Synced and renamed"),
    )
    .unwrap();
    let server = start_server(&repo);
    let addr = server.addr;
    let before = repo.commit_count();
    let (status, _, body) = post(
        &addr,
        "/api/issues/IAM-1/sync",
        r#"{"summary": "rewrote the summary", "actor": "human:tuomas"}"#,
        Some("test-token-1"),
    );
    assert!(status.contains("201"), "sync: {status} body {body}");
    assert_eq!(repo.commit_count(), before + 1, "sync = exactly one commit");
    let log = repo.git(&["log", "-1", "--oneline"]);
    assert!(log.contains("sync IAM-1"), "commit message shape: {log}");
    let stream =
        std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-1/agent.jsonl")).unwrap();
    assert!(stream.contains("human-sync"), "record type: {stream}");
    assert!(stream.contains("rewrote the summary"), "summary: {stream}");
    // Auth still required: no token -> 401, wrong token -> 401.
    // #136: no changes since the last sync -> 200 synced:false, not 400.
    let (status, _, body) = post(
        &addr,
        "/api/issues/IAM-1/sync",
        r#"{"summary": "nothing moved"}"#,
        Some("test-token-1"),
    );
    assert!(
        status.contains("200") && body.contains("\"synced\":false"),
        "nothing to sync is a no-op result, not an error: {status} {body}"
    );
    let (status, _, _) = post(&addr, "/api/issues/IAM-1/sync", r#"{"summary":"x"}"#, None);
    assert!(status.contains("401"), "no token: {status}");
    let (status, _, _) = post(
        &addr,
        "/api/issues/IAM-1/sync",
        r#"{"summary":"x"}"#,
        Some("wrong"),
    );
    assert!(status.contains("401"), "wrong token: {status}");
}

/// #134: creating via POST /api/issues with a body lands the body in the
/// human format and commits like the CLI.
#[test]
fn create_via_api_with_body_commits() {
    let repo = Scratch::new("create-body");
    repo.set_token("test-token-1");
    let server = start_server(&repo);
    let addr = server.addr;
    let before = repo.commit_count();
    let (status, _, body) = post(
        &addr,
        "/api/issues",
        r###"{"id":"IAM-7","title":"From the GUI","body":"## Summary\n\n- works","actor":"human:gui"}"###,
        Some("test-token-1"),
    );
    assert!(status.contains("201"), "create: {status} body {body}");
    assert_eq!(repo.commit_count(), before + 1, "create = one commit");
    let issue = std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-7/issue.md")).unwrap();
    assert!(issue.contains("From the GUI"), "title written: {issue}");
    assert!(issue.contains("## Summary"), "body written: {issue}");
    // The new issue appears in the read the GUI uses to navigate.
    let (status, body) = get(&addr, "/api/issues");
    assert!(
        status.contains("200") && body.contains("IAM-7"),
        "listed: {body}"
    );
    // Duplicate id and bad id fail cleanly.
    let (status, _, body) = post(
        &addr,
        "/api/issues",
        r#"{"id":"IAM-7","title":"dup"}"#,
        Some("test-token-1"),
    );
    assert!(
        status.contains("400") && body.contains("already exists"),
        "dup: {status} {body}"
    );
    let (status, _, _) = post(
        &addr,
        "/api/issues",
        r#"{"id":"../etc","title":"bad"}"#,
        Some("test-token-1"),
    );
    assert!(status.contains("400"), "bad id refused: {status}");
}

/// #173: POST /api/releases creates a release through the same core path
/// as `tissue release create` — one commit, v+semver validation shared with
/// the CLI, duplicates refused — and the GUI's read endpoint lists it.
#[test]
fn create_release_via_api_shares_cli_validation() {
    let repo = Scratch::new("release-api");
    repo.set_token("test-token-1");
    let server = start_server(&repo);
    let addr = server.addr;

    // Auth first, same as every write endpoint: missing/wrong token -> 401.
    let (status, _, _) = post(
        &addr,
        "/api/releases",
        r#"{"id":"v1.0.0","title":"x"}"#,
        None,
    );
    assert!(status.contains("401"), "no token: {status}");
    let (status, _, _) = post(
        &addr,
        "/api/releases",
        r#"{"id":"v1.0.0","title":"x"}"#,
        Some("wrong-token"),
    );
    assert!(status.contains("401"), "wrong token: {status}");

    let before = repo.commit_count();
    let (status, _, body) = post(
        &addr,
        "/api/releases",
        r#"{"id":"v1.0.0","title":"First cut","actor":"human:gui"}"#,
        Some("test-token-1"),
    );
    assert!(status.contains("201"), "create: {status} body {body}");
    assert!(body.contains("v1.0.0"), "created id in response: {body}");
    assert_eq!(
        repo.commit_count(),
        before + 1,
        "release create = one commit"
    );
    let log = repo.git(&["log", "-1", "--pretty=%s"]);
    assert!(
        log.contains("release create v1.0.0"),
        "commit message: {log}"
    );
    let rel = std::fs::read_to_string(repo.root.join(".tissue/releases/v1.0.0/release.md"))
        .expect("release.md written");
    assert!(rel.contains("First cut"), "title written: {rel}");
    assert!(rel.contains("draft"), "starts as draft: {rel}");

    // The GUI's read endpoint lists it.
    let (status, body) = get(&addr, "/api/releases");
    assert!(
        status.contains("200") && body.contains("v1.0.0"),
        "listed: {status} {body}"
    );

    // Duplicate -> 409; malformed/traversal ids -> 400 with the shared
    // v+semver rule, and failed creates commit nothing.
    let (status, _, body) = post(
        &addr,
        "/api/releases",
        r#"{"id":"v1.0.0","title":"dup"}"#,
        Some("test-token-1"),
    );
    assert!(
        status.contains("409") && body.contains("already exists"),
        "dup: {status} {body}"
    );
    for bad in ["1.0.0", "v1.2", "../v9.9.9", "v1.2.3-rc1"] {
        let (status, _, body) = post(
            &addr,
            "/api/releases",
            &format!(r#"{{"id":"{bad}","title":"bad"}}"#),
            Some("test-token-1"),
        );
        assert!(
            status.contains("400") && body.contains("v+semver"),
            "bad id {bad}: {status} {body}"
        );
    }
    assert!(
        !repo.root.join(".tissue/releases/v9.9.9").exists(),
        "traversal id never reached the filesystem"
    );
    assert_eq!(
        repo.commit_count(),
        before + 1,
        "failed creates commit nothing"
    );
}

/// #118 regression: a CLI-synced issue must stay writable via the API.
/// `tissue sync` used to write the human-sync record without the
/// `reconciliation` field; strict `AgentStream::open` then rejected
/// every later append with 400 — permanently, for that issue.
#[test]
fn append_via_api_after_cli_sync() {
    let repo = Scratch::new("sync-then-append");
    repo.set_token("test-token-1");
    repo.tissue(&["create", "IAM-1", "Synced first", "--body", "b"]);
    // #136: sync only records when the human format actually changed —
    // make a real human edit first.
    let issue_md = repo.root.join(".tissue/issues/IAM-1/issue.md");
    let text = std::fs::read_to_string(&issue_md).unwrap();
    assert!(text.contains("title: Synced first"), "precondition: {text}");
    std::fs::write(
        &issue_md,
        text.replace("title: Synced first", "title: Synced and edited"),
    )
    .unwrap();
    let out = repo.tissue(&["sync", "IAM-1", "--summary", "human edited"]);
    assert!(out.status.success(), "cli sync failed: {out:?}");
    let server = start_server(&repo);
    let addr = server.addr;
    let before = repo.commit_count();
    let (status, _, body) = post(
        &addr,
        "/api/issues/IAM-1/records",
        r#"{"actor":"agent:dev-1","data":{"type":"observation","summary":"after a sync"}}"#,
        Some("test-token-1"),
    );
    assert!(
        status.contains("201"),
        "append after cli sync: {status} body: {body}"
    );
    assert!(
        body.contains("\"seq\":2"),
        "seq continues past the sync record: {body}"
    );
    assert_eq!(repo.commit_count(), before + 1, "append = one commit");
    let stream =
        std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-1/agent.jsonl")).unwrap();
    assert!(
        stream.contains("\"reconciliation\""),
        "sync record now carries the schema-required field: {stream}"
    );
}

#[test]
fn gui_edit_endpoint_and_conflict_handling() {
    let repo = Scratch::new("gui-edit");
    repo.set_token("test-token-1");
    repo.tissue(&["create", "IAM-1", "Editable", "--body", "b"]);
    let server = start_server(&repo);
    let addr = server.addr;
    // Successful edit: revision 1 (fresh create) -> title + status change.
    let (status, _, body) = post(
        &addr,
        "/api/issues/IAM-1/edit",
        r#"{"expected_revision": 1, "title": "Edited title", "status": "in-progress", "actor": "human:tuomas"}"#,
        Some("test-token-1"),
    );
    assert!(status.contains("200"), "edit: {status} body {body}");
    let issue = std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-1/issue.md")).unwrap();
    assert!(issue.contains("Edited title"), "title applied: {issue}");
    assert!(issue.contains("in-progress"), "status applied: {issue}");
    assert!(
        issue.contains("revision: 2"),
        "revision bumped (write guard): {issue}"
    );
    // Conflict (AC2): a second edit claiming the stale revision 1 must 409
    // with the re-apply message, not silently overwrite.
    let (status, _, body) = post(
        &addr,
        "/api/issues/IAM-1/edit",
        r#"{"expected_revision": 1, "title": "Clobber attempt"}"#,
        Some("test-token-1"),
    );
    assert!(
        status.contains("409"),
        "conflict must 409: {status} body {body}"
    );
    assert!(
        body.contains("changed while you were editing"),
        "conflict message: {body}"
    );
    assert!(body.contains("re-apply"), "recovery guidance: {body}");
    let issue = std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-1/issue.md")).unwrap();
    assert!(issue.contains("Edited title"), "no clobber: {issue}");
    assert!(!issue.contains("Clobber"), "no clobber (title): {issue}");
    // Invalid status refused with a clear 400.
    let (status, _, body) = post(
        &addr,
        "/api/issues/IAM-1/edit",
        r#"{"expected_revision": 2, "status": "fixed"}"#,
        Some("test-token-1"),
    );
    assert!(status.contains("400"), "bad status: {status} body {body}");
    assert!(
        body.contains("open, in-progress, review, done, rejected"),
        "enum hint: {body}"
    );
}

/// #159 AC1/AC2: managed hash-only tokens — `tissue token issue` stores
/// only the SHA-256 in config.yml (never the plaintext), the issued token
/// authorizes writes, `revoke` makes it fail closed (401), and
/// `token list` shows id/label/created without the secret.
#[test]
fn managed_tokens_issue_use_revoke() {
    let repo = Scratch::new("managed-tokens");
    // No legacy api_token line, no env token.
    let server = start_server(&repo);
    let addr = &server.addr;

    // Write API disabled with no token configured at all.
    let (status, _h, body) = post(
        addr,
        "/api/projects",
        "{\"name\":\"x\",\"prefix\":\"X\"}",
        Some("anything"),
    );
    assert!(status.contains("503"), "{status} {body}");

    // Issue a token via the CLI; the secret appears in stdout exactly once.
    let out = repo.tissue(&["token", "issue", "--label", "human:gui"]);
    assert!(out.status.success(), "token issue failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let secret = stdout
        .lines()
        .find_map(|l| l.strip_prefix("token: "))
        .expect("secret printed once")
        .trim()
        .to_string();
    assert_eq!(secret.len(), 43);

    // config.yml holds only the hash, never the plaintext secret.
    let config = std::fs::read_to_string(repo.root.join(".tissue/config.yml")).unwrap();
    assert!(
        !config.contains(&secret),
        "plaintext leaked into config.yml: {config}"
    );
    assert!(config.contains("sha256:"));
    assert!(config.contains("human:gui"));

    // The issued token now authorizes a write (201), and one commit landed.
    let before = repo.commit_count();
    let (status, _h, body) = post(
        addr,
        "/api/projects",
        "{\"name\":\"ops\",\"prefix\":\"OPS\"}",
        Some(&secret),
    );
    assert!(status.contains("201"), "{status} {body}");
    assert_eq!(repo.commit_count(), before + 1);

    // token list shows the entry, never the secret.
    let out = repo.tissue(&["token", "list"]);
    assert!(out.status.success());
    let list = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        list.contains("tok-01") && list.contains("human:gui"),
        "{list}"
    );
    assert!(!list.contains(&secret), "list leaks secret: {list}");

    // Revoke: the old token now 401s (fails closed).
    let out = repo.tissue(&["token", "revoke", "tok-01"]);
    assert!(out.status.success(), "revoke failed: {out:?}");
    // With the only token revoked and no env override, writes are disabled
    // (503) — and even if another token existed, the revoked one must 401.
    let (status, _h, body) = post(
        addr,
        "/api/projects",
        "{\"name\":\"y\",\"prefix\":\"Y\"}",
        Some(&secret),
    );
    assert!(
        status.contains("401") || status.contains("503"),
        "revoked token must fail closed: {status} {body}"
    );
    // Issue a second token: the revoked secret still must not work.
    let out = repo.tissue(&["token", "issue", "--label", "agent:dev-1"]);
    assert!(out.status.success());
    let (status, _h, body) = post(
        addr,
        "/api/projects",
        "{\"name\":\"y\",\"prefix\":\"Y\"}",
        Some(&secret),
    );
    assert!(
        status.contains("401"),
        "revoked token must 401 while another is valid: {status} {body}"
    );
    // And the store no longer contains the revoked hash (the second token's
    // own hash is present; compare against the revoked one's digest).
    let config = std::fs::read_to_string(repo.root.join(".tissue/config.yml")).unwrap();
    let revoked_hash = sha256_of(&secret);
    assert!(
        !config.contains(&revoked_hash),
        "revoked hash still in config.yml: {config}"
    );
}

/// SHA-256 hex of a string, via the system tool (tests may shell out).
fn sha256_of(s: &str) -> String {
    let out = std::process::Command::new("sha256sum")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.take().unwrap().write_all(s.as_bytes())?;
            c.wait_with_output()
        })
        .expect("sha256sum");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string()
}

/// #159 AC2: the TISSUE_API_TOKEN env override still works for bootstrap
/// and takes precedence over managed tokens.
#[test]
fn env_token_override_still_works() {
    let repo = Scratch::new("env-token");
    let server = start_server_with_env(&repo, &[("TISSUE_API_TOKEN", "env-secret-123")]);
    let addr = &server.addr;
    let (status, _h, _b) = post(
        addr,
        "/api/projects",
        "{\"name\":\"ops\",\"prefix\":\"OPS\"}",
        Some("env-secret-123"),
    );
    assert!(status.contains("201"), "{status}");
    let (status, _h, _b) = post(addr, "/api/projects", "{}", Some("wrong"));
    assert!(status.contains("401"), "{status}");
}
/// #172/#176: project assignment at creation — CLI --project, API project
/// field, shared validation (mismatch refused, never a hardcoded IAM).
#[test]
fn create_issue_project_validation_shared() {
    let repo = Scratch::new("create-project-172");
    repo.set_token("tok");
    // A second project exists.
    let out = repo.tissue(&["project", "add", "ops", "OPS"]);
    assert!(out.status.success(), "{out:?}");

    // CLI: --project ops requires the OPS- prefix.
    let out = repo.tissue(&[
        "create",
        "IAM-9",
        "wrong prefix",
        "--project",
        "ops",
        "--body",
        "b",
    ]);
    assert!(!out.status.success(), "mismatch must fail: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("OPS-"), "clear mismatch error: {stderr}");
    let out = repo.tissue(&[
        "create",
        "OPS-1",
        "right prefix",
        "--project",
        "ops",
        "--body",
        "b",
    ]);
    assert!(out.status.success(), "match ok: {out:?}");
    // CLI: default project (no --project) uses the configured default (IAM).
    let out = repo.tissue(&["create", "IAM-1", "default project", "--body", "b"]);
    assert!(out.status.success(), "default project: {out:?}");
    let out = repo.tissue(&[
        "create",
        "OPS-2",
        "no project flag, wrong prefix",
        "--body",
        "b",
    ]);
    assert!(!out.status.success(), "no silent prefix: {out:?}");

    // API: explicit project field; mismatch -> 400 with the prefix named.
    let server = start_server(&repo);
    let addr = &server.addr;
    let (status, _h, body) = post(
        addr,
        "/api/issues",
        r#"{"id":"IAM-9","title":"api mismatch","actor":"human:gui","project":"ops"}"#,
        Some("tok"),
    );
    assert!(
        status.contains("400"),
        "api mismatch refused: {status} {body}"
    );
    assert!(
        body.contains("OPS-"),
        "error names the right prefix: {body}"
    );
    // API: no project field -> default project prefix required.
    let (status, _h, body) = post(
        addr,
        "/api/issues",
        r#"{"id":"OPS-3","title":"api default","actor":"human:gui"}"#,
        Some("tok"),
    );
    assert!(
        status.contains("400"),
        "no hardcoded fallback: {status} {body}"
    );
    // API: explicit correct project works.
    let (status, _h, body) = post(
        addr,
        "/api/issues",
        r#"{"id":"OPS-4","title":"api ok","actor":"human:gui","project":"ops"}"#,
        Some("tok"),
    );
    assert!(
        status.contains("201"),
        "api create with project: {status} {body}"
    );
}

// ---------------------------------------------------------------------------
// #174: the token value is visible only where intended — the Settings page
// (GET/POST /api/tokens, authed) and once at CLI issue time. List/get/logs
// carry no secret material; legacy plaintext api_token: migrates to hash-only.
// ---------------------------------------------------------------------------

/// #174 AC1/AC2: the token API is authed, masked, and one-commit-per-mutation;
/// the issued secret works for writes and disappears from every read.
#[test]
fn tokens_api_issue_list_revoke_masked_and_authed() {
    let repo = Scratch::new("tokens-api");
    repo.set_token("legacy-secret-1"); // legacy plaintext api_token: line
    let server = start_server(&repo);
    let addr = server.addr;

    // The token API itself requires a bearer token — even the masked list.
    let (status, body) = get(&addr, "/api/tokens");
    assert!(
        status.contains("401"),
        "list without token: {status} {body}"
    );
    let req = "GET /api/tokens HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer wrong\r\nConnection: close\r\n\r\n";
    let (status, body) = http(&addr, req);
    assert!(status.contains("401"), "list with wrong token: {status}");
    assert!(!body.contains("legacy-secret-1"), "no echo: {body}");

    // List with the legacy token: masked, flags the legacy line, no hashes.
    let req = "GET /api/tokens HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer legacy-secret-1\r\nConnection: close\r\n\r\n";
    let (status, body) = http(&addr, req);
    assert!(status.contains("200"), "list: {status} {body}");
    assert!(body.contains("\"legacy\":true"), "legacy flag: {body}");
    assert!(body.contains("\"tokens\":[]"), "empty list: {body}");
    assert!(!body.contains("legacy-secret-1"), "no plaintext: {body}");
    assert!(!body.contains("sha256"), "no hashes either: {body}");

    // Issue: the secret appears exactly once, in the issue response.
    let before = repo.commit_count();
    let (status, _, body) = post(
        &addr,
        "/api/tokens",
        r#"{"label":"gui-test","actor":"human:gui"}"#,
        Some("legacy-secret-1"),
    );
    assert!(status.contains("201"), "issue: {status} {body}");
    assert!(
        body.contains("\"secret\":"),
        "secret in issue response: {body}"
    );
    let secret = body
        .split("\"secret\":\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("secret present")
        .to_string();
    assert!(!secret.is_empty());
    assert_eq!(repo.commit_count(), before + 1, "issue = one commit");
    let cfg = std::fs::read_to_string(repo.root.join(".tissue/config.yml")).unwrap();
    assert!(
        !cfg.contains(&secret),
        "secret never stored plaintext: {cfg}"
    );
    assert!(cfg.contains("sha256:"), "hash-only storage: {cfg}");

    // The new token works for writes.
    let (status, _, body) = post(
        &addr,
        "/api/issues",
        r#"{"id":"IAM-5","title":"via new token","actor":"human:gui"}"#,
        Some(&secret),
    );
    assert!(
        status.contains("201"),
        "write with issued token: {status} {body}"
    );

    // List now shows the entry, still masked.
    let req = "GET /api/tokens HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer legacy-secret-1\r\nConnection: close\r\n\r\n".to_string();
    let (status, body) = http(&addr, &req);
    assert!(
        status.contains("200") && body.contains("gui-test"),
        "listed: {body}"
    );
    assert!(!body.contains(&secret), "list leaks no secret: {body}");
    assert!(!body.contains("sha256"), "list leaks no hashes: {body}");

    // GET /api/config never carries token material of any kind.
    let (status, body) = get(&addr, "/api/config");
    assert!(status.contains("200"));
    assert!(!body.contains("api_token"), "config view: {body}");
    assert!(
        !body.contains("\"tokens\""),
        "tokens block not in config view: {body}"
    );
    assert!(
        !body.contains(&secret) && !body.contains("legacy-secret-1"),
        "{body}"
    );

    // Duplicate label -> 409; unknown revoke -> 404.
    let (status, _, _) = post(
        &addr,
        "/api/tokens",
        r#"{"label":"gui-test"}"#,
        Some("legacy-secret-1"),
    );
    assert!(status.contains("409"), "dup label: {status}");
    let (status, _, _) = post(
        &addr,
        "/api/tokens/tok-99/revoke",
        r#"{"actor":"human:gui"}"#,
        Some("legacy-secret-1"),
    );
    assert!(status.contains("404"), "unknown id: {status}");

    // Revoke fails closed: one commit, the secret stops working.
    let before = repo.commit_count();
    let (status, _, body) = post(
        &addr,
        "/api/tokens/tok-01/revoke",
        r#"{"actor":"human:gui"}"#,
        Some("legacy-secret-1"),
    );
    assert!(status.contains("200"), "revoke: {status} {body}");
    assert_eq!(repo.commit_count(), before + 1, "revoke = one commit");
    let (status, _, _) = post(
        &addr,
        "/api/issues",
        r#"{"id":"IAM-6","title":"x","actor":"human:gui"}"#,
        Some(&secret),
    );
    assert!(
        status.contains("401"),
        "revoked token fails closed: {status}"
    );
}

/// #174 migration: a legacy plaintext `api_token:` line is migrated to a
/// hash-only `tokens:` entry (one commit) on `token list` / `token issue`,
/// and the old secret keeps working afterwards.
#[test]
fn legacy_api_token_migrates_to_hash_only_on_cli() {
    let repo = Scratch::new("legacy-migrate");
    repo.set_token("legacy-secret-2");
    let before = repo.commit_count();
    let out = repo.tissue(&["token", "list"]);
    assert!(out.status.success(), "token list failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("migrated"), "migration announced: {stdout}");
    assert_eq!(repo.commit_count(), before + 1, "migration = one commit");
    let cfg = std::fs::read_to_string(repo.root.join(".tissue/config.yml")).unwrap();
    assert!(!cfg.contains("legacy-secret-2"), "plaintext gone: {cfg}");
    assert!(
        !cfg.lines()
            .any(|l| l.trim_start().starts_with("api_token:")),
        "line removed: {cfg}"
    );
    assert!(cfg.contains("sha256:"), "hash entry stored: {cfg}");
    // A second list does not migrate again.
    let before = repo.commit_count();
    let out = repo.tissue(&["token", "list"]);
    assert!(out.status.success());
    assert_eq!(repo.commit_count(), before, "idempotent");
    // The old secret still authenticates serve (now via the hash entry).
    let server = start_server(&repo);
    let (status, _, body) = post(
        &server.addr,
        "/api/issues",
        r#"{"id":"IAM-2","title":"post-migration","actor":"human:gui"}"#,
        Some("legacy-secret-2"),
    );
    assert!(
        status.contains("201"),
        "legacy secret still works: {status} {body}"
    );

    // `token issue` also migrates a legacy line.
    let repo2 = Scratch::new("legacy-migrate-issue");
    repo2.set_token("legacy-secret-3");
    let out = repo2.tissue(&["token", "issue", "--label", "human:test"]);
    assert!(out.status.success(), "{out:?}");
    let cfg2 = std::fs::read_to_string(repo2.root.join(".tissue/config.yml")).unwrap();
    assert!(!cfg2.contains("legacy-secret-3"), "{cfg2}");
    assert!(cfg2.contains("sha256:"), "{cfg2}");
}

/// #174 AC2: serve's own logs never contain the token value.
#[test]
fn serve_logs_never_contain_the_token() {
    let repo = Scratch::new("log-audit");
    let mut server =
        start_server_with_env(&repo, &[("TISSUE_API_TOKEN", "super-secret-env-token")]);
    let addr = server.addr;
    // A read and a failed write (wrong token), then a real write.
    let _ = get(&addr, "/api/issues");
    let _ = post(
        &addr,
        "/api/issues",
        r#"{"id":"IAM-1","title":"x"}"#,
        Some("wrong"),
    );
    let (status, _, _) = post(
        &addr,
        "/api/issues",
        r#"{"id":"IAM-1","title":"ok","actor":"human:gui"}"#,
        Some("super-secret-env-token"),
    );
    assert!(status.contains("201"), "{status}");
    let _ = server.child.kill();
    let _ = server.child.wait();
    let mut logs = String::new();
    if let Some(mut so) = server.child.stdout.take() {
        use std::io::Read as _;
        let _ = so.read_to_string(&mut logs);
    }
    if let Some(mut se) = server.child.stderr.take() {
        use std::io::Read as _;
        let _ = se.read_to_string(&mut logs);
    }
    assert!(
        !logs.contains("super-secret-env-token"),
        "token in serve logs: {logs}"
    );
}

/// #192 regression: actor identity is bound to the token label.
/// (a) agent token + spoofed `actor:"human:*"` in body → still agent
///     (create = proposal, no issue.md); (b) agent token approve → 403;
/// (c) human token + `actor:"agent:*"` in body → still human (direct).
#[test]
fn actor_identity_bound_to_token_label() {
    let repo = Scratch::new("actor-binding-192");
    repo.set_token("human-token");
    let server = start_server(&repo);
    let addr = &server.addr;

    let out = repo.tissue(&["token", "issue", "--label", "agent:dev-1"]);
    assert!(out.status.success(), "{out:?}");
    let agent_secret = String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("token: "))
        .expect("agent token")
        .trim()
        .to_string();

    // (a) agent token + spoofed human actor in the body → still a proposal.
    let (status, _, body) = post(
        addr,
        "/api/issues",
        r#"{"id":"IAM-1","title":"spoof attempt","actor":"human:root"}"#,
        Some(&agent_secret),
    );
    assert!(status.contains("201"), "{status} {body}");
    assert!(
        body.contains("proposed-needs-review"),
        "agent token stays agent despite body actor: {body}"
    );
    assert!(
        !repo.root.join(".tissue/issues/IAM-1/issue.md").exists(),
        "no direct human-format write"
    );

    // (b) agent token cannot approve → 403 from the core actor-kind check.
    let (status, _, body) = post(
        addr,
        "/api/issues/IAM-1/approve",
        r#"{"proposal_id":"prop-01"}"#,
        Some(&agent_secret),
    );
    assert!(
        status.contains("403"),
        "agent approve must 403: {status} {body}"
    );
    assert!(body.contains("human"), "error explains the rule: {body}");

    // (c) human token + agent actor in the body → still human (direct create).
    let (status, _, body) = post(
        addr,
        "/api/issues",
        r#"{"id":"IAM-2","title":"human create","actor":"agent:fake"}"#,
        Some("human-token"),
    );
    assert!(status.contains("201"), "{status} {body}");
    assert!(
        body.contains("\"direct\""),
        "human token stays human: {body}"
    );
    assert!(repo.root.join(".tissue/issues/IAM-2/issue.md").exists());

    // The human token CAN approve the agent's proposal.
    let (status, _, body) = post(
        addr,
        "/api/issues/IAM-1/approve",
        r#"{"proposal_id":"prop-01"}"#,
        Some("human-token"),
    );
    assert!(
        status.contains("201"),
        "human approve works: {status} {body}"
    );
    assert!(repo.root.join(".tissue/issues/IAM-1/issue.md").exists());
}

/// #194: serve is thread-per-connection — a slow client (partial request,
/// socket held open) must not block other requests, and a storm of
/// concurrent writes must produce exactly one commit each with no torn
/// state (the per-repo mutation lock serializes add+commit).
#[test]
fn serve_concurrent_writes_and_slow_client() {
    use std::io::{Read as _, Write as _};
    let repo = Scratch::new("concurrency-194");
    repo.set_token("tok");
    // Pre-create issues to edit concurrently.
    for n in 1..=8 {
        let out = repo.tissue(&["create", &format!("IAM-{n}"), "seed", "--body", "b"]);
        assert!(out.status.success(), "{out:?}");
    }
    let server = start_server(&repo);
    let addr = server.addr;

    // Slow client: open a socket, send a partial request line, hold it open.
    let mut slow = std::net::TcpStream::connect(addr).unwrap();
    slow.write_all(b"POST /api/issues HTTP/1.1\r\n").unwrap();
    slow.flush().unwrap();

    // While the slow client holds its connection, other requests must work.
    let (status, body) = get(&addr, "/api/issues");
    assert!(
        status.contains("200"),
        "reads work during slow client: {status} {body}"
    );

    // Fire 8 concurrent edits (one per issue) from threads.
    let mut threads = Vec::new();
    for n in 1..=8u32 {
        let a = addr;
        threads.push(std::thread::spawn(move || {
            let body = format!(
                r#"{{"expected_revision":1,"title":"edited-{n}"}}"#
            );
            let req = format!(
                "POST /api/issues/IAM-{n}/edit HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer tok\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let mut s = std::net::TcpStream::connect(a).unwrap();
            s.write_all(req.as_bytes()).unwrap();
            let mut resp = String::new();
            s.read_to_string(&mut resp).unwrap();
            resp
        }));
    }
    let mut ok = 0;
    let mut conflict = 0;
    for t in threads {
        let resp = t.join().unwrap();
        if resp.contains(" 200 ") || resp.contains(" 201 ") {
            ok += 1;
        } else if resp.contains(" 409 ") {
            conflict += 1;
        } else {
            panic!("unexpected response: {}", resp);
        }
    }
    assert_eq!(
        ok + conflict,
        8,
        "all 8 edits answered: ok={ok} conflict={conflict}"
    );
    // No torn commits: git log stays linear and every issue dir is intact.
    let log = repo.git(&["log", "--oneline"]);
    assert!(!log.is_empty());
    for n in 1..=8 {
        assert!(
            repo.root
                .join(format!(".tissue/issues/IAM-{n}/issue.md"))
                .exists(),
            "IAM-{n} intact"
        );
    }
    let _ = slow.shutdown(std::net::Shutdown::Both);
}

/// #196: API error bodies never contain absolute filesystem paths — the
/// detail goes to server stderr, the response carries a generic message.
#[test]
fn api_error_bodies_contain_no_absolute_paths() {
    let repo = Scratch::new("error-sanitize-196");
    repo.set_token("tok");
    let server = start_server(&repo);
    let addr = &server.addr;
    // 404 on a missing issue detail — the core error embeds the issue path.
    let (status, body) = get(addr, "/api/issues/NOPE-1");
    assert!(status.contains("404"), "{status}");
    assert!(!body.contains("/tmp/"), "leaked fs path: {body}");
    assert!(
        !body.contains(&repo.root.display().to_string()),
        "leaked repo path: {body}"
    );
    // 400 on a malformed write body.
    let (status, _, body) = post(addr, "/api/issues", "{not json", Some("tok"));
    assert!(status.contains("400"), "{status}");
    assert!(!body.contains("/tmp/"), "leaked fs path: {body}");
    // 409 conflict on duplicate create — error text must be path-free too.
    let out = repo.tissue(&["create", "IAM-1", "exists", "--body", "b"]);
    assert!(out.status.success());
    let (status, _, body) = post(
        addr,
        "/api/issues",
        r#"{"id":"IAM-1","title":"dup"}"#,
        Some("tok"),
    );
    assert!(status.contains("400") || status.contains("409"), "{status}");
    assert!(!body.contains("/tmp/"), "leaked fs path: {body}");
}

/// #211 (decision 15): types + parent + workflow flow through the API —
/// create with type/parent, summary/detail JSON carry them, the edit
/// endpoint enforces workflow transitions and parent validation, and
/// agents can propose type/parent/status like any field.
#[test]
fn issue_types_and_workflow_via_api() {
    let repo = Scratch::new("types-211");
    repo.set_token("test-token-1");
    let server = start_server(&repo);
    let addr = server.addr;

    // Create a feature + a child bug through the API.
    let (status, _, body) = post(
        &addr,
        "/api/issues",
        r#"{"id":"IAM-1","title":"Login flow","type":"feature","actor":"human:gui"}"#,
        Some("test-token-1"),
    );
    assert!(status.contains("201"), "create feature: {status} {body}");
    let (status, _, body) = post(
        &addr,
        "/api/issues",
        r#"{"id":"IAM-2","title":"Redirect loop","type":"bug","parent":"IAM-1","actor":"human:gui"}"#,
        Some("test-token-1"),
    );
    assert!(status.contains("201"), "create child bug: {status} {body}");
    let issue = std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-2/issue.md")).unwrap();
    assert!(
        issue.contains("type: bug") && issue.contains("parent: IAM-1"),
        "{issue}"
    );

    // Summary + detail JSON carry type/parent.
    let (status, body) = get(&addr, "/api/issues");
    assert!(status.contains("200"), "{status}");
    assert!(
        body.contains("\"type\":\"feature\"") && body.contains("\"parent\":\"IAM-1\""),
        "summaries carry type/parent: {body}"
    );
    let (status, body) = get(&addr, "/api/issues/IAM-2");
    assert!(
        status.contains("200")
            && body.contains("\"type\":\"bug\"")
            && body.contains("\"parent\":\"IAM-1\""),
        "{body}"
    );

    // Parent validation through the API: missing / non-feature parent -> 400.
    let (status, _, body) = post(
        &addr,
        "/api/issues",
        r#"{"id":"IAM-3","title":"x","type":"bug","parent":"IAM-99","actor":"human:gui"}"#,
        Some("test-token-1"),
    );
    assert!(
        status.contains("400") && body.contains("does not exist"),
        "{status} {body}"
    );
    let (status, _, body) = post(
        &addr,
        "/api/issues",
        r#"{"id":"IAM-3","title":"x","type":"feature","parent":"IAM-1","actor":"human:gui"}"#,
        Some("test-token-1"),
    );
    assert!(
        status.contains("400") && body.contains("feature cannot have a parent"),
        "{status} {body}"
    );

    // Edit endpoint: read revision, then a legal transition open->in-progress.
    let (_, body) = get(&addr, "/api/issues/IAM-1");
    let rev: u64 = {
        let s = body.split("\"revision\":").nth(1).expect("revision");
        s.split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let (status, _, body) = post(
        &addr,
        "/api/issues/IAM-1/edit",
        r#"{"expected_revision":1,"status":"in-progress","actor":"human:gui"}"#,
        Some("test-token-1"),
    );
    assert!(status.contains("200"), "legal transition: {status} {body}");
    assert!(rev >= 1);
    // Illegal transition in-progress->done (must go via review) -> 400.
    let (_, body) = get(&addr, "/api/issues/IAM-1");
    let rev2: u64 = {
        let s = body.split("\"revision\":").nth(1).expect("revision");
        s.split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let (status, _, body) = post(
        &addr,
        "/api/issues/IAM-1/edit",
        &format!(r#"{{"expected_revision":{rev2},"status":"done","actor":"human:gui"}}"#),
        Some("test-token-1"),
    );
    assert!(
        status.contains("400") && body.contains("illegal status transition"),
        "{status} {body}"
    );
    // Legal: in-progress -> review -> done (closed alias accepted for done).
    let (_, body) = get(&addr, "/api/issues/IAM-1");
    let rev3: u64 = {
        let s = body.split("\"revision\":").nth(1).expect("revision");
        s.split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let (status, _, body) = post(
        &addr,
        "/api/issues/IAM-1/edit",
        &format!(r#"{{"expected_revision":{rev3},"status":"review","actor":"human:gui"}}"#),
        Some("test-token-1"),
    );
    assert!(status.contains("200"), "-> review: {status} {body}");
    let (_, body) = get(&addr, "/api/issues/IAM-1");
    let rev4: u64 = {
        let s = body.split("\"revision\":").nth(1).expect("revision");
        s.split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let (status, _, body) = post(
        &addr,
        "/api/issues/IAM-1/edit",
        &format!(r#"{{"expected_revision":{rev4},"status":"closed","actor":"human:gui"}}"#),
        Some("test-token-1"),
    );
    assert!(
        status.contains("200"),
        "closed alias -> done: {status} {body}"
    );
    let issue = std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-1/issue.md")).unwrap();
    assert!(
        issue.contains("status: done") && !issue.contains("status: closed"),
        "{issue}"
    );

    // Edit type/parent through the API (task IAM-2 -> stays bug, clear parent).
    let (status, _, body) = post(
        &addr,
        "/api/issues/IAM-2/edit",
        r#"{"expected_revision":1,"parent":"","actor":"human:gui"}"#,
        Some("test-token-1"),
    );
    assert!(status.contains("200"), "clear parent: {status} {body}");
    let issue = std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-2/issue.md")).unwrap();
    assert!(!issue.contains("parent:"), "parent cleared: {issue}");

    // Agent proposes type via the API; human approves via CLI; applied.
    let (status, _, body) = post(
        &addr,
        "/api/issues/IAM-2/propose",
        r#"{"field":"type","current":"bug","proposed":"task","rationale":"triage","actor":"agent:dev-1"}"#,
        Some("test-token-1"),
    );
    assert!(status.contains("201"), "propose type: {status} {body}");
    let out = repo.tissue(&["approve", "IAM-2", "prop-01", "--actor", "human:gui"]);
    assert!(out.status.success(), "approve: {out:?}");
    let issue = std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-2/issue.md")).unwrap();
    assert!(
        issue.contains("type: task"),
        "type applied on approve: {issue}"
    );
}
