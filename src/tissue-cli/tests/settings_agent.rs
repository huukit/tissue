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
        common::output(&mut cmd, common::default_timeout()).expect("run tissue binary")
    }

    fn git(&self, args: &[&str]) -> String {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(&self.root);
        let out = common::output(&mut cmd, common::default_timeout()).expect("run git");
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
        .env_remove("TISSUE_API_TOKEN")
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

// ---------------------------------------------------------------------------
// #169: agent HTTP-endpoint mode (STATE.md decision 13) — the key is a
// secret (secrets.yml / env), never in config or the stream; sync POSTs the
// envelope with the bearer header and records the outcome as data.
// ---------------------------------------------------------------------------

/// Mock agent endpoint: accepts exactly one HTTP request, captures the raw
/// request (head + body), and replies with the given status/body.
fn mock_agent_endpoint(
    status: &str,
    resp_body: &str,
) -> (SocketAddr, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind mock");
    let addr = listener.local_addr().expect("addr");
    let status = status.to_string();
    let resp_body = resp_body.to_string();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut raw = String::new();
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("read line");
            if line.trim().is_empty() {
                break;
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                content_length = v.trim().parse().unwrap_or(0);
            }
            raw.push_str(&line);
        }
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).expect("read body");
        raw.push_str(&String::from_utf8_lossy(&body));
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
            resp_body.len()
        );
        stream.write_all(resp.as_bytes()).expect("write resp");
        raw
    });
    (addr, handle)
}

fn write_secrets(repo: &Scratch, content: &str) {
    std::fs::write(repo.root.join(".tissue/secrets.yml"), content).expect("write secrets.yml");
}

/// #169 AC: http mode POSTs the JSON envelope with the bearer key resolved
/// from the gitignored .tissue/secrets.yml; outcome recorded; key never in
/// config, stream, or git.
#[test]
fn http_mode_sync_posts_envelope_with_bearer_key() {
    let repo = Scratch::new("http-sync");
    let (addr, mock) = mock_agent_endpoint("200 OK", "{\"ok\":true}");
    write_secrets(&repo, "mistral: \"test-agent-key-123\"\n");
    assert_eq!(
        repo.git(&["check-ignore", ".tissue/secrets.yml"]).trim(),
        ".tissue/secrets.yml",
        "secrets.yml is gitignored from init"
    );
    repo.set_config(&format!(
        "agent:\n  mode: http\n  endpoint: \"http://{addr}/v1/chat/completions\"\n  model: \"mistral-large-latest\"\n  api_key_ref: mistral\n  timeout_sec: 10\n"
    ));
    let out = repo.tissue(&["sync", "IAM-1", "--summary", "human edit"]);
    assert!(out.status.success(), "sync failed: {out:?}");
    let request = mock.join().expect("mock served one request");
    assert!(
        request.starts_with("POST /v1/chat/completions HTTP/1.1"),
        "request line: {request}"
    );
    assert!(
        request.contains("Authorization: Bearer test-agent-key-123"),
        "bearer header: {request}"
    );
    assert!(
        request.contains("\"issue\":\"IAM-1\""),
        "envelope: {request}"
    );
    assert!(
        request.contains("\"model\":\"mistral-large-latest\""),
        "model in envelope: {request}"
    );
    let stream = repo.agent_stream("IAM-1");
    assert!(stream.contains("\"type\":\"agent-run\""), "{stream}");
    assert!(stream.contains("\"mode\":\"http\""), "{stream}");
    assert!(stream.contains("\"http_status\":200"), "{stream}");
    assert!(
        !stream.contains("test-agent-key-123"),
        "key never recorded: {stream}"
    );
    let cfg = repo.config_yml();
    assert!(!cfg.contains("test-agent-key-123"), "config clean: {cfg}");
    assert!(
        !repo.git(&["ls-files"]).contains("secrets.yml"),
        "secrets.yml never committed"
    );
}

/// #169: a rejected key (401) is recorded as a failed outcome — the sync
/// itself still succeeds (failures are data).
#[test]
fn http_mode_wrong_key_is_recorded_not_fatal() {
    let repo = Scratch::new("http-401");
    let (addr, mock) = mock_agent_endpoint("401 Unauthorized", "{\"error\":\"bad key\"}");
    write_secrets(&repo, "mistral: \"wrong-key\"\n");
    repo.set_config(&format!(
        "agent:\n  mode: http\n  endpoint: \"http://{addr}/v1/x\"\n  api_key_ref: mistral\n  timeout_sec: 10\n"
    ));
    let out = repo.tissue(&["sync", "IAM-1", "--summary", "edit"]);
    assert!(out.status.success(), "sync must not fail: {out:?}");
    let request = mock.join().expect("mock");
    assert!(
        request.contains("Authorization: Bearer wrong-key"),
        "{request}"
    );
    let stream = repo.agent_stream("IAM-1");
    assert!(stream.contains("\"http_status\":401"), "{stream}");
    assert!(
        !stream.contains("wrong-key"),
        "key never recorded: {stream}"
    );
}

