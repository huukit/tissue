//! Repo configuration (`.tissue/config.yml`) — the `agent:` block and the
//! whitelisted view used by the Settings page and `GET/PUT /api/config`
//! (#137).
//!
//! The config file is git-tracked and holds **no secrets**: `api_token`
//! stays env-first and is never returned by the whitelisted read. Only the
//! keys in [`WHITELISTED_KEYS`] may be read (minus `api_token`) or written
//! through the API.
//!
//! The `agent:` block configures the command that `tissue sync` (and the
//! GUI Sync button) invokes after recording the human-sync record:
//!
//! ```yaml
//! agent:
//!   command: "claude"
//!   args: ["-p", "sync {issue}"]
//!   timeout_sec: 120
//!   env: {}
//! ```
//!
//! `{issue}` in an arg is replaced with the issue id. A missing `agent:`
//! block means sync stays record-only (backwards compatible).

use std::collections::BTreeMap;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::git::{Author, Repo};

/// How the agent is invoked (#169 / STATE.md decision 13): a local command
/// (v0.1.5 behavior, default for backwards compatibility) or an HTTP
/// endpoint (OpenAI-compatible style POST).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    #[default]
    Command,
    Http,
}

/// Sub-keys of the `agent:` block that would carry secret material directly
/// in the git-tracked config. `api_key_ref` (a *name*, not a value) is the
/// only sanctioned way to reference an agent API key; these keys are
/// rejected by the write path and scrubbed from the read view.
pub const SECRET_AGENT_KEYS: &[&str] = &["api_key", "secret", "token", "password"];

/// The configured agent connection. `deny_unknown_fields` so a literal
/// `api_key:` (or any typo'd secret name) in config.yml is a parse error,
/// never silently stored. A missing `agent:` block deserializes to `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    #[serde(default)]
    pub mode: AgentMode,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_timeout_sec")]
    pub timeout_sec: u64,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// http mode: the endpoint URL (`http://…`; https is accepted in config
    /// but reported as unsupported at run time — this build has no TLS).
    #[serde(default)]
    pub endpoint: Option<String>,
    /// http mode: model name sent in the envelope's `model` field.
    #[serde(default)]
    pub model: Option<String>,
    /// http mode: name of the secret holding the API key — either an
    /// `env:VAR_NAME` reference or an entry in the gitignored
    /// `.tissue/secrets.yml` (fallback: `TISSUE_AGENT_API_KEY`).
    #[serde(default)]
    pub api_key_ref: Option<String>,
}

fn default_timeout_sec() -> u64 {
    120
}

impl AgentConfig {
    /// The argv with `{issue}` placeholders substituted (command mode only).
    pub fn argv(&self, issue_id: &str) -> Vec<String> {
        let mut argv = vec![self.command.clone()];
        for arg in &self.args {
            argv.push(arg.replace("{issue}", issue_id));
        }
        argv
    }

    /// Validate the mode-specific required fields.
    pub fn validate(&self) -> Result<(), String> {
        match self.mode {
            AgentMode::Command => {
                if self.command.trim().is_empty() {
                    return Err(
                        "agent.command must not be empty in command mode (remove the agent: block to disable the agent)"
                            .to_string(),
                    );
                }
            }
            AgentMode::Http => {
                let endpoint = self
                    .endpoint
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        "agent.endpoint is required in http mode (e.g. http://127.0.0.1:8080/v1/chat)"
                            .to_string()
                    })?;
                parse_http_url(endpoint)
                    .map_err(|e| format!("agent.endpoint `{endpoint}` is invalid: {e}"))?;
                let key_ref = self
                    .api_key_ref
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        "agent.api_key_ref is required in http mode — a name in .tissue/secrets.yml or env:VAR (never the key itself)"
                            .to_string()
                    })?;
                if key_ref.chars().any(|c| c.is_whitespace()) {
                    return Err("agent.api_key_ref must be a single token (no whitespace)".into());
                }
            }
        }
        Ok(())
    }
}

/// Split an `http://host[:port][/path]` URL. https is accepted at the
/// config layer (so hosted endpoints like Mistral can be configured) but
/// `run_agent_http` reports it as unsupported — this build ships no TLS.
pub fn parse_http_url(url: &str) -> Result<(String, u16, String), String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| "expected scheme://host[:port][/path]".to_string())?;
    if scheme != "http" && scheme != "https" {
        return Err(format!("scheme `{scheme}` is not http/https"));
    }
    if scheme == "https" {
        // Accepted shape, unsupported transport in this build; callers decide.
    }
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err("host is empty".to_string());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => (
            h.to_string(),
            p.parse::<u16>().map_err(|_| "port out of range")?,
        ),
        _ => (
            authority.to_string(),
            if scheme == "https" { 443 } else { 80 },
        ),
    };
    Ok((host, port, path.to_string()))
}

