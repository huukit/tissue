"use strict";
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
 * Stage 4 (#99): human editing + sync — the first mutating controls.
 * Token lives in sessionStorage ONLY (never persisted server-side, never
 * logged); writes go through the token-protected API propose/sync
 * endpoints, and the #78 revision guard's stale-current refusals surface
 * as explicit "changed while you were editing" messages, never silent
 * overwrites. All user input is HTML-escaped before touching the DOM.
 * Markdown is rendered by a small, escape-first renderer: all input
 * text is HTML-escaped before tags are generated, so issue content
 * can never inject HTML (acceptance criterion 3, AGENTS.md §5).
 */
const NEEDS_REVIEW = "needs-review";
/** API token, sessionStorage only — never persisted server-side (#98 AC5). */
const TOKEN_KEY = "tissue-api-token";
function getToken() {
    var _a;
    return (_a = sessionStorage.getItem(TOKEN_KEY)) !== null && _a !== void 0 ? _a : "";
}
function setToken(t) {
    if (t)
        sessionStorage.setItem(TOKEN_KEY, t);
    else
        sessionStorage.removeItem(TOKEN_KEY);
}
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
async function postJson(url, body) {
    var _a;
    const token = getToken();
    const res = await fetch(url, {
        method: "POST",
        headers: Object.assign({ "Content-Type": "application/json" }, (token ? { Authorization: `Bearer ${token}` } : {})),
        body: JSON.stringify(body),
    });
    const data = (await res.json().catch(() => null));
    if (!res.ok) {
        throw new Error((_a = data === null || data === void 0 ? void 0 : data.error) !== null && _a !== void 0 ? _a : `request failed: ${res.status}`);
    }
    return data;
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
/** Token field (sessionStorage only) + hint text; shown when no token. */
function tokenBarHtml() {
    const has = !!getToken();
    return `<div class="token-bar">
    <input type="password" id="api-token" class="token-input"
      placeholder="API token (sessionStorage only)" aria-label="API token"
      autocomplete="off">
    <button id="token-save" class="btn">${has ? "Update token" : "Save token"}</button>
    ${has ? `<button id="token-clear" class="btn btn-ghost">Clear</button>` : ""}
    <span class="token-hint">stored in this browser session only — never sent to disk</span>
  </div>`;
}
function bindTokenBar() {
    const save = document.getElementById("token-save");
    const input = document.getElementById("api-token");
    if (save && input) {
        save.addEventListener("click", () => {
            setToken(input.value.trim());
            void route();
        });
    }
    const clear = document.getElementById("token-clear");
    if (clear) {
        clear.addEventListener("click", () => {
            setToken("");
            void route();
        });
    }
}
function proposalCardHtml(entry) {
    const p = entry.proposal;
    return `<li class="proposal-card" id="proposal-${esc(p.proposal_id)}">
    <div class="proposal-head">
      <a class="issue-link issue-id" href="#/issue/${encodeURIComponent(entry.issue.id)}">${esc(entry.issue.id)}</a>
      <span class="proposal-id">${esc(p.proposal_id)}</span>
      <span class="proposal-field">field: <code>${esc(p.field)}</code></span>
    </div>
    <div class="proposal-diff">
      <div class="diff-row"><span class="diff-label">current</span> <code>${esc(p.current)}</code></div>
      <div class="diff-row"><span class="diff-label">proposed</span> <code class="proposed">${esc(p.proposed)}</code></div>
    </div>
    ${p.rationale ? `<p class="proposal-rationale">${esc(p.rationale)}</p>` : ""}
    <div class="proposal-actions">
      <button class="btn btn-approve" data-issue="${esc(entry.issue.id)}"
        data-proposal="${esc(p.proposal_id)}">Approve</button>
      <button class="btn btn-reject" data-issue="${esc(entry.issue.id)}"
        data-proposal="${esc(p.proposal_id)}">Reject…</button>
      <span class="proposal-result" data-result-for="${esc(p.proposal_id)}"></span>
    </div>
  </li>`;
}
async function decideFromGui(issueId, proposalId, decision) {
    var _a;
    const resultEl = document.querySelector(`[data-result-for="${CSS.escape(proposalId)}"]`);
    const reason = decision === "reject"
        ? (_a = window.prompt(`Reason for rejecting ${proposalId} (required — rejections are data):`)) !== null && _a !== void 0 ? _a : ""
        : undefined;
    if (decision === "reject" && !reason) {
        if (resultEl)
            resultEl.textContent = "rejection requires a reason";
        return;
    }
    try {
        const res = await postJson(`/api/issues/${encodeURIComponent(issueId)}/${decision}`, Object.assign(Object.assign({ proposal_id: proposalId }, (reason !== undefined ? { reason } : {})), { actor: "human:gui" }));
        if (resultEl) {
            resultEl.textContent = `${res.decision} (${res.commit.slice(0, 7)}) — refreshing…`;
        }
        await loadIssues();
        await route();
    }
    catch (e) {
        if (resultEl) {
            resultEl.textContent = e instanceof Error ? e.message : String(e);
        }
    }
}
function bindProposalActions() {
    document.querySelectorAll(".btn-approve").forEach((b) => {
        b.addEventListener("click", () => {
            var _a, _b;
            const issue = (_a = b.dataset.issue) !== null && _a !== void 0 ? _a : "";
            const proposal = (_b = b.dataset.proposal) !== null && _b !== void 0 ? _b : "";
            void decideFromGui(issue, proposal, "approve");
        });
    });
    document.querySelectorAll(".btn-reject").forEach((b) => {
        b.addEventListener("click", () => {
            var _a, _b;
            const issue = (_a = b.dataset.issue) !== null && _a !== void 0 ? _a : "";
            const proposal = (_b = b.dataset.proposal) !== null && _b !== void 0 ? _b : "";
            void decideFromGui(issue, proposal, "reject");
        });
    });
}
async function renderReviewQueue() {
    const el = app();
    if (!el)
        return;
    const flagged = allIssues.filter((i) => i.tags.includes(NEEDS_REVIEW));
    el.innerHTML = `
    <h2 class="view-title">Needs Review</h2>
    ${tokenBarHtml()}`;
    bindTokenBar();
    if (flagged.length === 0) {
        el.insertAdjacentHTML("beforeend", `<p class="placeholder">Nothing waiting for review. Proposals tagged
      <code>needs-review</code> appear here.</p>`);
        return;
    }
    const entries = [];
    for (const issue of flagged) {
        try {
            const proposals = await fetchJson(`/api/issues/${encodeURIComponent(issue.id)}/proposals`);
            for (const p of proposals)
                entries.push({ issue, proposal: p });
        }
        catch (_a) {
            // unreadable stream: the badge still shows in the issue list
        }
    }
    if (entries.length === 0) {
        el.insertAdjacentHTML("beforeend", `<p class="placeholder">${flagged.length} issue(s) tagged needs-review but no
      pending proposals could be read.</p>`);
        return;
    }
    el.insertAdjacentHTML("beforeend", `<ul class="proposal-list">${entries.map(proposalCardHtml).join("")}</ul>`);
    bindProposalActions();
}
async function renderReleases() {
    const el = app();
    if (!el)
        return;
    try {
        const releases = await fetchJson("/api/releases");
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
    }
    catch (e) {
        el.innerHTML = `<p class="error">${esc(e instanceof Error ? e.message : String(e))}</p>`;
    }
}
async function renderReleaseDetail(id) {
    var _a;
    const el = app();
    if (!el)
        return;
    try {
        const data = await fetchJson(`/api/releases/${encodeURIComponent(id)}`);
        const r = data.release;
        const cls = r.status === "cut" ? "status-closed" : "status-open";
        const fm = [
            ["id", esc(r.id)],
            ["status", `<span class="status ${cls}">${esc(r.status)}</span>`],
            ["target", esc((_a = r.target) !== null && _a !== void 0 ? _a : "—")],
            ["created", esc(r.created)],
            ["updated", esc(r.updated)],
        ]
            .map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`)
            .join("");
        const issues = data.issues.length
            ? data.issues
                .map((i) => `<li>
            <span class="issue-id">${esc(i.id)}</span>
            <a class="issue-link issue-title" href="#/${encodeURIComponent(i.id)}">${esc(i.title)}</a>
            <span class="status ${statusClass(i.status)}">${esc(i.status)}</span>
          </li>`)
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
const TOKEN_KEY = "tissue-api-token";
function getToken() {
    var _a;
    return (_a = sessionStorage.getItem(TOKEN_KEY)) !== null && _a !== void 0 ? _a : "";
}
function setToken(value) {
    sessionStorage.setItem(TOKEN_KEY, value);
}
async function postJson(url, body) {
    var _a;
    const token = getToken();
    if (!token)
        throw new Error("no API token set — enter one in the token field first");
    const res = await fetch(url, {
        method: "POST",
        headers: {
            "Content-Type": "application/json",
            Authorization: `Bearer ${token}`,
        },
        body: JSON.stringify(body),
    });
    const data = (await res.json().catch(() => null));
    if (!res.ok) {
        throw new Error((_a = data === null || data === void 0 ? void 0 : data.error) !== null && _a !== void 0 ? _a : `request failed: ${res.status}`);
    }
    return data;
}
function tokenBoxHtml() {
    return `
    <div class="token-box">
      <label for="api-token">API token</label>
      <input id="api-token" type="password" autocomplete="off"
             placeholder="paste token (kept in sessionStorage only)"
             value="${esc(getToken())}">
    </div>`;
}
function wireTokenBox() {
    const input = document.getElementById("api-token");
    if (input) {
        input.addEventListener("change", () => setToken(input.value));
    }
}
function editFormHtml(issue) {
    var _a;
    return `
    <details class="edit-box">
      <summary>Edit front matter</summary>
      <div class="edit-fields">
        <label>Title
          <input id="edit-title" type="text" value="${esc(issue.title)}">
        </label>
        <label>Status
          <select id="edit-status">
            ${["open", "in-progress", "closed"]
        .map((s) => `<option value="${s}"${s === issue.status ? " selected" : ""}>${s}</option>`)
        .join("")}
          </select>
        </label>
        <label>Release
          <input id="edit-release" type="text" value="${esc((_a = issue.release) !== null && _a !== void 0 ? _a : "")}"
                 placeholder="e.g. v1.2.0 (empty = unscheduled)">
        </label>
        <button id="edit-submit">Propose change</button>
        <button id="sync-btn" title="Record a human edit note in the agent stream">Add sync note</button>
      </div>
      <p id="edit-result" class="edit-result" aria-live="polite"></p>
    </details>`;
}
function wireEditForm(issue) {
    const submit = document.getElementById("edit-submit");
    const sync = document.getElementById("sync-btn");
    const result = document.getElementById("edit-result");
    if (!submit || !sync || !result)
        return;
    submit.addEventListener("click", async () => {
        var _a;
        result.textContent = "…";
        const title = document.getElementById("edit-title").value;
        const status = document.getElementById("edit-status").value;
        const release = document.getElementById("edit-release").value;
        const changed = title !== issue.title || status !== issue.status || (release || "") !== ((_a = issue.release) !== null && _a !== void 0 ? _a : "");
        if (!changed) {
            result.textContent = "No changes — nothing to save.";
            return;
        }
        try {
            // Direct human edit (decision 4: humans are the authority on the
            // human format) guarded by the revision the form loaded with (#78).
            await postJson(`/api/issues/${encodeURIComponent(issue.id)}/edit`, {
                expected_revision: issue.revision,
                title,
                status,
                release,
                actor: "human:gui",
            });
            result.textContent = "Saved — front matter updated.";
            void route();
        }
        catch (e) {
            const msg = e instanceof Error ? e.message : String(e);
            result.textContent = /CONFLICT|changed while you were editing|revision/i.test(msg)
                ? "Changed while you were editing — re-open the issue and re-apply your edit. (" +
                    msg.replace(/^CONFLICT:?\s*/i, "") +
                    ")"
                : msg;
        }
    });
    sync.addEventListener("click", async () => {
        result.textContent = "…";
        const summary = prompt("One-line summary of your manual edit (recorded in the agent stream):");
        if (!summary) {
            result.textContent = "Sync cancelled — no summary given.";
            return;
        }
        try {
            await postJson(`/api/issues/${encodeURIComponent(issue.id)}/sync`, { summary, actor: "human:gui" });
            result.textContent = "Recorded. Agents reconcile from this note.";
        }
        catch (e) {
            result.textContent = e instanceof Error ? e.message : String(e);
        }
    });
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
        // Agent-stream timeline (#98 AC4): all record types, collapsed by
        // default — heavy agent activity must not break readability (#3).
        let timeline = "";
        try {
            const records = await fetchJson(`/api/issues/${encodeURIComponent(id)}/records`);
            if (records.length > 0) {
                const rows = records
                    .map((r) => {
                    var _a;
                    return `<details class="timeline-record">
            <summary>
              <span class="rec-type">${esc(String((_a = r.data.type) !== null && _a !== void 0 ? _a : "?"))}</span>
              <span class="rec-actor">${esc(r.actor)}</span>
              <span class="rec-ts">${esc(r.ts)}</span>
            </summary>
            <pre class="rec-data">${esc(JSON.stringify(r.data, null, 2))}</pre>
          </details>`;
                })
                    .join("");
                timeline = `
        <section class="timeline">
          <h3>Agent stream (${records.length} record${records.length === 1 ? "" : "s"}, collapsed)</h3>
          ${rows}
        </section>`;
            }
        }
        catch (_b) {
            // no stream or unreadable: the timeline section is simply omitted
        }
        el.innerHTML = `
      <a class="back-link" href="#/issues">&larr; All issues</a>
      <article class="detail">
        <h2>${esc(issue.id)} — ${esc(issue.title)} ${badgeHtml(issue.tags)}</h2>
        ${tokenBoxHtml()}
        <dl class="front-matter">${fm}</dl>
        ${editFormHtml(issue)}
        <div class="body">${renderMarkdown(issue.body)}</div>
        ${timeline}
      </article>`;
        wireTokenBox();
        wireEditForm(issue);
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
        await renderReviewQueue();
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