/// #169: a missing key produces a clear error record naming only the ref.
#[test]
fn http_mode_missing_key_records_clear_error() {
    let repo = Scratch::new("http-nokey");
    let (addr, _mock) = mock_agent_endpoint("200 OK", "{}");
    repo.set_config(&format!(
        "agent:\n  mode: http\n  endpoint: \"http://{addr}/v1/x\"\n  api_key_ref: mistral\n  timeout_sec: 10\n"
    ));
    let out = repo.tissue_env(
        &["sync", "IAM-1", "--summary", "edit"],
        &[("TISSUE_AGENT_API_KEY", "")],
    );
    assert!(out.status.success(), "{out:?}");
    let stream = repo.agent_stream("IAM-1");
    assert!(stream.contains("not found"), "clear error: {stream}");
    assert!(stream.contains("mistral"), "names the ref: {stream}");
    assert!(!stream.contains("\"http_status\":200"), "{stream}");
}

/// #169: https endpoints are configurable but reported unsupported at run
/// time (this build has no TLS) — recorded as data, sync still succeeds.
#[test]
fn http_mode_https_endpoint_recorded_as_unsupported() {
    let repo = Scratch::new("http-https");
    write_secrets(&repo, "k: v\n");
    repo.set_config(
        "agent:\n  mode: http\n  endpoint: \"https://api.mistral.ai/v1/chat/completions\"\n  api_key_ref: k\n  timeout_sec: 5\n",
    );
    let out = repo.tissue(&["sync", "IAM-1", "--summary", "edit"]);
    assert!(out.status.success(), "{out:?}");
    let stream = repo.agent_stream("IAM-1");
    assert!(stream.contains("not supported"), "{stream}");
}

/// #169 AC2: GET /api/config never returns key material (only the ref);
/// PUT /api/config rejects a literal secret under the agent block; a valid
/// http-mode block stores just the reference. One commit per PUT.
#[test]
fn config_api_never_leaks_agent_key_and_rejects_secret_names() {
    let repo = Scratch::new("cfg-secret");
    repo.set_token("tok-cfg");
    write_secrets(&repo, "mistral: \"super-secret-agent-key\"\n");
    repo.set_config(
        "agent:\n  mode: http\n  endpoint: \"http://127.0.0.1:9/v1/x\"\n  api_key_ref: mistral\n  timeout_sec: 10\n",
    );
    let server = start_server(&repo);
    let addr = &server.addr;

    let (status, body) = get(addr, "/api/config");
    assert!(status.contains("200"), "{status} {body}");
    assert!(body.contains("api_key_ref"), "{body}");
    assert!(
        !body.contains("super-secret-agent-key"),
        "no key material: {body}"
    );

    let before = repo.commit_count();
    let (status, body) = put(
        addr,
        "/api/config",
        "{\"agent\":{\"mode\":\"http\",\"endpoint\":\"http://127.0.0.1:9/v1/x\",\"api_key_ref\":\"mistral\",\"api_key\":\"literal-key\"},\"actor\":\"human:gui\"}",
        Some("tok-cfg"),
    );
    assert!(
        status.contains("400"),
        "literal key rejected: {status} {body}"
    );
    assert!(body.contains("secret"), "{body}");
    assert!(!repo.config_yml().contains("literal-key"), "not stored");
    assert_eq!(repo.commit_count(), before, "rejected PUT commits nothing");

    let (status, body) = put(
        addr,
        "/api/config",
        "{\"agent\":{\"mode\":\"http\",\"endpoint\":\"http://127.0.0.1:9/v1/x\",\"model\":\"m\",\"api_key_ref\":\"mistral\",\"timeout_sec\":10},\"actor\":\"human:gui\"}",
        Some("tok-cfg"),
    );
    assert!(status.contains("200"), "{status} {body}");
    assert_eq!(
        repo.commit_count(),
        before + 1,
        "one commit per accepted PUT"
    );
    let cfg = repo.config_yml();
    assert!(cfg.contains("api_key_ref: mistral"), "{cfg}");
    assert!(cfg.contains("mode: http"), "{cfg}");
    assert!(!cfg.contains("super-secret-agent-key"), "{cfg}");

    // And the read view after the update still leaks nothing.
    let (status, body) = get(addr, "/api/config");
    assert!(status.contains("200"));
    assert!(!body.contains("super-secret-agent-key"), "{body}");
}

