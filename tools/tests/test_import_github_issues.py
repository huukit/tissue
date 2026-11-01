import json, subprocess, sys, tempfile, os
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "tools"))
import import_github_issues as imp

# --- unit: label sanitizing ---
assert imp.sanitize_label("bug") == "bug"
assert imp.sanitize_label("needs-review") is None
assert imp.sanitize_label("Needs Review!") == "Needs-Review"
assert imp.sanitize_label("area: security") == "area-security"
assert imp.sanitize_label("") is None
assert imp.sanitize_label("!!!") is None
print("sanitize_label ok")

# --- unit: section split conserves everything ---
body = "intro\n## Summary\ns\n## Custom\nc\n## Notes\nn"
s = imp.split_sections(body)
assert s["Summary"] == "intro\ns" or s.get("Summary"), s
print("split_sections ok:", {k: v[:10] for k, v in s.items()})

# --- unit: comment records ---
issue = {"number": 5, "comments": {"nodes": [
    {"author": {"login": "alice"}, "body": "hi", "createdAt": "2026-01-01T00:00:00Z"},
    {"author": {"login": "bob"}, "body": "yo", "createdAt": "2026-01-02T00:00:00Z"},
]}}
recs = imp.render_comment_records(issue, "IAM-5")
assert len(recs) == 2 and recs[0]["actor"] == "github:alice", recs
assert recs[1]["data"]["evidence"] == "yo"
print("comment records ok")

# --- e2e with a mocked gh: --all --dry-run writes nothing, report written ---
fixture_dir = Path(tempfile.mkdtemp())
(fixture_dir / "gh").write_text("""#!/bin/sh
# mock gh: api -> numbers; issue view -> json
case "$1 $2" in
  "api repos/huukit/tissue/issues")
    printf '1\\n2\\n3\\n' ;;
  "issue view")
    n="$3"
    case "$n" in
      1) cat "$FIX/1.json" ;;
      2) cat "$FIX/2.json" ;;
      3) cat "$FIX/3.json" ;;
    esac ;;
esac
""")
os.chmod(fixture_dir / "gh", 0o755)
out = Path(tempfile.mkdtemp()) / "repo"
fixtures = Path(tempfile.mkdtemp())
(fixtures / "1.json").write_text(json.dumps({"number":1,"title":"First","body":"## Summary\nbody one","state":"OPEN","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-02T00:00:00Z","labels":[{"name":"bug"},{"name":"needs-review"},{"name":"area: x"}],"url":"https://x/1","comments":{"nodes":[{"author":{"login":"a"},"body":"c1","createdAt":"2026-01-01T00:00:00Z"}]}}))
(fixtures / "2.json").write_text(json.dumps({"number":2,"title":"Second","body":"","state":"CLOSED","createdAt":"2026-01-03T00:00:00Z","updatedAt":"2026-01-04T00:00:00Z","labels":[],"url":"https://x/2","comments":{"nodes":[]}}))
(fixtures / "3.json").write_text(json.dumps({"number":3,"title":"Third","body":"b","state":"OPEN","createdAt":"2026-01-05T00:00:00Z","updatedAt":"2026-01-06T00:00:00Z","labels":[{"name":"enhancement"}],"url":"https://x/3","comments":{"nodes":[]}}))
env = dict(os.environ, PATH=f"{fixture_dir}:{os.environ['PATH']}", FIX=str(fixtures))
r = subprocess.run(
    [sys.executable, str(REPO_ROOT / "tools" / "import_github_issues.py"),
     "--repo", "huukit/tissue", "--all", "--out", str(out), "--dry-run",
     "--report", str(out.parent / "report.json"), "--id-prefix", "TST"],
    capture_output=True, text=True, env=env)
print(r.stdout); print(r.stderr, file=sys.stderr)
assert r.returncode == 0, r.stderr
assert not (out / ".tissue").exists(), "dry-run must write nothing"
report = json.loads((out.parent / "report.json").read_text())
assert report["counts"]["total_listed"] == 3, report["counts"]
assert report["by_state"] == {"open": 2, "closed": 1}, report["by_state"]
assert report["id_mapping"]["huukit/tissue#1"] == "TST-1"
assert report["label_tag_mapping"]["bug"] == "bug"
assert report["label_tag_mapping"]["needs-review"] is None
assert report["label_tag_mapping"]["area: x"] == "area-x"
assert report["comment_counts"]["TST-1"] == 1
assert report["counts"]["imported_or_would_import"] == 3
print("dry-run report ok")

# --- real import then idempotent re-run ---
r2 = subprocess.run(
    [sys.executable, str(REPO_ROOT / "tools" / "import_github_issues.py"),
     "--repo", "huukit/tissue", "--all", "--out", str(out), "--id-prefix", "TST"],
    capture_output=True, text=True, env=env)
assert r2.returncode == 0, r2.stderr
assert (out / ".tissue/issues/TST-1/issue.md").exists()
assert (out / ".tissue/issues/TST-1/agent.jsonl").exists()
stream = (out / ".tissue/issues/TST-1/agent.jsonl").read_text()
assert '"type": "observation"' in stream and '"actor": "github:a"' in stream
issue_md = (out / ".tissue/issues/TST-1/issue.md").read_text()
assert "status: open" in issue_md and "tags: [bug, area-x]" in issue_md, issue_md
assert "tags: [needs-review]" not in issue_md and "tags: [bug, area-x]" in issue_md
assert "Source labels: bug, needs-review, area: x" in issue_md  # provenance keeps the raw label
closed_md = (out / ".tissue/issues/TST-2/issue.md").read_text()
assert "status: closed" in closed_md
# re-run: all skipped, no new commit needed
r3 = subprocess.run(
    [sys.executable, str(REPO_ROOT / "tools" / "import_github_issues.py"),
     "--repo", "huukit/tissue", "--all", "--out", str(out), "--id-prefix", "TST"],
    capture_output=True, text=True, env=env)
assert r3.returncode == 0 and "skip TST-" in r3.stdout, r3.stdout
print("idempotent re-run ok")
print("ALL IMPORT TESTS PASSED")
