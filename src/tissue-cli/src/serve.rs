//! `tissue serve`: read-only, loopback-only local viewer server (#30).
//!
//! Wraps the same core scan as the CLI's `list`/`show` commands and exposes
//! a small JSON API plus static files for the GUI. GET-only, 127.0.0.1-only:
//! no mutating controls, no public exposure (AGENTS.md §5).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use tissue_core::human::{IssueFile, ReleaseFile};
use tissue_core::state::Decision;

/// GUI assets embedded at compile time so a standalone binary serves the
/// viewer with no external files (#38 standalone path).
const GUI_INDEX_HTML: &str = include_str!("../../gui/index.html");
const GUI_APP_JS: &str = include_str!("../../gui/app.js");
const GUI_STYLE_CSS: &str = include_str!("../../gui/style.css");

use crate::{issues_dir, tissue_dir};

/// Default port for the loopback viewer.
pub const DEFAULT_PORT: u16 = 7323;

fn releases_dir(repo: &Path) -> PathBuf {
    tissue_dir(repo).join("releases")
}

pub fn serve(
    repo: PathBuf,
    port: u16,
    gui_root: PathBuf,
    project_prefix: Option<&str>,
) -> Result<(), String> {
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
        "tissue viewer (read-only) on http://{addr}/ — repo: {}{}",
        repo.display(),
        match project_prefix {
            Some(p) => format!(" (project prefix {p})"),
            None => String::new(),
        }
    );
    println!("press Ctrl-C to stop");
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                if let Err(e) = handle(s, &repo, &gui_root, project_prefix) {
                    eprintln!("warning: request failed: {e}");
                }
            }
            Err(e) => eprintln!("warning: connection failed: {e}"),
        }
    }
    Ok(())
}

fn handle(
    mut stream: TcpStream,
    repo: &Path,
    gui_root: &Path,
    project_prefix: Option<&str>,
) -> Result<(), String> {
    let mut reader = BufReader::new(&mut stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("reading request line: {e}"))?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    // Drain headers (until empty line) so the connection is reusable,
    // tracking Content-Length so the request body can be drained too —
    // closing with an undrained body sends a TCP RST that can destroy the
    // queued response on the client side.
    let mut content_length: u64 = 0;
    let mut headers = String::new();
    loop {
        let mut h = String::new();
        let n = reader
            .read_line(&mut h)
            .map_err(|e| format!("header: {e}"))?;
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
        headers.push_str(&h);
    }
    let mut body = Vec::new();
    if content_length > 0 {
        body = vec![0u8; content_length as usize];
        reader
            .read_exact(&mut body)
            .map_err(|e| format!("body: {e}"))?;
    }
    let path = target.split('?').next().unwrap_or("/").to_string();

    if let Some(rest) = path.strip_prefix("/api/") {
        return api_route(
            &mut stream,
            &method,
            rest,
            repo,
            &headers,
            &body,
            project_prefix,
        );
    }

    // Outside /api the viewer stays read-only: GET only (405 with JSON error).
    if method != "GET" {
        return respond_json(
            &mut stream,
            405,
            "{\"error\":\"read-only viewer: GET only\"}",
        );
    }

    static_route(&mut stream, &path, gui_root)
}

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

