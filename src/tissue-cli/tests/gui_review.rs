//! End-to-end tests for the GUI review surface (#98): the awaiting-decision
//! data path and the approve/reject loop over the token-authenticated
//! API — exactly what the browser does, minus the browser. Pins:
//! proposals and records are readable without a token (loopback read),
//! approve/reject require one, decisions commit exactly like the CLI,
//! and rejections without a reason are refused (rejections are data).

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
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
        common::output(&mut cmd, common::default_timeout()).expect("run tissue binary")
    }

    fn git(&self, args: &[&str]) -> String {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(&self.root);
        let out = common::output(&mut cmd, common::default_timeout()).expect("run git");
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

// The GuardedChild drop kills the whole process group and reaps the child
// (#228): a panicking test cannot leave a `tissue serve` runaway behind.
struct Server {
    _child: common::GuardedChild,
    addr: SocketAddr,
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
    let mut child = common::spawn(&mut cmd).expect("spawn tissue serve");
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    for _ in 0..100 {
        if TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(50)).is_ok() {
            return Server {
                _child: child,
                addr,
            };
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.kill_group();
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

/// #168: the served GUI ships the design-pass control system — focus
/// rings, button variants, unified text controls, and the .error banner —
/// and the primary actions use btn-primary.
#[test]
fn gui_ships_design_pass_controls() {
    let repo = Scratch::new("design-pass");
    let out = repo.tissue(&["create", "IAM-1", "Styled", "--body", "x"]);
    assert!(out.status.success(), "{out:?}");
    let server = start_server(&repo);
    let addr = server.addr;

    let (status, css) = get(&addr, "/style.css");
    assert!(status.contains("200"), "{status}");
    for needle in [
        ":focus-visible",
        ".btn-primary",
        ".btn-danger",
        ".btn:disabled",
        ".error {",
    ] {
        assert!(css.contains(needle), "style.css has {needle}: missing");
    }
    let (status, js) = get(&addr, "/app.js");
    assert!(
        status.contains("200") && js.contains("btn-primary"),
        "app.js primary buttons: {status}"
    );
    assert!(js.contains("placeholder"), "app.js empty states");
    let (status, html) = get(&addr, "/");
    assert!(
        status.contains("200") && html.contains("style.css"),
        "index.html links the design-pass stylesheet (btn-primary markup lives in app.ts templates since #178)"
    );
}

/// #199: the token entry input exists ONLY in the Settings route markup —
/// every other view renders a masked chip (no autofillable password field),
/// and the Settings input is hardened (autocomplete=new-password, name).
#[test]
fn token_input_only_on_settings() {
    let repo = Scratch::new("token-199");
    let out = repo.tissue(&["create", "IAM-1", "x", "--body", "b"]);
    assert!(out.status.success(), "{out:?}");
    let server = start_server(&repo);
    let addr = server.addr;

    let (status, js) = get(&addr, "/app.js");
    assert!(status.contains("200"), "{status}");
    // Exactly one token entry input in the whole app (the Settings one).
    assert_eq!(
        js.matches("id=\"api-token\"").count(),
        1,
        "one token input total: app.js"
    );
    assert!(
        js.contains("autocomplete=\"new-password\""),
        "settings token input hardened: app.js"
    );
    assert!(
        js.contains("name=\"tissue-api-token-entry\""),
        "dedicated field name: app.js"
    );
    // Non-settings views: masked chip + Change link to Settings, no input.
    assert!(
        js.contains("API token set (this browser session only)"),
        "masked chip: app.js"
    );
    assert!(
        js.contains("href=\"#/settings\""),
        "change link to settings: app.js"
    );
    // The served shell carries no token field either.
    let (status, html) = get(&addr, "/");
    assert!(
        status.contains("200") && !html.contains("api-token"),
        "index.html has no token input: {status}"
    );
}

/// #206: the Releases view renders the "+ New release" toolbar + form even
/// on an empty repo (the v0.1.6 early-return hid them), and creating the
/// first release from the GUI works end to end.
#[test]
fn releases_empty_state_has_create_button() {
    let repo = Scratch::new("releases-empty-206");
    // Issue a managed token (hash-only); legacy api_token: lines migrate on
    // first `token list` (#174) and are not the default test path anymore.
    let out = repo.tissue(&["token", "issue", "--label", "human:gui"]);
    assert!(out.status.success(), "{out:?}");
    let tok = String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("token: "))
        .expect("secret")
        .trim()
        .to_string();
    let server = start_server(&repo);
    // Empty repo: GET /api/releases is [].
    let (status, body) = get(&server.addr, "/api/releases");
    assert!(
        status.contains("200") && body.contains("[]"),
        "{status} {body}"
    );
    // The app shell + app.js ship the new-release button/form wiring.
    let (status, js) = get(&server.addr, "/app.js");
    assert!(status.contains("200"), "{status}");
    assert!(js.contains("new-release-btn"), "button id in app.js");
    assert!(js.contains("new-release-form"), "form id in app.js");
    // Create the first release via the API the button calls.
    let (status, body) = post(
        &server.addr,
        "/api/releases",
        r#"{"id":"v0.1.0","title":"First"}"#,
        Some(&tok),
    );
    assert!(status.contains("201"), "{status} {body}");
    let (status, body) = get(&server.addr, "/api/releases");
    assert!(
        status.contains("200") && body.contains("v0.1.0"),
        "listed after create: {status} {body}"
    );
}

/// #201: Projects page counts come from a fresh unfiltered /api/issues, row
/// links carry #/issues?project=<prefix>, route() applies the param to the
/// switcher, and the prefix badge is not uppercased.
#[test]
fn projects_page_counts_nav_and_prefix_case() {
    let repo = Scratch::new("projects-201");
    assert!(repo
        .tissue(&["project", "add", "ops", "OPS"])
        .status
        .success());
    assert!(repo
        .tissue(&["create", "IAM-1", "iam issue", "--body", "b"])
        .status
        .success());
    assert!(repo
        .tissue(&[
            "create",
            "OPS-1",
            "ops one",
            "--project",
            "ops",
            "--body",
            "b"
        ])
        .status
        .success());
    assert!(repo
        .tissue(&[
            "create",
            "OPS-2",
            "ops two",
            "--project",
            "ops",
            "--body",
            "b"
        ])
        .status
        .success());
    let server = start_server(&repo);
    let addr = server.addr;

    // The unfiltered list carries every project's issues (count source).
    let (status, body) = get(&addr, "/api/issues");
    assert!(status.contains("200"), "{status}");
    for id in ["IAM-1", "OPS-1", "OPS-2"] {
        assert!(body.contains(id), "unfiltered list has {id}: {body}");
    }

    let (status, js) = get(&addr, "/app.js");
    assert!(status.contains("200"), "{status}");
    assert!(
        js.contains("#/issues?project="),
        "project row links carry ?project=: app.js"
    );
    assert!(
        js.contains("URLSearchParams") && js.contains("get(\"project\")"),
        "route reads ?project=: app.js"
    );
    assert!(
        js.contains("countSource"),
        "counts use the fresh unfiltered list: app.js"
    );

    // Prefix badge renders as-is (no uppercase transform).
    let (status, css) = get(&addr, "/style.css");
    assert!(status.contains("200"), "{status}");
    let badge_rule = css
        .split(".project-badge")
        .nth(1)
        .unwrap_or("")
        .split('}')
        .next()
        .unwrap_or("");
    assert!(
        badge_rule.contains("text-transform: none"),
        "project badge keeps prefix case: {badge_rule}"
    );
}

/// #221: the sidebar project dropdown is fed by GET /api/projects through the
/// single projectPrefixes() code path — a project with zero issues appears in
/// the authoritative list (the dropdown's source AND the Projects page), the
/// served app.js carries the helper and no longer the `prefixes = []` rebase
/// debris, and dropdown selection filters via applyFilters/projectOf.
#[test]
fn sidebar_dropdown_lists_issue_less_projects_221() {
    let repo = Scratch::new("dropdown-221");
    assert!(repo
        .tissue(&["create", "IAM-1", "iam issue", "--body", "b"])
        .status
        .success());
    let server = start_server(&repo);
    let addr = server.addr;

    // Create a project with zero issues through the GUI write path.
    let (status, body) = post(
        &addr,
        "/api/projects",
        r#"{"name":"ops","prefix":"OPS","actor":"human:gui"}"#,
        Some("gui-test-token"),
    );
    assert!(status.contains("201"), "create: {status} {body}");

    // The authoritative project list — the dropdown's source and the Projects
    // page's source — carries the issue-less project next to IAM.
    let (status, body) = get(&addr, "/api/projects");
    assert!(status.contains("200"), "{status} {body}");
    assert!(
        body.contains("\"prefix\":\"IAM\"") && body.contains("\"prefix\":\"OPS\""),
        "zero-issue project OPS listed: {body}"
    );

    // The served dropdown element exists; the served app.js ships the single
    // projectPrefixes code path and the debris is gone.
    let (status, html) = get(&addr, "/");
    assert!(
        status.contains("200") && html.contains("id=\"project-select\""),
        "dropdown element in served index.html: {status}"
    );
    let (status, js) = get(&addr, "/app.js");
    assert!(status.contains("200"), "{status}");
    assert!(
        js.contains("projectPrefixes") && js.contains("knownProjects"),
        "single helper + issue-derived fallback ship: app.js"
    );
    assert!(
        !js.contains("prefixes = [];"),
        "rebase debris removed from served app.js"
    );
    // Dropdown selection filters the list (client-side applyFilters path).
    assert!(
        js.contains("applyFilters") && js.contains("projectOf"),
        "dropdown filter path ships: app.js"
    );
}

/// #202: the footer version links to the GitHub release-tag page for that
/// version (from /api/meta), the license links to LICENSE, links are plain
/// with underline on hover, and the compiled-in fallback links the tag too.
#[test]
fn footer_links_release_and_license() {
    let repo = Scratch::new("footer-202");
    let server = start_server(&repo);
    let addr = server.addr;
    let ver = env!("CARGO_PKG_VERSION");

    // Served index.html fallback: version link + license link baked in.
    let (status, html) = get(&addr, "/");
    assert!(status.contains("200"), "{status}");
    assert!(
        html.contains(&format!("releases/tag/v{ver}")),
        "fallback footer links the release tag: {ver}"
    );
    assert!(html.contains("blob/main/LICENSE"), "fallback license link");
    assert!(
        html.contains("target=\"_blank\"") && html.contains("rel=\"noopener noreferrer\""),
        "links open safely"
    );

    // Runtime footer (from /api/meta): app.js builds the release-tag link.
    let (status, js) = get(&addr, "/app.js");
    assert!(status.contains("200"), "{status}");
    assert!(
        js.contains("releases/tag/v"),
        "app.js builds the release-tag link from meta.version"
    );
    assert!(
        js.contains("rel=\\\"noopener noreferrer\\\"") || js.contains("noopener noreferrer"),
        "app.js links carry rel=noopener"
    );

    // Plain look: footer links inherit color, underline only on hover.
    let (status, css) = get(&addr, "/style.css");
    assert!(status.contains("200"), "{status}");
    assert!(
        css.contains(".app-footer a { color: inherit; text-decoration: none; }"),
        "plain footer links: {css}"
    );
    assert!(
        css.contains(".app-footer a:hover { text-decoration: underline; }"),
        "hover underline: {css}"
    );
}

/// #200: the Settings view is restructured into three bordered sections —
/// Agent connection, API tokens, About — in that order, with the token entry
/// field inside the API tokens section, and /api/meta carries repo_path for
/// the About section.
#[test]
fn settings_page_has_three_sections() {
    let repo = Scratch::new("settings-sections-200");
    let out = repo.tissue(&["create", "IAM-1", "x", "--body", "b"]);
    assert!(out.status.success(), "{out:?}");
    let server = start_server(&repo);
    let (status, js) = get(&server.addr, "/app.js");
    assert!(status.contains("200"), "{status}");
    let i_agent = js.find("id=\"section-agent\"").expect("agent section");
    let i_tokens = js.find("id=\"section-tokens\"").expect("tokens section");
    let i_about = js.find("id=\"section-about\"").expect("about section");
    assert!(
        i_agent < i_tokens && i_tokens < i_about,
        "section order agent < tokens < about"
    );
    for h in ["Agent connection", "API tokens", "About"] {
        assert!(js.contains(h), "heading present: {h}");
    }
    // The token bar is rendered via settingsTokenBarHtml() inside the tokens
    // section (the compiled app.js concatenates it there, after section-tokens).
    let tail = &js[i_tokens..];
    let call = tail.find("settingsTokenBarHtml()").expect("token bar call");
    let about = tail.find("id=\"section-about\"").expect("about section");
    assert!(call < about, "token bar rendered inside the tokens section");
    assert!(js.contains("id=\"about-meta\""), "about meta container");
    // #200: /api/meta carries repo_path for the About section.
    let (status, body) = get(&server.addr, "/api/meta");
    assert!(
        status.contains("200") && body.contains("repo_path"),
        "meta repo_path: {status} {body}"
    );
}

/// #205: the Agent connection section carries a copy-pasteable hosted-agent
/// (Mistral) example with the key-ref rule and the no-TLS transport note
/// (this build refuses https:// endpoints; use a local TLS-terminating proxy).
#[test]
fn settings_agent_note_has_hosted_example() {
    let repo = Scratch::new("settings-note-205");
    let out = repo.tissue(&["create", "IAM-1", "x", "--body", "b"]);
    assert!(out.status.success(), "{out:?}");
    let server = start_server(&repo);
    let (status, js) = get(&server.addr, "/app.js");
    assert!(status.contains("200"), "{status}");
    assert!(
        js.contains("api.mistral.ai"),
        "Mistral target in example: app.js"
    );
    assert!(js.contains("mistral-small-latest"), "example model: app.js");
    assert!(js.contains("api_key_ref"), "key-ref rule: app.js");
    assert!(
        js.contains("unsupported-transport"),
        "transport note: app.js"
    );
    assert!(
        js.contains("TLS-terminating proxy"),
        "proxy guidance: app.js"
    );
}

/// #211: the served GUI carries the type/parent/workflow UI — type badge on
/// rows, type+status filter selects in the list toolbar, type/parent pickers
/// on "+ New issue", the five-state status dropdown + parent field in the
/// edit form, the feature children section — and the issue JSON carries
/// type/parent; the sidebar has the new status links and a type filter.
#[test]
fn gui_ships_types_and_workflow_211() {
    let repo = Scratch::new("gui-211");
    assert!(repo
        .tissue(&[
            "create",
            "IAM-1",
            "Login flow",
            "--type",
            "feature",
            "--body",
            "b"
        ])
        .status
        .success());
    assert!(repo
        .tissue(&[
            "create",
            "IAM-2",
            "Redirect loop",
            "--type",
            "bug",
            "--parent",
            "IAM-1",
            "--body",
            "b"
        ])
        .status
        .success());
    let server = start_server(&repo);
    let addr = server.addr;

    // Issue JSON carries type/parent (list + detail).
    let (status, body) = get(&addr, "/api/issues");
    assert!(status.contains("200"), "{status} {body}");
    assert!(
        body.contains("\"type\":\"feature\"")
            && body.contains("\"type\":\"bug\"")
            && body.contains("\"parent\":\"IAM-1\""),
        "list JSON: {body}"
    );
    let (status, body) = get(&addr, "/api/issues/IAM-2");
    assert!(
        status.contains("200")
            && body.contains("\"type\":\"bug\"")
            && body.contains("\"parent\":\"IAM-1\""),
        "detail JSON: {body}"
    );

    // Served app.js: new-issue pickers, toolbar filters, edit form fields,
    // type badge, feature children, five workflow statuses.
    let (status, js) = get(&addr, "/app.js");
    assert!(status.contains("200"), "app.js: {status}");
    for needle in [
        "new-issue-type",
        "new-issue-parent",
        "filter-type",
        "filter-status",
        "edit-type",
        "edit-parent",
        "edit-status",
        "type-badge",
        "\"rejected\"",
        "\"review\"",
        "Tasks / bugs in this feature",
    ] {
        assert!(js.contains(needle), "app.js has {needle}");
    }

    // Shell: sidebar status links use the new enum; type filter block exists.
    let (status, html) = get(&addr, "/");
    assert!(status.contains("200"), "{status}");
    for needle in [
        "#/issues?status=review",
        "#/issues?status=done",
        "#/issues?status=rejected",
        "#/issues?type=feature",
        "#/issues?type=bug",
    ] {
        assert!(html.contains(needle), "index.html has {needle}");
    }

    // Styles: new status colors + type badge.
    let (status, css) = get(&addr, "/style.css");
    assert!(status.contains("200"), "{status}");
    for needle in [
        ".status-review",
        ".status-done",
        ".status-rejected",
        ".type-badge",
    ] {
        assert!(css.contains(needle), "style.css has {needle}");
    }
}

/// #212: search is one shared matcher — CLI `list --search <q>` and
/// `GET /api/issues?q=<q>` return identical id sets, covering id, title,
/// body, tags, type, status — case-insensitively, unicode included.
/// Combinable with --status/--project on the CLI. The served GUI searches
/// via the API (`/api/issues?q=`); toolbar dropdowns are #211's hash-param
/// selects (filter-type / filter-status).
#[test]
fn search_cli_and_api_agree_212() {
    let repo = Scratch::new("search-212");
    assert!(repo
        .tissue(&["project", "add", "ops", "OPS"])
        .status
        .success());
    assert!(repo
        .tissue(&[
            "create",
            "IAM-1",
            "Fix login bug",
            "--body",
            "session cookie dropped"
        ])
        .status
        .success());
    assert!(repo
        .tissue(&[
            "create",
            "IAM-2",
            "Café menu polish",
            "--type",
            "bug",
            "--body",
            "unicode ü body"
        ])
        .status
        .success());
    assert!(repo
        .tissue(&[
            "create",
            "OPS-1",
            "deploy notes",
            "--project",
            "ops",
            "--body",
            "systemd unit"
        ])
        .status
        .success());
    // Tag on IAM-1 via the human format (serve/CLI both read the working tree).
    let p = repo.root.join(".tissue/issues/IAM-1/issue.md");
    let md = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, md.replace("tags: []", "tags: [urgent]")).unwrap();

    let server = start_server(&repo);
    let addr = server.addr;

    let cli_ids = |q: &str| -> Vec<String> {
        let out = repo.tissue(&["list", "--search", q]);
        assert!(out.status.success(), "list --search {q}: {out:?}");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.split_whitespace().next().map(|s| s.to_string()))
            .filter(|t| t.contains('-'))
            .collect()
    };
    let api_ids = |path: &str| -> Vec<String> {
        let (status, body) = get(&addr, path);
        assert!(status.contains("200"), "{path}: {status} {body}");
        let mut ids = Vec::new();
        let mut rest = body.as_str();
        while let Some(i) = rest.find("\"id\":\"") {
            rest = &rest[i + 6..];
            let end = rest.find('"').unwrap();
            ids.push(rest[..end].to_string());
            rest = &rest[end..];
        }
        ids.sort();
        ids
    };

    // (query, cli arg, url-encoded api path, expected ids)
    let cases: &[(&str, &str, &str, &[&str])] = &[
        ("login", "login", "login", &["IAM-1"]),
        ("cookie", "cookie", "cookie", &["IAM-1"]), // body-only match
        ("urgent", "urgent", "urgent", &["IAM-1"]), // tag match
        ("bug", "bug", "bug", &["IAM-1", "IAM-2"]), // title match + type match
        ("feature", "feature", "feature", &[]),     // no feature in this scratch repo
        ("open", "open", "open", &["IAM-1", "IAM-2", "OPS-1"]), // status match
        ("CAFÉ", "CAFÉ", "caf%C3%A9", &["IAM-2"]),  // unicode case-insensitive
        ("systemd", "systemd", "systemd", &["OPS-1"]),
        ("zzz-no-match", "zzz-no-match", "zzz-no-match", &[]),
    ];
    for (label, cli_q, api_q, expected) in cases {
        let mut cli = cli_ids(cli_q);
        cli.sort();
        let api = api_ids(&format!("/api/issues?q={api_q}"));
        let mut exp: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        exp.sort();
        assert_eq!(cli, exp, "CLI ids for {label:?}");
        assert_eq!(api, exp, "API ids for {label:?}");
        assert_eq!(cli, api, "CLI/API parity for {label:?}");
    }

    // Combinable with --status/--project on the CLI.
    let out = repo.tissue(&["list", "--search", "open", "--status", "closed"]);
    assert!(out.status.success());
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("IAM-1"),
        "search+status combine"
    );
    let out = repo.tissue(&["list", "--search", "systemd", "--project", "ops"]);
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("OPS-1"), "search+project: {s}");
    let out = repo.tissue(&["list", "--search", "bug", "--type", "feature"]);
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(!s.contains("IAM-2"), "search+type combine: {s}");

    // GUI: search goes through the API query; toolbar dropdowns ship (#211).
    let (status, js) = get(&addr, "/app.js");
    assert!(status.contains("200"), "{status}");
    assert!(
        js.contains("/api/issues?q="),
        "GUI searches via API: app.js"
    );
    assert!(
        !js.contains("searchMatches"),
        "id/title-only matcher removed"
    );
    assert!(
        js.contains("filter-status"),
        "status dropdown ships: app.js"
    );
    assert!(js.contains("filter-type"), "type dropdown ships: app.js");
}

