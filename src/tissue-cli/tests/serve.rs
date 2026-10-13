//! E2E tests for `tissue serve` (#33): the read-only loopback viewer server.
//!
//! Spawns the server as a subprocess on an ephemeral loopback port against a
//! scratch tissue repo and asserts the #30 acceptance-criteria invariants:
//! loopback-only binding, GET-only (read-only), JSON contract shape, path
//! traversal rejection, and zero mutation of the issue repo while browsing.

use std::io::{BufRead, BufReader, Write};
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
            "tissue-serve-{}-{}-{}",
            label,
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let s = Self { root: root.clone() };
        s.git(&["init", "--quiet"]);
        s.git(&["config", "user.email", "e2e@tissue.test"]);
        s.git(&["config", "user.name", "e2e"]);
        s.tissue(&["init"]);
        s.tissue(&["create", "IAM-1", "First issue", "--body", "## H\n- a"]);
        s.tissue(&["create", "IAM-2", "Second issue", "--body", "b"]);
        // one pending proposal so needs-review surfaces in list
        s.tissue(&[
            "propose",
            "IAM-1",
            "--field",
            "title",
            "--current",
            "First issue",
            "--proposed",
            "Renamed issue",
        ]);
        // one release with one assigned issue (release-detail contract, #97)
        s.tissue(&[
            "release",
            "create",
            "v1.0.0",
            "First cut",
            "--body",
            "## Goals",
        ]);
        s.tissue(&["release", "assign", "IAM-2", "v1.0.0"]);
        s
    }

    fn tissue(&self, args: &[&str]) -> Output {
        let exe: PathBuf = env!("CARGO_BIN_EXE_tissue").into();
        let out = Command::new(exe)
            .args(args)
            .current_dir(&self.root)
            .env_remove("EDITOR")
            .output()
            .expect("run tissue binary");
        assert!(out.status.success(), "tissue {args:?} failed: {out:?}");
        out
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

    fn status_porcelain(&self) -> String {
        let out = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&self.root)
            .output()
            .expect("git status");
        String::from_utf8_lossy(&out.stdout).into_owned()
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

/// Start `tissue serve` on an ephemeral loopback port.
fn start_server(repo: &Scratch) -> Server {
    // Reserve an ephemeral port, release it, and hope the race is kind;
    // retries make this deterministic enough for CI.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("reserve port");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);

    let exe: PathBuf = env!("CARGO_BIN_EXE_tissue").into();
    let mut child = Command::new(exe)
        .args(["serve", "--port", &port.to_string()])
        .current_dir(&repo.root)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn tissue serve");

    // Poll until the server accepts connections.
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    for _ in 0..100 {
        if TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(50)).is_ok() {
            return Server { child, addr };
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if let Ok(Some(status)) = child.try_wait() {
        panic!("tissue serve exited early with {status}");
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("server did not come up on {addr}");
}

/// One raw HTTP request; returns (status_line, headers, body).
///
/// Retries with backoff: under parallel test load the first connect or the
/// status read can fail transiently (refused/reset), which used to flake CI.
fn http(addr: &SocketAddr, request: &str) -> (String, String, String) {
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
    panic!("request to {addr} failed after retries: {:?}", last_err);
}

fn http_once(addr: &SocketAddr, request: &str) -> Result<(String, String, String), String> {
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
        .read_line(&mut body)
        .map_err(|e| format!("read body: {e}"))?;
    Ok((status, headers, body))
}

fn get(addr: &SocketAddr, path: &str) -> (String, String) {
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    let (status, _headers, body) = http(addr, &req);
    (status, body)
}

#[test]
fn serve_is_loopback_only() {
    let scratch = Scratch::new("loopback");
    let server = start_server(&scratch);
    let addr = server.addr;
    assert_eq!(addr.ip(), Ipv4Addr::LOCALHOST, "must bind 127.0.0.1");
    // Non-loopback interface of this host must refuse the connection.
    let host = std::env::var("HOSTNAME").unwrap_or_default();
    if let Ok(addrs) = std::net::ToSocketAddrs::to_socket_addrs(&format!("{host}:{}", addr.port()))
    {
        for a in addrs {
            if a.ip() == Ipv4Addr::LOCALHOST {
                continue;
            }
            let refused =
                TcpStream::connect_timeout(&a, std::time::Duration::from_millis(500)).is_err();
            assert!(refused, "non-loopback {} must refuse", a);
        }
    }
}

#[test]
fn serve_is_read_only_get_only() {
    let scratch = Scratch::new("readonly");
    let server = start_server(&scratch);
    let addr = server.addr;

    // POST outside the API is rejected with 405 (viewer stays read-only).
    let req = "POST /index.html HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\nConnection: close\r\n\r\nx=";
    let (status, _headers, body) = http(&addr, req);
    let body = body.trim();
    assert!(status.contains("405"), "POST must 405: {status}");
    assert!(
        body.contains("GET only") || body.contains("read-only"),
        "405 body: {body}"
    );
    // POST to the agent write API without a configured token is an
    // explicit auth error, not a silent mutation (#80).
    let req = "POST /api/issues HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\nConnection: close\r\n\r\nx=";
    let (status, _headers, body) = http(&addr, req);
    let body = body.trim();
    assert!(
        status.contains("503") || status.contains("401"),
        "unauthenticated API POST must fail explicitly: {status} {body}"
    );
    assert!(
        body.contains("disabled") || body.contains("token"),
        "explicit error body: {body}"
    );

    // DELETE also rejected.
    let req = "DELETE /api/issues/IAM-1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let (status, _, _) = http(&addr, req);
    assert!(status.contains("405"), "DELETE must 405: {status}");

    // A full browse session mutates nothing in the issue repo.
    let before = scratch.status_porcelain();
    for path in [
        "/",
        "/app.js",
        "/style.css",
        "/api/issues",
        "/api/issues/IAM-1",
        "/api/releases",
    ] {
        let (status, _) = get(&addr, path);
        assert!(status.contains("200"), "GET {path}: {status}");
    }
    let after = scratch.status_porcelain();
    assert_eq!(before, after, "browsing must not mutate the repo");
    assert!(after.is_empty(), "scratch repo must be clean: {after}");
}

#[test]
fn serve_json_contract() {
    let scratch = Scratch::new("contract");
    let server = start_server(&scratch);
    let addr = server.addr;

    // List: array of issue objects with the canonical fields.
    let (status, body) = get(&addr, "/api/issues");
    assert!(status.contains("200"), "{status}");
    assert!(body.contains("\"issues\""), "list wrapper: {body}");
    assert!(
        body.contains("\"IAM-1\"") && body.contains("\"IAM-2\""),
        "ids: {body}"
    );
    assert!(
        body.contains("\"needs-review\""),
        "needs-review tag: {body}"
    );
    assert!(
        body.contains("\"status\"") && body.contains("\"created\""),
        "fields: {body}"
    );

    // Detail: issue wrapper with body inside.
    let (status, body) = get(&addr, "/api/issues/IAM-1");
    assert!(status.contains("200"), "{status}");
    assert!(body.contains("\"issue\""), "detail wrapper: {body}");
    assert!(body.contains("## H"), "markdown body: {body}");

    // Unknown issue: JSON 404.
    let (status, body) = get(&addr, "/api/issues/IAM-999");
    assert!(status.contains("404"), "{status}");
    assert!(body.contains("error"), "404 body: {body}");

    // Unknown API route: 404.
    let (status, _) = get(&addr, "/api/nope");
    assert!(status.contains("404"), "{status}");
}

#[test]
fn serve_release_detail_contract() {
    let scratch = Scratch::new("release-detail");
    let server = start_server(&scratch);
    let addr = server.addr;
    // Detail: release front matter + assigned issues (derived from the
    // issues' release links, matching `tissue release show`).
    let (status, body) = get(&addr, "/api/releases/v1.0.0");
    assert!(status.contains("200"), "{status}");
    assert!(body.contains("\"release\""), "wrapper: {body}");
    assert!(body.contains("\"v1.0.0\""), "id: {body}");
    assert!(body.contains("First cut"), "title: {body}");
    assert!(body.contains("\"draft\""), "status: {body}");
    assert!(body.contains("\"issues\""), "assigned wrapper: {body}");
    assert!(
        body.contains("\"IAM-2\"") && body.contains("Second issue"),
        "assigned issue: {body}"
    );
    assert!(
        !body.contains("\"IAM-1\""),
        "unassigned issue must not appear: {body}"
    );
    // Unknown release: JSON 404.
    let (status, body) = get(&addr, "/api/releases/v9.9.9");
    assert!(status.contains("404"), "{status}");
    assert!(body.contains("error"), "404 body: {body}");
    // Traversal-shaped id: refused as not-found (bare v+semver only).
    let (status, _) = get(&addr, "/api/releases/..%2f..%2fetc");
    assert!(status.contains("404"), "traversal id must 404: {status}");
}

#[test]
fn serve_rejects_path_traversal() {
    let scratch = Scratch::new("traversal");
    let server = start_server(&scratch);
    let addr = server.addr;

    for path in [
        "/static/..%2f..%2fsrc/tissue-cli/src/serve.rs",
        "/..%2f.git/config",
        "/api/issues/../../etc/passwd",
    ] {
        let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        let (status, _headers, body) = http(&addr, &req);
        assert!(
            status.contains("404"),
            "traversal {path} must 404, got {status} body {body}"
        );
    }
}
