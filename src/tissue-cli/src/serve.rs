//! `tissue serve`: read-only, loopback-only local viewer server (#30).
//!
//! Wraps the same core scan as the CLI's `list`/`show` commands and exposes
//! a small JSON API plus static files for the GUI. GET-only, 127.0.0.1-only:
//! no mutating controls, no public exposure (AGENTS.md §5).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use tissue_core::human::{IssueFile, IssueStatus, ReleaseFile};
use tissue_core::state::Decision;

/// GUI assets embedded at compile time so a standalone binary serves the
/// viewer with no external files (#38 standalone path).
const GUI_INDEX_HTML_RAW: &str = include_str!("../../gui/index.html");
/// #143: the served index.html with the compiled-in version baked into the
/// footer fallback (placeholder `__TISSUE_VERSION__` replaced once).
static GUI_INDEX_HTML: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn gui_index_html() -> &'static str {
    GUI_INDEX_HTML
        .get_or_init(|| GUI_INDEX_HTML_RAW.replace("__TISSUE_VERSION__", tissue_core::VERSION))
}
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
    // #120: with no token resolvable, GUI write actions and agent POSTs
    // 401 with no startup hint. Reads are unaffected either way.
    if !write_auth_configured(&repo) {
        eprintln!(
            "note: no API token configured (tissue token issue, or TISSUE_API_TOKEN env) — the GUI and agent write API are read-only until one is set"
        );
    }
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
    // #174: token management API — every verb is bearer-authed (the list is
    // masked; only POST /api/tokens returns the secret, exactly once).
    if rest == "tokens" || rest.starts_with("tokens/") {
        return api_tokens_route(stream, method, rest, repo, headers, body);
    }
    // Write endpoints (agent API, #80): POST only, token-protected.
    // A GET on a write route is refused with 405 (write path is POST-only).
    if method == "POST" {
        return api_write_route(stream, rest, repo, headers, body);
    }
    // #137: PUT is the config-update verb (whitelisted keys only).
    if method == "PUT" {
        return api_put_route(stream, rest, repo, headers, body);
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
    if rest == "projects" {
        return api_projects(stream, repo);
    }
    // #137: whitelisted, secret-free config view (api_token never returned).
    if rest == "config" {
        return api_config_get(stream, repo);
    }
    // #143: product/version/license/API-version metadata (tokenless read).
    if rest == "meta" {
        return api_meta(stream);
    }
    if rest == "releases" {
        return api_releases(stream, repo);
    }
    if let Some(id) = rest.strip_prefix("releases/") {
        return api_release_detail(stream, repo, id);
    }
    respond_json(stream, 404, "{\"error\":\"unknown api route\"}")
}

/// GET /api/meta (#143): product metadata — tokenless read like every GET.
/// Shape: {"product":"tissue","version":"…","api_version":"v1",
/// "license":"GPL-3.0-only","license_url":"…","build":{"target":"…","profile":"release"}}.
fn api_meta(stream: &mut TcpStream) -> Result<(), String> {
    let body = format!(
        "{{\"product\":\"tissue\",\"version\":{},\"api_version\":{},\"license\":{},\"license_url\":{},\"build\":{{\"target\":{},\"profile\":\"release\"}}}}",
        json_str(tissue_core::VERSION),
        json_str(tissue_core::API_VERSION),
        json_str(tissue_core::LICENSE),
        json_str("https://github.com/huukit/tissue/blob/main/LICENSE"),
        json_str(tissue_core::BUILD_TARGET),
    );
    respond_json(stream, 200, &body)
}