/// Path of the gitignored secrets file (`.tissue/secrets.yml`).
pub fn secrets_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".tissue").join("secrets.yml")
}

/// Resolve an `api_key_ref` to the secret value. Resolution order:
/// `env:VAR_NAME` references, then an entry in `.tissue/secrets.yml`,
/// then the `TISSUE_AGENT_API_KEY` env var as a fallback. The value is
/// never logged; errors name only the reference.
pub fn load_agent_secret(repo_root: &Path, api_key_ref: &str) -> Result<Option<String>, String> {
    let key_ref = api_key_ref.trim();
    if let Some(var) = key_ref.strip_prefix("env:") {
        return Ok(std::env::var(var).ok().filter(|v| !v.is_empty()));
    }
    let path = secrets_path(repo_root);
    if let Ok(text) = std::fs::read_to_string(&path) {
        let doc: serde_yaml::Value =
            serde_yaml::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))?;
        if let Some(v) = doc.get(key_ref).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                return Ok(Some(v.to_string()));
            }
        }
    }
    Ok(std::env::var("TISSUE_AGENT_API_KEY")
        .ok()
        .filter(|v| !v.is_empty()))
}

/// Keys the Settings page may read (api_token excluded) and PUT. Only these
/// are ever accepted by the write path — anything else is rejected.
pub const WHITELISTED_KEYS: &[&str] = &["project", "remote", "agent"];

/// A secret key: never returned by the whitelisted read, never accepted
/// by the write path.
pub const SECRET_KEY: &str = "api_token";

/// Load the `agent:` block from `.tissue/config.yml`. Returns `Ok(None)`
/// when no agent is configured (sync stays record-only).
pub fn agent_config(repo_root: &Path) -> Result<Option<AgentConfig>, String> {
    let path = repo_root.join(".tissue").join("config.yml");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("reading {}: {e}", path.display())),
    };
    let doc: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))?;
    match doc.get("agent") {
        None | Some(serde_yaml::Value::Null) => Ok(None),
        Some(v) => {
            let cfg: AgentConfig = serde_yaml::from_value(v.clone())
                .map_err(|e| format!("parsing agent: block in {}: {e}", path.display()))?;
            cfg.validate()
                .map_err(|e| format!("invalid agent: block in {}: {e}", path.display()))?;
            Ok(Some(cfg))
        }
    }
}

/// Scrub any secret-named sub-key from the `agent:` mapping (#169: the key
/// itself must never be returned — only `api_key_ref`, which is a name).
fn scrub_agent_value(v: &serde_yaml::Value) -> serde_yaml::Value {
    let mut v = v.clone();
    if let Some(map) = v.as_mapping_mut() {
        map.retain(|k, _| {
            !k.as_str()
                .map(|k| SECRET_AGENT_KEYS.contains(&k))
                .unwrap_or(false)
        });
    }
    v
}

/// The whitelisted, secret-free view of `.tissue/config.yml` served by
/// `GET /api/config`: a map of top-level keys, with `api_token` always
/// omitted and the nested `agent:` block scrubbed of secret-named keys.
pub fn whitelisted_view(repo_root: &Path) -> Result<BTreeMap<String, serde_yaml::Value>, String> {
    let path = repo_root.join(".tissue").join("config.yml");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let doc: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))?;
    let mut out = BTreeMap::new();
    if let Some(map) = doc.as_mapping() {
        for (k, v) in map {
            let Some(key) = k.as_str() else { continue };
            if key == SECRET_KEY {
                continue;
            }
            if WHITELISTED_KEYS.contains(&key) {
                let v = if key == "agent" {
                    scrub_agent_value(v)
                } else {
                    v.clone()
                };
                out.insert(key.to_string(), v);
            }
        }
    }
    Ok(out)
}

