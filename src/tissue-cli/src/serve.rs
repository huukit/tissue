//! `tissue serve`: read-only, loopback-only local viewer server (#30).
//!
//! Wraps the same core scan as the CLI's `list`/`show` commands and exposes
//! a small JSON API plus static files for the GUI. GET-only, 127.0.0.1-only:
//! no mutating controls, no public exposure (AGENTS.md §5).

use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use tissue_core::human::{IssueFile, ReleaseFile};

use crate::{issues_dir, tissue_dir};

/// Default port for the loopback viewer.
pub const DEFAULT_PORT: u16 = 7323;

fn releases_dir(repo: &Path) -> PathBuf {
    tissue_dir(repo).join("releases")
}

pub fn serve(repo: PathBuf, port: u16, gui_root: PathBuf) -> Result<(), String> {
    if !tissue_dir(&repo).is_dir() {
        return Err(format!(
            "not a tissue repository (no `{}` in {})",
            tissue_dir(&repo).display(),
            repo.display()
        ));
    }
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let listener = TcpListener::bind(addr).map_err(|e| format!("binding {addr}: {e}"))?;
    println!(
        "tissue viewer (read-only) on http://{addr}/ — repo: {}",
        repo.display()
    );
    println!("press Ctrl-C to stop");
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                if let Err(e) = handle(s, &repo, &gui_root) {
                    eprintln!("warning: request failed: {e}");
                }
            }
            Err(e) => eprintln!("warning: connection failed: {e}"),
        }
    }
    Ok(())
}

fn handle(mut stream: TcpStream, repo: &Path, gui_root: &Path) -> Result<(), String> {
    let mut reader = BufReader::new(&mut stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("reading request line: {e}"))?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    // Drain headers (until empty line) so the connection is reusable.
    loop {
        let mut h = String::new();
        let n = reader
            .read_line(&mut h)
            .map_err(|e| format!("header: {e}"))?;
        if n == 0 || h.trim().is_empty() {
            break;
        }
    }
    let path = target.split('?').next().unwrap_or("/").to_string();

    // Read-only: refuse anything but GET (405 with JSON error).
    if method != "GET" {
        return respond_json(
            &mut stream,
            405,
            "{\"error\":\"read-only viewer: GET only\"}",
        );
    }

    if let Some(rest) = path.strip_prefix("/api/") {
        return api_route(&mut stream, rest, repo);
    }
    static_route(&mut stream, &path, gui_root)
}

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

fn api_route(stream: &mut TcpStream, rest: &str, repo: &Path) -> Result<(), String> {
    let rest = rest.trim_matches('/');
    if rest == "issues" {
        return api_issues(stream, repo);
    }
    if let Some(id) = rest.strip_prefix("issues/") {
        return api_issue_detail(stream, repo, id);
    }
    if rest == "releases" {
        return api_releases(stream, repo);
    }
    respond_json(stream, 404, "{\"error\":\"unknown api route\"}")
}

fn scan_issues(repo: &Path) -> Vec<IssueFile> {
    let mut issues = Vec::new();
    let Ok(rd) = std::fs::read_dir(issues_dir(repo)) else {
        return issues;
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Ok(issue) = IssueFile::from_path(&path.join("issue.md")) {
            issues.push(issue);
        }
    }
    issues.sort_by(|a, b| a.id.cmp(&b.id));
    issues
}

fn issue_summary_json(issue: &IssueFile) -> String {
    format!(
        "{{\"id\":{},\"title\":{},\"status\":\"{}\",\"release\":{},\"tags\":{},\"needsReview\":{},\"created\":{},\"updated\":{}}}",
        json_str(&issue.id),
        json_str(&issue.title),
        issue.status.as_str(),
        match &issue.release {
            Some(r) => json_str(r),
            None => "null".to_string(),
        },
        json_list(&issue.tags),
        issue.needs_review(),
        json_str(&issue.created),
        json_str(&issue.updated),
    )
}

