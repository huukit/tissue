//! Project mapping: named groupings of issue-id prefixes (#79, #119, #135).
//!
//! The single source of truth is the `projects:` block in
//! `.tissue/config.yml`. CLI (`tissue project`) and the write API
//! (`POST /api/projects`) both go through this module so their validation
//! and file handling cannot drift (#118's lesson, applied up front).

use std::path::Path;

use crate::git::{Author, Repo};

pub struct Projects {
    repo: Repo,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProjectError {
    Io(String),
    InvalidName(String),
    InvalidPrefix(String),
    DuplicatePrefix {
        prefix: String,
        project: String,
    },
    NameMapsToOtherPrefix {
        name: String,
        prefix: String,
    },
    UnknownName(String),
    HasIssues {
        name: String,
        prefix: String,
        count: usize,
        example: String,
    },
    IsDefault(String),
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::Io(e) => write!(f, "project config error: {e}"),
            ProjectError::InvalidName(name) => write!(
                f,
                "invalid project name `{name}`: use letters, digits, `-` or `_`"
            ),
            ProjectError::InvalidPrefix(prefix) => write!(
                f,
                "invalid prefix `{prefix}`: must be uppercase ASCII letters (e.g. IAM)"
            ),
            ProjectError::DuplicatePrefix { prefix, project } => write!(
                f,
                "prefix {prefix} is already mapped to project `{project}`"
            ),
            ProjectError::NameMapsToOtherPrefix { name, prefix } => write!(
                f,
                "project `{name}` already maps to prefix {prefix}; remove it first or add with the same name to overwrite"
            ),
            ProjectError::UnknownName(name) => {
                write!(f, "unknown project `{name}`: no mapping in config.yml")
            }
            ProjectError::HasIssues { name, prefix, count, example } => write!(
                f,
                "project `{name}` has {count} issue(s) with prefix {prefix} (e.g. {example}); use --force to remove the mapping anyway"
            ),
            ProjectError::IsDefault(name) => write!(
                f,
                "project `{name}` is the default project in config.yml; change `project:` first"
            ),
        }
    }
}

impl Projects {
    pub fn new(repo: Repo) -> Self {
        Projects { repo }
    }

    fn config_path(&self) -> std::path::PathBuf {
        self.repo
            .resolve(&[".tissue", "config.yml"])
            .unwrap_or_else(|_| self.repo.root().join(".tissue").join("config.yml"))
    }

    /// The `projects:` mapping in file order.
    pub fn list(&self) -> Result<Vec<(String, String)>, String> {
        let path = self.config_path();
        let config = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let mut projects = Vec::new();
        let mut in_projects = false;
        for line in config.lines() {
            if line.starts_with("projects:") {
                in_projects = true;
                continue;
            }
            if in_projects {
                let trimmed = line.trim_start();
                if trimmed.starts_with('#') || trimmed.is_empty() {
                    continue;
                }
                if !line.starts_with(' ') {
                    break;
                }
                if let Some((name, prefix)) = trimmed.split_once(':') {
                    let prefix = prefix.trim().trim_matches('"');
                    if !prefix.is_empty() {
                        projects.push((name.trim().to_string(), prefix.to_string()));
                    }
                }
            }
        }
        Ok(projects)
    }