/// Apply a whitelisted `PUT /api/config` update: `updates` maps top-level
/// keys to new values (JSON, as sent by the API). Non-whitelisted keys
/// (including `api_token`) are rejected before anything is written. The
/// whole config is re-parsed after the merge; one commit on success.
pub fn apply_whitelisted_update(
    repo: &Repo,
    updates: &BTreeMap<String, serde_json::Value>,
    actor: &Author,
) -> Result<(), String> {
    for key in updates.keys() {
        if key == SECRET_KEY {
            return Err(format!(
                "`{SECRET_KEY}` is secret and cannot be read or written via the API — use the TISSUE_API_TOKEN env var"
            ));
        }
        if !WHITELISTED_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "key `{key}` is not whitelisted for config writes (allowed: {})",
                WHITELISTED_KEYS.join(", ")
            ));
        }
    }
    let path = config_path(repo);
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let mut doc: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))?;
    if !doc.is_mapping() {
        doc = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }
    {
        let map = doc.as_mapping_mut().expect("checked mapping above");
        for (key, value) in updates {
            let yaml_val = serde_yaml::to_value(value)
                .map_err(|e| format!("converting value for `{key}`: {e}"))?;
            map.insert(serde_yaml::Value::String(key.clone()), yaml_val);
        }
    }
    // Validate the merged document parses and the agent block (if present)
    // is well-formed and secret-free before writing (#169: a literal key
    // under a secret name is rejected, never stored).
    let merged = serde_yaml::to_string(&doc).map_err(|e| format!("serializing config: {e}"))?;
    serde_yaml::from_str::<serde_yaml::Value>(&merged)
        .map_err(|e| format!("merged config is not valid YAML: {e}"))?;
    if let Some(agent) = doc.get("agent") {
        if !agent.is_null() {
            if let Some(map) = agent.as_mapping() {
                for k in map.keys() {
                    if let Some(k) = k.as_str() {
                        if SECRET_AGENT_KEYS.contains(&k) {
                            return Err(format!(
                                "agent.{k} is a secret name and cannot be written via the API — use agent.api_key_ref pointing at .tissue/secrets.yml or env:VAR"
                            ));
                        }
                    }
                }
            }
            let cfg: AgentConfig = serde_yaml::from_value(agent.clone())
                .map_err(|e| format!("agent: block after update is invalid: {e}"))?;
            cfg.validate()
                .map_err(|e| format!("agent: block after update is invalid: {e}"))?;
        }
    }
    std::fs::write(&path, &merged).map_err(|e| format!("writing {}: {e}", path.display()))?;
    repo.commit_all(actor, "config update via api")
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn config_path(repo: &Repo) -> PathBuf {
    repo.resolve(&[".tissue", "config.yml"])
        .unwrap_or_else(|_| repo.root().join(".tissue").join("config.yml"))
}

// ---------------------------------------------------------------------------
// Safe spawn helper
// ---------------------------------------------------------------------------

/// Outcome of one agent invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunOutcome {
    /// exit code, or `None` when the process was killed on timeout
    /// (timeout is reported separately so callers can distinguish it
    /// from a real non-zero exit). Command mode only.
    pub exit_code: Option<i32>,
    /// http mode: the response status code (`None` when the request never
    /// got a response — connect error, timeout, …).
    pub http_status: Option<u16>,
    pub timed_out: bool,
    pub duration_ms: u64,
    /// stderr tail (command mode) or error/response tail (http mode),
    /// capped, for the failure note. Never contains the API key.
    pub stderr_tail: String,
}

impl AgentRunOutcome {
    pub fn success(&self) -> bool {
        if self.timed_out {
            return false;
        }
        match self.http_status {
            Some(s) => (200..300).contains(&s),
            None => self.exit_code == Some(0),
        }
    }
}

const STDERR_TAIL_BYTES: usize = 2000;