/// #244: the issue detail view is restructured for at-a-glance reading —
/// type/status/project badges inline in the header, the remaining
/// front-matter fields as a compact meta strip, the body in a bounded card,
/// the edit form a collapsed <details> block; styles ship and stay
/// flex-wrap (mobile-degradable, #239).
#[test]
fn detail_view_hierarchy_244() {
    let repo = Scratch::new("detail-244");
    let out = repo.tissue(&["create", "IAM-1", "Restructured", "--body", "hello"]);
    assert!(out.status.success(), "create: {out:?}");
    let server = start_server(&repo);
    let (status, js) = get(&server.addr, "/app.js");
    assert!(status.contains("200"), "app.js: {status}");
    for needle in ["detail-badges", "meta-strip", "meta-item", "body-card"] {
        assert!(js.contains(needle), "app.js ships {needle}");
    }
    // edit form stays collapsed under an "Edit" summary, after the body card
    assert!(
        js.contains("<summary>Edit</summary>"),
        "collapsed edit: {js}"
    );
    let (status, css) = get(&server.addr, "/style.css");
    assert!(status.contains("200"), "style.css: {status}");
    for needle in [".detail-badges", ".meta-strip", ".body-card"] {
        assert!(css.contains(needle), "style.css ships {needle}");
    }
}