    /// The default `project:` entry from config.yml ("" if unset).
    pub fn default_name(&self) -> Result<String, String> {
        let config = std::fs::read_to_string(self.config_path())
            .map_err(|e| format!("reading config.yml: {e}"))?;
        for line in config.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("project:") {
                if trimmed.starts_with("projects:") {
                    continue;
                }
                let name = rest.trim().trim_matches('"');
                if !name.is_empty() {
                    return Ok(name.to_string());
                }
            }
        }
        Ok(String::new())
    }

    /// Add (or overwrite) a mapping; one commit on success.
    pub fn add(&self, name: &str, prefix: &str, actor: &Author) -> Result<(), ProjectError> {
        if !valid_project_name(name) {
            return Err(ProjectError::InvalidName(name.to_string()));
        }
        if !valid_project_prefix(prefix) {
            return Err(ProjectError::InvalidPrefix(prefix.to_string()));
        }
        let projects = self.list().map_err(ProjectError::unknown_io)?;
        if let Some((_, existing)) = projects.iter().find(|(n, _)| n == name) {
            if existing != prefix {
                return Err(ProjectError::NameMapsToOtherPrefix {
                    name: name.to_string(),
                    prefix: existing.clone(),
                });
            }
        }
        if let Some((other, _)) = projects.iter().find(|(n, p)| p == prefix && n != name) {
            return Err(ProjectError::DuplicatePrefix {
                prefix: prefix.to_string(),
                project: other.clone(),
            });
        }
        let mut updated = projects.clone();
        updated.retain(|(n, _)| n != name);
        updated.push((name.to_string(), prefix.to_string()));
        self.write_projects(&updated)
            .map_err(ProjectError::unknown_io)?;
        self.commit(&format!("project add: {name} -> {prefix}"), actor)
            .map_err(ProjectError::unknown_io)?;
        Ok(())
    }

    /// Remove a mapping; one commit on success. Refuses while issues with
    /// the prefix exist unless `force`; always refuses the default project.
    pub fn remove(&self, name: &str, force: bool, actor: &Author) -> Result<(), ProjectError> {
        let projects = self.list().map_err(ProjectError::unknown_io)?;
        let prefix = projects
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, p)| p.clone())
            .ok_or_else(|| ProjectError::UnknownName(name.to_string()))?;
        let existing = self
            .issues_with_prefix(&prefix)
            .map_err(ProjectError::unknown_io)?;
        if !existing.is_empty() && !force {
            return Err(ProjectError::HasIssues {
                name: name.to_string(),
                prefix: prefix.clone(),
                count: existing.len(),
                example: existing[0].clone(),
            });
        }
        if name == self.default_name().map_err(ProjectError::unknown_io)? {
            return Err(ProjectError::IsDefault(name.to_string()));
        }
        let mut updated = projects.clone();
        updated.retain(|(n, _)| n != name);
        self.write_projects(&updated)
            .map_err(ProjectError::unknown_io)?;
        self.commit(&format!("project remove: {name} ({prefix})"), actor)
            .map_err(ProjectError::unknown_io)?;
        Ok(())
    }

    fn issues_with_prefix(&self, prefix: &str) -> Result<Vec<String>, String> {
        let mut found = Vec::new();
        let dir = self
            .repo
            .resolve(&[".tissue", "issues"])
            .unwrap_or_else(|_| self.repo.root().join(".tissue").join("issues"));
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return Ok(found),
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            if let Some(rest) = name.strip_prefix(&format!("{prefix}-")) {
                if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                    found.push(name.to_string());
                }
            }
        }
        found.sort();
        Ok(found)
    }

    /// Rewrite the `projects:` block, preserving everything outside it.
    fn write_projects(&self, projects: &[(String, String)]) -> Result<(), String> {
        let path = self.config_path();
        let config = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let mut out = String::new();
        let mut in_projects = false;
        let mut wrote_block = false;
        for line in config.lines() {
            if line.starts_with("projects:") {
                out.push_str("projects:\n");
                for (name, prefix) in projects {
                    out.push_str(&format!("  {name}: {prefix}\n"));
                }
                in_projects = true;
                wrote_block = true;
                continue;
            }
            if in_projects {
                let trimmed = line.trim_start();
                if trimmed.starts_with('#') || trimmed.is_empty() {
                    continue;
                }
                if line.starts_with(' ') {
                    continue;
                }
                in_projects = false;
            }
            out.push_str(line);
            out.push('\n');
        }
        if !wrote_block {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("projects:\n");
            for (name, prefix) in projects {
                out.push_str(&format!("  {name}: {prefix}\n"));
            }
        }
        std::fs::write(&path, out).map_err(|e| format!("writing {}: {e}", path.display()))
    }

    fn commit(&self, message: &str, author: &Author) -> Result<String, String> {
        self.repo
            .commit_all(author, message)
            .map_err(|e| e.to_string())
    }
}

impl ProjectError {
    fn unknown_io(e: String) -> ProjectError {
        ProjectError::Io(e)
    }
}

/// A project name must be a valid config key: letters, digits, `-`, `_`.
pub fn valid_project_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A prefix must match the issue-id charset: uppercase ASCII letters.
pub fn valid_project_prefix(prefix: &str) -> bool {
    !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_uppercase())
}

/// Resolve a project name to its prefix via config.yml (used by
/// `list --project` / `serve --project`).
/// #172/#176: shared create-time validation — resolve the project to its
/// prefix and check the issue id matches it. `project: None` means the
/// configured default project; there is never a hardcoded prefix fallback.
pub fn validate_create_id(repo: &Path, project: Option<&str>, id: &str) -> Result<String, String> {
    let grepo = crate::git::Repo::open(repo).map_err(|e| format!("opening git repo: {e}"))?;
    let projects = Projects::new(grepo);
    let list = projects.list()?;
    let name = match project {
        Some(n) => n.to_string(),
        None => projects.default_name()?,
    };
    let Some((_, prefix)) = list.iter().find(|(n, _)| n == &name) else {
        return Err(format!(
            "unknown project `{name}` — configured projects: {}",
            list.iter()
                .map(|(n, p)| format!("{n} ({p})"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    };
    if !id.starts_with(&format!("{prefix}-")) {
        return Err(format!(
            "id `{id}` does not match project `{name}` — its prefix is {prefix}, so the id must start with `{prefix}-`"
        ));
    }
    Ok(prefix.clone())
}

pub fn prefix_for(repo: &Path, name: &str) -> Result<String, String> {
    let config = std::fs::read_to_string(repo.join(".tissue").join("config.yml"))
        .map_err(|e| format!("reading config.yml: {e}"))?;
    let key = format!("{name}:");
    let mut in_projects = false;
    for line in config.lines() {
        let trimmed = line.trim_start();
        if line.starts_with("projects:") {
            in_projects = true;
            continue;
        }
        if in_projects {
            if trimmed.starts_with('#') || trimmed.is_empty() {
                continue;
            }
            if !line.starts_with(' ') {
                in_projects = false;
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix(&key) {
                let prefix = rest.trim().trim_matches('"');
                if !prefix.is_empty() {
                    return Ok(prefix.to_string());
                }
            }
        }
    }
    Err(format!(
        "unknown project `{name}`: no mapping in config.yml"
    ))
}
