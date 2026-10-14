/**
 * Tissue read-only issue viewer — frontend (#30 slice, #96 Stage 1 shell).
 *
 * Contract with the local server (see coordination issue #1):
 *   GET /api/issues        -> { issues: IssueSummary[] }
 *   GET /api/issues/:id    -> { issue: IssueDetail } | 404 { error }
 *   GET /api/releases      -> [{ id, title, status, created, updated }]
 *   GET /api/releases/:id  -> ReleaseDetail | 404 { error }  (Stage 2, #97)
 *
 * Stage 1 (#96): app shell with sidebar navigation
 * (Issues / Needs Review / Releases), a project switcher derived from
 * issue-ID prefixes (honoring serve's --project filter when the server
 * already scopes the data), status filtering, and search by id/title.
 * Releases and Needs Review render placeholder views in this stage.
 *
 * Read-only by construction: only GET fetches, no mutating controls.
 * Markdown is rendered by a small, escape-first renderer: all input
 * text is HTML-escaped before tags are generated, so issue content
 * can never inject HTML (acceptance criterion 3, AGENTS.md §5).
 */

interface IssueSummary {
  id: string;
  title: string;
  status: "open" | "in-progress" | "closed";
  release: string | null;
  tags: string[];
  created: string;
  updated: string;
}

interface IssueDetail extends IssueSummary {
  body: string;
}

interface ReleaseSummary {
  id: string;
  title: string;
  status: string;
  created: string;
  updated: string;
}

interface ReleaseDetail {
  release: ReleaseSummary & { target?: string | null };
  issues: { id: string; title: string; status: string }[];
}

const NEEDS_REVIEW = "needs-review";

/** App state: the full issue list plus the current view/filters. */
let allIssues: IssueSummary[] = [];

const app = () => {
  const el = document.getElementById("app");
  if (!el) return;
  return el;
};

