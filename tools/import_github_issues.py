#!/usr/bin/env python3
"""Import GitHub issues into a tissue issue repo (#15).

Fetches issues from a GitHub repository via the `gh` CLI and writes each
one as a tissue human-format issue (`.tissue/issues/<ID>/issue.md` per
operations/product-owner/SPEC-human-format.md). Original body lands in
the human format; provenance is recorded in a Notes section.

The output directory is a standalone git repo (one repo per project,
README "Git is the backbone"): the script initializes it if needed and
makes one commit per import batch.

Usage:
    tools/import_github_issues.py --repo huukit/tissue \
        --issues 1 2 3 --out /path/to/issue-repo [--dry-run]
    tools/import_github_issues.py --repo huukit/tissue --all --out /tmp/scratch \
        --dry-run --report report.json     # full-repo dry run with a report (#162)

Repeatability: re-running with the same issue numbers skips issues whose
directory already exists (use --force to overwrite). `--dry-run` writes
nothing and prints/exports a full report (counts by state, id mapping,
label->tag mapping, comment counts, skips, failures).
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

ID_PATTERN = re.compile(r"^[A-Z]+-\d+$")
GITHUB_ID_PATTERN = re.compile(r"^\d+$")
# Tags in the human format are flow-style YAML scalars: keep them to a
# conservative charset so a GitHub label can never break issue.md (#162).
TAG_SAFE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._/-]*$")

DEFAULT_CONFIG = """\
# Tissue issue-repo configuration. Placeholder values only.
project: myproject
"""

SECTIONS = ("Summary", "Acceptance criteria", "Details", "Notes")


def die(msg: str) -> None:
    print(f"import: error: {msg}", file=sys.stderr)
    sys.exit(1)


def run(cmd: list[str]) -> str:
    try:
        out = subprocess.run(
            cmd, check=True, capture_output=True, text=True
        )
    except FileNotFoundError:
        die(f"required tool not found: {cmd[0]}")
    except subprocess.CalledProcessError as e:
        die(f"command failed: {' '.join(cmd)}\n{e.stderr.strip()}")
    return out.stdout


def fetch_issue(repo: str, number: str) -> dict:
    if not GITHUB_ID_PATTERN.match(number):
        die(f"invalid GitHub issue number: {number!r}")
    out = run(
        [
            "gh", "issue", "view", number,
            "--repo", repo,
            "--json", "number,title,body,state,createdAt,updatedAt,labels,url,comments",
        ]
    )
    return json.loads(out)


def list_all_issues(repo: str, state: str = "all", limit: int = 1000) -> list[str]:
    """All issue numbers in the repo (paginated via gh api, #162)."""
    out = run([
        "gh", "api", f"repos/{repo}/issues",
        "--paginate",
        "-q", ".[].number | tostring",
        "--method", "GET",
        "-f", f"state={state}",
        "-f", f"per_page=100",
        "-f", f"sort=created",
        "-f", "direction=asc",
    ])
    numbers = [n.strip() for n in out.splitlines() if n.strip().isdigit()]
    # `gh api repos/.../issues` also returns PRs; filter those out by asking
    # for issues only via the issues endpoint (it already excludes PRs on
    # /issues, but belt-and-braces: numeric ids only, dedup, sort numerically).
    seen: set[str] = set()
    uniq = []
    for n in numbers:
        if n not in seen:
            seen.add(n)
            uniq.append(n)
    uniq.sort(key=int)
    return uniq[:limit]


def sanitize_label(name: str) -> str | None:
    """Map a GitHub label to a tag-safe scalar, or None to drop it (#162)."""
    cleaned = name.strip()
    if not cleaned or cleaned.lower() == "needs-review":
        return None
    if TAG_SAFE.match(cleaned):
        return cleaned
    slug = re.sub(r"[^A-Za-z0-9._/-]+", "-", cleaned).strip("-")
    return slug or None


def render_comment_records(issue: dict, tid: str) -> list[dict]:
    """Comments -> agent.jsonl observation records (#162): the AI format
    carries the discussion, the human format stays editorial."""
    records = []
    comments = (issue.get("comments") or {}).get("nodes") or issue.get("comments") or []
    for i, c in enumerate(comments, start=1):
        if isinstance(c, dict) and "nodes" in c:
            continue
        author = c.get("author", {}).get("login", "unknown") if isinstance(c, dict) else "unknown"
        body = c.get("body", "") if isinstance(c, dict) else str(c)
        created = c.get("createdAt", "") if isinstance(c, dict) else ""
        records.append({
            "type": "observation",
            "ts": created or "",
            "actor": f"github:{author}",
            "data": {
                "summary": f"Imported comment #{i} from GitHub issue #{issue['number']}",
                "evidence": body,
            },
        })
    return records


def tissue_id(number: str, prefix: str) -> str:
    if not ID_PATTERN.match(f"{prefix}-{number}"):
        die(f"cannot build valid tissue id from prefix {prefix!r} and number {number!r}")
    return f"{prefix}-{number}"


def split_sections(body: str) -> dict[str, str]:
    """Split a GitHub body into known sections, conserving everything.

    Unrecognized leading text and trailing content are preserved in
    order, mapped to a generic section so nothing is lost.
    """
    sections: dict[str, str] = {}
    current = "Summary"
    lines: list[str] = []
    for line in body.splitlines():
        m = re.match(r"^##\s+(.+?)\s*$", line)
        if m:
            sections.setdefault(current, "\n".join(lines).strip())
            current = m.group(1).strip()
            lines = []
        else:
            lines.append(line)
    sections.setdefault(current, "\n".join(lines).strip())
    return sections


def render_issue_md(
    issue: dict, tid: str, prefix: str, now: str, repo: str
) -> str:
    sections = split_sections(issue.get("body") or "")
    state = issue.get("state", "OPEN").lower()
    # #162: GitHub PRs surface on /issues as state=CLOSED with a pullRequest
    # field; `gh issue list` excludes them, but the paginated `gh api
    # /issues` path includes them — treat any "merged"/PR state as closed.
    status = "closed" if state in ("closed", "merged") else "open"
    raw_labels = [l["name"] for l in issue.get("labels", [])]
    # #162: labels -> tags are sanitized to the tag-safe charset; unsafe or
    # reserved (`needs-review`) labels are dropped and recorded as skipped.
    labels, dropped_labels = [], []
    for name in raw_labels:
        tag = sanitize_label(name)
        (labels.append(tag) if tag else dropped_labels.append(name))
    labels = list(dict.fromkeys(labels))  # dedupe, keep order

    def section(name: str) -> str:
        return sections.get(name, "").strip()

    summary = section("Summary") or section("Details") or "(imported; no summary in source)"
    details = section("Details") if section("Details") != summary else ""
    acceptance = section("Acceptance criteria")
    notes = section("Notes")

    provenance = (
        f"Imported from GitHub issue "
        f"[{repo}#{issue['number']}]({issue['url']}) "
        f"on {now} by `tools/import_github_issues.py` (#15). "
        f"Original state: {state}."
    )
    if raw_labels:
        provenance += f" Source labels: {', '.join(raw_labels)}."
    if dropped_labels:
        provenance += (
            f" Dropped as tags (unsafe/reserved charset): {', '.join(dropped_labels)}."
        )
    if issue.get("createdAt"):
        provenance += f" Created: {issue['createdAt']}."

    parts = [
        "---",
        f"id: {tid}",
        f"title: {yaml_str(single_line(issue['title']))}",
        f"status: {status}",
        "release:",
        f"tags: [{', '.join(labels)}]",
        f"created: {issue.get('createdAt') or now}",
        f"updated: {issue.get('updatedAt') or now}",
        "---",
        "",
        "## Summary",
        summary,
        "",
    ]
    if acceptance:
        parts += ["## Acceptance criteria", acceptance, ""]
    if details:
        parts += ["## Details", details, ""]
    parts += ["## Notes", notes + ("\n" if notes else "") + provenance, ""]
    return "\n".join(parts)


def single_line(title: str) -> str:
    return " ".join(title.split())


def yaml_str(value: str) -> str:
    """Render a double-quoted YAML scalar, safe for `: `, leading keys, etc."""
    escaped = (
        value.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("\n", "\\n")
        .replace("\t", "\\t")
    )
    return f'"{escaped}"'


def init_repo(out: Path) -> None:
    out.mkdir(parents=True, exist_ok=True)
    cfg = out / ".tissue" / "config.yml"
    cfg.parent.mkdir(parents=True, exist_ok=True)
    if not cfg.exists():
        cfg.write_text(DEFAULT_CONFIG)
    git_dir = out / ".git"
    if not git_dir.exists():
        run(["git", "init", str(out)])
        run(["git", "-C", str(out), "checkout", "-b", "main"])


def commit_batch(out: Path, count: int, author: str) -> None:
    run(["git", "-C", str(out), "add", "-A"])
    if subprocess.run(
        ["git", "-C", str(out), "diff", "--cached", "--quiet"]
    ).returncode != 0:
        run([
            "git", "-C", str(out), "commit",
            "-m", f"Import {count} issue(s) from GitHub",
            f"--author={author}",
        ])
    else:
        print("nothing to commit")


def main() -> None:
    ap = argparse.ArgumentParser(
        description="Import GitHub issues into a tissue issue repo (#15, #162)"
    )
    ap.add_argument("--repo", required=True, help="GitHub repo as OWNER/NAME")
    ap.add_argument(
        "--issues", nargs="*", default=None,
        help="GitHub issue numbers to import (default with --all: every issue)",
    )
    ap.add_argument(
        "--all", action="store_true",
        help="import every issue in the repo (open and closed), paginated (#162)",
    )
    ap.add_argument(
        "--out", required=True, type=Path, help="issue repo root directory"
    )
    ap.add_argument(
        "--id-prefix", default="IAM",
        help="tissue id prefix (default: IAM)",
    )
    ap.add_argument(
        "--force", action="store_true",
        help="overwrite existing issue directories",
    )
    ap.add_argument(
        "--dry-run", action="store_true",
        help="fetch and render, write nothing (#162: implies a full report)",
    )
    ap.add_argument(
        "--report", type=Path, default=None,
        help="write a JSON import report here (counts by state, id mapping, "
             "label->tag mapping, comment counts, skips, failures) (#162)",
    )
    args = ap.parse_args()

    if not re.match(r"^[A-Z]+$", args.id_prefix):
        die(f"invalid id prefix (must match [A-Z]+): {args.id_prefix!r}")
    if not re.match(r"^[\w.-]+/[\w.-]+$", args.repo):
        die(f"invalid repo (must be OWNER/NAME): {args.repo!r}")
    if args.all:
        numbers = list_all_issues(args.repo)
        print(f"listed {len(numbers)} issue(s) from {args.repo} (state=all)")
    else:
        if not args.issues:
            die("pass --issues N... or --all")
        numbers = args.issues

    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    report = {
        "repo": args.repo,
        "id_prefix": args.id_prefix,
        "dry_run": bool(args.dry_run),
        "by_state": Counter(),
        "id_mapping": {},
        "label_tag_mapping": {},
        "dropped_labels": Counter(),
        "comment_counts": {},
        "skipped": [],
        "imported": [],
        "failures": [],
    }
    imported = 0
    for number in numbers:
        tid = tissue_id(number, args.id_prefix)
        target = args.out / ".tissue" / "issues" / tid / "issue.md"
        if target.exists() and not args.force:
            print(f"skip {tid}: already imported (use --force)")
            report["skipped"].append(tid)
            continue
        try:
            issue = fetch_issue(args.repo, number)
        except SystemExit:
            report["failures"].append({"issue": number, "error": "gh issue view failed"})
            continue
        state = issue.get("state", "OPEN").lower()
        if state == "merged":
            state = "closed (merged PR)"
        report["by_state"][state] += 1
        report["id_mapping"][f"{args.repo}#{number}"] = tid
        comments = (issue.get("comments") or {})
        n_comments = len(comments.get("nodes") or []) if isinstance(comments, dict) else len(comments or [])
        report["comment_counts"][tid] = n_comments
        for l in issue.get("labels", []):
            name = l["name"]
            tag = sanitize_label(name)
            report["label_tag_mapping"][name] = tag
            if tag is None:
                report["dropped_labels"][name] += 1
        content = render_issue_md(issue, tid, args.id_prefix, now, args.repo)
        if args.dry_run:
            print(f"[dry-run] would write {target} ({len(content)} bytes, {n_comments} comment(s))")
            report["imported"].append(tid)
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)
        # #162: comments land in agent.jsonl as observation records.
        records = render_comment_records(issue, tid)
        if records:
            stream = target.parent / "agent.jsonl"
            with stream.open("w") as f:
                for seq, rec in enumerate(records, start=1):
                    rec["seq"] = seq
                    rec["id"] = f"rec-import-{seq:04d}"
                    rec["refs"] = []
                    f.write(json.dumps(rec) + "\n")
        imported += 1
        report["imported"].append(tid)
        print(f"imported {tid} <- {args.repo}#{number}")

    report["by_state"] = dict(report["by_state"])
    report["dropped_labels"] = dict(report["dropped_labels"])
    report["counts"] = {
        "total_listed": len(numbers),
        "imported_or_would_import": len(report["imported"]),
        "skipped": len(report["skipped"]),
        "failed": len(report["failures"]),
    }
    if args.report:
        args.report.write_text(json.dumps(report, indent=2) + "\n")
        print(f"report written to {args.report}")
    print(
        f"summary: {report['counts']} by_state={report['by_state']} "
        f"labels_dropped={report['dropped_labels']}"
    )
    if args.dry_run or imported == 0:
        return

    init_repo(args.out)
    me = run(["git", "config", "user.name"]).strip() or "tissue import"
    email = run(["git", "config", "user.email"]).strip() or "import@tissue.local"
    commit_batch(args.out, imported, f"{me} <{email}>")
    print(f"committed {imported} issue(s) to {args.out}")


if __name__ == "__main__":
    main()
