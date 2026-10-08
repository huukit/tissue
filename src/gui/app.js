"use strict";
/**
 * Tissue read-only issue viewer — frontend (#30, Dev 4 slice).
 *
 * Contract with the local server (see coordination issue #1):
 *   GET /api/issues        -> { issues: IssueSummary[] }
 *   GET /api/issues/:id    -> { issue: IssueDetail } | 404 { error }
 *
 * Read-only by construction: only GET fetches, no mutating controls.
 * Markdown is rendered by a small, escape-first renderer: all input
 * text is HTML-escaped before tags are generated, so issue content
 * can never inject HTML (acceptance criterion 3, AGENTS.md §5).
 */
const NEEDS_REVIEW = "needs-review";
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
    <a class="issue-link issue-title" href="#/${encodeURIComponent(issue.id)}">${title}</a>
    <span class="status ${statusClass(issue.status)}">${esc(issue.status)}</span>
    ${badgeHtml(issue.tags)}
    <span class="issue-updated" title="updated">${updated}</span>
  </li>`;
}
function issueNumber(id) {
    const m = /^([A-Z]+)-(\d+)$/.exec(id);
    return m ? parseInt(m[2], 10) : Number.MAX_SAFE_INTEGER;
}
async function renderList() {
    const el = app();
    if (!el)
        return;
    try {
        const data = await fetchJson("/api/issues");
        const issues = [...data.issues].sort((a, b) => issueNumber(a.id) - issueNumber(b.id));
        if (issues.length === 0) {
            el.innerHTML = `<p class="loading">No issues found in this issue repo.</p>`;
            return;
        }
        el.innerHTML = `<ul class="issue-list">${issues.map(rowHtml).join("")}</ul>`;
    }
    catch (e) {
        el.innerHTML = `<p class="error">${esc(e instanceof Error ? e.message : String(e))}</p>`;
    }
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
      <a class="back-link" href="#">&larr; All issues</a>
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
function route() {
    const hash = window.location.hash;
    const m = /^#\/([^/]+)$/.exec(hash);
    if (m) {
        void renderDetail(decodeURIComponent(m[1]));
    }
    else {
        void renderList();
    }
}
window.addEventListener("hashchange", route);
route();
