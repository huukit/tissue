//! E2E tests for #137: the `agent:` config block, the agent-invoking
//! sync (CLI + API), and the whitelisted `GET/PUT /api/config`.
//!
//! AC1: no `agent:` block => sync is record-only (backwards compatible).
//! AC2: a stub agent command => sync records human-sync + agent-run,
//!       the stub runs with the issue repo as cwd, all committed.
//! AC3: agent timeout / non-zero exit is recorded, the sync still
//!       succeeds, and the server stays up.
//! AC4: GET /api/config never returns api_token; PUT rejects
//!      non-whitelisted keys (incl. api_token).
//! AC5: e2e with a stub agent script.

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
            "tissue-settings-{}-{}-{}",
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
        let out = s.tissue(&["create", "IAM-1", "First", "--body", "body"]);
        assert!(out.status.success(), "tissue create failed: {out:?}");
        s
    }

    fn tissue(&self, args: &[&str]) -> Output {
        self.tissue_env(args, &[])
    }

    fn tissue_env(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let exe: PathBuf = env!("CARGO_BIN_EXE_tissue").into();
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .current_dir(&self.root)
            .env_remove("EDITOR")
            .env_remove("TISSUE_API_TOKEN");
        for (k, v) in env {
            cmd.env(k, v);
        }
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

    fn config_yml(&self) -> String {
        std::fs::read_to_string(self.root.join(".tissue/config.yml")).expect("config.yml")
    }

    fn set_config(&self, extra: &str) {
        let config = self.root.join(".tissue/config.yml");
        let content = std::fs::read_to_string(&config).expect("config.yml exists");
        std::fs::write(&config, format!("{content}{extra}")).expect("write config");
        self.git(&["add", "-A"]);
        self.git(&["commit", "-m", "test: agent config"]);
    }

    fn set_token(&self, token: &str) {
        self.set_config(&format!("api_token: \"{token}\"\n"));
    }

    fn agent_stream(&self, id: &str) -> String {
        let p = self
            .root
            .join(".tissue/issues")
            .join(id)
            .join("agent.jsonl");
        std::fs::read_to_string(p).unwrap_or_default()
    }

    fn stdout(out: &Output) -> String {
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn stderr(out: &Output) -> String {
        String::from_utf8_lossy(&out.stderr).into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A stub agent: asserts the token is NOT in its env, checks the {issue}
/// arg, and appends a marker file in the repo (cwd).
fn write_stub_agent(root: &std::path::Path, name: &str, extra_check: &str) -> PathBuf {
    let stub = root.join(name);
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\ncat >/dev/null\n[ -z \"$TISSUE_API_TOKEN\" ] || exit 3\n{extra_check}\necho ran \"$1\" >> agent-ran.txt\n"
        ),
    )
    .unwrap();
    make_executable(&stub);
    stub
}

#[cfg(unix)]
fn make_executable(p: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_p: &std::path::Path) {}

// ---------------------------------------------------------------------------
// CLI sync + agent
// ---------------------------------------------------------------------------

/// AC1: no agent block — sync is record-only, exactly one commit, no
/// agent-run record.
#[test]
fn sync_without_agent_block_is_record_only() {
    let repo = Scratch::new("no-agent");
    let before = repo.commit_count();
    let out = repo.tissue(&["sync", "IAM-1", "--summary", "baseline"]);
    assert!(
        out.status.success(),
        "sync failed: {}",
        Scratch::stderr(&out)
    );
    assert_eq!(repo.commit_count(), before + 1, "sync = one commit");
    let stream = repo.agent_stream("IAM-1");
    assert!(stream.contains("\"type\":\"human-sync\""), "{stream}");
    assert!(
        !stream.contains("agent-run"),
        "no agent-run without config: {stream}"
    );
    assert!(
        Scratch::stdout(&out).contains("no agent configured"),
        "stdout should note record-only: {}",
        Scratch::stdout(&out)
    );
}

/// AC2: with a stub agent configured, sync records human-sync, runs the
/// agent (cwd = repo, {issue} substituted, token scrubbed), and records
/// the agent-run outcome — all committed.
#[test]
fn sync_invokes_configured_agent_and_records_outcome() {
    let repo = Scratch::new("agent-ok");
    let stub = write_stub_agent(
        &repo.root,
        "stub.sh",
        "[ \"$1\" = \"sync IAM-1\" ] || exit 4",
    );
    repo.set_config(&format!(
        "agent:\n  command: \"{}\"\n  args: [\"sync {{issue}}\"]\n  timeout_sec: 30\n",
        stub.display()
    ));
    let before = repo.commit_count();
    // The token must not leak into the agent's environment.
    let out = repo.tissue_env(
        &["sync", "IAM-1", "--summary", "human edit"],
        &[("TISSUE_API_TOKEN", "must-not-leak")],
    );
    assert!(
        out.status.success(),
        "sync failed: {}",
        Scratch::stderr(&out)
    );
    assert_eq!(
        repo.commit_count(),
        before + 2,
        "sync + agent-run = two commits"
    );
    let stream = repo.agent_stream("IAM-1");
    assert!(stream.contains("\"type\":\"human-sync\""), "{stream}");
    assert!(stream.contains("\"type\":\"agent-run\""), "{stream}");
    assert!(stream.contains("\"exit_code\":0"), "{stream}");
    assert!(stream.contains("\"timed_out\":false"), "{stream}");
    // The stub ran with the repo as cwd and the substituted arg.
    let ran = std::fs::read_to_string(repo.root.join("agent-ran.txt")).unwrap_or_default();
    assert_eq!(ran.trim(), "ran sync IAM-1", "stub saw arg: {ran}");
    assert!(
        Scratch::stdout(&out).contains("agent run ok"),
        "{}",
        Scratch::stdout(&out)
    );
}

/// AC3: a failing agent does not fail the sync — the failure is recorded.
#[test]
fn failing_agent_is_recorded_not_fatal() {
    let repo = Scratch::new("agent-fail");
    let stub = repo.root.join("fail.sh");
    std::fs::write(&stub, "#!/bin/sh\necho boom >&2\nexit 7\n").unwrap();
    make_executable(&stub);
    repo.set_config(&format!(
        "agent:\n  command: \"{}\"\n  timeout_sec: 30\n",
        stub.display()
    ));
    let out = repo.tissue(&["sync", "IAM-1", "--summary", "edit"]);
    assert!(
        out.status.success(),
        "sync must succeed even when the agent fails: {}",
        Scratch::stderr(&out)
    );
    let stream = repo.agent_stream("IAM-1");
    assert!(stream.contains("\"type\":\"agent-run\""), "{stream}");
    assert!(stream.contains("\"exit_code\":7"), "{stream}");
    assert!(stream.contains("boom"), "stderr tail recorded: {stream}");
    assert!(
        Scratch::stdout(&out).contains("agent run failed"),
        "{}",
        Scratch::stdout(&out)
    );
}

// ---------------------------------------------------------------------------
// /api/config
// ---------------------------------------------------------------------------

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
    let mut child = Command::new(exe)
        .args(["serve", "--port", &port.to_string()])
        .current_dir(&repo.root)
        .env_remove("TISSUE_API_TOKEN")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn tissue serve");
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
    panic!("request failed after retries: {last_err:?}");
}

fn http_once(addr: &SocketAddr, request: &str) -> Result<(String, String), String> {
    let mut stream = TcpStream::connect(addr).map_err(|e| format!("connect: {e}"))?;
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader
        .read_line(&mut status)
        .map_err(|e| format!("status: {e}"))?;
    if status.is_empty() {
        return Err("empty status".into());
    }
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("header: {e}"))?;
        if line.trim().is_empty() {
            break;
        }
    }
    let mut body = String::new();
    reader
        .read_to_string(&mut body)
        .map_err(|e| format!("body: {e}"))?;
    Ok((status, body))
}

