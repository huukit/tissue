//! End-to-end tests for the GUI review surface (#98): the Needs Review
//! data path and the approve/reject loop over the token-authenticated
//! API — exactly what the browser does, minus the browser. Pins:
//! proposals and records are readable without a token (loopback read),
//! approve/reject require one, decisions commit exactly like the CLI,
//! and rejections without a reason are refused (rejections are data).

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
            "tissue-gui-review-{}-{}-{}",
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
        s.set_token("gui-test-token");
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

    fn set_token(&self, token: &str) {
        let config = self.root.join(".tissue/config.yml");
        let content = std::fs::read_to_string(&config).expect("config.yml exists after init");
        std::fs::write(&config, format!("{content}api_token: \"{token}\"\n"))
            .expect("write config");
        self.git(&["add", "-A"]);
        self.git(&["commit", "-m", "test: set api token"]);
    }

    fn read_issue(&self, id: &str) -> String {
        let p = self.root.join(".tissue/issues").join(id).join("issue.md");
        std::fs::read_to_string(p).expect("read issue.md")
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
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("reserve port");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    let exe: PathBuf = env!("CARGO_BIN_EXE_tissue").into();
    let mut cmd = Command::new(exe);
    cmd.args(["serve", "--port", &port.to_string()])
        .current_dir(&repo.root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
    panic!("http failed after retries: {last_err:?}");
}

fn http_once(addr: &SocketAddr, request: &str) -> Result<(String, String), String> {
    let mut stream = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|e| e.to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string();
    let mut content_length = 0usize;
    loop {
        let mut h = String::new();
        let n = reader.read_line(&mut h).map_err(|e| e.to_string())?;
        if n == 0 || h.trim().is_empty() {
            break;
        }
        if let Some(v) = h
            .to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse().ok())
        {
            content_length = v;
        }
    }
    let mut body = String::new();
    reader
        .take(content_length as u64)
        .read_to_string(&mut body)
        .map_err(|e| e.to_string())?;
    Ok((status, body))
}

fn post(addr: &SocketAddr, path: &str, body: &str, token: Option<&str>) -> (String, String) {
    let auth = match token {
        Some(t) => format!("Authorization: Bearer {t}\r\n"),
        None => String::new(),
    };
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    http(addr, &req)
}

fn get(addr: &SocketAddr, path: &str) -> (String, String) {
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    http(addr, &req)
}