fn api_route(
    stream: &mut TcpStream,
    method: &str,
    rest: &str,
    repo: &Path,
    headers: &str,
    body: &[u8],
    project_prefix: Option<&str>,
) -> Result<(), String> {
    let rest = rest.trim_matches('/');
    // Write endpoints (agent API, #80): POST only, token-protected.
    // A GET on a write route is refused with 405 (write path is POST-only).
    if method == "POST" {
        return api_write_route(stream, rest, repo, headers, body);
    }
    // Reads: GET only, loopback-only, tokenless. A GET on a write-only
    // subroute (records/propose) tells the caller to POST with a token.
    if let Some(t) = rest.strip_prefix("issues/") {
        // records/proposals are GET-readable (#98 timeline/queue); only the
        // POST-only writes (records append, propose, approve, reject) are
        // refused with guidance on GET.
        if matches!(
            t.rsplit_once('/'),
            Some((_, "propose" | "approve" | "reject"))
        ) {
            return respond_json(
                stream,
                405,
                "{\"error\":\"write endpoint: POST with Authorization: Bearer <token>\"}",
            );
        }
    }
    if method != "GET" {
        return respond_json(stream, 405, "{\"error\":\"read-only viewer: GET only\"}");
    }
    if rest == "issues" {
        return api_issues(stream, repo, project_prefix);
    }
    if let Some(tail) = rest.strip_prefix("issues/") {
        if let Some((id, verb)) = tail.split_once('/') {
            match verb {
                "proposals" => return api_issue_proposals(stream, repo, id),
                "records" => return api_issue_records(stream, repo, id),
                _ => {}
            }
        }
    }
    if let Some(id) = rest.strip_prefix("issues/") {
        if id.contains('/') {
            return respond_json(stream, 404, "{\"error\":\"unknown api route\"}");
        }
        return api_issue_detail(stream, repo, id);
    }
    if rest == "releases" {
        return api_releases(stream, repo);
    }
    if let Some(id) = rest.strip_prefix("releases/") {
        return api_release_detail(stream, repo, id);
    }
    respond_json(stream, 404, "{\"error\":\"unknown api route\"}")
}

// ---------------------------------------------------------------------------
// Agent write API (#80): append/propose only, never the human format directly
// (STATE.md decision 4). All writes go through tissue-core so the
// commit-per-mutation audit trail and stream invariants hold.
// ---------------------------------------------------------------------------