fn get(addr: &SocketAddr, path: &str) -> (String, String) {
    http(
        addr,
        &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
    )
}

fn put(addr: &SocketAddr, path: &str, body: &str, token: Option<&str>) -> (String, String) {
    let auth = match token {
        Some(t) => format!("Authorization: Bearer {t}\r\n"),
        None => String::new(),
    };
    http(
        addr,
        &format!(
            "PUT {path} HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
}

fn post(addr: &SocketAddr, path: &str, body: &str, token: Option<&str>) -> (String, String) {
    let auth = match token {
        Some(t) => format!("Authorization: Bearer {t}\r\n"),
        None => String::new(),
    };
    http(
        addr,
        &format!(
            "POST {path} HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
}

/// AC4: GET /api/config returns whitelisted keys but never api_token.
#[test]
fn get_config_never_returns_api_token() {
    let repo = Scratch::new("get-config");
    repo.set_token("super-secret-token");
    let server = start_server(&repo);
    let (status, body) = get(&server.addr, "/api/config");
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    assert!(body.contains("\"project\""), "{body}");
    assert!(!body.contains("api_token"), "{body}");
    assert!(!body.contains("super-secret-token"), "{body}");
}

/// AC4: PUT /api/config rejects api_token and non-whitelisted keys without
/// writing; accepts a whitelisted key with one commit.
#[test]
fn put_config_enforces_whitelist() {
    let repo = Scratch::new("put-config");
    repo.set_token("tok-123");
    let server = start_server(&repo);
    let addr = &server.addr;

    // No token => 401.
    let (status, _) = put(addr, "/api/config", "{\"remote\":\"/x\"}", None);
    assert!(status.contains("401"), "{status}");

    // api_token in the payload => 400, file untouched.
    let (status, body) = put(
        addr,
        "/api/config",
        "{\"api_token\":\"evil\"}",
        Some("tok-123"),
    );
    assert!(status.contains("400"), "{status} {body}");
    assert!(body.contains("api_token"), "{body}");
    assert!(repo.config_yml().contains("tok-123"));
    assert!(!repo.config_yml().contains("evil"));

    // Non-whitelisted key => 400.
    let (status, _) = put(addr, "/api/config", "{\"projects\":{}}", Some("tok-123"));
    assert!(status.contains("400"), "{status}");

    // Whitelisted key => 200, one commit, agent block readable afterwards.
    let before = repo.commit_count();
    let (status, body) = put(
        addr,
        "/api/config",
        "{\"agent\":{\"command\":\"stub-agent\",\"args\":[\"sync {issue}\"],\"timeout_sec\":15},\"actor\":\"human:gui\"}",
        Some("tok-123"),
    );
    assert!(status.contains("200"), "{status} {body}");
    assert_eq!(repo.commit_count(), before + 1, "one commit per PUT");
    let (status, body) = get(addr, "/api/config");
    assert!(status.contains("200"));
    assert!(body.contains("stub-agent"), "{body}");
    assert!(!body.contains("api_token"), "{body}");
}

/// AC2 (API side): POST /api/issues/<id>/sync with an agent configured
/// returns the agent-run outcome; the stream carries both records.
#[test]
fn api_sync_runs_agent_and_reports_outcome() {
    let repo = Scratch::new("api-sync-agent");
    repo.set_token("tok-sync");
    let stub = write_stub_agent(
        &repo.root,
        "stub2.sh",
        "[ \"$1\" = \"sync IAM-1\" ] || exit 4",
    );
    repo.set_config(&format!(
        "agent:\n  command: \"{}\"\n  args: [\"sync {{issue}}\"]\n  timeout_sec: 30\n",
        stub.display()
    ));
    let server = start_server(&repo);
    let addr = &server.addr;

    let (status, body) = post(
        addr,
        "/api/issues/IAM-1/sync",
        "{\"summary\":\"human edit\",\"actor\":\"human:gui\"}",
        Some("tok-sync"),
    );
    assert!(status.contains("201"), "{status} {body}");
    assert!(body.contains("\"synced\":true"), "{body}");
    assert!(body.contains("\"agent_run\""), "{body}");
    assert!(body.contains("\"success\":true"), "{body}");
    let stream = repo.agent_stream("IAM-1");
    assert!(stream.contains("\"type\":\"human-sync\""), "{stream}");
    assert!(stream.contains("\"type\":\"agent-run\""), "{stream}");
    assert!(stream.contains("\"exit_code\":0"), "{stream}");
}

/// AC1 (API side): without an agent block, sync returns agent_run: null.
#[test]
fn api_sync_without_agent_returns_null_outcome() {
    let repo = Scratch::new("api-sync-noagent");
    repo.set_token("tok-sync2");
    let server = start_server(&repo);
    let (status, body) = post(
        &server.addr,
        "/api/issues/IAM-1/sync",
        "{\"summary\":\"baseline\"}",
        Some("tok-sync2"),
    );
    assert!(status.contains("201"), "{status} {body}");
    assert!(body.contains("\"agent_run\":null"), "{body}");
    assert!(!repo.agent_stream("IAM-1").contains("agent-run"));
}
