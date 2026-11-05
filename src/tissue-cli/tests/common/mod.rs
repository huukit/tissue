//! Shared subprocess safeguards for the integration-test battery (#228).
//!
//! Constrained dev sandboxes exhaust their process table when the battery
//! leaks children (fork -> EAGAIN, "Cannot fork"). Every child spawned by
//! the tests goes through this module so that:
//! - the child runs in its own process group, so a kill hits the whole tree;
//! - the child is always reaped (`wait`) — on success, on timeout (after a
//!   group kill), and on Drop — so no zombie children accumulate;
//! - before every fork, /proc headroom is checked and a full table fails
//!   with a clear message instead of a raw EAGAIN;
//! - a per-test-binary cap bounds simultaneously live children under
//!   parallel test threads.

// Each tests/*.rs is its own crate; not every crate uses every helper.
#![allow(dead_code)]

use std::io::Write;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Default per-command timeout. Battery commands (git, the tissue CLI) are
/// sub-second; long-running servers use [`spawn`] plus the caller's poll.
pub fn default_timeout() -> Duration {
    Duration::from_secs(60)
}

/// Max simultaneously live children per test binary process (#228).
const MAX_LIVE_CHILDREN: usize = 24;

struct Live {
    count: Mutex<usize>,
    room: Condvar,
}

fn live() -> &'static Live {
    static L: OnceLock<Live> = OnceLock::new();
    L.get_or_init(|| Live {
        count: Mutex::new(0),
        room: Condvar::new(),
    })
}

struct ChildSlot;

impl ChildSlot {
    fn acquire() -> Self {
        let l = live();
        let mut n = l.count.lock().unwrap();
        while *n >= MAX_LIVE_CHILDREN {
            n = l.room.wait(n).unwrap();
        }
        *n += 1;
        ChildSlot
    }
}

impl Drop for ChildSlot {
    fn drop(&mut self) {
        let l = live();
        let mut n = l.count.lock().unwrap();
        *n -= 1;
        drop(n);
        l.room.notify_one();
    }
}

/// Pre-fork check (#228): on Linux, refuse to fork when the process table is
/// nearly full, with a message that names the real cause (unreaped zombies)
/// instead of surfacing a bare EAGAIN from the child spawn.
pub fn ensure_headroom() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let Some(total) = count_proc_entries() else {
            return Ok(());
        };
        let pid_max = read_pid_max().unwrap_or(4_194_304);
        if total * 100 >= pid_max * 95 {
            let zombies = count_zombies().unwrap_or(0);
            return Err(format!(
                "process table nearly exhausted: {total}/{pid_max} /proc entries, \
                 {zombies} zombie(s); a non-reaping parent is leaking children and fork \
                 would fail with EAGAIN (inspect with tools/procwatch.py)"
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn count_proc_entries() -> Option<usize> {
    let mut n = 0;
    for e in std::fs::read_dir("/proc").ok()?.flatten() {
        if e.file_name()
            .to_str()
            .map(|s| s.bytes().all(|b| b.is_ascii_digit()))
            == Some(true)
        {
            n += 1;
        }
    }
    Some(n)
}

#[cfg(target_os = "linux")]
fn read_pid_max() -> Option<usize> {
    std::fs::read_to_string("/proc/sys/kernel/pid_max")
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(target_os = "linux")]
fn count_zombies() -> Option<usize> {
    let mut n = 0;
    for e in std::fs::read_dir("/proc").ok()?.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{name}/stat")) {
            if let Some(rparen) = stat.rfind(')') {
                if stat[rparen + 1..].split_whitespace().next() == Some("Z") {
                    n += 1;
                }
            }
        }
    }
    Some(n)
}

/// A spawned child that is always reaped and group-killed on drop (#228).
pub struct GuardedChild {
    child: Option<Child>,
    _slot: ChildSlot,
}

impl GuardedChild {
    pub fn id(&self) -> u32 {
        self.child.as_ref().map(Child::id).unwrap_or(0)
    }

    pub fn child_mut(&mut self) -> Option<&mut Child> {
        self.child.as_mut()
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        match self.child.as_mut() {
            Some(c) => c.try_wait(),
            None => Ok(None),
        }
    }

    /// Kill the child's whole process group (unix) so descendants cannot
    /// outlive the test.
    pub fn kill_group(&mut self) {
        if let Some(c) = self.child.as_ref() {
            kill_group(c.id());
        }
    }

    /// Reap the child, consuming the guard (Drop becomes a no-op).
    pub fn wait(mut self) -> std::io::Result<ExitStatus> {
        self.child.take().expect("child taken once").wait()
    }

    /// Reap and collect piped stdout/stderr, consuming the guard.
    pub fn wait_with_output(mut self) -> std::io::Result<Output> {
        self.child
            .take()
            .expect("child taken once")
            .wait_with_output()
    }

    /// Kill the whole group, reap, then drain piped stdout/stderr to EOF
    /// (safe: the group kill also kills descendants holding the pipe).
    pub fn kill_and_collect(mut self) -> std::io::Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
        use std::io::Read;
        let mut c = self.child.take().expect("child taken once");
        kill_group(c.id());
        let _ = c.kill();
        let status = c.wait()?;
        let mut out = Vec::new();
        let mut err = Vec::new();
        if let Some(mut so) = c.stdout.take() {
            let _ = so.read_to_end(&mut out);
        }
        if let Some(mut se) = c.stderr.take() {
            let _ = se.read_to_end(&mut err);
        }
        Ok((status, out, err))
    }
}

