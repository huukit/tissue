//! Project bootstrap (#37): `tissue init` with remote configuration and
//! automatic GitHub import.
//!
//! Three sources:
//! - bare `tissue init` — fresh empty issue repo (unchanged behavior)
//! - `--remote PATH` — filesystem remote, fully offline
//! - `--from-github OWNER/REPO` — import existing GitHub issues via `gh`
//!
//! Import reuses the same `gh issue view --json` pipeline as
//! `tools/import_github_issues.py` (#15): the script remains available for
//! power use, but the documented owner path needs no separate tool.

use std::path::{Path, PathBuf};
use std::process::Command;

use tissue_core::git::{Author, Repo};
use tissue_core::human::NEEDS_REVIEW;

fn tissue_author() -> Author {
    Author {
        name: "tissue".to_string(),
        email: "tissue@localhost".to_string(),
    }
}

pub struct InitOptions {
    pub project: Option<String>,
    pub from_github: Option<String>,
    pub remote: Option<PathBuf>,
    pub dir: Option<PathBuf>,
    pub id_prefix: String,
    pub dry_run: bool,
}

struct IssueJson {
    number: u64,
    title: String,
    body: String,
    state: String,
    labels: Vec<String>,
    url: String,
    created_at: String,
    updated_at: String,
}

pub fn run(repo_root: &Path, opts: &InitOptions) -> Result<(), String> {
    let project = opts
        .project
        .clone()
        .unwrap_or_else(|| default_project_name(repo_root));
    let target = match &opts.dir {
        Some(d) => d.clone(),
        None => repo_root.join(format!("{}-issues", project)),
    };

    if opts.dry_run {
        return dry_run_report(&project, &target, opts);
    }

    let uses_target = opts.from_github.is_some() || opts.remote.is_some() || opts.dir.is_some();
    let issue_repo = if uses_target {
        target
    } else {
        repo_root.to_path_buf()
    };

    if issue_repo.join(".tissue").exists() {
        return Err(format!(
            "repository already initialized at {}",
            issue_repo.display()
        ));
    }

    init_issue_repo(&issue_repo, &project, opts.remote.as_deref())?;

    if let Some(gh_repo) = &opts.from_github {
        import_github(gh_repo, &issue_repo, &opts.id_prefix)?;
    }

    println!(
        "initialized tissue repository for `{project}` in {}",
        issue_repo.display()
    );
    if let Some(remote) = &opts.remote {
        println!("remote origin: {}", remote.display());
    }
    if let Some(gh) = &opts.from_github {
        println!("imported issues from {gh}");
    }
    Ok(())
}

fn dry_run_report(project: &str, target: &Path, opts: &InitOptions) -> Result<(), String> {
    println!("dry run: would initialize issue repo for `{project}`");
    if opts.from_github.is_some() || opts.remote.is_some() || opts.dir.is_some() {
        println!("  location: {}", target.display());
    } else {
        println!("  location: current directory");
    }
    if let Some(remote) = &opts.remote {
        println!("  remote origin: {}", remote.display());
    }
    if let Some(gh) = &opts.from_github {
        println!("  import: all issues from {gh} (prefix {})", opts.id_prefix);
    }
    Ok(())
}

fn default_project_name(repo_root: &Path) -> String {
    repo_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "myproject".to_string())
}

fn init_issue_repo(root: &Path, project: &str, remote: Option<&Path>) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("creating {}: {e}", root.display()))?;
    let grepo = Repo::init(root).map_err(|e| format!("initializing repository: {e}"))?;

    let remote_line = remote
        .map(|r| format!("remote: {}\n", escape_remote(r)))
        .unwrap_or_default();
    let config = format!(
        "# Tissue configuration.\nproject: {project}\n{remote_line}# Timestamps are RFC3339 UTC, e.g. 2026-10-08T09:00:00Z\n",
        project = yaml_scalar(project),
        remote_line = remote_line,
    );
    let config_path = root.join(".tissue").join("config.yml");
    std::fs::write(&config_path, config).map_err(|e| format!("writing config.yml: {e}"))?;
    grepo
        .commit_all_if_changed(&tissue_author(), "tissue init: config.yml")
        .map_err(|e| format!("committing config.yml: {e}"))?;

    if let Some(remote_path) = remote {
        set_filesystem_remote(&grepo, remote_path)?;
    }
    Ok(())
}

