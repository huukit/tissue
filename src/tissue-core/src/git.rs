//! Git operations: every mutation is a commit; offline local repo only (PoC).
//!
//! PoC decision: shell out to the git CLI; `gix` deferred (ADR-001).
//! A [`Repo`] is a working tree plus its `.git`; operations stay inside it.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::human;

#[derive(Debug)]
pub enum Error {
    /// git CLI missing or unusable.
    GitUnavailable,
    Io(String),
    /// git exited non-zero: (args summary, stderr).
    CommandFailed(String, String),
    /// A path resolved outside the issue repo working tree.
    PathOutsideRepo(String),
    Human(human::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::GitUnavailable => write!(f, "git CLI not found or not executable"),
            Error::Io(m) => write!(f, "io error: {m}"),
            Error::CommandFailed(args, stderr) => write!(f, "git {args} failed: {stderr}"),
            Error::PathOutsideRepo(p) => write!(f, "path escapes the issue repo: {p}"),
            Error::Human(e) => write!(f, "human format error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<human::Error> for Error {
    fn from(e: human::Error) -> Self {
        Error::Human(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A commit author, expressed as git name/email. Human approvals are
/// authored by the human identity; agent writes by the agent identity
/// (issue #12: "author = the actor").
#[derive(Debug, Clone)]
pub struct Author {
    pub name: String,
    pub email: String,
}

/// Run one git command inside the repo working tree.
fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::GitUnavailable,
            _ => Error::Io(e.to_string()),
        })?;
    if !output.status.success() {
        return Err(Error::CommandFailed(
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// An issue repo: a git working tree containing `.tissue/`.
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    /// Wrap an existing working tree; verifies it is a git repository.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        git(&root, &["rev-parse", "--git-dir"])?;
        Ok(Repo { root })
    }

    /// `tissue init`: create a fresh issue repo at `root`
    /// (git init + initial empty commit + `.tissue/` skeleton).
    pub fn init(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        git(&root, &["init"])?;
        git(&root, &["config", "user.name", "tissue"])?;
        git(&root, &["config", "user.email", "tissue@localhost"])?;
        let tissue = root.join(".tissue");
        std::fs::create_dir_all(tissue.join("issues"))?;
        std::fs::create_dir_all(tissue.join("releases"))?;
        let repo = Repo { root: root.clone() };
        // Initial (empty) commit so every later mutation has a base; keeps history linear.
        git(
            &root,
            &[
                "-c",
                "user.name=tissue",
                "-c",
                "user.email=tissue@localhost",
                "commit",
                "--allow-empty",
                "-m",
                "tissue init",
            ],
        )?;
        Ok(repo)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a relative path inside the working tree, rejecting traversal.
    pub fn resolve(&self, rel: &[&str]) -> Result<PathBuf> {
        let mut path = self.root.clone();
        for part in rel {
            if part.contains("..") || part.contains('/') || part.contains('\\') || part.is_empty() {
                return Err(Error::PathOutsideRepo(part.to_string()));
            }
            path.push(part);
        }
        Ok(path)
    }

    /// The last commit's short id, for tests and diagnostics.
    pub fn head_short(&self) -> Result<String> {
        Ok(git(&self.root, &["rev-parse", "--short", "HEAD"])?
            .trim()
            .to_string())
    }

    /// Stage everything and create exactly one commit. Returns its short id.
    pub fn commit_all(&self, author: &Author, message: &str) -> Result<String> {
        git(&self.root, &["add", "-A"])?;
        git(
            &self.root,
            &[
                "-c",
                &format!("user.name={}", author.name),
                "-c",
                &format!("user.email={}", author.email),
                "commit",
                "-m",
                message,
            ],
        )?;
        self.head_short()
    }

    /// Stage everything and create one commit; nothing if the tree is clean.
    pub fn commit_all_if_changed(&self, author: &Author, message: &str) -> Result<Option<String>> {
        let status = git(&self.root, &["status", "--porcelain"])?;
        if status.trim().is_empty() {
            return Ok(None);
        }
        self.commit_all(author, message).map(Some)
    }

    /// Create an annotated tag `name` at HEAD. Fails if the tag exists.
    pub fn create_tag(&self, name: &str, message: &str) -> Result<()> {
        git(
            &self.root,
            &[
                "-c",
                "user.name=tissue",
                "-c",
                "user.email=tissue@localhost",
                "tag",
                "-a",
                name,
                "-m",
                message,
            ],
        )
        .map(|_| ())
    }

    /// Number of commits reachable from HEAD (commit-per-mutation check).
    pub fn commit_count(&self) -> Result<usize> {
        let out = git(&self.root, &["rev-list", "--count", "HEAD"])?;
        out.trim()
            .parse::<usize>()
            .map_err(|_| Error::CommandFailed("rev-list --count".into(), out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_repo(tag: &str) -> (PathBuf, Repo) {
        let dir =
            std::env::temp_dir().join(format!("tissue-git-test-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = Repo::init(&dir).unwrap();
        (dir, repo)
    }

    #[test]
    fn init_creates_linear_history_with_one_commit() {
        let (dir, repo) = temp_repo("init");
        assert!(dir.join(".tissue/issues").is_dir());
        assert!(dir.join(".tissue/releases").is_dir());
        assert_eq!(repo.commit_count().unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mutation_produces_exactly_one_commit_with_actor_author() {
        let (dir, repo) = temp_repo("commit");
        let author = Author {
            name: "agent:dev-1".into(),
            email: "agent-dev-1@tissue.local".into(),
        };
        std::fs::write(dir.join(".tissue/issues/IAM-1.md"), "x").unwrap();
        let head = repo.commit_all(&author, "add issue").unwrap();
        assert_eq!(repo.commit_count().unwrap(), 2);
        let log = git(&dir, &["log", "-1", "--format=%an %s"]).unwrap();
        assert!(log.contains("agent:dev-1"));
        assert!(log.contains("add issue"));
        assert_eq!(repo.head_short().unwrap(), head);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clean_tree_commit_is_skipped() {
        let (dir, repo) = temp_repo("clean");
        let author = Author {
            name: "n".into(),
            email: "e".into(),
        };
        assert!(repo
            .commit_all_if_changed(&author, "noop")
            .unwrap()
            .is_none());
        assert_eq!(repo.commit_count().unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_traversal_is_rejected() {
        let (dir, repo) = temp_repo("traversal");
        assert!(repo
            .resolve(&[".tissue", "issues", "..", "..", "etc"])
            .is_err());
        assert!(repo.resolve(&[".tissue", "is/sues"]).is_err());
        assert!(repo.resolve(&[".tissue", ""]).is_err());
        assert!(repo
            .resolve(&[".tissue", "issues", "IAM-42", "issue.md"])
            .is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_rejects_non_git_dir() {
        let dir =
            std::env::temp_dir().join(format!("tissue-git-test-nogit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(matches!(Repo::open(&dir), Err(Error::CommandFailed(_, _))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