impl Drop for GuardedChild {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            kill_group(c.id());
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Spawn `cmd` in its own process group with the #228 headroom + slot checks.
pub fn spawn(cmd: &mut Command) -> std::io::Result<GuardedChild> {
    ensure_headroom().map_err(std::io::Error::other)?;
    let slot = ChildSlot::acquire();
    prepare_group(cmd);
    // git commit runs `git maintenance run --auto --detach`, a child git
    // never waits for; under a non-reaping PID 1 (dev sandbox) each one
    // becomes a permanent zombie (#228). Env config propagates to the git
    // grandchildren spawned by the tissue binary as well.
    cmd.env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "gc.auto")
        .env("GIT_CONFIG_VALUE_0", "0")
        .env("GIT_CONFIG_KEY_1", "maintenance.auto")
        .env("GIT_CONFIG_VALUE_1", "0");
    let child = cmd.spawn()?;
    Ok(GuardedChild {
        child: Some(child),
        _slot: slot,
    })
}

/// `Command::output()` with the #228 safeguards: stdin null, stdout/stderr
/// piped, hard `timeout` after which the whole group is killed, and the
/// child is always reaped.
pub fn output(cmd: &mut Command, timeout: Duration) -> std::io::Result<Output> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run(spawn(cmd)?, None, timeout)
}

/// Like [`output`] but writes `input` to the child's stdin first.
pub fn output_with_input(
    cmd: &mut Command,
    input: &[u8],
    timeout: Duration,
) -> std::io::Result<Output> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run(spawn(cmd)?, Some(input), timeout)
}

fn run(mut g: GuardedChild, input: Option<&[u8]>, timeout: Duration) -> std::io::Result<Output> {
    if let Some(input) = input {
        if let Some(mut stdin) = g.child_mut().and_then(|c| c.stdin.take()) {
            let _ = stdin.write_all(input);
            let _ = stdin.flush();
            drop(stdin);
        }
    }
    let deadline = Instant::now() + timeout;
    loop {
        if g.try_wait()?.is_some() {
            return g.wait_with_output();
        }
        if Instant::now() >= deadline {
            let id = g.id();
            g.kill_group();
            let _ = g.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("child pid {id} exceeded {timeout:?} (group killed and reaped)"),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Write no-auto-maintenance config into a repo (bare remotes included).
/// receive-pack on the REMOTE side decides from the remote's own config
/// whether to spawn the detached `git maintenance run --auto` child, so
/// client-side flags/env cannot cover pushes — the remote repo itself needs
/// `receive.autogc=false` (#228).
pub fn disable_auto_maintenance(dir: &std::path::Path) {
    for (key, value) in [
        ("gc.auto", "0"),
        ("maintenance.auto", "false"),
        ("receive.autogc", "false"),
    ] {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(dir).args(["config", key, value]);
        output(&mut cmd, default_timeout()).expect("git config no-auto-maintenance");
    }
}

#[cfg(unix)]
fn prepare_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            if libc_setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn prepare_group(_cmd: &mut Command) {}

#[cfg(unix)]
pub fn kill_group(pid: u32) {
    if pid == 0 {
        return;
    }
    unsafe {
        libc_kill(-(pid as i32), 9);
    }
}

#[cfg(not(unix))]
pub fn kill_group(_pid: u32) {}

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
