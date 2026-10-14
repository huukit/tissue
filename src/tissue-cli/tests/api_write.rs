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

    // create via API: 201, one commit
    let before = repo.commit_count();
    let (status, _, body) = post(
        addr,
        "/api/issues",
        r#"{"id":"IAM-9","title":"From the API","actor":"agent:dev-1"}"#,
        Some("test-token-1"),
    );
    assert!(
        status.contains("201"),
        "create status: {status} body: {body}"
    );
    assert_eq!(repo.commit_count(), before + 1, "create = one commit");

    // append an agent record via API: 201, one commit, seq reported
    let (status, _, body) = post(
        addr,
        "/api/issues/IAM-9/records",
        r#"{"actor":"agent:dev-1","data":{"type":"observation","summary":"investigated","evidence":"logs"}}"#,
        Some("test-token-1"),
    );
    assert!(
        status.contains("201"),
        "append status: {status} body: {body}"
    );
    assert!(body.contains("\"seq\":1"), "seq reported: {body}");
    assert_eq!(repo.commit_count(), before + 2, "append = one commit");

    // propose via API: 201, one commit, needs-review tag set
    let (status, _, body) = post(
        addr,
        "/api/issues/IAM-9/propose",
        r#"{"field":"status","current":"open","proposed":"in-progress","rationale":"work started","actor":"agent:dev-1"}"#,
        Some("test-token-1"),
    );
    assert!(
        status.contains("201"),
        "propose status: {status} body: {body}"
    );
    assert!(body.contains("prop-01"), "proposal id: {body}");
    assert_eq!(repo.commit_count(), before + 3, "propose = one commit");

    // list (read, tokenless) shows the needs-review flag
    let (status, body) = get(addr, "/api/issues");
    assert!(status.contains("200"), "read status: {status}");
    assert!(body.contains("IAM-9"), "issue listed: {body}");
    assert!(body.contains("needs-review"), "flag visible: {body}");

    // human approves via the CLI: human format updates, tag clears
    let out = repo.tissue(&["approve", "IAM-9", "prop-01"]);
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
        r#"{"id":"IAM-1","title":"Env created","actor":"agent:dev-1"}"#,
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
        body.contains("open, in-progress, closed"),
        "enum hint: {body}"
    );
}