/// #135: GET /api/projects lists the mapping tokenless; POST /api/projects
/// creates one with a commit through the same core module as the CLI —
/// CLI/API parity for the same inputs, duplicate/invalid refused cleanly.
#[test]
fn projects_api_create_list_and_parity() {
    let repo = Scratch::new("projects-api");
    let server = start_server(&repo);
    let addr = server.addr;

    // Read: tokenless, shows the init project as default.
    let (status, body) = get(&addr, "/api/projects");
    assert!(status.contains("200"), "projects read: {status} {body}");
    assert!(body.contains("\"default\":true"), "default marked: {body}");
    assert!(body.contains("IAM"), "prefix present: {body}");

    // Create via API: one commit, mapping lands.
    let before = repo
        .git(&["rev-list", "--count", "HEAD"])
        .trim()
        .parse::<usize>()
        .expect("count");
    let (status, body) = post(
        &addr,
        "/api/projects",
        r#"{"name":"second","prefix":"VER","actor":"human:gui"}"#,
        Some("gui-test-token"),
    );
    assert!(status.contains("201"), "create: {status} {body}");
    let after = repo
        .git(&["rev-list", "--count", "HEAD"])
        .trim()
        .parse::<usize>()
        .expect("count");
    assert_eq!(after, before + 1, "create = one commit");
    let config = std::fs::read_to_string(repo.root.join(".tissue/config.yml")).unwrap();
    assert!(config.contains("second: VER"), "mapping written: {config}");

    // AC2: the CLI sees it immediately (shared core, no drift).
    let out = repo.tissue(&["list", "--project", "second"]);
    assert!(out.status.success(), "list --project second: {out:?}");
    let (status, body) = get(&addr, "/api/projects");
    assert!(
        status.contains("200") && body.contains("\"name\":\"second\""),
        "new project in read: {body}"
    );

    // Conflicts: duplicate prefix under a different name -> 409.
    let (status, body) = post(
        &addr,
        "/api/projects",
        r#"{"name":"other","prefix":"VER"}"#,
        Some("gui-test-token"),
    );
    assert!(status.contains("409"), "duplicate prefix: {status} {body}");
    assert!(body.contains("already mapped"), "conflict message: {body}");
    // Same name, different prefix -> 409 (remap refusal).
    let (status, body) = post(
        &addr,
        "/api/projects",
        r#"{"name":"second","prefix":"OPS"}"#,
        Some("gui-test-token"),
    );
    assert!(status.contains("409"), "remap refused: {status} {body}");
    // Invalid input -> 400.
    let (status, body) = post(
        &addr,
        "/api/projects",
        r#"{"name":"bad","prefix":"ver"}"#,
        Some("gui-test-token"),
    );
    assert!(status.contains("400"), "lowercase prefix: {status} {body}");
    // Auth still required: no token -> 401.
    let (status, _) = post(&addr, "/api/projects", r#"{"name":"x","prefix":"X"}"#, None);
    assert!(status.contains("401"), "no token: {status}");

    // AC3 parity: the CLI refuses the exact same conflicting input.
    let out = repo.tissue(&["project", "add", "other", "VER"]);
    assert!(!out.status.success(), "CLI parity: duplicate refused");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("already mapped"),
        "CLI parity message: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// AC1 + AC2: a proposal created via CLI appears in the proposals read;
/// approving via the API applies the edit and commits like the CLI.
#[test]
fn approve_via_api_applies_edit_and_commits() {
    let repo = Scratch::new("approve");
    repo.tissue(&["create", "IAM-1", "Old title", "--body", "b"]);
    // A proposal exists (created via CLI, like an agent would).
    let out = repo.tissue(&[
        "propose",
        "IAM-1",
        "--field",
        "title",
        "--current",
        "Old title",
        "--proposed",
        "New title",
        "--rationale",
        "clearer",
        "--actor",
        "agent:e2e",
    ]);
    assert!(out.status.success(), "propose failed: {out:?}");

    let server = start_server(&repo);

    // AC1: the queue data path — pending proposals readable, tokenless GET.
    let (status, body) = get(&server.addr, "/api/issues/IAM-1/proposals");
    assert_eq!(status, "200", "proposals read: {body}");
    assert!(body.contains("New title"), "proposal content: {body}");
    assert!(body.contains("proposal_id"), "proposal id: {body}");

    // Agent-stream timeline read (AC4 data path).
    let (status, body) = get(&server.addr, "/api/issues/IAM-1/records");
    assert_eq!(status, "200", "records read: {body}");
    assert!(body.contains("proposal"), "stream has the proposal: {body}");

    // Approve without a token: refused.
    let (status, body) = post(
        &server.addr,
        "/api/issues/IAM-1/approve",
        r#"{"proposal_id":"prop-01","actor":"human:gui"}"#,
        None,
    );
    assert_eq!(status, "401", "unauthenticated approve: {body}");

    // Approve with the token (what the GUI does).
    let (status, body) = post(
        &server.addr,
        "/api/issues/IAM-1/approve",
        r#"{"proposal_id":"prop-01","actor":"human:gui"}"#,
        Some("gui-test-token"),
    );
    assert_eq!(status, "201", "approve: {body}");
    assert!(body.contains("approved"), "approve body: {body}");

    // AC2: the human format was updated, exactly as the CLI would.
    let issue = repo.read_issue("IAM-1");
    assert!(issue.contains("New title"), "applied: {issue}");
    // The needs-review tag is cleared.
    assert!(!issue.contains("needs-review"), "tag cleared: {issue}");
    // The commit trail matches the CLI shape.
    let log = repo.git(&["log", "--oneline"]);
    assert!(log.contains("approve IAM-1"), "commit: {log}");

    // No pending proposals remain.
    let (status, body) = get(&server.addr, "/api/issues/IAM-1/proposals");
    assert_eq!(status, "200", "proposals re-read: {body}");
    assert_eq!(body.trim(), "[]", "queue drained: {body}");
}

/// AC3: rejection via the API records the reason as data, not an error —
/// and a reason is mandatory.
#[test]
fn reject_via_api_records_reason_as_data() {
    let repo = Scratch::new("reject");
    repo.tissue(&["create", "IAM-1", "Stay", "--body", "b"]);
    let out = repo.tissue(&[
        "propose",
        "IAM-1",
        "--field",
        "status",
        "--current",
        "open",
        "--proposed",
        "closed",
        "--rationale",
        "done?",
        "--actor",
        "agent:e2e",
    ]);
    assert!(out.status.success(), "propose failed: {out:?}");

    let server = start_server(&repo);

    // Reject without a reason: refused — rejections are data.
    let (status, body) = post(
        &server.addr,
        "/api/issues/IAM-1/reject",
        r#"{"proposal_id":"prop-01","actor":"human:gui"}"#,
        Some("gui-test-token"),
    );
    assert_eq!(status, "400", "reasonless reject: {body}");
    assert!(body.contains("reason"), "reason required: {body}");

    // Reject with a reason.
    let (status, body) = post(
        &server.addr,
        "/api/issues/IAM-1/reject",
        r#"{"proposal_id":"prop-01","reason":"not done yet","actor":"human:gui"}"#,
        Some("gui-test-token"),
    );
    assert_eq!(status, "201", "reject: {body}");

    // The rejection is recorded in the stream, human format untouched.
    let stream = std::fs::read_to_string(repo.root.join(".tissue/issues/IAM-1/agent.jsonl"))
        .expect("stream");
    assert!(stream.contains("rejection"), "rejection record: {stream}");
    assert!(stream.contains("not done yet"), "reason as data: {stream}");
    let issue = repo.read_issue("IAM-1");
    assert!(issue.contains("open"), "human format untouched: {issue}");
    assert!(!issue.contains("needs-review"), "tag cleared: {issue}");

    // The timeline data path shows the rejection record type.
    let (status, body) = get(&server.addr, "/api/issues/IAM-1/records");
    assert_eq!(status, "200");
    assert!(body.contains("rejection"), "timeline rejection: {body}");
}

/// #175 AC1: a project created via POST /api/projects is persisted in
/// config.yml + one commit AND survives a full serve restart (the owner's
/// smoke-pass symptom: "created in the GUI, gone after reload" — the GUI
/// switcher also derived from issue prefixes, so an issue-less project
/// vanished; app.ts now reads GET /api/projects).
#[test]
fn projects_api_create_persists_across_restart() {
    let repo = Scratch::new("projects-persist-175");
    repo.set_token("gui-test-token");
    let server = start_server(&repo);
    let addr = server.addr;

    let (status, body) = post(
        &addr,
        "/api/projects",
        r#"{"name":"ops","prefix":"OPS","actor":"human:gui"}"#,
        Some("gui-test-token"),
    );
    assert!(status.contains("201"), "create: {status} {body}");
    // Response carries the created project (#175).
    assert!(
        body.contains("\"name\":\"ops\"") && body.contains("\"prefix\":\"OPS\""),
        "{body}"
    );
    let config = std::fs::read_to_string(repo.root.join(".tissue/config.yml")).unwrap();
    assert!(config.contains("ops: OPS"), "mapping written: {config}");
    let commits = repo
        .git(&["rev-list", "--count", "HEAD"])
        .trim()
        .to_string();

    // Kill and restart serve; the project must still be listed.
    drop(server);
    let server2 = start_server(&repo);
    let (status, body) = get(&server2.addr, "/api/projects");
    assert!(status.contains("200"), "{status} {body}");
    assert!(
        body.contains("\"name\":\"ops\"") && body.contains("\"prefix\":\"OPS\""),
        "project survives restart: {body}"
    );
    // Exactly one commit for the create, and the config commit is on disk.
    let commits2 = repo
        .git(&["rev-list", "--count", "HEAD"])
        .trim()
        .to_string();
    assert_eq!(commits2, commits, "no extra commits after restart");
    let log = repo.git(&["log", "--oneline", "-1"]);
    assert!(
        log.contains("project add: ops -> OPS"),
        "commit message: {log}"
    );
}

/// #170: the issue detail ships a Human/AI tab split — the served GUI
/// carries the tab markup + `/ai` route, and every data path the AI tab
/// uses (issue, records, proposals) stays a tokenless loopback read.
#[test]
fn issue_detail_ai_tab_ships_and_data_paths_read() {
    let repo = Scratch::new("ai-tab");
    let out = repo.tissue(&["create", "IAM-1", "Tabbed issue", "--body", "hello"]);
    assert!(out.status.success(), "create failed: {out:?}");
    let out = repo.tissue(&[
        "propose",
        "IAM-1",
        "--field",
        "title",
        "--current",
        "Tabbed issue",
        "--proposed",
        "Better title",
        "--actor",
        "agent:e2e",
    ]);
    assert!(out.status.success(), "propose failed: {out:?}");
    let server = start_server(&repo);
    let addr = server.addr;

    // The compiled GUI served to the browser has the Human/AI tabs and
    // the #/issue/<id>/ai route.
    let (status, js) = get(&addr, "/app.js");
    assert!(status.contains("200"), "app.js: {status}");
    assert!(js.contains("data-tab=\"ai\""), "AI tab shipped: {js}");
    assert!(js.contains("/ai"), "ai route shipped: {js}");
    let (status, css) = get(&addr, "/style.css");
    assert!(
        status.contains("200") && css.contains(".tabs"),
        "tab styles shipped: {css}"
    );

    // The AI tab's data paths: all tokenless reads.
    let (status, body) = get(&addr, "/api/issues/IAM-1");
    assert!(
        status.contains("200") && body.contains("Tabbed issue"),
        "detail: {body}"
    );
    let (status, body) = get(&addr, "/api/issues/IAM-1/records");
    assert!(
        status.contains("200") && body.contains("proposal"),
        "records: {body}"
    );
    let (status, body) = get(&addr, "/api/issues/IAM-1/proposals");
    assert!(
        status.contains("200") && body.contains("Better title"),
        "proposals: {body}"
    );
}

/// #178: the Projects view data path — GET /api/projects lists every
/// configured project (incl. issue-less ones), and the issue list carries
/// the data the page needs for per-project counts. The sidebar/nav markup
/// change is static HTML (project selector on top, Projects nav item).
#[test]
fn projects_page_data_path() {
    let repo = Scratch::new("projects-page-178");
    // Two projects, one of them issue-less — both must be listed (#175).
    let out = repo.tissue(&["project", "add", "ops", "OPS"]);
    assert!(out.status.success(), "{out:?}");
    let out = repo.tissue(&["create", "IAM-1", "only issue", "--body", "b"]);
    assert!(out.status.success(), "{out:?}");
    let server = start_server(&repo);
    let (status, body) = get(&server.addr, "/api/projects");
    assert!(status.contains("200"), "{status} {body}");
    assert!(
        body.contains("\"prefix\":\"IAM\"") && body.contains("\"prefix\":\"OPS\""),
        "{body}"
    );
    // Issue summary carries id (prefix derivable client-side) + status + tags.
    let (status, body) = get(&server.addr, "/api/issues");
    assert!(
        status.contains("200") && body.contains("IAM-1"),
        "{status} {body}"
    );
}