fn api_issues(stream: &mut TcpStream, repo: &Path) -> Result<(), String> {
    let issues = scan_issues(repo);
    let items: Vec<String> = issues.iter().map(issue_summary_json).collect();
    let body = format!("[{}]", items.join(","));
    respond_json(stream, 200, &body)
}

fn api_issue_detail(stream: &mut TcpStream, repo: &Path, id: &str) -> Result<(), String> {
    let path = issues_dir(repo).join(id).join("issue.md");
    let Ok(issue) = IssueFile::from_path(&path) else {
        return respond_json(
            stream,
            404,
            &format!("{{\"error\":\"issue not found: {}\"}}", id),
        );
    };
    let body = format!(
        "{{\"issue\":{},\"body\":{}}}",
        issue_summary_json(&issue),
        json_str(&issue.body)
    );
    respond_json(stream, 200, &body)
}

fn api_releases(stream: &mut TcpStream, repo: &Path) -> Result<(), String> {
    let mut releases = Vec::new();
    if let Ok(rd) = std::fs::read_dir(releases_dir(repo)) {
        for entry in rd.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Ok(rel) = ReleaseFile::from_path(&path.join("release.md")) {
                releases.push(format!(
                    "{{\"id\":{},\"title\":{},\"status\":\"{}\",\"created\":{},\"updated\":{}}}",
                    json_str(&rel.id),
                    json_str(&rel.title),
                    rel.status.as_str(),
                    json_str(&rel.created),
                    json_str(&rel.updated),
                ));
            }
        }
    }
    let body = format!("[{}]", releases.join(","));
    respond_json(stream, 200, &body)
}

// ---------------------------------------------------------------------------
// Static files (GUI)
// ---------------------------------------------------------------------------

fn static_route(stream: &mut TcpStream, path: &str, gui_root: &Path) -> Result<(), String> {
    let rel = path.trim_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    // Reject traversal: only simple, separator-free names under the root.
    if rel
        .split('/')
        .any(|seg| seg.is_empty() || seg == ".." || seg.starts_with('.'))
    {
        return respond_json(stream, 404, "{\"error\":\"not found\"}");
    }
    let file = gui_root.join(rel);
    let Ok(bytes) = std::fs::read(&file) else {
        return respond_json(stream, 404, "{\"error\":\"not found\"}");
    };
    let ctype = match file.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    };
    write_response(stream, 200, ctype, &bytes)
}

// ---------------------------------------------------------------------------
// HTTP plumbing
// ---------------------------------------------------------------------------

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    ctype: &str,
    body: &[u8],
) -> Result<(), String> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|e| format!("writing response: {e}"))
}

fn respond_json(stream: &mut TcpStream, status: u16, body: &str) -> Result<(), String> {
    write_response(stream, status, "application/json", body.as_bytes())
}

/// JSON-encode a string scalar (escaping per RFC 8259 basics).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_list(items: &[String]) -> String {
    let parts: Vec<String> = items.iter().map(|s| json_str(s)).collect();
    format!("[{}]", parts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_str_escapes_control_chars() {
        assert_eq!(json_str("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
        assert_eq!(json_str("\u{1}"), "\"\\u0001\"");
    }

    #[test]
    fn json_list_encodes_each_item() {
        assert_eq!(json_list(&[]), "[]");
        assert_eq!(
            json_list(&["a".to_string(), "b".to_string()]),
            "[\"a\",\"b\"]"
        );
    }

    #[test]
    fn issue_summary_marks_needs_review() {
        let now = "2026-10-08T09:00:00Z";
        let mut issue = IssueFile::new(
            "IAM-1",
            "Title",
            tissue_core::human::IssueStatus::Open,
            "b",
            now,
        )
        .unwrap();
        assert!(!issue_summary_json(&issue).contains("\"needsReview\":true"));
        issue.add_tag(tissue_core::human::NEEDS_REVIEW, now);
        assert!(issue_summary_json(&issue).contains("\"needsReview\":true"));
    }
}