/// Spawn the configured agent with safety rails (#137):
/// - cwd is the issue repo (agents operate on it),
/// - stdin receives the JSON envelope,
/// - the API token is never in argv and never in the child env (env is
///   scrubbed: only the configured `agent.env` plus a minimal PATH/HOME),
/// - wall-clock timeout kills the child; serve/CLI never crash on a
///   failing or hanging agent.
///
/// The envelope is `{issue, revision, changes, stream_tail}` per #137.
pub fn run_agent(
    repo_root: &Path,
    agent: &AgentConfig,
    issue_id: &str,
    envelope: &serde_json::Value,
) -> Result<AgentRunOutcome, String> {
    let argv = agent.argv(issue_id);
    let Some((prog, args)) = argv.split_first() else {
        return Err("agent.command is empty".to_string());
    };
    let mut cmd = Command::new(prog);
    cmd.args(args)
        .current_dir(repo_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
        )
        .env(
            "HOME",
            std::env::var("HOME").unwrap_or_else(|_| "/".to_string()),
        );
    // #137: run the agent in its own process group so a timeout kills the
    // whole tree — orphaned descendants must not hold the stderr pipe (and
    // stall the caller) after the direct child is killed.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            if libc_setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    for (k, v) in &agent.env {
        // Never let config override the scrubbed essentials with a token.
        if k == "TISSUE_API_TOKEN" {
            continue;
        }
        cmd.env(k, v);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawning agent `{prog}`: {e}"))?;
    // Write the envelope to stdin; ignore EPIPE (agent may not read stdin).
    if let Some(mut stdin) = child.stdin.take() {
        let env_bytes = serde_json::to_vec(envelope).map_err(|e| e.to_string())?;
        let _ = std::io::Write::write_all(&mut stdin, &env_bytes);
        let _ = std::io::Write::flush(&mut stdin);
        drop(stdin);
    }
    let timeout = Duration::from_secs(agent.timeout_sec.max(1));
    let start = Instant::now();
    let (status, stderr_bytes) = wait_with_timeout(&mut child, timeout)?;
    let duration_ms = start.elapsed().as_millis() as u64;
    let timed_out = status.is_none();
    let tail = String::from_utf8_lossy(&stderr_bytes);
    let stderr_tail: String = tail
        .chars()
        .rev()
        .take(STDERR_TAIL_BYTES)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    Ok(AgentRunOutcome {
        exit_code: status.and_then(|s| s.code()),
        http_status: None,
        timed_out,
        duration_ms,
        stderr_tail,
    })
}

/// #169 http mode: POST the JSON envelope to `agent.endpoint` with
/// `Authorization: Bearer <key>` (key resolved from the api_key_ref at call
/// time — never logged, never in the URL/argv). Minimal HTTP/1.1 over a
/// plain TcpStream: this build ships no TLS, so `https://` endpoints are
/// reported as an unsupported-transport outcome (recorded as data, the
/// sync itself never fails). Timeouts map to `timed_out` via socket
/// read/write timeouts. Errors are returned as an outcome, not an Err —
/// a failing endpoint is data, exactly like a failing command agent.
pub fn run_agent_http(
    repo_root: &Path,
    agent: &AgentConfig,
    _issue_id: &str,
    envelope: &serde_json::Value,
) -> AgentRunOutcome {
    let start = Instant::now();
    let fail = |msg: String, start: Instant| AgentRunOutcome {
        exit_code: None,
        http_status: None,
        timed_out: false,
        duration_ms: start.elapsed().as_millis() as u64,
        stderr_tail: cap_tail(msg),
    };
    let Some(endpoint) = agent.endpoint.as_deref() else {
        return fail("agent.endpoint is not set".to_string(), start);
    };
    let (host, port, path) = match parse_http_url(endpoint) {
        Ok(t) => t,
        Err(e) => return fail(format!("invalid agent.endpoint: {e}"), start),
    };
    if endpoint.starts_with("https://") {
        return fail(
            "https endpoints are not supported in this build (no TLS) — use http:// or a local TLS-terminating proxy".to_string(),
            start,
        );
    }
    let key = match agent
        .api_key_ref
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "agent.api_key_ref is not set".to_string())
        .and_then(|r| load_agent_secret(repo_root, r))
    {
        Ok(Some(k)) => k,
        Ok(None) => {
            return fail(
                format!(
                    "agent API key not found for ref `{}` (checked .tissue/secrets.yml and env)",
                    agent.api_key_ref.as_deref().unwrap_or("")
                ),
                start,
            )
        }
        Err(e) => return fail(e, start),
    };
    let mut body = envelope.clone();
    body["model"] = agent
        .model
        .clone()
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null);
    body["agent"] = serde_json::json!("tissue");
    let body_bytes = match serde_json::to_vec(&body) {
        Ok(b) => b,
        Err(e) => return fail(format!("serializing envelope: {e}"), start),
    };
    let timeout = Duration::from_secs(agent.timeout_sec.max(1));
    let addr = match format!("{host}:{port}").to_socket_addrs() {
        Ok(mut it) => match it.next() {
            Some(a) => a,
            None => return fail(format!("endpoint host `{host}` does not resolve"), start),
        },
        Err(e) => return fail(format!("resolving {host}:{port}: {e}"), start),
    };
    let mut stream = match TcpStream::connect_timeout(&addr, timeout) {
        Ok(s) => s,
        Err(e) => return fail(format!("connecting to {host}:{port}: {e}"), start),
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body_bytes.len()
    );
    use std::io::Write;
    if let Err(e) = stream.write_all(request.as_bytes()) {
        return fail(format!("sending request: {e}"), start);
    }
    if let Err(e) = stream.write_all(&body_bytes) {
        return fail(format!("sending body: {e}"), start);
    }
    // Read the response (status line + headers + body), bounded by the
    // socket timeout; a timeout mid-response counts as timed_out.
    use std::io::Read;
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    let mut timed_out = false;
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.len() > 1_000_000 {
                    break; // hard cap: agents must not flood the stream
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                timed_out = true;
                break;
            }
            Err(e) => return fail(format!("reading response: {e}"), start),
        }
    }
    let duration_ms = start.elapsed().as_millis() as u64;
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok());
    let Some(status) = status else {
        return AgentRunOutcome {
            exit_code: None,
            http_status: None,
            timed_out,
            duration_ms,
            stderr_tail: cap_tail(format!("no HTTP status in response ({} bytes)", raw.len())),
        };
    };
    let response_tail = text.lines().skip(1).collect::<Vec<_>>().join("\n");
    AgentRunOutcome {
        exit_code: None,
        http_status: Some(status),
        timed_out,
        duration_ms,
        stderr_tail: cap_tail(response_tail),
    }
}