/// The repo's API token, read from `.tissue/config.yml` (`api_token:`).
/// Absent/empty means the write API is disabled.
fn api_token(repo: &Path) -> Option<String> {
    // TISSUE_API_TOKEN env var first: untracked, deployment-friendly
    // (e.g. the systemd unit's Environment= line).
    if let Ok(env_token) = std::env::var("TISSUE_API_TOKEN") {
        let t = env_token.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    // Fallback: api_token: in .tissue/config.yml (git-tracked — a real
    // token there lands in history; documented trade-off, see
    // docs/cli-reference.md).
    let config = std::fs::read_to_string(tissue_dir(repo).join("config.yml")).ok()?;
    let line = config
        .lines()
        .find(|l| l.trim_start().starts_with("api_token:"))?;
    let token = line
        .trim_start()
        .strip_prefix("api_token:")?
        .trim()
        .trim_matches('"');
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Constant-time-ish comparison: equal-length check first, then xor-fold.
fn token_matches(expected: &str, provided: &str) -> bool {
    let a = expected.as_bytes();
    let b = provided.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn bearer_token(headers: &str) -> Option<String> {
    headers.lines().find_map(|h| {
        let (name, value) = h.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("authorization") {
            return None;
        }
        let v = value.trim();
        let token = v.strip_prefix("Bearer ")?;
        Some(token.trim().to_string())
    })
}

fn api_write_route(
    stream: &mut TcpStream,
    rest: &str,
    repo: &Path,
    headers: &str,
    body: &[u8],
) -> Result<(), String> {
    // Auth failures are explicit and never log the token (AC4).
    let Some(token) = api_token(repo) else {
        return respond_json(
            stream,
            503,
            "{\"error\":\"write API disabled: no api_token in .tissue/config.yml\"}",
        );
    };
    match bearer_token(headers) {
        Some(provided) if token_matches(&token, &provided) => {}
        Some(_) => {
            return respond_json(stream, 401, "{\"error\":\"invalid or expired token\"}");
        }
        None => {
            return respond_json(
                stream,
                401,
                "{\"error\":\"missing Authorization: Bearer <token> header\"}",
            );
        }
    }
    let payload = match std::str::from_utf8(body) {
        Ok(s) => s,
        Err(_) => {
            return respond_json(
                stream,
                400,
                "{\"error\":\"request body must be UTF-8 JSON\"}",
            )
        }
    };
    let rest = rest.trim_matches('/');
    if rest == "issues" {
        return api_write_create_issue(stream, repo, payload);
    }
    if let Some(tail) = rest.strip_prefix("issues/") {
        let Some((id, verb)) = tail.split_once('/') else {
            return respond_json(stream, 404, "{\"error\":\"unknown api route\"}");
        };
        match verb {
            "records" => return api_write_append_record(stream, repo, id, payload),
            "propose" => return api_write_propose(stream, repo, id, payload),
            "approve" => return api_write_decide(stream, repo, id, payload, Decision::Approve),
            "reject" => return api_write_decide(stream, repo, id, payload, Decision::Reject),
            _ => {
                return respond_json(stream, 404, "{\"error\":\"unknown api route\"}");
            }
        }
    }
    respond_json(stream, 404, "{\"error\":\"unknown api route\"}")
}

fn json_field<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

fn parse_json_body(payload: &str) -> Result<serde_json::Value, u16> {
    serde_json::from_str(payload).map_err(|_| 400u16)
}

/// POST /api/issues  {"id": "IAM-9", "title": "...", "actor": "agent:dev-1"}
/// Creates an issue (human format untouched by the agent afterwards).
fn api_write_create_issue(
    stream: &mut TcpStream,
    repo: &Path,
    payload: &str,
) -> Result<(), String> {
    let v =
        match parse_json_body(payload) {
            Ok(v) => v,
            Err(code) => return respond_json(
                stream,
                code,
                "{\"error\":\"request body must be a JSON object: {\"id\",\"title\",\"actor\"}\"}",
            ),
        };
    let (Some(id), Some(title)) = (json_field(&v, "id"), json_field(&v, "title")) else {
        return respond_json(
            stream,
            400,
            "{\"error\":\"body must have string fields: id, title, (optional) actor\"}",
        );
    };
    let actor_id = json_field(&v, "actor").unwrap_or("agent:api");
    match crate::api_write::create_issue(repo, id, title, actor_id) {
        Ok(()) => respond_json(
            stream,
            201,
            &format!(
                "{{\"created\":{},\"title\":{}}}",
                json_str(id),
                json_str(title)
            ),
        ),
        Err(e) => respond_json(stream, 400, &format!("{{\"error\":{}}}", json_str(&e))),
    }
}

/// POST /api/issues/<id>/records  {"type": "observation", "data": {...}, "actor": "agent:dev-1"}
/// Appends an agent-writable record to the issue's stream.
fn api_write_append_record(
    stream: &mut TcpStream,
    repo: &Path,
    id: &str,
    payload: &str,
) -> Result<(), String> {
    let v =
        match parse_json_body(payload) {
            Ok(v) => v,
            Err(code) => return respond_json(
                stream,
                code,
                "{\"error\":\"request body must be a JSON object: {\"type\",\"data\",\"actor\"}\"}",
            ),
        };
    let actor_id = json_field(&v, "actor").unwrap_or("agent:api");
    let Some(data) = v.get("data") else {
        return respond_json(stream, 400, "{\"error\":\"body must have a data object\"}");
    };
    match crate::api_write::append_record(repo, id, actor_id, data) {
        Ok(seq) => respond_json(stream, 201, &format!("{{\"appended\":true,\"seq\":{seq}}}")),
        Err(e) => respond_json(stream, 400, &format!("{{\"error\":{}}}", json_str(&e))),
    }
}

/// GET /api/issues/<id>/proposals: pending proposals with review context.
/// Read-only; serves the GUI Needs Review queue (#98).
fn api_issue_proposals(stream: &mut TcpStream, repo: &Path, id: &str) -> Result<(), String> {
    match crate::api_write::pending_proposals(repo, id) {
        Ok(proposals) => {
            let body = serde_json::to_string(&proposals).unwrap_or_else(|_| "[]".into());
            respond_json(stream, 200, &body)
        }
        Err(e) => respond_json(stream, 404, &format!("{{\"error\":{}}}", json_str(&e))),
    }
}

/// GET /api/issues/<id>/records: the agent stream for the timeline view.
/// Read-only (#98).
fn api_issue_records(stream: &mut TcpStream, repo: &Path, id: &str) -> Result<(), String> {
    match crate::api_write::records(repo, id) {
        Ok(records) => {
            let body = serde_json::to_string(&records).unwrap_or_else(|_| "[]".into());
            respond_json(stream, 200, &body)
        }
        Err(e) => respond_json(stream, 404, &format!("{{\"error\":{}}}", json_str(&e))),
    }
}

/// POST /api/issues/<id>/approve and /reject: decide a proposal through
/// the core loop — one commit, stream records identical to the CLI path.
/// Token-protected like every write endpoint (#98).
fn api_write_decide(
    stream: &mut TcpStream,
    repo: &Path,
    id: &str,
    payload: &str,
    decision: Decision,
) -> Result<(), String> {
    let v = match parse_json_body(payload) {
        Ok(v) => v,
        Err(code) => {
            return respond_json(
                stream,
                code,
                "{\"error\":\"request body must be a JSON object: {\"proposal_id\",\"actor\",(reject)\"reason\"}\"}",
            )
        }
    };
    let Some(proposal_id) = json_field(&v, "proposal_id") else {
        return respond_json(
            stream,
            400,
            "{\"error\":\"body must have string fields: proposal_id, (reject) reason, actor\"}",
        );
    };
    let actor_id = json_field(&v, "actor").unwrap_or("human:gui");
    let reason = json_field(&v, "reason");
    if decision == Decision::Reject && reason.is_none() {
        return respond_json(
            stream,
            400,
            "{\"error\":\"rejections are data: a reason is required\"}",
        );
    }
    match crate::api_write::decide(repo, id, proposal_id, decision, reason, actor_id) {
        Ok(commit) => respond_json(
            stream,
            201,
            &format!(
                "{{\"decision\":{},\"proposal_id\":{},\"commit\":{}}}",
                json_str(if decision == Decision::Approve {
                    "approved"
                } else {
                    "rejected"
                }),
                json_str(proposal_id),
                json_str(&commit)
            ),
        ),
        Err(e) => respond_json(stream, 400, &format!("{{\"error\":{}}}", json_str(&e))),
    }
}

/// POST /api/issues/<id>/propose
/// {"field":"title","current":"...","proposed":"...","rationale":"...","actor":"agent:dev-1"}
/// Creates a proposal via the core loop: tags needs-review, one commit;
/// the human format is never written directly.
fn api_write_propose(
    stream: &mut TcpStream,
    repo: &Path,
    id: &str,
    payload: &str,
) -> Result<(), String> {
    let v = match parse_json_body(payload) {
        Ok(v) => v,
        Err(code) => {
            return respond_json(
                stream,
                code,
                "{\"error\":\"request body must be a JSON object: {\"field\",\"current\",\"proposed\",\"rationale\",\"actor\"}\"}",
            )
        }
    };
    let (Some(field), Some(current), Some(proposed)) = (
        json_field(&v, "field"),
        json_field(&v, "current"),
        json_field(&v, "proposed"),
    ) else {
        return respond_json(
            stream,
            400,
            "{\"error\":\"body must have string fields: field, current, proposed, (optional) rationale, actor\"}",
        );
    };
    let rationale = json_field(&v, "rationale").unwrap_or("");
    let actor_id = json_field(&v, "actor").unwrap_or("agent:api");
    match crate::api_write::propose(repo, id, field, current, proposed, rationale, actor_id) {
        Ok(proposal_id) => respond_json(
            stream,
            201,
            &format!(
                "{{\"proposal_id\":{},\"state\":\"proposed\",\"tagged\":\"needs-review\"}}",
                json_str(&proposal_id)
            ),
        ),
        Err(e) => respond_json(stream, 400, &format!("{{\"error\":{}}}", json_str(&e))),
    }
}

fn scan_issues(repo: &Path, project_prefix: Option<&str>) -> Vec<IssueFile> {
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
            if let Some(p) = project_prefix {
                if !issue.id.starts_with(&format!("{p}-")) {
                    continue;
                }
            }
            issues.push(issue);
        }
    }
    issues.sort_by(|a, b| a.id.cmp(&b.id));
    issues
}

fn issue_summary_json(issue: &IssueFile) -> String {
    format!(
        "{{\"id\":{},\"title\":{},\"status\":\"{}\",\"release\":{},\"tags\":{},\"created\":{},\"updated\":{}}}",
        json_str(&issue.id),
        json_str(&issue.title),
        issue.status.as_str(),
        match &issue.release {
            Some(r) => json_str(r),
            None => "null".to_string(),
        },
        json_list(&issue.tags),
        json_str(&issue.created),
        json_str(&issue.updated),
    )
}

fn api_issues(
    stream: &mut TcpStream,
    repo: &Path,
    project_prefix: Option<&str>,
) -> Result<(), String> {
    let issues = scan_issues(repo, project_prefix);
    let items: Vec<String> = issues.iter().map(issue_summary_json).collect();
    let body = format!("{{\"issues\":[{}]}}", items.join(","));
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
    // Body inside the issue object (canonical contract, PR #31):
    // {"issue": {<summary fields>, "body": "raw markdown"}}
    let mut summary = issue_summary_json(&issue);
    summary.pop();
    let body = "{\"issue\":".to_string() + &summary + ",\"body\":" + &json_str(&issue.body) + "}}";
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

/// GET /api/releases/{id}: release front matter + assigned issues
/// (derived from issues' front-matter release links, matching the CLI's
/// `release show` derivation via tissue-core::releases). Read-only.
fn api_release_detail(stream: &mut TcpStream, repo: &Path, id: &str) -> Result<(), String> {
    // Path-traversal safety: reject ids that are not a bare v+semver token.
    if !tissue_core::human::valid_release_id(id) {
        return respond_json(
            stream,
            404,
            &format!("{{\"error\":\"release not found: {}\"}}", id),
        );
    }
    let grepo = tissue_core::git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let releases = tissue_core::releases::Releases::new(grepo);
    let rel = match releases.load(id) {
        Ok(r) => r,
        Err(_) => {
            return respond_json(
                stream,
                404,
                &format!("{{\"error\":\"release not found: {}\"}}", id),
            )
        }
    };
    let assigned = releases
        .assigned_issues(id)
        .map_err(|e| format!("deriving assigned issues: {e}"))
        .unwrap_or_default();
    let issues: Vec<String> = assigned
        .iter()
        .map(|i| {
            format!(
                "{{\"id\":{},\"title\":{},\"status\":\"{}\"}}",
                json_str(&i.id),
                json_str(&i.title),
                i.status.as_str()
            )
        })
        .collect();
    let target = rel
        .target
        .as_deref()
        .map(|t| format!("\"target\":{},", json_str(t)))
        .unwrap_or_default();
    let body = format!(
        "{{\"release\":{{\"id\":{},\"title\":{},\"status\":\"{}\",{target}\"created\":{},\"updated\":{}}},\"issues\":[{}]}}",
        json_str(&rel.id),
        json_str(&rel.title),
        rel.status.as_str(),
        json_str(&rel.created),
        json_str(&rel.updated),
        issues.join(",")
    );
    respond_json(stream, 200, &body)
}

// ---------------------------------------------------------------------------
// Static files (GUI)
// ---------------------------------------------------------------------------

fn static_route(stream: &mut TcpStream, path: &str, gui_root: &Path) -> Result<(), String> {
    let _ = gui_root;
    let rel = path.trim_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    // Reject traversal: only simple, separator-free names under the root.
    if rel
        .split('/')
        .any(|seg| seg.is_empty() || seg == ".." || seg.starts_with('.'))
    {
        return respond_json(stream, 404, "{\"error\":\"not found\"}");
    }
    let (bytes, ctype) = match rel {
        "index.html" => (GUI_INDEX_HTML.as_bytes(), "text/html; charset=utf-8"),
        "app.js" => (
            GUI_APP_JS.as_bytes(),
            "application/javascript; charset=utf-8",
        ),
        "style.css" => (GUI_STYLE_CSS.as_bytes(), "text/css; charset=utf-8"),
        _ => return respond_json(stream, 404, "{\"error\":\"not found\"}"),
    };
    write_response(stream, 200, ctype, bytes)
}

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
    fn issue_summary_has_no_needs_review_field() {
        let now = "2026-10-08T09:00:00Z";
        let mut issue = IssueFile::new(
            "IAM-1",
            "Title",
            tissue_core::human::IssueStatus::Open,
            "b",
            now,
        )
        .unwrap();
        assert!(!issue_summary_json(&issue).contains("needsReview"));
        issue.add_tag(tissue_core::human::NEEDS_REVIEW, now);
        // The tag is the single source of truth; no derived boolean field.
        assert!(issue_summary_json(&issue).contains("\"needs-review\""));
        assert!(!issue_summary_json(&issue).contains("needsReview"));
    }
}
