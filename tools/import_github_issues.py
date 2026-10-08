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

Repeatability: re-running with the same issue numbers skips issues whose
directory already exists (use --force to overwrite).
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

ID_PATTERN = re.compile(r"^[A-Z]+-\d+$")
GITHUB_ID_PATTERN = re.compile(r"^\d+$")

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
            "--json", "number,title,body,state,createdAt,updatedAt,labels,url",
        ]
    )
    return json.loads(out)


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
    status = "closed" if state == "closed" else "open"
    labels = [l["name"] for l in issue.get("labels", [])]
    # `needs-review` is reserved by the spec; drop it at import so its
    # presence always means a live proposal.
    labels = [l for l in labels if l != "needs-review"]

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
    if labels:
        provenance += f" Source labels: {', '.join(labels)}."
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
        description="Import GitHub issues into a tissue issue repo (#15)"
    )
    ap.add_argument("--repo", required=True, help="GitHub repo as OWNER/NAME")
    ap.add_argument(
        "--issues", nargs="+", required=True,
        help="GitHub issue numbers to import",
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
        help="fetch and render, write nothing",
    )
    args = ap.parse_args()

    if not re.match(r"^[A-Z]+$", args.id_prefix):
        die(f"invalid id prefix (must match [A-Z]+): {args.id_prefix!r}")
    if not re.match(r"^[\w.-]+/[\w.-]+$", args.repo):
        die(f"invalid repo (must be OWNER/NAME): {args.repo!r}")

    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    imported = 0
    for number in args.issues:
        issue = fetch_issue(args.repo, number)
        tid = tissue_id(number, args.id_prefix)
        target = args.out / ".tissue" / "issues" / tid / "issue.md"
        if target.exists() and not args.force:
            print(f"skip {tid}: already imported (use --force)")
            continue
        content = render_issue_md(issue, tid, args.id_prefix, now, args.repo)
        if args.dry_run:
            print(f"[dry-run] would write {target}")
            print(content)
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)
        imported += 1
        print(f"imported {tid} <- {args.repo}#{number}")

    if args.dry_run or imported == 0:
        return

    init_repo(args.out)
    me = run(["git", "config", "user.name"]).strip() or "tissue import"
    email = run(["git", "config", "user.email"]).strip() or "import@tissue.local"
    commit_batch(args.out, imported, f"{me} <{email}>")
    print(f"committed {imported} issue(s) to {args.out}")


if __name__ == "__main__":
    main()