function esc(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

async function fetchJson<T>(url: string): Promise<T> {
  const res = await fetch(url, { method: "GET" });
  if (!res.ok) {
    const data = (await res.json().catch(() => null)) as { error?: string } | null;
    throw new Error(data?.error ?? `request failed: ${res.status}`);
  }
  return (await res.json()) as T;
}

function statusClass(status: string): string {
  if (status === "open") return "status-open";
  if (status === "in-progress") return "status-in-progress";
  return "status-closed";
}

function badgeHtml(tags: string[]): string {
  return tags.includes(NEEDS_REVIEW)
    ? `<span class="badge-review" title="An unreviewed proposal exists">NEEDS REVIEW</span>`
    : "";
}

function rowHtml(issue: IssueSummary): string {
  const title = esc(issue.title);
  const updated = esc(new Date(issue.updated).toLocaleString());
  return `<li class="issue-row">
    <span class="issue-id">${esc(issue.id)}</span>
    <a class="issue-link issue-title" href="#/issue/${encodeURIComponent(issue.id)}">${title}</a>
    <span class="status ${statusClass(issue.status)}">${esc(issue.status)}</span>
    ${badgeHtml(issue.tags)}
    <span class="issue-updated" title="updated">${updated}</span>
  </li>`;
}

function issueNumber(id: string): number {
  const m = /^([A-Z]+)-(\d+)$/.exec(id);
  return m ? parseInt(m[2], 10) : Number.MAX_SAFE_INTEGER;
}

/** Project = issue-ID prefix (e.g. IAM-1 -> IAM). */
function projectOf(id: string): string {
  const m = /^([A-Z]+)-\d+$/.exec(id);
  return m ? m[1] : "";
}

/** Distinct projects present in the loaded issues, sorted. */
function knownProjects(issues: IssueSummary[]): string[] {
  const set = new Set<string>();
  for (const i of issues) {
    const p = projectOf(i.id);
    if (p) set.add(p);
  }
  return [...set].sort();
}

/** Populate the project switcher from the issue data. */
function populateProjectSelect(issues: IssueSummary[]): void {
  const select = document.getElementById("project-select") as HTMLSelectElement | null;
  if (!select) return;
  const current = select.value;
  const projects = knownProjects(issues);
  // Rebuild options: "All projects" + one per prefix.
  select.innerHTML =
    `<option value="">All projects</option>` +
    projects.map((p) => `<option value="${esc(p)}">${esc(p)}</option>`).join("");
  // Keep the previous selection if still valid (e.g. hash links).
  if (projects.includes(current)) select.value = current;
}

function activeProject(): string {
  const select = document.getElementById("project-select") as HTMLSelectElement | null;
  return select ? select.value : "";
}

function activeStatus(): string {
  const m = /[#?]status=([^&]+)/.exec(window.location.hash);
  return m ? decodeURIComponent(m[1]) : "";
}

function applyFilters(issues: IssueSummary[]): IssueSummary[] {
  const project = activeProject();
  const status = activeStatus();
  let out = issues;
  if (project) out = out.filter((i) => projectOf(i.id) === project);
  if (status) out = out.filter((i) => i.status === status);
  return out;
}

function searchMatches(issue: IssueSummary, q: string): boolean {
  if (!q) return true;
  const needle = q.toLowerCase();
  return (
    issue.id.toLowerCase().includes(needle) ||
    issue.title.toLowerCase().includes(needle)
  );
}

function searchInputHtml(): string {
  return `<input type="search" id="search" class="search" placeholder="Search id or title…" aria-label="Search issues">`;
}

function bindSearch(): void {
  const input = document.getElementById("search") as HTMLInputElement | null;
  if (!input) return;
  input.addEventListener("input", () => {
    const q = input.value;
    const visible = applyFilters(allIssues).filter((i) => searchMatches(i, q));
    const list = document.getElementById("issue-list");
    if (list) {
      list.innerHTML =
        visible.length > 0
          ? visible.map(rowHtml).join("")
          : `<li class="empty-row">No matching issues.</li>`;
    }
  });
}

function renderIssueList(): void {
  const el = app();
  if (!el) return;
  const visible = applyFilters(allIssues);
  if (visible.length === 0) {
    el.innerHTML = `<p class="loading">No issues found in this issue repo.</p>`;
    return;
  }
  el.innerHTML = `
    ${searchInputHtml()}
    <ul class="issue-list" id="issue-list">${visible.map(rowHtml).join("")}</ul>`;
  bindSearch();
}

function renderReviewQueue(): void {
  const el = app();
  if (!el) return;
  const flagged = allIssues.filter((i) => i.tags.includes(NEEDS_REVIEW));
  el.innerHTML = `
    <h2 class="view-title">Needs Review</h2>
    <p class="placeholder">The review queue arrives in Stage 3 (#98). Issues
    currently tagged <code>needs-review</code> (${flagged.length}) are shown
    with a badge in the Issues list.</p>`;
}

async function renderReleases(): Promise<void> {
  const el = app();
  if (!el) return;
  try {
    const releases = await fetchJson<ReleaseSummary[]>("/api/releases");
    if (releases.length === 0) {
      el.innerHTML = `<p class="loading">No releases in this issue repo yet.</p>`;
      return;
    }
    const rows = releases
      .slice()
      .sort((a, b) => b.id.localeCompare(a.id))
      .map((r) => {
        const cls = r.status === "cut" ? "status-closed" : "status-open";
        return `<li class="issue-row">
        <span class="issue-id">${esc(r.id)}</span>
        <a class="issue-link issue-title" href="#/releases/${encodeURIComponent(r.id)}">${esc(r.title)}</a>
        <span class="status ${cls}">${esc(r.status)}</span>
        <span class="issue-updated" title="updated">${esc(new Date(r.updated).toLocaleString())}</span>
      </li>`;
      })
      .join("");
    el.innerHTML = `<ul class="issue-list">${rows}</ul>`;
  } catch (e) {
    el.innerHTML = `<p class="error">${esc(e instanceof Error ? e.message : String(e))}</p>`;
  }
}

async function renderReleaseDetail(id: string): Promise<void> {
  const el = app();
  if (!el) return;
  try {
    const data = await fetchJson<ReleaseDetail>(
      `/api/releases/${encodeURIComponent(id)}`
    );
    const r = data.release;
    const cls = r.status === "cut" ? "status-closed" : "status-open";
    const fm = [
      ["id", esc(r.id)],
      ["status", `<span class="status ${cls}">${esc(r.status)}</span>`],
      ["target", esc(r.target ?? "—")],
      ["created", esc(r.created)],
      ["updated", esc(r.updated)],
    ]
      .map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`)
      .join("");
    const issues = data.issues.length
      ? data.issues
          .map(
            (i) => `<li>
            <span class="issue-id">${esc(i.id)}</span>
            <a class="issue-link issue-title" href="#/${encodeURIComponent(i.id)}">${esc(i.title)}</a>
            <span class="status ${statusClass(i.status)}">${esc(i.status)}</span>
          </li>`
          )
          .join("")
      : `<li class="loading">(none assigned)</li>`;
    el.innerHTML = `
      <a class="back-link" href="#/releases">&larr; All releases</a>
      <article class="detail">
        <h2>${esc(r.id)} — ${esc(r.title)}</h2>
        <dl class="front-matter">${fm}</dl>
        <h3>Assigned issues</h3>
        <ul class="issue-list">${issues}</ul>
      </article>`;
  } catch (e) {
    el.innerHTML = `<p class="error">${esc(e instanceof Error ? e.message : String(e))}</p>`;
  }
}

function renderInline(text: string): string {
  let s = esc(text);
  s = s.replace(/`([^`]+)`/g, "<code>$1</code>");
  s = s.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  s = s.replace(/\*([^*]+)\*/g, "<em>$1</em>");
  s = s.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, (_m, label: string, href: string) => {
    const safeHref = href.replace(/"/g, "&quot;");
    return `<a href="${safeHref}" rel="noopener noreferrer" target="_blank">${label}</a>`;
  });
  return s;
}

function renderMarkdown(src: string): string {
  const out: string[] = [];
  const lines = src.split("\n");
  let i = 0;
  let listType: "ul" | "ol" | null = null;
  const closeList = () => {
    if (listType) {
      out.push(`</${listType}>`);
      listType = null;
    }
  };
  while (i < lines.length) {
    const line = lines[i];
    if (/^```/.test(line.trim())) {
      closeList();
      const buf: string[] = [];
      i++;
      while (i < lines.length && !/^```/.test(lines[i].trim())) {
        buf.push(lines[i]);
        i++;
      }
      i++;
      out.push(`<pre><code>${esc(buf.join("\n"))}</code></pre>`);
      continue;
    }
    const h = /^(#{1,3})\s+(.*)$/.exec(line);
    if (h) {
      closeList();
      out.push(`<h${h[1].length}>${renderInline(h[2])}</h${h[1].length}>`);
      i++;
      continue;
    }
    const ul = /^\s*[-*]\s+(.*)$/.exec(line);
    const ol = /^\s*\d+\.\s+(.*)$/.exec(line);
    if (ul || ol) {
      const want: "ul" | "ol" = ul ? "ul" : "ol";
      if (listType !== want) {
        closeList();
        out.push(`<${want}>`);
        listType = want;
      }
      out.push(`<li>${renderInline((ul ?? ol)![1])}</li>`);
      i++;
      continue;
    }
    if (line.trim() === "") {
      closeList();
      i++;
      continue;
    }
    if (/^\s*>\s?/.test(line)) {
      closeList();
      out.push(`<blockquote><p>${renderInline(line.replace(/^\s*>\s?/, ""))}</p></blockquote>`);
      i++;
      continue;
    }
    closeList();
    const para: string[] = [line];
    i++;
    while (i < lines.length && lines[i].trim() !== "" && !/^(#{1,3}\s|\s*[-*]\s|\s*\d+\.\s|```|\s*>\s?)/.test(lines[i])) {
      para.push(lines[i]);
      i++;
    }
    out.push(`<p>${renderInline(para.join("\n"))}</p>`);
  }
  closeList();
  return out.join("\n");
}

async function renderDetail(id: string): Promise<void> {
  const el = app();
  if (!el) return;
  try {
    const data = await fetchJson<{ issue: IssueDetail }>(
      `/api/issues/${encodeURIComponent(id)}`
    );
    const issue = data.issue;
    const fm = [
      ["id", esc(issue.id)],
      ["status", `<span class="status ${statusClass(issue.status)}">${esc(issue.status)}</span>`],
      ["release", esc(issue.release ?? "—")],
      ["tags", issue.tags.length ? esc(issue.tags.join(", ")) : "—"],
      ["created", esc(issue.created)],
      ["updated", esc(issue.updated)],
    ]
      .map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`)
      .join("");
    el.innerHTML = `
      <a class="back-link" href="#/issues">&larr; All issues</a>
      <article class="detail">
        <h2>${esc(issue.id)} — ${esc(issue.title)} ${badgeHtml(issue.tags)}</h2>
        <dl class="front-matter">${fm}</dl>
        <div class="body">${renderMarkdown(issue.body)}</div>
      </article>`;
  } catch (e) {
    el.innerHTML = `<p class="error">${esc(e instanceof Error ? e.message : String(e))}</p>`;
  }
}

/** Highlight the active nav link for the current view. */
function updateNav(view: string): void {
  document.querySelectorAll<HTMLElement>(".nav-link").forEach((a) => {
    a.classList.toggle("active", a.dataset.view === view);
  });
  document.querySelectorAll<HTMLElement>(".filter-link").forEach((a) => {
    a.classList.toggle("active", (a.dataset.status ?? "") === activeStatus());
  });
}

async function loadIssues(): Promise<void> {
  try {
    const data = await fetchJson<{ issues: IssueSummary[] }>("/api/issues");
    allIssues = [...data.issues].sort((a, b) => issueNumber(a.id) - issueNumber(b.id));
    populateProjectSelect(allIssues);
  } catch (e) {
    const el = app();
    if (el) el.innerHTML = `<p class="error">${esc(e instanceof Error ? e.message : String(e))}</p>`;
  }
}

async function route(): Promise<void> {
  const hash = window.location.hash || "#/issues";
  const detail = /^#\/issue\/([^/]+)$/.exec(hash);
  if (detail) {
    updateNav("");
    await renderDetail(decodeURIComponent(detail[1]));
    return;
  }
  const releaseDetail = /^#\/releases\/([^/]+)$/.exec(hash);
  if (releaseDetail) {
    updateNav("releases");
    await renderReleaseDetail(decodeURIComponent(releaseDetail[1]));
    return;
  }
  await loadIssues();
  const view = /^#\/(review|releases)/.exec(hash);
  if (view && view[1] === "review") {
    updateNav("review");
    renderReviewQueue();
    return;
  }
  if (view && view[1] === "releases") {
    updateNav("releases");
    renderReleases();
    return;
  }
  updateNav("issues");
  renderIssueList();
}

function init(): void {
  const select = document.getElementById("project-select");
  if (select) {
    select.addEventListener("change", () => {
      const el = app();
      if (el && /^#\/(issues)?$/.test(window.location.hash || "#/issues")) {
        renderIssueList();
      }
    });
  }
  window.addEventListener("hashchange", () => void route());
  void route();
}

init();