/// GET /api/projects — the configured project mapping (tokenless read,
/// same as every GET). Shape: {"default": "...", "projects": [...]}.
fn api_projects(stream: &mut TcpStream, repo: &Path) -> Result<(), String> {
    let grepo = tissue_core::git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let projects = tissue_core::projects::Projects::new(grepo);
    let list = projects.list()?;
    let default = projects.default_name()?;
    let items: Vec<String> = list
        .iter()
        .map(|(name, prefix)| {
            format!(
                "{{\"name\":{},\"prefix\":{},\"default\":{}}}",
                json_str(name),
                json_str(prefix),
                if *name == default { "true" } else { "false" }
            )
        })
        .collect();
    respond_json(
        stream,
        200,
        &format!(
            "{{\"default\":{},\"projects\":[{}]}}",
            json_str(&default),
            items.join(",")
        ),
    )
}

/// GET /api/config (#137): the whitelisted view of `.tissue/config.yml` —
/// tokenless read like every GET. `api_token` is never included.
fn api_config_get(stream: &mut TcpStream, repo: &Path) -> Result<(), String> {
    match tissue_core::config::whitelisted_view(repo) {
        Ok(view) => {
            let body = serde_json::to_string(&view).unwrap_or_else(|_| "{}".into());
            respond_json(stream, 200, &body)
        }
        Err(e) => respond_json(stream, 400, &format!("{{\"error\":{}}}", json_str(&e))),
    }
}

