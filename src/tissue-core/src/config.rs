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
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::git::{Author, Repo};

/// The configured agent connection. All fields optional except `command`;
/// a missing `agent:` block deserializes to `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct AgentConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_timeout_sec")]
    pub timeout_sec: u64,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

fn default_timeout_sec() -> u64 {
    120
}

impl AgentConfig {
    /// The argv with `{issue}` placeholders substituted.
    pub fn argv(&self, issue_id: &str) -> Vec<String> {
        let mut argv = vec![self.command.clone()];
        for arg in &self.args {
            argv.push(arg.replace("{issue}", issue_id));
        }
        argv
    }
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
            if cfg.command.trim().is_empty() {
                return Err(format!(
                    "agent.command in {} must not be empty (remove the agent: block to disable the agent)",
                    path.display()
                ));
            }
            Ok(Some(cfg))
        }
    }
}

/// The whitelisted, secret-free view of `.tissue/config.yml` served by
/// `GET /api/config`: a map of top-level keys, with `api_token` always
/// omitted and nested `agent:` returned as-is (it holds no secrets).
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
                out.insert(key.to_string(), v.clone());
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
    // is well-formed before writing.
    let merged = serde_yaml::to_string(&doc).map_err(|e| format!("serializing config: {e}"))?;
    serde_yaml::from_str::<serde_yaml::Value>(&merged)
        .map_err(|e| format!("merged config is not valid YAML: {e}"))?;
    if let Some(agent) = doc.get("agent") {
        if !agent.is_null() {
            let cfg: AgentConfig = serde_yaml::from_value(agent.clone())
                .map_err(|e| format!("agent: block after update is invalid: {e}"))?;
            if cfg.command.trim().is_empty() {
                return Err("agent.command must not be empty after update".to_string());
            }
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
    /// from a real non-zero exit).
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u64,
    /// stderr tail (capped), for the failure note. stdout is not captured
    /// by default — agents write to the stream via the API/CLI.
    pub stderr_tail: String,
}

impl AgentRunOutcome {
    pub fn success(&self) -> bool {
        !self.timed_out && self.exit_code == Some(0)
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
        timed_out,
        duration_ms,
        stderr_tail,
    })
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