fn set_filesystem_remote(grepo: &Repo, remote_path: &Path) -> Result<(), String> {
    if !remote_path.exists() {
        let parent = remote_path
            .parent()
            .ok_or_else(|| format!("invalid remote path: {}", remote_path.display()))?;
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating remote parent {}: {e}", parent.display()))?;
        git_command(&["init", remote_path.to_string_lossy().as_ref()])?;
    }
    git_command(&[
        "-C",
        grepo.root().to_string_lossy().as_ref(),
        "remote",
        "add",
        "origin",
        remote_path.to_string_lossy().as_ref(),
    ])
}

fn git_command(args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("running git {}: {e}", args.join(" ")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn import_github(gh_repo: &str, issue_repo: &Path, id_prefix: &str) -> Result<(), String> {
    let numbers = list_github_issues(gh_repo)?;
    if numbers.is_empty() {
        println!("no open issues found in {gh_repo}; nothing to import");
        return Ok(());
    }
    let mut count = 0;
    for number in numbers {
        let issue = fetch_github_issue(gh_repo, number)?;
        let tid = format!("{id_prefix}-{number}");
        let dir = issue_repo.join(".tissue").join("issues").join(&tid);
        std::fs::create_dir_all(&dir).map_err(|e| format!("creating {tid}: {e}"))?;
        let content = render_issue_md(&issue, &tid, gh_repo);
        std::fs::write(dir.join("issue.md"), content).map_err(|e| format!("writing {tid}: {e}"))?;
        count += 1;
    }
    let grepo = Repo::open(issue_repo).map_err(|e| format!("opening repo: {e}"))?;
    let msg = format!("Import {count} issue(s) from GitHub");
    grepo
        .commit_all_if_changed(&tissue_author(), &msg)
        .map_err(|e| format!("committing import: {e}"))?;
    Ok(())
}

fn list_github_issues(gh_repo: &str) -> Result<Vec<u64>, String> {
    let out = Command::new("gh")
        .args([
            "issue", "list", "--repo", gh_repo, "--state", "open", "--limit", "1000", "--json",
            "number",
        ])
        .output()
        .map_err(|e| format!("running gh: {e} (is it installed and authenticated?)"))?;
    if !out.status.success() {
        return Err(format!(
            "gh issue list for {gh_repo} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let numbers = parse_json_numbers(&stdout)?;
    Ok(numbers)
}

fn parse_json_numbers(stdout: &str) -> Result<Vec<u64>, String> {
    let v: serde_json::Value =
        serde_json::from_str(stdout).map_err(|e| format!("gh output: {e}"))?;
    let arr = v
        .as_array()
        .ok_or_else(|| "gh output: expected array".to_string())?;
    let mut out = Vec::new();
    for item in arr {
        let n = item
            .get("number")
            .and_then(|n| n.as_u64())
            .ok_or_else(|| "gh output: missing number".to_string())?;
        out.push(n);
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

fn fetch_github_issue(gh_repo: &str, number: u64) -> Result<IssueJson, String> {
    let out = Command::new("gh")
        .args([
            "issue",
            "view",
            &number.to_string(),
            "--repo",
            gh_repo,
            "--json",
            "number,title,body,state,labels,url,createdAt,updatedAt",
        ])
        .output()
        .map_err(|e| format!("running gh: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "gh issue view {gh_repo}#{number} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("gh output for #{number}: {e}"))?;
    Ok(IssueJson {
        number: v
            .get("number")
            .and_then(|n| n.as_u64())
            .ok_or_else(|| format!("gh output for #{number}: missing number"))?,
        title: string_field(&v, "title", number)?,
        body: v
            .get("body")
            .and_then(|b| b.as_str())
            .unwrap_or("")
            .to_string(),
        state: string_field(&v, "state", number)?.to_lowercase(),
        labels: v
            .get("labels")
            .and_then(|l| l.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|l| l.get("name").and_then(|n| n.as_str()))
                    .filter(|n| *n != NEEDS_REVIEW)
                    .map(|n| n.to_string())
                    .collect()
            })
            .unwrap_or_default(),
        url: string_field(&v, "url", number)?,
        created_at: string_field(&v, "createdAt", number)?,
        updated_at: string_field(&v, "updatedAt", number)?,
    })
}

fn string_field(v: &serde_json::Value, key: &str, number: u64) -> Result<String, String> {
    v.get(key)
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("gh output for #{number}: missing {key}"))
}

fn render_issue_md(issue: &IssueJson, tid: &str, gh_repo: &str) -> String {
    let now = now_rfc3339();
    let status = if issue.state == "closed" {
        "closed"
    } else {
        "open"
    };
    let mut sections = split_sections(&issue.body);
    let summary = sections
        .remove("Summary")
        .filter(|s| !s.is_empty())
        .or_else(|| sections.remove("Details").filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "(imported; no summary in source)".to_string());
    let details = sections.remove("Details").unwrap_or_default();
    let acceptance = sections.remove("Acceptance criteria").unwrap_or_default();
    let notes = sections.remove("Notes").unwrap_or_default();
    let provenance = format!(
        "Imported from GitHub issue [{}#{}]({}) on {} by `tissue init --from-github` (#37). Original state: {}.",
        gh_repo, issue.number, issue.url, now, issue.state
    );

    let mut parts = vec![
        "---".to_string(),
        format!("id: {tid}"),
        format!("title: {}", yaml_scalar(&single_line(&issue.title))),
        format!("status: {status}"),
        "release:".to_string(),
        format!("tags: [{}]", issue.labels.join(", ")),
        format!("created: {}", issue.created_at,),
        format!("updated: {}", issue.updated_at),
        "---".to_string(),
        String::new(),
        "## Summary".to_string(),
        summary,
        String::new(),
    ];
    if !acceptance.is_empty() {
        parts.push("## Acceptance criteria".to_string());
        parts.push(acceptance);
        parts.push(String::new());
    }
    if !details.is_empty() {
        parts.push("## Details".to_string());
        parts.push(details);
        parts.push(String::new());
    }
    parts.push("## Notes".to_string());
    if !notes.is_empty() {
        parts.push(notes);
        parts.push(String::new());
    }
    parts.push(provenance);
    parts.push(String::new());
    parts.join("\n")
}

fn split_sections(body: &str) -> std::collections::BTreeMap<String, String> {
    let mut sections = std::collections::BTreeMap::new();
    let mut current = "Summary".to_string();
    let mut lines = Vec::new();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            let name = rest.trim();
            sections.insert(current.clone(), lines.join("\n").trim().to_string());
            current = name.to_string();
            lines.clear();
        } else {
            lines.push(line);
        }
    }
    sections.insert(current, lines.join("\n").trim().to_string());
    sections
}

fn single_line(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn yaml_scalar(value: &str) -> String {
    if value.is_empty() || value.contains(": ") || value.contains('\n') || value.starts_with('[') {
        format!("{:?}", value)
    } else {
        value.to_string()
    }
}

fn escape_remote(path: &Path) -> String {
    yaml_scalar(&path.to_string_lossy())
}

fn now_rfc3339() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days = secs / 86400;
    let (year, month, day) = civil_from_days(days as i64);
    let rem = secs % 86400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_split_on_headers() {
        let s = split_sections("intro\n## Summary\nsum body\n## Details\ndet");
        assert_eq!(s["Summary"], "sum body");
        assert_eq!(s["Details"], "det");
    }

    #[test]
    fn sections_keeps_leading_text_in_summary() {
        let s = split_sections("loose leading text");
        assert_eq!(s["Summary"], "loose leading text");
    }

    #[test]
    fn yaml_scalar_quotes_risky_values() {
        assert_eq!(yaml_scalar("plain"), "plain");
        assert_eq!(yaml_scalar("a: b"), "\"a: b\"");
        assert_eq!(yaml_scalar(""), "\"\"");
    }

    #[test]
    fn single_line_collapses_whitespace() {
        assert_eq!(single_line("a  b\n c"), "a b c");
    }

    #[test]
    fn civil_from_days_matches_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn parse_json_numbers_sorts() {
        let n = parse_json_numbers(r#"[{"number":3},{"number":1}]"#).unwrap();
        assert_eq!(n, vec![1, 3]);
    }
}