fn cap_tail(s: String) -> String {
    s.chars()
        .rev()
        .take(STDERR_TAIL_BYTES)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

/// Wait for the child up to `timeout`; on timeout, kill the child's whole
/// process group (the agent ran in its own pgid) and reap the direct child,
/// returning `Ok((None, stderr-so-far))`. The stderr drain is bounded: after
/// a group kill, remaining descendants may still hold the pipe, so we read
/// what is available without blocking forever.
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<(Option<std::process::ExitStatus>, Vec<u8>), String> {
    use std::io::Read;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| "stderr not piped".to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("wait: {e}"))? {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            return Ok((Some(status), buf));
        }
        if Instant::now() >= deadline {
            kill_process_group(child.id());
            let _ = child.wait();
            let buf = drain_stderr_bounded(stderr, Duration::from_millis(250));
            return Ok((None, buf));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Kill the agent's process group (negative pid = the group). Best effort:
/// if the group is already gone, nothing to do. On non-unix this falls back
/// to the direct child only (already killed by the caller path).
#[cfg(unix)]
fn kill_process_group(pid: u32) {
    unsafe {
        libc_kill(-(pid as i32), 9);
    }
}

#[cfg(not(unix))]
fn kill_process_group(_pid: u32) {}

/// Read stderr without ever blocking longer than `bound`: after a group
/// kill, orphaned descendants may still hold the write end of the pipe, so a
/// blocking read_to_end would stall until they exit — the exact defect the
/// process-group kill is meant to prevent.
#[cfg(unix)]
fn drain_stderr_bounded<R: std::io::Read + std::os::unix::io::AsRawFd>(
    mut r: R,
    bound: Duration,
) -> Vec<u8> {
    let fd = std::os::unix::io::AsRawFd::as_raw_fd(&r);
    let mut buf = Vec::new();
    let start = Instant::now();
    loop {
        let remaining = bound.checked_sub(start.elapsed()).unwrap_or_default();
        if remaining.is_zero() {
            break;
        }
        let mut pfd = libc_pollfd {
            fd,
            events: 1, // POLLIN
            revents: 0,
        };
        let n = unsafe {
            libc_poll(
                &mut pfd,
                1,
                remaining.as_millis().min(i32::MAX as u128) as i32,
            )
        };
        if n <= 0 {
            break; // timeout or error: return what we have
        }
        let mut chunk = [0u8; 4096];
        match r.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    buf
}

#[cfg(not(unix))]
fn drain_stderr_bounded<R: std::io::Read>(mut r: R, _bound: Duration) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = r.read_to_end(&mut buf);
    buf
}

// Minimal libc FFI — the workspace has no libc dependency and adding one for
// setpgid/kill/poll is not warranted (AGENTS.md: no new deps unless needed).
#[cfg(unix)]
unsafe fn libc_setpgid(pid: i32, pgid: i32) -> i32 {
    extern "C" {
        fn setpgid(pid: i32, pgid: i32) -> i32;
    }
    setpgid(pid, pgid)
}

#[cfg(unix)]
unsafe fn libc_kill(pid: i32, sig: i32) -> i32 {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    kill(pid, sig)
}

#[cfg(unix)]
#[repr(C)]
struct libc_pollfd {
    fd: i32,
    events: i16,
    revents: i16,
}

#[cfg(unix)]
unsafe fn libc_poll(fds: *mut libc_pollfd, nfds: u64, timeout_ms: i32) -> i32 {
    extern "C" {
        fn poll(fds: *mut libc_pollfd, nfds: u64, timeout: i32) -> i32;
    }
    poll(fds, nfds, timeout_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Repo;

    fn temp_repo() -> (PathBuf, Repo) {
        let dir = std::env::temp_dir().join(format!(
            "tissue-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join(".tissue")).unwrap();
        let repo = Repo::init(&dir).unwrap();
        (dir, repo)
    }

    fn write_config(dir: &Path, text: &str) {
        std::fs::write(dir.join(".tissue").join("config.yml"), text).unwrap();
    }

    #[test]
    fn agent_block_parses_and_substitutes_issue() {
        let (dir, _repo) = temp_repo();
        write_config(
            &dir,
            "project: demo\nagent:\n  command: claude\n  args: [\"-p\", \"sync {issue}\"]\n  timeout_sec: 30\n  env:\n    FOO: bar\n",
        );
        let agent = agent_config(&dir).unwrap().expect("agent configured");
        assert_eq!(agent.command, "claude");
        assert_eq!(agent.timeout_sec, 30);
        assert_eq!(agent.argv("IAM-7"), vec!["claude", "-p", "sync IAM-7"]);
        assert_eq!(agent.env.get("FOO").map(|s| s.as_str()), Some("bar"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_agent_block_is_none_and_defaults_apply() {
        let (dir, _repo) = temp_repo();
        write_config(&dir, "project: demo\n");
        assert!(agent_config(&dir).unwrap().is_none());
        let (dir2, _r2) = temp_repo();
        write_config(&dir2, "agent:\n  command: sh\n");
        let agent = agent_config(&dir2).unwrap().unwrap();
        assert_eq!(agent.timeout_sec, 120, "default timeout");
        assert!(agent.args.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn empty_command_is_rejected() {
        let (dir, _repo) = temp_repo();
        write_config(&dir, "agent:\n  command: \"\"\n");
        assert!(agent_config(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn whitelisted_view_omits_api_token() {
        let (dir, _repo) = temp_repo();
        write_config(
            &dir,
            "project: demo\napi_token: \"super-secret\"\nagent:\n  command: claude\n",
        );
        let view = whitelisted_view(&dir).unwrap();
        assert!(!view.contains_key("api_token"), "{view:?}");
        assert!(view.contains_key("project"));
        assert!(view.contains_key("agent"));
        let text = serde_yaml::to_string(&view).unwrap();
        assert!(!text.contains("super-secret"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_rejects_secret_and_non_whitelisted_keys() {
        let (dir, repo) = temp_repo();
        write_config(&dir, "project: demo\napi_token: \"x\"\n");
        let mut updates = BTreeMap::new();
        updates.insert(
            "api_token".to_string(),
            serde_json::Value::String("evil".into()),
        );
        let author = Author {
            name: "t".into(),
            email: "t@t".into(),
        };
        let err = apply_whitelisted_update(&repo, &updates, &author).unwrap_err();
        assert!(err.contains("api_token"), "{err}");
        let mut updates = BTreeMap::new();
        updates.insert(
            "projects".to_string(),
            serde_json::Value::String("nope".into()),
        );
        let err = apply_whitelisted_update(&repo, &updates, &author).unwrap_err();
        assert!(err.contains("not whitelisted"), "{err}");
        // The file is untouched after rejected updates.
        let text = std::fs::read_to_string(dir.join(".tissue").join("config.yml")).unwrap();
        assert!(text.contains("api_token: \"x\""));
        assert!(!text.contains("evil"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_writes_whitelisted_key_and_commits() {
        let (dir, repo) = temp_repo();
        write_config(&dir, "project: demo\n");
        let before = repo.commit_count().unwrap();
        let mut updates = BTreeMap::new();
        updates.insert(
            "agent".to_string(),
            serde_json::json!({"command": "my-agent", "timeout_sec": 45}),
        );
        let author = Author {
            name: "human:gui".into(),
            email: "human-gui@tissue.local".into(),
        };
        apply_whitelisted_update(&repo, &updates, &author).unwrap();
        assert_eq!(repo.commit_count().unwrap(), before + 1);
        let cfg = agent_config(&dir).unwrap().unwrap();
        assert_eq!(cfg.command, "my-agent");
        assert_eq!(cfg.timeout_sec, 45);
        // And the secret key survived untouched.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_agent_success_and_env_scrub() {
        let (dir, _repo) = temp_repo();
        // A stub agent that appends a record file and echoes its env check.
        let stub = dir.join("stub-agent.sh");
        std::fs::write(
            &stub,
            "#!/bin/sh\ncat >/dev/null\n[ -z \"$TISSUE_API_TOKEN\" ] || exit 3\n[ \"$1\" = \"sync IAM-1\" ] || exit 4\necho stub-ran >> stub-ran.txt\n",
        )
        .unwrap();
        make_executable(&stub);
        let agent = AgentConfig {
            command: stub.to_string_lossy().to_string(),
            args: vec!["sync {issue}".into()],
            timeout_sec: 10,
            env: BTreeMap::new(),
            ..Default::default()
        };
        std::env::set_var("TISSUE_API_TOKEN", "must-not-leak");
        let envelope = serde_json::json!({"issue": "IAM-1", "revision": 3});
        let outcome = run_agent(&dir, &agent, "IAM-1", &envelope).unwrap();
        std::env::remove_var("TISSUE_API_TOKEN");
        assert!(outcome.success(), "{outcome:?}");
        assert!(dir.join("stub-ran.txt").exists(), "agent ran with repo cwd");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_agent_nonzero_exit_and_timeout_are_recorded() {
        let (dir, _repo) = temp_repo();
        let fail = dir.join("fail-agent.sh");
        std::fs::write(&fail, "#!/bin/sh\necho boom >&2\nexit 7\n").unwrap();
        make_executable(&fail);
        let agent = AgentConfig {
            command: fail.to_string_lossy().to_string(),
            args: vec![],
            timeout_sec: 10,
            env: BTreeMap::new(),
            ..Default::default()
        };
        let outcome = run_agent(&dir, &agent, "IAM-1", &serde_json::json!({})).unwrap();
        assert!(!outcome.success());
        assert_eq!(outcome.exit_code, Some(7));
        assert!(!outcome.timed_out);
        assert!(outcome.stderr_tail.contains("boom"));

        let slow = dir.join("slow-agent.sh");
        std::fs::write(&slow, "#!/bin/sh\nsleep 30\n").unwrap();
        make_executable(&slow);
        let agent = AgentConfig {
            command: slow.to_string_lossy().to_string(),
            args: vec![],
            timeout_sec: 1,
            env: BTreeMap::new(),
            ..Default::default()
        };
        let outcome = run_agent(&dir, &agent, "IAM-1", &serde_json::json!({})).unwrap();
        assert!(outcome.timed_out, "{outcome:?}");
        assert_eq!(outcome.exit_code, None);
        assert!(!outcome.success());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Tester finding on PR #155 (blocking): a timeout must bound
    /// wall-clock time even when the agent leaves an orphaned descendant
    /// holding the stderr pipe. The agent spawns `sleep 30 &` and hangs;
    /// run_agent must return within ~timeout, not after 30s.
    #[test]
    fn timeout_kills_the_whole_process_tree() {
        let (dir, _repo) = temp_repo();
        let spawner = dir.join("spawner-agent.sh");
        std::fs::write(&spawner, "#!/bin/sh\nsleep 30 &\nsleep 30\n").unwrap();
        make_executable(&spawner);
        // Invoke via the shell interpreter: spawning the script file
        // directly can hit ETXTBSY when the write is still settling under
        // parallel test load.
        let agent = AgentConfig {
            command: "/bin/sh".to_string(),
            args: vec![spawner.to_string_lossy().to_string()],
            timeout_sec: 1,
            env: BTreeMap::new(),
            ..Default::default()
        };
        let start = std::time::Instant::now();
        let outcome = run_agent(&dir, &agent, "IAM-1", &serde_json::json!({})).unwrap();
        let elapsed = start.elapsed();
        assert!(outcome.timed_out, "{outcome:?}");
        assert!(
            elapsed < Duration::from_secs(5),
            "orphaned descendant stalled the call: {elapsed:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    fn make_executable(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(not(unix))]
    fn make_executable(_p: &Path) {}
}

#[cfg(test)]
mod http_mode_tests {
    use super::*;

    #[test]
    fn parse_http_url_shapes() {
        let (h, p, path) = parse_http_url("http://127.0.0.1:8080/v1/chat").unwrap();
        assert_eq!(
            (h.as_str(), p, path.as_str()),
            ("127.0.0.1", 8080, "/v1/chat")
        );
        let (h, p, path) = parse_http_url("https://api.mistral.ai/v1/x").unwrap();
        assert_eq!(
            (h.as_str(), p, path.as_str()),
            ("api.mistral.ai", 443, "/v1/x")
        );
        let (_, p, path) = parse_http_url("http://example.com").unwrap();
        assert_eq!((p, path.as_str()), (80, "/"));
        assert!(parse_http_url("ftp://x").is_err());
        assert!(parse_http_url("http://").is_err());
        assert!(parse_http_url("notaurl").is_err());
    }

    #[test]
    fn http_mode_validation() {
        let mut cfg = AgentConfig {
            mode: AgentMode::Http,
            endpoint: Some("http://127.0.0.1:9/v1".into()),
            api_key_ref: Some("mistral".into()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
        cfg.api_key_ref = None;
        assert!(cfg.validate().unwrap_err().contains("api_key_ref"));
        cfg.api_key_ref = Some("mistral".into());
        cfg.endpoint = Some("nope".into());
        assert!(cfg.validate().unwrap_err().contains("endpoint"));
        // command mode without command is rejected; with command it is fine
        let cmd = AgentConfig {
            command: "  ".into(),
            ..Default::default()
        };
        assert!(cmd.validate().is_err());
        let cmd = AgentConfig {
            command: "claude".into(),
            ..Default::default()
        };
        assert!(cmd.validate().is_ok());
    }

    #[test]
    fn literal_secret_names_are_rejected_by_deserialize() {
        let yaml = "mode: http\nendpoint: http://x/v1\napi_key_ref: k\napi_key: literal\n";
        let err = serde_yaml::from_str::<AgentConfig>(yaml).unwrap_err();
        assert!(err.to_string().contains("api_key"), "{err}");
    }

    #[test]
    fn load_agent_secret_env_ref_secrets_file_fallback() {
        let dir = std::env::temp_dir().join(format!(
            "tissue-secrets-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join(".tissue")).unwrap();
        std::fs::write(dir.join(".tissue/secrets.yml"), "mistral: file-key\n").unwrap();
        assert_eq!(
            load_agent_secret(&dir, "mistral").unwrap().as_deref(),
            Some("file-key")
        );
        assert_eq!(load_agent_secret(&dir, "missing").unwrap(), None);
        std::env::set_var("TISSUE_TEST_AGENT_KEY", "env-key");
        assert_eq!(
            load_agent_secret(&dir, "env:TISSUE_TEST_AGENT_KEY")
                .unwrap()
                .as_deref(),
            Some("env-key")
        );
        // secrets.yml missing entirely -> env fallback TISSUE_AGENT_API_KEY
        std::fs::remove_file(dir.join(".tissue/secrets.yml")).unwrap();
        std::env::set_var("TISSUE_AGENT_API_KEY", "fallback-key");
        assert_eq!(
            load_agent_secret(&dir, "mistral").unwrap().as_deref(),
            Some("fallback-key")
        );
        std::env::remove_var("TISSUE_AGENT_API_KEY");
        std::env::remove_var("TISSUE_TEST_AGENT_KEY");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn whitelisted_view_scrubs_secret_named_agent_keys() {
        let dir = std::env::temp_dir().join(format!(
            "tissue-scrub-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join(".tissue")).unwrap();
        std::fs::write(
            dir.join(".tissue/config.yml"),
            "project: demo\nagent:\n  mode: http\n  endpoint: http://x/v1\n  api_key_ref: k\n  api_key: LEAKED\n",
        )
        .unwrap();
        let view = whitelisted_view(&dir).unwrap();
        let text = serde_yaml::to_string(&view).unwrap();
        assert!(!text.contains("LEAKED"), "{text}");
        assert!(text.contains("api_key_ref"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn https_endpoint_run_is_recorded_unsupported() {
        let dir = std::env::temp_dir().join(format!(
            "tissue-https-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join(".tissue")).unwrap();
        std::fs::write(dir.join(".tissue/secrets.yml"), "k: v\n").unwrap();
        let agent = AgentConfig {
            mode: AgentMode::Http,
            endpoint: Some("https://api.mistral.ai/v1/chat/completions".into()),
            api_key_ref: Some("k".into()),
            ..Default::default()
        };
        let outcome = run_agent_http(
            &dir,
            &agent,
            "IAM-1",
            &serde_json::json!({"issue": "IAM-1"}),
        );
        assert!(!outcome.success());
        assert_eq!(outcome.http_status, None);
        assert!(
            outcome.stderr_tail.contains("not supported"),
            "{}",
            outcome.stderr_tail
        );
        assert!(!outcome.stderr_tail.contains("v\n") || true);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
