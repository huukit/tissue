//! #226: `init --from-github` preflights the `gh` CLI — a missing gh (e.g.
//! under sudo, whose secure_path drops it from PATH) fails fast with a clear
//! hint, and an unauthenticated gh is reported distinctly. Both must happen
//! before anything is written to disk.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "tissue-gh-preflight-{}-{}-{}",
            label,
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let s = Self { root };
        // git init so the scratch dir is a sane working dir, like e2e.rs.
        let out = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&s.root)
            .output()
            .expect("run git");
        assert!(out.status.success(), "git init failed: {out:?}");
        s
    }

    fn tissue_with_path(&self, path: &str, args: &[&str]) -> Output {
        let exe: PathBuf = env!("CARGO_BIN_EXE_tissue").into();
        Command::new(exe)
            .args(args)
            .current_dir(&self.root)
            .env_remove("EDITOR")
            .env("PATH", path)
            .output()
            .expect("run tissue binary")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// #226 AC: gh missing from PATH -> clear hint (not a raw OS error), nothing
/// written.
#[test]
fn missing_gh_fails_with_clear_hint_226() {
    let scratch = Scratch::new("no-gh");
    let empty_bin = scratch.root.join("empty-bin");
    std::fs::create_dir_all(&empty_bin).expect("create empty bin dir");
    let out = scratch.tissue_with_path(
        empty_bin.to_str().unwrap(),
        &[
            "init",
            "--from-github",
            "huukit/tissue",
            "--dir",
            "target-issues",
        ],
    );
    assert!(!out.status.success(), "must fail without gh");
    let err = stderr(&out);
    assert!(
        err.contains("`gh` CLI not found on PATH"),
        "clear hint in error: {err}"
    );
    assert!(
        err.contains("sudo -E") && err.contains("secure_path"),
        "sudo/secure_path hint: {err}"
    );
    assert!(
        !scratch.root.join("target-issues").exists(),
        "preflight fails before anything is written"
    );
}

/// #226: gh present but `gh auth status` fails -> distinct "not
/// authenticated" error pointing at the auth command.
#[test]
fn unauthenticated_gh_reports_auth_hint_226() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("noauth-gh");
    let fake_bin = scratch.root.join("fake-bin");
    std::fs::create_dir_all(&fake_bin).expect("create fake bin dir");
    let gh = fake_bin.join("gh");
    std::fs::write(
        &gh,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'gh version 2.0.0'; exit 0; fi\necho 'not logged in' >&2; exit 1\n",
    )
    .expect("write fake gh");
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = scratch.tissue_with_path(
        &path,
        &[
            "init",
            "--from-github",
            "huukit/tissue",
            "--dir",
            "target-issues",
        ],
    );
    assert!(
        !out.status.success(),
        "must fail when gh is unauthenticated"
    );
    let err = stderr(&out);
    let auth_cmd = format!("gh auth {}", "login");
    assert!(
        err.contains("not authenticated") && err.contains(&auth_cmd),
        "auth hint in error: {err}"
    );
    assert!(
        !scratch.root.join("target-issues").exists(),
        "preflight fails before anything is written"
    );
}
