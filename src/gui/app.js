"use strict";
/**
 * Tissue read-only issue viewer — frontend (#30 slice, #96 Stage 1 shell).
 *
 * Contract with the local server (see coordination issue #1):
 *   GET /api/issues        -> { issues: IssueSummary[] }
 *   GET /api/issues/:id    -> { issue: IssueDetail } | 404 { error }
 *   GET /api/releases      -> [{ id, title, status, created, updated }]
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
const NEEDS_REVIEW = "needs-review";
/** App state: the full issue list plus the current view/filters. */
let allIssues = [];
const app = () => {
    const el = document.getElementById("app");
    if (!el)
        return;
    return el;
};
function esc(s) {
    return s
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;")
        .replace(/'/g, "&#39;");
}
async function fetchJson(url) {
    var _a;
    const res = await fetch(url, { method: "GET" });
    if (!res.ok) {
        const data = (await res.json().catch(() => null));
        throw new Error((_a = data === null || data === void 0 ? void 0 : data.error) !== null && _a !== void 0 ? _a : `request failed: ${res.status}`);
    }
    return (await res.json());
}
function statusClass(status) {
    if (status === "open")
        return "status-open";
    if (status === "in-progress")
        return "status-in-progress";
    return "status-closed";
}
function badgeHtml(tags) {
    return tags.includes(NEEDS_REVIEW)
        ? `<span class="badge-review" title="An unreviewed proposal exists">NEEDS REVIEW</span>`
        : "";
}
function rowHtml(issue) {
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
function issueNumber(id) {
    const m = /^([A-Z]+)-(\d+)$/.exec(id);
    return m ? parseInt(m[2], 10) : Number.MAX_SAFE_INTEGER;
}
/** Project = issue-ID prefix (e.g. IAM-1 -> IAM). */
function projectOf(id) {
    const m = /^([A-Z]+)-\d+$/.exec(id);
    return m ? m[1] : "";
}
/** Distinct projects present in the loaded issues, sorted. */
function knownProjects(issues) {
    const set = new Set();
    for (const i of issues) {
        const p = projectOf(i.id);
        if (p)
            set.add(p);
    }
    return [...set].sort();
}
/** Populate the project switcher from the issue data. */
function populateProjectSelect(issues) {
    const select = document.getElementById("project-select");
    if (!select)
        return;
    const current = select.value;
    const projects = knownProjects(issues);
    // Rebuild options: "All projects" + one per prefix.
    select.innerHTML =
        `<option value="">All projects</option>` +
            projects.map((p) => `<option value="${esc(p)}">${esc(p)}</option>`).join("");
    // Keep the previous selection if still valid (e.g. hash links).
    if (projects.includes(current))
        select.value = current;
}
function activeProject() {
    const select = document.getElementById("project-select");
    return select ? select.value : "";
}
function activeStatus() {
    const m = /[#?]status=([^&]+)/.exec(window.location.hash);
    return m ? decodeURIComponent(m[1]) : "";
}
function applyFilters(issues) {
    const project = activeProject();
    const status = activeStatus();
    let out = issues;
    if (project)
        out = out.filter((i) => projectOf(i.id) === project);
    if (status)
        out = out.filter((i) => i.status === status);
    return out;
}
function searchMatches(issue, q) {
    if (!q)
        return true;
    const needle = q.toLowerCase();
    return (issue.id.toLowerCase().includes(needle) ||
        issue.title.toLowerCase().includes(needle));
}
function searchInputHtml() {
    return `<input type="search" id="search" class="search" placeholder="Search id or title…" aria-label="Search issues">`;
}
function bindSearch() {
    const input = document.getElementById("search");
    if (!input)
        return;
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
function renderIssueList() {
    const el = app();
    if (!el)
        return;
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
function renderReviewQueue() {
    const el = app();
    if (!el)
        return;
    const flagged = allIssues.filter((i) => i.tags.includes(NEEDS_REVIEW));
    el.innerHTML = `
    <h2 class="view-title">Needs Review</h2>
    <p class="placeholder">The review queue arrives in Stage 3 (#98). Issues
    currently tagged <code>needs-review</code> (${flagged.length}) are shown
    with a badge in the Issues list.</p>`;
}
function renderReleases() {
    const el = app();
    if (!el)
        return;
    el.innerHTML = `
    <h2 class="view-title">Releases</h2>
    <p class="placeholder">The releases view arrives in Stage 2 (#97).</p>`;
}
function renderInline(text) {
    let s = esc(text);
    s = s.replace(/`([^`]+)`/g, "<code>$1</code>");
    s = s.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
    s = s.replace(/\*([^*]+)\*/g, "<em>$1</em>");
    s = s.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, (_m, label, href) => {
        const safeHref = href.replace(/"/g, "&quot;");
        return `<a href="${safeHref}" rel="noopener noreferrer" target="_blank">${label}</a>`;
    });
    return s;
}
function renderMarkdown(src) {
    const out = [];
    const lines = src.split("\n");
    let i = 0;
    let listType = null;
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
            const buf = [];
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
            const want = ul ? "ul" : "ol";
            if (listType !== want) {
                closeList();
                out.push(`<${want}>`);
                listType = want;
            }
            out.push(`<li>${renderInline((ul !== null && ul !== void 0 ? ul : ol)[1])}</li>`);
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
        const para = [line];
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
async function renderDetail(id) {
    var _a;
    const el = app();
    if (!el)
        return;
    try {
        const data = await fetchJson(`/api/issues/${encodeURIComponent(id)}`);
        const issue = data.issue;
        const fm = [
            ["id", esc(issue.id)],
            ["status", `<span class="status ${statusClass(issue.status)}">${esc(issue.status)}</span>`],
            ["release", esc((_a = issue.release) !== null && _a !== void 0 ? _a : "—")],
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
    }
    catch (e) {
        el.innerHTML = `<p class="error">${esc(e instanceof Error ? e.message : String(e))}</p>`;
    }
}
/** Highlight the active nav link for the current view. */
function updateNav(view) {
    document.querySelectorAll(".nav-link").forEach((a) => {
        a.classList.toggle("active", a.dataset.view === view);
    });
    document.querySelectorAll(".filter-link").forEach((a) => {
        var _a;
        a.classList.toggle("active", ((_a = a.dataset.status) !== null && _a !== void 0 ? _a : "") === activeStatus());
    });
}
async function loadIssues() {
    try {
        const data = await fetchJson("/api/issues");
        allIssues = [...data.issues].sort((a, b) => issueNumber(a.id) - issueNumber(b.id));
        populateProjectSelect(allIssues);
    }
    catch (e) {
        const el = app();
        if (el)
            el.innerHTML = `<p class="error">${esc(e instanceof Error ? e.message : String(e))}</p>`;
    }
}
async function route() {
    const hash = window.location.hash || "#/issues";
    const detail = /^#\/issue\/([^/]+)$/.exec(hash);
    if (detail) {
        updateNav("");
        await renderDetail(decodeURIComponent(detail[1]));
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
function init() {
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