/// #169: `tissue init` writes a .gitignore covering .tissue/secrets.yml.
#[test]
fn init_gitignores_secrets_file() {
    let repo = Scratch::new("init-gitignore");
    let gi = std::fs::read_to_string(repo.root.join(".gitignore")).expect(".gitignore at init");
    assert!(gi.contains(".tissue/secrets.yml"), "{gi}");
    assert_eq!(
        repo.git(&["check-ignore", ".tissue/secrets.yml"]).trim(),
        ".tissue/secrets.yml"
    );
}

/// #169 backwards compat: a v0.1.5 command-mode config (no `mode:` key)
/// still parses, validates, and runs.
#[test]
fn v015_command_mode_config_still_works() {
    let repo = Scratch::new("v015-compat");
    let stub = write_stub_agent(&repo.root, "stub3.sh", "cat >/dev/null");
    repo.set_config(&format!(
        "agent:\n  command: \"{}\"\n  args: [\"sync {{issue}}\"]\n  timeout_sec: 30\n",
        stub.display()
    ));
    let out = repo.tissue(&["sync", "IAM-1", "--summary", "legacy config"]);
    assert!(out.status.success(), "{out:?}");
    let stream = repo.agent_stream("IAM-1");
    assert!(stream.contains("\"mode\":\"command\""), "{stream}");
    assert!(stream.contains("\"exit_code\":0"), "{stream}");
}

/// #189: POST /api/agent/test probes the configured agent — command mode
/// success, failing command, no agent configured, and no secret leakage.
#[test]
fn agent_test_endpoint() {
    use std::io::Write as _;
    // command-mode agent that succeeds on --tissue-ping
    let repo = Scratch::new("agent-test-ok");
    repo.set_token("tok");
    let ping = repo.root.join("ping-agent.sh");
    std::fs::write(
        &ping,
        "#!/bin/sh\n[ \"$1\" = \"--tissue-ping\" ] && exit 0\nexit 9\n",
    )
    .unwrap();
    make_executable(&ping);
    let cfg = repo.root.join(".tissue/config.yml");
    let mut content = std::fs::read_to_string(&cfg).unwrap();
    content.push_str(&format!(
        "agent:\n  mode: command\n  command: {}\n  timeout_sec: 5\n",
        ping.display()
    ));
    std::fs::write(&cfg, content).unwrap();

    let server = start_server(&repo);
    let addr = &server.addr;
    let (status, body) = post(addr, "/api/agent/test", "{}", Some("tok"));
    assert!(status.contains("200"), "ok probe: {status} {body}");
    assert!(
        body.contains("\"ok\":true") && body.contains("\"mode\":\"command\""),
        "{body}"
    );
    assert!(
        !body.contains(&repo.root.display().to_string()),
        "no fs path: {body}"
    );
    let _ = std::io::stdout().flush();

    // failing agent -> 502 with sanitized error, no path
    let repo2 = Scratch::new("agent-test-fail");
    repo2.set_token("tok");
    let bad = repo2.root.join("bad-agent.sh");
    std::fs::write(&bad, "#!/bin/sh\necho boom >&2\nexit 3\n").unwrap();
    make_executable(&bad);
    let cfg2 = repo2.root.join(".tissue/config.yml");
    let mut c2 = std::fs::read_to_string(&cfg2).unwrap();
    c2.push_str(&format!(
        "agent:\n  mode: command\n  command: {}\n  timeout_sec: 5\n",
        bad.display()
    ));
    std::fs::write(&cfg2, c2).unwrap();
    let server2 = start_server(&repo2);
    let (status, body) = post(&server2.addr, "/api/agent/test", "{}", Some("tok"));
    assert!(status.contains("502"), "fail probe: {status} {body}");
    assert!(
        body.contains("\"ok\":false") && body.contains("exit code 3"),
        "{body}"
    );
    assert!(body.contains("boom"), "stderr tail shown: {body}");
    assert!(
        !body.contains(&repo2.root.display().to_string()),
        "no fs path: {body}"
    );

    // no agent configured -> 400 with ok:false
    let repo3 = Scratch::new("agent-test-none");
    repo3.set_token("tok");
    let server3 = start_server(&repo3);
    let (status, body) = post(&server3.addr, "/api/agent/test", "{}", Some("tok"));
    assert!(
        status.contains("400") && body.contains("\"ok\":false"),
        "{status} {body}"
    );

    // auth required
    let (status, _) = post(&server3.addr, "/api/agent/test", "{}", None);
    assert!(status.contains("401"), "{status}");
}

#[cfg(unix)]
fn make_executable(p: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}