/// #245: the Settings page renders one setting per line — every setting row
/// is a `.settings-row` grid (label column + control), buttons/results on
/// their own rows at the end of their section, and the whitelisted config
/// dump is a collapsed <details>. All ids unchanged.
#[test]
fn settings_page_one_setting_per_row_245() {
    let repo = Scratch::new("settings-245");
    let server = start_server(&repo);
    let (status, js) = get(&server.addr, "/app.js");
    assert!(status.contains("200"), "app.js: {status}");
    for needle in ["settings-row", "settings-actions", "settings-config-dump"] {
        assert!(js.contains(needle), "app.js ships {needle}");
    }
    // every settings input/select now sits in a row with a label[for]
    for id in [
        "cfg-agent-mode",
        "cfg-agent-command",
        "cfg-agent-endpoint",
        "cfg-remote",
        "token-label",
    ] {
        assert!(
            js.contains(&format!("for=\"{id}\"")),
            "label[for={id}] in row: {js}"
        );
    }
    // config dump collapsed in a details at the bottom of the agent section
    assert!(
        js.contains("Current config (whitelisted)") && js.contains("id=\"cfg-view\""),
        "config dump kept, collapsed: {js}"
    );
    let (status, css) = get(&server.addr, "/style.css");
    assert!(status.contains("200"), "style.css: {status}");
    for needle in [
        ".settings-row",
        ".settings-actions",
        "grid-template-columns",
    ] {
        assert!(css.contains(needle), "style.css ships {needle}");
    }
}

/// #251: Sync / Approve / Reject are self-explanatory — each operation has a
/// visible plain-language explanation (not only a tooltip), the Sync button
/// sits outside the collapsed edit form with a caption, and the review queue
/// header states what the queue means.
#[test]
fn operations_are_self_explanatory_251() {
    let repo = Scratch::new("wording-251");
    let server = start_server(&repo);
    let (status, js) = get(&server.addr, "/app.js");
    assert!(status.contains("200"), "app.js: {status}");
    // proposal cards: visible explanations for both actions
    assert!(
        js.contains("apply this proposed change to") && js.contains("stays unchanged"),
        "approve/reject explanations: {js}"
    );
    // sync moved out of the edit form, with a visible caption
    assert!(js.contains("sync-caption"), "sync caption: {js}");
    assert!(
        js.contains("exact field-level diff the agent will reconcile from"),
        "sync caption text: {js}"
    );
    // review queue header states the queue's meaning
    assert!(
        js.contains("nothing changes in the") && js.contains("until you approve"),
        "queue header meaning: {js}"
    );
    let (status, css) = get(&server.addr, "/style.css");
    assert!(
        status.contains("200") && css.contains(".proposal-explain") && css.contains(".sync-row"),
        "css: {css}"
    );
}