/// PUT /api/config (#137): update whitelisted top-level keys of
/// `.tissue/config.yml` — token-protected, one commit per change.
/// `api_token` and any non-whitelisted key are rejected (400) before
/// anything is written.
fn api_put_route(
    stream: &mut TcpStream,
    rest: &str,
    repo: &Path,
    headers: &str,
    body: &[u8],
) -> Result<(), String> {
    if rest != "config" {
        return respond_json(stream, 404, "{\"error\":\"unknown api route\"}");
    }
    // #159: same token resolution as every write route (env override →
    // managed hashes → legacy line).
    if !write_auth_configured(repo) {
        return respond_json(
            stream,
            503,
            "{\"error\":\"write API disabled: no token configured (tissue token issue, or TISSUE_API_TOKEN env)\"}",
        );
    }
    match bearer_token(headers) {
        Some(provided) if resolve_token(repo, &provided).is_some() => {}
        Some(_) => {
            return respond_json(stream, 401, "{\"error\":\"invalid or revoked token\"}");
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
    let v = match parse_json_body(payload) {
        Ok(v) => v,
        Err(_) => {
            return respond_json(
                stream,
                400,
                "{\"error\":\"request body must be a JSON object of top-level config keys\"}",
            )
        }
    };
    let Some(obj) = v.as_object() else {
        return respond_json(
            stream,
            400,
            "{\"error\":\"request body must be a JSON object of top-level config keys\"}",
        );
    };
    let mut updates = std::collections::BTreeMap::new();
    for (k, val) in obj {
        if k == "actor" {
            continue; // metadata, not a config key
        }
        updates.insert(k.clone(), val.clone());
    }
    let actor_id = obj
        .get("actor")
        .and_then(|a| a.as_str())
        .unwrap_or("human:gui");
    let grepo = tissue_core::git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let author = tissue_core::git::Author {
        name: actor_id.to_string(),
        email: format!("{actor_id}@tissue.local"),
    };
    match tissue_core::config::apply_whitelisted_update(&grepo, &updates, &author) {
        Ok(()) => respond_json(stream, 200, "{\"updated\":true}"),
        Err(e) => respond_json(stream, 400, &format!("{{\"error\":{}}}", json_str(&e))),
    }
}

/// POST /api/projects {"name": "...", "prefix": "...", "actor"?} —
/// create a project through the same core module as the CLI (#135):
/// one validation, one commit path, no drift.
fn api_write_create_project(
    stream: &mut TcpStream,
    repo: &Path,
    payload: &str,
) -> Result<(), String> {
    let v = match parse_json_body(payload) {
        Ok(v) => v,
        Err(code) => return respond_json(
            stream,
            code,
            "{\"error\":\"request body must be a JSON object: {\"name\",\"prefix\",(\"actor\")}\"}",
        ),
    };
    let (Some(name), Some(prefix)) = (json_field(&v, "name"), json_field(&v, "prefix")) else {
        return respond_json(
            stream,
            400,
            "{\"error\":\"body must have string fields: name, prefix\"}",
        );
    };
    let actor_id = json_field(&v, "actor").unwrap_or("human:gui");
    let grepo = tissue_core::git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let author = tissue_core::git::Author {
        name: actor_id.to_string(),
        email: format!("{actor_id}@tissue.local"),
    };
    match tissue_core::projects::Projects::new(grepo).add(name, prefix, &author) {
        // #175: return the created project so the GUI can select it without
        // a re-read race.
        Ok(()) => respond_json(
            stream,
            201,
            &format!(
                "{{\"created\":true,\"project\":{{\"name\":{},\"prefix\":{},\"default\":false}}}}",
                json_str(name),
                json_str(prefix)
            ),
        ),
        // Conflicts (duplicate prefix, name remap, invalid input) are 409s
        // for existing state, 400s for bad input — both clean JSON.
        Err(
            e @ (tissue_core::projects::ProjectError::DuplicatePrefix { .. }
            | tissue_core::projects::ProjectError::NameMapsToOtherPrefix { .. }),
        ) => respond_json(
            stream,
            409,
            &format!("{{\"error\":{}}}", json_str(&e.to_string())),
        ),
        Err(e) => respond_json(
            stream,
            400,
            &format!("{{\"error\":{}}}", json_str(&e.to_string())),
        ),
    }
}

/// POST /api/releases {"id": "v1.2.3", "title": "...", ("body", "actor")?} —
/// create a release through the same core path as `tissue release create`
/// (#81/#173): one validation, one commit, no CLI/API drift.
fn api_write_create_release(
    stream: &mut TcpStream,
    repo: &Path,
    payload: &str,
) -> Result<(), String> {
    let v = match parse_json_body(payload) {
        Ok(v) => v,
        Err(code) => return respond_json(
            stream,
            code,
            "{\"error\":\"request body must be a JSON object: {\"id\",\"title\",(\"body\",\"actor\")}\"}",
        ),
    };
    let (Some(id), Some(title)) = (json_field(&v, "id"), json_field(&v, "title")) else {
        return respond_json(
            stream,
            400,
            "{\"error\":\"body must have string fields: id, title\"}",
        );
    };
    let body = json_field(&v, "body").unwrap_or("");
    let actor_id = json_field(&v, "actor").unwrap_or("human:gui");
    let grepo = tissue_core::git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let author = tissue_core::git::Author {
        name: actor_id.to_string(),
        email: format!("{actor_id}@tissue.local"),
    };
    match tissue_core::releases::Releases::new(grepo).create(
        id,
        title,
        body,
        &crate::now_utc(),
        &author,
    ) {
        Ok(()) => respond_json(
            stream,
            201,
            &format!(
                "{{\"created\":{},\"title\":{}}}",
                json_str(id),
                json_str(title)
            ),
        ),
        // Duplicate release is a state conflict; bad id/title is bad input.
        Err(tissue_core::releases::Error::Conflict(m)) => {
            respond_json(stream, 409, &format!("{{\"error\":{}}}", json_str(&m)))
        }
        Err(e) => respond_json(
            stream,
            400,
            &format!("{{\"error\":{}}}", json_str(&e.to_string())),
        ),
    }
}

// ---------------------------------------------------------------------------
// Agent write API (#80): append/propose only, never the human format directly
// (STATE.md decision 4). All writes go through tissue-core so the
// commit-per-mutation audit trail and stream invariants hold.
// ---------------------------------------------------------------------------

/// The repo's API token inputs for write auth (#159): the
/// TISSUE_API_TOKEN env override first (untracked, deployment-friendly),
/// then the managed hash-only `tokens:` entries plus a legacy plaintext
/// `api_token:` line in config.yml (kept working; `tissue token list`
/// reports it as a migration item). Absent/empty everywhere means the
/// write API is disabled.
fn env_token_override() -> Option<String> {
    std::env::var("TISSUE_API_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn token_store(repo: &Path) -> Option<tissue_core::tokens::TokenStore> {
    tissue_core::git::Repo::open(repo)
        .ok()
        .map(tissue_core::tokens::TokenStore::new)
}

/// Resolve a presented bearer token to a label, or None when it matches no
/// valid token (env override, managed hash entry, or legacy plaintext line).
/// All comparisons are constant-time (tissue-core::tokens).
fn resolve_token(repo: &Path, presented: &str) -> Option<String> {
    let env = env_token_override();
    if let Some(store) = token_store(repo) {
        if let Some(entry) = store.resolve(presented, env.as_deref()) {
            return Some(entry.label);
        }
    }
    // Legacy plaintext line (pre-#159 repos).
    if let Some(legacy) = tissue_core::tokens::legacy_plaintext_token(repo) {
        if tissue_core::tokens::constant_time_eq(&legacy, presented) {
            return Some("api_token (legacy config line)".to_string());
        }
    }
    None
}

/// True when any write auth is configured at all (env override, a managed
/// token, or a legacy line) — used for the startup note (#120).
fn write_auth_configured(repo: &Path) -> bool {
    if env_token_override().is_some() {
        return true;
    }
    if let Some(store) = token_store(repo) {
        if !store.list().unwrap_or_default().is_empty() {
            return true;
        }
    }
    tissue_core::tokens::legacy_plaintext_token(repo).is_some()
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

/// Shared bearer gate for write-ish routes (#159/#174): 503 when no token
/// is configured at all, 401 on missing/invalid/revoked. The token value is
/// never logged or echoed. Returns Ok(true) when authed; Ok(false) when the
/// error response was already sent (caller must stop).
fn require_bearer(stream: &mut TcpStream, repo: &Path, headers: &str) -> Result<bool, String> {
    if !write_auth_configured(repo) {
        respond_json(
            stream,
            503,
            "{\"error\":\"write API disabled: no token configured (tissue token issue, or TISSUE_API_TOKEN env)\"}",
        )?;
        return Ok(false);
    }
    match bearer_token(headers) {
        Some(provided) if resolve_token(repo, &provided).is_some() => Ok(true),
        Some(_) => {
            respond_json(stream, 401, "{\"error\":\"invalid or revoked token\"}")?;
            Ok(false)
        }
        None => {
            respond_json(
                stream,
                401,
                "{\"error\":\"missing Authorization: Bearer <token> header\"}",
            )?;
            Ok(false)
        }
    }
}

/// #174: token management for the Settings page. Every verb is
/// bearer-authed. `GET /api/tokens` lists id/label/created (masked — never
/// hashes, never secrets) plus a `legacy` flag when a plaintext `api_token:`
/// line still exists; `POST /api/tokens {"label"}` issues one and returns
/// the secret exactly once; `POST /api/tokens/<id>/revoke` fails closed.
/// Each mutation is one commit via TokenStore (hash-only storage).
fn api_tokens_route(
    stream: &mut TcpStream,
    method: &str,
    rest: &str,
    repo: &Path,
    headers: &str,
    body: &[u8],
) -> Result<(), String> {
    if !require_bearer(stream, repo, headers)? {
        return Ok(());
    }
    let rest = rest.trim_matches('/');
    let Some(store) = token_store(repo) else {
        return respond_json(stream, 500, "{\"error\":\"opening git repo\"}");
    };
    if rest == "tokens" {
        return match method {
            "GET" => {
                let entries = store.list()?;
                let items: Vec<String> = entries
                    .iter()
                    .map(|t| {
                        format!(
                            "{{\"id\":{},\"label\":{},\"created\":{}}}",
                            json_str(&t.id),
                            json_str(&t.label),
                            json_str(&t.created)
                        )
                    })
                    .collect();
                let legacy = store.has_legacy_plaintext_token();
                respond_json(
                    stream,
                    200,
                    &format!("{{\"tokens\":[{}],\"legacy\":{}}}", items.join(","), legacy),
                )
            }
            "POST" => {
                let payload = match std::str::from_utf8(body) {
                    Ok(s) => s,
                    Err(_) => {
                        return respond_json(stream, 400, "{\"error\":\"body must be UTF-8 JSON\"}")
                    }
                };
                let v = match parse_json_body(payload) {
                    Ok(v) => v,
                    Err(code) => {
                        return respond_json(
                            stream,
                            code,
                            "{\"error\":\"body must be a JSON object: {\\\"label\\\"}\"}",
                        )
                    }
                };
                let Some(label) = json_field(&v, "label") else {
                    return respond_json(
                        stream,
                        400,
                        "{\"error\":\"body must have a string field: label\"}",
                    );
                };
                let actor_id = json_field(&v, "actor").unwrap_or("human:gui");
                let author = tissue_core::git::Author {
                    name: actor_id.to_string(),
                    email: format!("{actor_id}@tissue.local"),
                };
                match store.issue(label, &author, &crate::now_utc()) {
                    Ok((secret, id)) => respond_json(
                        stream,
                        201,
                        &format!(
                            "{{\"id\":{},\"label\":{},\"secret\":{}}}",
                            json_str(&id),
                            json_str(label),
                            json_str(&secret)
                        ),
                    ),
                    Err(tissue_core::tokens::TokenError::DuplicateLabel(m)) => {
                        respond_json(stream, 409, &format!("{{\"error\":{}}}", json_str(&m)))
                    }
                    Err(e) => respond_json(
                        stream,
                        400,
                        &format!("{{\"error\":{}}}", json_str(&e.to_string())),
                    ),
                }
            }
            _ => respond_json(stream, 405, "{\"error\":\"method not allowed\"}"),
        };
    }
    if let Some(tail) = rest.strip_prefix("tokens/") {
        if method == "POST" && tail.ends_with("/revoke") {
            let id = tail.strip_suffix("/revoke").unwrap_or(tail);
            let payload = match std::str::from_utf8(body) {
                Ok(s) => s,
                Err(_) => {
                    return respond_json(stream, 400, "{\"error\":\"body must be UTF-8 JSON\"}")
                }
            };
            let v = parse_json_body(payload).unwrap_or(serde_json::Value::Null);
            let actor_id = json_field(&v, "actor").unwrap_or("human:gui");
            let author = tissue_core::git::Author {
                name: actor_id.to_string(),
                email: format!("{actor_id}@tissue.local"),
            };
            return match store.revoke(id, &author) {
                Ok(entry) => respond_json(
                    stream,
                    200,
                    &format!(
                        "{{\"revoked\":{},\"label\":{}}}",
                        json_str(&entry.id),
                        json_str(&entry.label)
                    ),
                ),
                Err(tissue_core::tokens::TokenError::UnknownId(m)) => {
                    respond_json(stream, 404, &format!("{{\"error\":{}}}", json_str(&m)))
                }
                Err(e) => respond_json(
                    stream,
                    400,
                    &format!("{{\"error\":{}}}", json_str(&e.to_string())),
                ),
            };
        }
        return respond_json(stream, 404, "{\"error\":\"unknown api route\"}");
    }
    respond_json(stream, 404, "{\"error\":\"unknown api route\"}")
}

fn api_write_route(
    stream: &mut TcpStream,
    rest: &str,
    repo: &Path,
    headers: &str,
    body: &[u8],
) -> Result<(), String> {
    // Auth failures are explicit and never log the token (AC4). #159: the
    // presented token resolves against the env override, the managed
    // hash-only `tokens:` entries, or a legacy plaintext line — all with
    // constant-time compares.
    if !require_bearer(stream, repo, headers)? {
        return Ok(());
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
    if rest == "projects" {
        return api_write_create_project(stream, repo, payload);
    }
    if rest == "releases" {
        return api_write_create_release(stream, repo, payload);
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
            "sync" => return api_write_sync(stream, repo, id, payload),
            "edit" => return api_write_edit(stream, repo, id, payload),
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
    // #134: optional body (the GUI prefills the CLI's Summary/AC template).
    let body = json_field(&v, "body");
    // #172: optional project — same shared validation as the CLI.
    let project = json_field(&v, "project");
    match crate::api_write::create_issue(repo, id, title, body, actor_id, project) {
        Ok(()) => respond_json(
            stream,
            201,
            &format!(
                "{{\"created\":{},\"title\":{},\"flow\":{}}}",
                json_str(id),
                json_str(title),
                if actor_id.starts_with("agent:") {
                    json_str("proposed-needs-review")
                } else {
                    json_str("direct")
                }
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

/// POST /api/issues/<id>/sync {"summary": "...", "actor": "human:..."}
/// The sync button (#99 AC3): same core path as the CLI — one human-sync
/// record + one commit with the same message shape.
fn api_write_sync(
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
                "{\"error\":\"request body must be a JSON object: {\"summary\",(\"actor\")}\"}",
            )
        }
    };
    let Some(summary) = json_field(&v, "summary") else {
        return respond_json(
            stream,
            400,
            "{\"error\":\"body must have a string field: summary (what changed and why)\"}",
        );
    };
    let actor_id = json_field(&v, "actor").unwrap_or("human:gui");
    match crate::api_write::sync(repo, id, summary, actor_id) {
        Ok(agent_outcome) => {
            // #137: surface the agent run outcome (if configured) so the
            // GUI can show success / failure / timeout on the Sync button.
            let agent_json = match &agent_outcome {
                None => "null".to_string(),
                Some(o) => format!(
                    "{{\"exit_code\":{},\"timed_out\":{},\"duration_ms\":{},\"success\":{}}}",
                    match o.exit_code {
                        Some(c) => c.to_string(),
                        None => "null".to_string(),
                    },
                    o.timed_out,
                    o.duration_ms,
                    o.success()
                ),
            };
            respond_json(
                stream,
                201,
                &format!("{{\"synced\":true,\"agent_run\":{agent_json}}}"),
            )
        }
        Err(e) if e.contains("nothing to sync") => {
            // #136: not an error — the human format simply has no changes
            // since the last sync baseline. No record, no commit.
            respond_json(
                stream,
                200,
                &format!("{{\"synced\":false,\"reason\":{}}}", json_str(&e)),
            )
        }
        Err(e) => respond_json(
            stream,
            400,
            &format!("\"error\":{}}}", json_str(&e)).replace("\"error\"", "{\"error\""),
        ),
    }
}

/// POST /api/issues/<id>/edit: human front-matter edit with the #78
/// revision guard. {"expected_revision": N, optional "title"/"status"/
/// "release", "actor": "human:..."}. Conflict => 409 with the re-read-
/// and-re-apply message; never a silent overwrite (#99 AC2).
fn api_write_edit(
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
                "{\"error\":\"request body must be a JSON object: {\"expected_revision\", optional \"title\"/\"status\"/\"release\", (\"actor\")}\"}",
            )
        }
    };
    let Some(rev) = v
        .get("expected_revision")
        .and_then(serde_json::Value::as_u64)
    else {
        return respond_json(
            stream,
            400,
            "{\"error\":\"body must have a numeric expected_revision (the revision you read)\"}",
        );
    };
    let title = json_field(&v, "title");
    let status = json_field(&v, "status");
    let release = json_field(&v, "release");
    if title.is_none() && status.is_none() && release.is_none() {
        return respond_json(
            stream,
            400,
            "{\"error\":\"nothing to edit: provide at least one of title, status, release\"}",
        );
    }
    if let Some(s) = status {
        if !matches!(s, "open" | "in-progress" | "closed") {
            return respond_json(
                stream,
                400,
                "{\"error\":\"status must be one of: open, in-progress, closed\"}",
            );
        }
    }
    let actor_id = json_field(&v, "actor").unwrap_or("human:gui");
    match crate::api_write::edit_issue(repo, id, rev, title, status, release, actor_id) {
        Ok(()) => respond_json(stream, 200, "{\"edited\":true}"),
        Err(e) => {
            let conflict = e.starts_with("CONFLICT:");
            let code = if conflict { 409 } else { 400 };
            respond_json(
                stream,
                code,
                &format!("\"error\":{}}}", json_str(&e)).replace("\"error\"", "{\"error\""),
            )
        }
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
            continue;
        }
        // #139: a creation draft (agent-proposed issue, no issue.md yet)
        // surfaces as a needs-review pseudo-issue so the review queue
        // and list can show it.
        if let Some(draft) = creation_draft_summary(repo, &path) {
            if let Some(p) = project_prefix {
                if !draft.id.starts_with(&format!("{p}-")) {
                    continue;
                }
            }
            issues.push(draft);
        }
    }
    issues.sort_by(|a, b| a.id.cmp(&b.id));
    issues
}

/// A creation draft as a reviewable pseudo-issue: needs-review tag, the
/// draft's title, timestamps from the proposal record.
fn creation_draft_summary(repo: &Path, dir: &Path) -> Option<IssueFile> {
    let name = dir.file_name().and_then(|n| n.to_str())?;
    let Ok(grepo) = tissue_core::git::Repo::open(repo) else {
        return None;
    };
    let core = tissue_core::sync::Core::new(grepo);
    let draft = core.creation_draft(name).ok()??;
    let (_pid, title, _body) = draft;
    let mut issue = IssueFile::new(name, &title, IssueStatus::Open, "", "").ok()?;
    issue.add_tag(tissue_core::human::NEEDS_REVIEW, "");
    Some(issue)
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
    let grepo = tissue_core::git::Repo::open(repo).ok();
    let items: Vec<String> = issues
        .iter()
        .map(|i| {
            let mut s = issue_summary_json(i);
            s.pop(); // closing brace
                     // #138: derived last-touch on the summary rows.
            let lt = grepo
                .as_ref()
                .and_then(|g| {
                    tissue_core::sync::Core::new(g.clone())
                        .last_touch(&i.id)
                        .ok()
                })
                .unwrap_or("human");
            s.push_str(&format!(",\"last_touch\":{}}}", json_str(lt)));
            s
        })
        .collect();
    let body = format!("{{\"issues\":[{}]}}", items.join(","));
    respond_json(stream, 200, &body)
}

fn api_issue_detail(stream: &mut TcpStream, repo: &Path, id: &str) -> Result<(), String> {
    let path = issues_dir(repo).join(id).join("issue.md");
    let Ok(issue) = IssueFile::from_path(&path) else {
        // #139: a creation draft has no issue.md yet — serve the draft so
        // the review queue's links work; approval materializes the file.
        if let Some(draft) = creation_draft_summary(repo, &issues_dir(repo).join(id)) {
            let mut summary = issue_summary_json(&draft);
            summary.pop();
            let body = "{\"issue\":".to_string() + &summary + ",\"revision\":0,\"body\":\"\"}";
            return respond_json(stream, 200, &body);
        }
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
    // revision exposed so GUI edits can send expected_revision (#99);
    // last_touch derived from the stream (#138).
    let lt = tissue_core::git::Repo::open(repo)
        .ok()
        .and_then(|g| tissue_core::sync::Core::new(g).last_touch(&issue.id).ok())
        .unwrap_or("human");
    let body = "{\"issue\":".to_string()
        + &summary
        + ",\"revision\":"
        + &issue.revision.to_string()
        + ",\"last_touch\":"
        + &json_str(lt)
        + ",\"body\":"
        + &json_str(&issue.body)
        + "}}";
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
        "index.html" => (gui_index_html().as_bytes(), "text/html; charset=utf-8"),
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
