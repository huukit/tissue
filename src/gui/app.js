"use strict";
/**
 * Tissue read-only issue viewer — frontend (#30 slice, #96 Stage 1 shell).
 *
 * Contract with the local server (see coordination issue #1):
 *   GET /api/issues        -> { issues: IssueSummary[] }
 *   GET /api/issues/:id    -> { issue: IssueDetail } | 404 { error }
 *   GET /api/releases      -> [{ id, title, status, created, updated }]
 *   GET /api/releases/:id  -> ReleaseDetail | 404 { error }  (Stage 2, #97)
 *   POST /api/releases     -> 201 { created, title } | 400/409/401 { error }  (#173)
 *
 * Stage 1 (#96): app shell with sidebar navigation
 * (Issues / Awaiting Decision / Releases), a project switcher derived from
 * issue-ID prefixes (honoring serve's --project filter when the server
 * already scopes the data), status filtering, and search by id/title.
 * Releases and Awaiting Decision render placeholder views in this stage.
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
/** #175: the authoritative project list — GET /api/projects (the config
 * mapping), NOT issue-id prefixes: a project with zero issues must still
 * appear in the switcher, and a created project must survive reload. */
/** #175/#178: the authoritative project list — GET /api/projects (the
 * config mapping), NOT issue-id prefixes: a project with zero issues must
 * still appear in the switcher and on the Projects page. */
async function loadProjects() {
    var _a;
    const data = await fetchJson("/api/projects");
    return (_a = data.projects) !== null && _a !== void 0 ? _a : [];
}
/** #178: cached project list for forms/selectors, refreshed by loadIssues. */
let cachedProjects = [];
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
/** #137: PUT helper for /api/config (token-protected write). */
async function putJson(url, body) {
    var _a;
    const token = getToken();
    const res = await fetch(url, {
        method: "PUT",
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
/** #174: GET with the bearer token (the token-management endpoints are
 * authed even for reads — the list is masked, only issue returns a secret). */
async function fetchAuthed(url) {
    var _a;
    const token = getToken();
    const res = await fetch(url, {
        method: "GET",
        headers: token ? { Authorization: `Bearer ${token}` } : {},
    });
    const data = (await res.json().catch(() => null));
    if (!res.ok) {
        throw new Error((_a = data === null || data === void 0 ? void 0 : data.error) !== null && _a !== void 0 ? _a : `request failed: ${res.status}`);
    }
    return data;
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
        ? `<span class="badge-review" title="An agent proposed a change (or a new issue); you approve or reject it. Nothing in the human format changes until you decide.">AWAITING DECISION</span>`
        : "";
}
/** #138: the derived last-touch badge. */
function lastTouchBadge(issue) {
    var _a;
    const lt = (_a = issue.last_touch) !== null && _a !== void 0 ? _a : "human";
    const label = lt === "synced" ? "synced" : lt === "ai-proposal" ? "AI proposal" : "human";
    return `<span class="last-touch last-touch-${esc(lt)}" title="who last touched this issue">${esc(label)}</span>`;
}
function rowHtml(issue) {
    const title = esc(issue.title);
    const updated = esc(new Date(issue.updated).toLocaleString());
    return `<li class="issue-row">
    <span class="issue-id">${esc(issue.id)}</span>
    <a class="issue-link issue-title" href="#/issue/${encodeURIComponent(issue.id)}">${title}</a>
    <span class="badge project-badge" title="project (id prefix)">${esc(projectOf(issue.id))}</span>
    <span class="status ${statusClass(issue.status)}">${esc(issue.status)}</span>
    ${lastTouchBadge(issue)}
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
/** Populate the project switcher from GET /api/projects (#175: the config
 * mapping is authoritative — a project with no issues yet still shows). */
/** #175/#178: populate the project switcher from GET /api/projects (the
 * persisted config mapping is authoritative); issue-derived prefixes are
 * unioned in as a fallback so ad-hoc prefixes still filter. */
async function populateProjectSelect(issues) {
    const select = document.getElementById("project-select");
    if (!select)
        return;
    const current = select.value;
    let prefixes;
    try {
        prefixes = (await loadProjects()).map((p) => p.prefix);
    }
    catch (_a) {
        // Fallback if the read fails: derive from issue ids (pre-#175 behavior).
        prefixes = knownProjects(issues);
    }
    // Union with issue-derived prefixes so ad-hoc prefixes still filter.
    prefixes = cachedProjects.length ? cachedProjects.map((p) => p.prefix) : await loadProjects().then((ps) => ps.map((p) => p.prefix));
    prefixes = [];
    prefixes = [...new Set([...prefixes, ...knownProjects(issues)])].sort();
    // Rebuild options: "All projects" + one per prefix.
    select.innerHTML =
        `<option value="">All projects</option>` +
            prefixes.map((p) => `<option value="${esc(p)}">${esc(p)}</option>`).join("");
    // Keep the previous selection if still valid (e.g. hash links).
    if (prefixes.includes(current))
        select.value = current;
}
/** #135: "+ New project" — create a project via POST /api/projects
 * (token-protected) and repopulate the switcher. */
function bindNewProject() {
    const btn = document.getElementById("new-project-btn");
    const form = document.getElementById("new-project-form");
    if (!btn || !form)
        return;
    btn.addEventListener("click", () => {
        form.hidden = !form.hidden;
        if (!form.hidden) {
            const name = document.getElementById("new-project-name");
            if (name)
                name.focus();
        }
    });
    const cancel = document.getElementById("new-project-cancel");
    cancel === null || cancel === void 0 ? void 0 : cancel.addEventListener("click", () => {
        form.hidden = true;
    });
    // Prefill the prefix uppercase from the name as it is typed.
    const nameEl = document.getElementById("new-project-name");
    const prefixEl = document.getElementById("new-project-prefix");
    nameEl === null || nameEl === void 0 ? void 0 : nameEl.addEventListener("input", () => {
        if (prefixEl && nameEl)
            prefixEl.value = nameEl.value.toUpperCase().replace(/[^A-Z]/g, "");
    });
    const create = document.getElementById("new-project-create");
    const result = document.getElementById("new-project-result");
    create === null || create === void 0 ? void 0 : create.addEventListener("click", async () => {
        if (!result || !nameEl || !prefixEl)
            return;
        const name = nameEl.value.trim();
        const prefix = prefixEl.value.trim();
        if (!name || !prefix) {
            result.textContent = "Both a name and an uppercase prefix are required (e.g. ops + OPS).";
            return;
        }
        if (!/^[A-Z]+$/.test(prefix)) {
            result.textContent = "The prefix must be uppercase ASCII letters (e.g. OPS).";
            return;
        }
        result.textContent = "…";
        try {
            await postJson("/api/projects", { name, prefix, actor: "human:gui" });
            result.textContent = `Created ${name} (${prefix}).`;
            form.hidden = true;
            nameEl.value = "";
            prefixEl.value = "";
            // #175: repopulate the switcher from GET /api/projects (the persisted
            // config mapping, not issue prefixes) and select the new project.
            await loadIssues();
            const select = document.getElementById("project-select");
            if (select)
                select.value = prefix;
            if (select) {
                const opt = Array.from(select.options).find((o) => o.value === prefix);
                if (opt)
                    select.value = prefix;
                else {
                    const o = document.createElement("option");
                    o.value = prefix;
                    o.textContent = prefix;
                    select.add(o);
                    select.value = prefix;
                }
            }
            // #178: if the Projects page is showing, re-render it with the new row.
            if ((window.location.hash || "").startsWith("#/projects"))
                await renderProjects();
        }
        catch (e) {
            const msg = e instanceof Error ? e.message : String(e);
            result.textContent = /401|token/i.test(msg)
                ? "Saving failed — set the API token first (it protects all write actions)."
                : msg;
        }
    });
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
/** The issue body template the CLI's `create` uses (#134). */
const CREATE_BODY_TEMPLATE = "## Summary\n\nDescribe the issue.\n\n## Acceptance criteria\n\n- [ ] First criterion\n";
/** #134/#172/#176: "+ New issue" form — an explicit project selector
 * (populated from GET /api/projects, defaulting to the current filter or
 * the configured default project); the id prefix is derived from the
 * selected project. There is deliberately NO hardcoded "IAM" fallback:
 * with no projects configured the form shows an error instead. */
function newIssueFormHtml() {
    var _a, _b;
    const projects = cachedProjects;
    const current = activeProject();
    const def = (_a = projects.find((p) => p.default)) !== null && _a !== void 0 ? _a : projects[0];
    const selected = (_b = projects.find((p) => p.prefix === current)) !== null && _b !== void 0 ? _b : def;
    if (!selected) {
        return `<div class="new-issue-form" id="new-issue-form" hidden>
    <h3>New issue</h3>
    <p class="error">No projects configured — create a project first ("+ New project").</p>
    <button id="new-issue-cancel" class="btn btn-ghost">Cancel</button>
  </div>`;
    }
    const prefix = selected.prefix;
    const next = nextIssueNumber(prefix);
    const options = projects
        .map((p) => `<option value="${esc(p.prefix)}"${p.prefix === prefix ? " selected" : ""}>${esc(p.name)} (${esc(p.prefix)})</option>`)
        .join("");
    return `<div class="new-issue-form" id="new-issue-form" hidden>
    <h3>New issue</h3>
    <label>Project
      <select id="new-issue-project" aria-label="Project">${options}</select>
    </label>
    <label>Id
      <input id="new-issue-id" type="text" value="${esc(prefix + "-" + next)}"
             aria-label="Issue id (PREFIX-number)">
    </label>
    <label>Title
      <input id="new-issue-title" type="text" placeholder="One line" aria-label="Issue title">
    </label>
    <label>Body
      <textarea id="new-issue-body" rows="8" aria-label="Issue body">${esc(CREATE_BODY_TEMPLATE)}</textarea>
    </label>
    <button id="new-issue-create" class="btn btn-primary">Create issue</button>
    <button id="new-issue-cancel" class="btn btn-ghost">Cancel</button>
    <p id="new-issue-result" class="edit-result" aria-live="polite"></p>
  </div>`;
}
/** Next free number under a prefix, from the loaded issues. */
function nextIssueNumber(prefix) {
    let max = 0;
    for (const i of allIssues) {
        const m = new RegExp(`^${prefix}-(\\d+)$`).exec(i.id);
        if (m)
            max = Math.max(max, parseInt(m[1], 10));
    }
    return max + 1;
}
function bindNewIssue() {
    var _a, _b;
    const form = document.getElementById("new-issue-form");
    if (!form)
        return;
    const result = document.getElementById("new-issue-result");
    const idEl = document.getElementById("new-issue-id");
    const projectEl = document.getElementById("new-issue-project");
    const titleEl = document.getElementById("new-issue-title");
    const bodyEl = document.getElementById("new-issue-body");
    (_a = document.getElementById("new-issue-cancel")) === null || _a === void 0 ? void 0 : _a.addEventListener("click", () => {
        form.hidden = true;
    });
    // #172/#176: changing the project re-derives the id prefix — no silent
    // fallback to another project's prefix.
    projectEl === null || projectEl === void 0 ? void 0 : projectEl.addEventListener("change", () => {
        if (idEl && projectEl) {
            idEl.value = `${projectEl.value}-${nextIssueNumber(projectEl.value)}`;
        }
    });
    (_b = document.getElementById("new-issue-create")) === null || _b === void 0 ? void 0 : _b.addEventListener("click", async () => {
        if (!result || !idEl || !titleEl)
            return;
        const id = idEl.value.trim();
        const title = titleEl.value.trim();
        const projectPrefix = projectEl ? projectEl.value : "";
        if (!/^[A-Z]+-\d+$/.test(id)) {
            result.textContent = "The id must look like PREFIX-number (e.g. IAM-6).";
            return;
        }
        // #176: the id prefix must match the selected project — mismatch is an
        // inline error, never a silent re-prefixing.
        if (projectPrefix && !id.startsWith(`${projectPrefix}-`)) {
            result.textContent = `Id ${id} does not match the selected project's prefix ${projectPrefix} — use ${projectPrefix}-<number>.`;
            return;
        }
        if (!title) {
            result.textContent = "A one-line title is required.";
            return;
        }
        result.textContent = "…";
        try {
            const selected = cachedProjects.find((p) => p.prefix === projectPrefix);
            await postJson("/api/issues", {
                id,
                title,
                body: bodyEl ? bodyEl.value : undefined,
                actor: "human:gui",
                project: selected === null || selected === void 0 ? void 0 : selected.name,
            });
            // Created: navigate to the new issue's detail.
            window.location.hash = `#/issue/${encodeURIComponent(id)}`;
        }
        catch (e) {
            const msg = e instanceof Error ? e.message : String(e);
            result.textContent = /401|token/i.test(msg)
                ? "Creating failed — set the API token first (it protects all write actions)."
                : /already exists/i.test(msg)
                    ? `Issue ${id} already exists — pick another number.`
                    : msg;
        }
    });
}
function renderIssueList() {
    var _a, _b;
    const el = app();
    if (!el)
        return;
    const visible = applyFilters(allIssues);
    // #134: the token bar lives on the list header so every write action
    // (new issue, approve/reject, sync, edit) shares one token entry point.
    const header = `
    ${tokenBarHtml()}
    <div class="list-toolbar">
      ${searchInputHtml()}
      <button id="new-issue-btn" class="btn" title="Create an issue (needs the API token)">+ New issue</button>
    </div>
    ${newIssueFormHtml()}`;
    bindTokenBar();
    if (visible.length === 0) {
        el.innerHTML = `
    ${header}
    <p class="placeholder">No issues found in this issue repo.</p>`;
        (_a = document.getElementById("new-issue-btn")) === null || _a === void 0 ? void 0 : _a.addEventListener("click", () => {
            const f = document.getElementById("new-issue-form");
            if (f)
                f.hidden = !f.hidden;
        });
        bindNewIssue();
        bindSearch();
        return;
    }
    el.innerHTML = `
    ${header}
    <ul class="issue-list" id="issue-list">${visible.map(rowHtml).join("")}</ul>`;
    (_b = document.getElementById("new-issue-btn")) === null || _b === void 0 ? void 0 : _b.addEventListener("click", () => {
        const f = document.getElementById("new-issue-form");
        if (f)
            f.hidden = !f.hidden;
        if (!(f === null || f === void 0 ? void 0 : f.hidden)) {
            const t = document.getElementById("new-issue-title");
            t === null || t === void 0 ? void 0 : t.focus();
        }
    });
    bindNewIssue();
    bindSearch();
}
/** #199: outside Settings, never render a token input — browsers/password
 *  managers autofill password fields, leaking the token onto every view.
 *  Masked status chip with Change (→ Settings) / Clear instead. */
function tokenBarHtml() {
    const has = !!getToken();
    return `<div class="token-bar">
    ${has
        ? `<span class="token-hint">API token set (this browser session only)</span>
         <a class="btn btn-ghost" id="token-change" href="#/settings">Change</a>
         <button id="token-clear" class="btn btn-ghost">Clear</button>`
        : `<span class="token-hint">No API token — writes are read-only. <a id="token-change" href="#/settings">Set it on Settings</a></span>`}
  </div>`;
}
/** #199: the token entry field exists ONLY on the Settings page, hardened
 *  against autofill (autocomplete="new-password", dedicated name). */
function settingsTokenBarHtml() {
    const has = !!getToken();
    return `<div class="token-bar">
    <input type="password" id="api-token" name="tissue-api-token-entry" class="token-input"
      placeholder="API token (sessionStorage only)" aria-label="API token"
      autocomplete="new-password">
    <button id="token-save" class="btn btn-primary">${has ? "Update token" : "Save token"}</button>
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
            input.value = ""; // #199: never leave the token in the field
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
    <h2 class="view-title">Awaiting your decision</h2>
    ${tokenBarHtml()}`;
    bindTokenBar();
    if (flagged.length === 0) {
        el.insertAdjacentHTML("beforeend", `<p class="placeholder">Nothing awaiting your decision yet. Proposals (stored
      tag <code>needs-review</code>) appear here.</p>`);
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
        el.insertAdjacentHTML("beforeend", `<p class="placeholder">${flagged.length} issue(s) awaiting your decision but no
      pending proposals could be read.</p>`);
        return;
    }
    el.insertAdjacentHTML("beforeend", `<ul class="proposal-list">${entries.map(proposalCardHtml).join("")}</ul>`);
    bindProposalActions();
}
/** #173: "+ New release" form — POST /api/releases with the token, then
 * navigate to the created release. */
function newReleaseFormHtml() {
    return `<div class="new-issue-form" id="new-release-form" hidden>
    <h3>New release</h3>
    <label>Id
      <input id="new-release-id" type="text" placeholder="v1.2.3" aria-label="Release id (v + semver)">
    </label>
    <label>Title
      <input id="new-release-title" type="text" placeholder="One line" aria-label="Release title">
    </label>
    <button id="new-release-create" class="btn">Create release</button>
    <button id="new-release-cancel" class="btn btn-ghost">Cancel</button>
    <p id="new-release-result" class="edit-result" aria-live="polite"></p>
  </div>`;
}
function bindNewRelease() {
    var _a, _b;
    const form = document.getElementById("new-release-form");
    if (!form)
        return;
    const result = document.getElementById("new-release-result");
    const idEl = document.getElementById("new-release-id");
    const titleEl = document.getElementById("new-release-title");
    (_a = document.getElementById("new-release-cancel")) === null || _a === void 0 ? void 0 : _a.addEventListener("click", () => {
        form.hidden = true;
    });
    (_b = document.getElementById("new-release-create")) === null || _b === void 0 ? void 0 : _b.addEventListener("click", async () => {
        if (!result || !idEl || !titleEl)
            return;
        const id = idEl.value.trim();
        const title = titleEl.value.trim();
        if (!/^v\d+\.\d+\.\d+$/.test(id)) {
            result.textContent = "The id must be v + semver (e.g. v1.2.3).";
            return;
        }
        if (!title) {
            result.textContent = "A one-line title is required.";
            return;
        }
        result.textContent = "…";
        try {
            await postJson("/api/releases", { id, title, actor: "human:gui" });
            window.location.hash = `#/releases/${encodeURIComponent(id)}`;
        }
        catch (e) {
            const msg = e instanceof Error ? e.message : String(e);
            result.textContent = /401|token/i.test(msg)
                ? "Creating failed — set the API token first (it protects all write actions)."
                : /already exists|409/i.test(msg)
                    ? `Release ${id} already exists — pick another version.`
                    : msg;
        }
    });
}
/** #178: the Projects page — every configured project with prefix,
 * default marker, issue counts (derived client-side from GET /api/issues),
 * a per-project link, and the "+ New project" create form (POST
 * /api/projects, fixed in #175). */
async function renderProjects() {
    var _a;
    const el = app();
    if (!el)
        return;
    try {
        const projects = await loadProjects();
        // Counts per prefix from the loaded issue list (existing endpoints only).
        const counts = new Map();
        for (const i of allIssues) {
            const p = projectOf(i.id);
            const c = (_a = counts.get(p)) !== null && _a !== void 0 ? _a : { open: 0, review: 0, total: 0 };
            c.total += 1;
            if (i.status === "open" || i.status === "in-progress")
                c.open += 1;
            if (i.tags.includes("needs-review"))
                c.review += 1;
            counts.set(p, c);
        }
        if (projects.length === 0) {
            el.innerHTML = `
        <h2>Projects</h2>
        <p class="loading">No projects configured yet — issues need a project (#176). Create the first one:</p>
        ${newProjectFormHtml()}
        ${tokenBarHtml()}`;
            bindTokenBar();
            bindNewProject();
            return;
        }
        const rows = projects
            .map((p) => {
            var _a;
            const c = (_a = counts.get(p.prefix)) !== null && _a !== void 0 ? _a : { open: 0, review: 0, total: 0 };
            return `<li class="issue-row project-row">
          <a class="issue-link issue-title" href="#/issues">${esc(p.name)}</a>
          <span class="badge project-badge">${esc(p.prefix)}</span>
          ${p.default ? '<span class="badge">default</span>' : ""}
          <span class="hint">${c.total} issue(s) · ${c.open} open · ${c.review} awaiting decision</span>
        </li>`;
        })
            .join("");
        el.innerHTML = `
      <div class="list-toolbar">
        <h2>Projects</h2>
        <button id="new-project-btn" class="btn" title="Create a project (needs the API token)">+ New project</button>
      </div>
      ${newProjectFormHtml()}
      <ul class="issue-list">${rows}</ul>
      ${tokenBarHtml()}`;
        bindTokenBar();
        bindNewProject();
        // Clicking a project row filters the issue list to that project.
        document.querySelectorAll(".project-row").forEach((row, idx) => {
            row.addEventListener("click", () => {
                const select = document.getElementById("project-select");
                if (select && projects[idx]) {
                    select.value = projects[idx].prefix;
                    select.dispatchEvent(new Event("change"));
                    window.location.hash = "#/issues";
                }
            });
        });
    }
    catch (e) {
        el.innerHTML = `<p class="error">${esc(e instanceof Error ? e.message : String(e))}</p>`;
    }
}
/** #178: the "+ New project" create form (moved from the sidebar to the
 * Projects page; #175/#166 semantics unchanged). */
function newProjectFormHtml() {
    return `<div id="new-project-form" class="new-project-form" hidden>
    <input id="new-project-name" type="text" placeholder="name (e.g. ops)" aria-label="Project name">
    <input id="new-project-prefix" type="text" placeholder="prefix (e.g. OPS)" aria-label="Issue-id prefix">
    <button id="new-project-create" class="btn btn-primary">Create</button>
    <button id="new-project-cancel" class="btn btn-ghost">Cancel</button>
    <p id="new-project-result" class="edit-result" aria-live="polite"></p>
  </div>`;
}
async function renderReleases() {
    var _a;
    const el = app();
    if (!el)
        return;
    try {
        const releases = await fetchJson("/api/releases");
        // #206: never early-return on empty — the toolbar + create form must
        // render so the first release can be created from the GUI.
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
        el.innerHTML = `
      ${tokenBarHtml()}
      <div class="list-toolbar">
        <button id="new-release-btn" class="btn" title="Create a release (needs the API token)">+ New release</button>
      </div>
      ${newReleaseFormHtml()}
      ${releases.length === 0
            ? `<p class="loading">No releases in this issue repo yet.</p>`
            : `<ul class="issue-list">${rows}</ul>`}`;
        bindTokenBar();
        (_a = document.getElementById("new-release-btn")) === null || _a === void 0 ? void 0 : _a.addEventListener("click", () => {
            const f = document.getElementById("new-release-form");
            if (f)
                f.hidden = !f.hidden;
            if (!(f === null || f === void 0 ? void 0 : f.hidden)) {
                const t = document.getElementById("new-release-id");
                t === null || t === void 0 ? void 0 : t.focus();
            }
        });
        bindNewRelease();
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
/** #137: the Settings view — whitelisted config (agent block, project,
 * remote) with token-authed editing via PUT /api/config. `api_token` is
 * never fetched, displayed, or accepted by the API. */
/** #137/#169: the Settings view — whitelisted config with a mode-aware
 * agent form (local command or HTTP endpoint). `api_token` and agent key
 * material are never fetched, displayed, or accepted by the API: the key
 * field is write-only and only ever used to print a one-time hint for
 * `.tissue/secrets.yml` / env — the API stores just the `api_key_ref`. */
async function renderSettings() {
    var _a, _b, _c, _d, _e, _f, _g, _h, _j, _k, _l, _o, _p, _q;
    const el = app();
    if (!el)
        return;
    el.innerHTML = `
    <h2 class="view-title">Settings</h2>
    ${settingsTokenBarHtml()}
    <div class="settings-box">
      <p class="settings-note">Non-secret parts of <code>.tissue/config.yml</code>.
        The API token is never shown here — set it via <code>TISSUE_API_TOKEN</code>
        or the <code>api_token:</code> line in the repo (env var recommended).</p>
      <button id="agent-test-btn" class="btn" type="button" title="Probe the configured agent (POST /api/agent/test)">Test connection</button>
      <p id="agent-test-result" class="edit-result" aria-live="polite"></p>
      <label>Agent mode
        <select id="cfg-agent-mode" aria-label="Agent mode">
          <option value="none">None (sync stays record-only)</option>
          <option value="command">Local command</option>
          <option value="http">HTTP endpoint</option>
        </select>
      </label>
      <div id="cfg-mode-command">
        <label>Agent command
          <input id="cfg-agent-command" type="text" placeholder="e.g. claude">
        </label>
        <label>Agent args (one per line; <code>{issue}</code> is replaced with the issue id)
          <textarea id="cfg-agent-args" rows="3"></textarea>
        </label>
      </div>
      <div id="cfg-mode-http" hidden>
        <label>Endpoint URL
          <input id="cfg-agent-endpoint" type="text" placeholder="e.g. http://127.0.0.1:8080/v1/chat/completions">
        </label>
        <label>Model
          <input id="cfg-agent-model" type="text" placeholder="e.g. mistral-large-latest">
        </label>
        <label>API key reference (<code>api_key_ref</code> — a name, never the key)
          <input id="cfg-agent-keyref" type="text" placeholder="e.g. mistral  or  env:TISSUE_AGENT_API_KEY">
        </label>
        <label>API key (write-only — never stored in config or sent to the API)
          <span class="key-field">
            <input id="cfg-agent-key" type="password" autocomplete="off" placeholder="only needed once, to fill secrets.yml">
            <button id="cfg-agent-key-toggle" type="button" class="btn btn-ghost">Show</button>
          </span>
        </label>
      </div>
      <label>Agent timeout (seconds)
        <input id="cfg-agent-timeout" type="number" min="1" step="1">
      </label>
      <label>Remote
        <input id="cfg-remote" type="text" placeholder="e.g. /srv/tissue-remote (empty = none)">
      </label>
      <button id="cfg-save" class="btn btn-primary">Save settings</button>
      <p id="cfg-result" class="edit-result" aria-live="polite"></p>
      <h3>Current config (whitelisted)</h3>
      <pre id="cfg-view" class="cfg-view">loading…</pre>
      <h3>API tokens</h3>
      <p class="settings-note">The token value is shown <strong>only once</strong>, right
        after issue — copy it then. The list shows id/label/created, never the secret.
        Managing tokens needs the API token itself (the field at the top).</p>
      <div id="tokens-list" class="tokens-list">loading…</div>
      <label>New token label
        <input id="token-label" type="text" placeholder="e.g. human:tuomas or agent:dev-1" aria-label="New token label">
      </label>
      <button id="token-issue" class="btn">Issue token</button>
      <div id="token-once" class="token-once" hidden>
        <p>New token — shown once, never again:</p>
        <span class="key-field">
          <input id="token-secret" type="text" readonly aria-label="New token value (shown once)">
          <button id="token-copy" type="button" class="btn btn-ghost">Copy</button>
        </span>
      </div>
      <p id="token-result" class="edit-result" aria-live="polite"></p>
    </div>`;
    bindTokenBar();
    let cfg = {};
    try {
        cfg = await fetchJson("/api/config");
    }
    catch (e) {
        document.getElementById("cfg-view").textContent = String(e instanceof Error ? e.message : e);
    }
    const agent = cfg.agent;
    const mode = (_a = agent === null || agent === void 0 ? void 0 : agent.mode) !== null && _a !== void 0 ? _a : ((agent === null || agent === void 0 ? void 0 : agent.command) ? "command" : "none");
    document.getElementById("cfg-agent-mode").value = mode;
    document.getElementById("cfg-agent-command").value = (_b = agent === null || agent === void 0 ? void 0 : agent.command) !== null && _b !== void 0 ? _b : "";
    document.getElementById("cfg-agent-args").value =
        ((_c = agent === null || agent === void 0 ? void 0 : agent.args) !== null && _c !== void 0 ? _c : []).join("\n");
    document.getElementById("cfg-agent-endpoint").value = (_d = agent === null || agent === void 0 ? void 0 : agent.endpoint) !== null && _d !== void 0 ? _d : "";
    document.getElementById("cfg-agent-model").value = (_e = agent === null || agent === void 0 ? void 0 : agent.model) !== null && _e !== void 0 ? _e : "";
    document.getElementById("cfg-agent-keyref").value = (_f = agent === null || agent === void 0 ? void 0 : agent.api_key_ref) !== null && _f !== void 0 ? _f : "";
    document.getElementById("cfg-agent-timeout").value =
        String((_g = agent === null || agent === void 0 ? void 0 : agent.timeout_sec) !== null && _g !== void 0 ? _g : 120);
    document.getElementById("cfg-remote").value = (_h = cfg.remote) !== null && _h !== void 0 ? _h : "";
    document.getElementById("cfg-view").textContent = JSON.stringify(cfg, null, 2);
    const syncModeSections = () => {
        const m = document.getElementById("cfg-agent-mode").value;
        const cmd = document.getElementById("cfg-mode-command");
        const http = document.getElementById("cfg-mode-http");
        if (cmd)
            cmd.hidden = m !== "command";
        if (http)
            http.hidden = m !== "http";
    };
    syncModeSections();
    (_j = document.getElementById("cfg-agent-mode")) === null || _j === void 0 ? void 0 : _j.addEventListener("change", syncModeSections);
    // #189: "Test connection" probes the configured agent via POST /api/agent/test.
    (_k = document.getElementById("agent-test-btn")) === null || _k === void 0 ? void 0 : _k.addEventListener("click", async () => {
        var _a, _b, _c, _d, _e;
        const result = document.getElementById("agent-test-result");
        if (!result)
            return;
        result.textContent = "Testing…";
        result.className = "edit-result";
        try {
            const r = await postJson("/api/agent/test", {});
            if (r.ok) {
                result.textContent = `OK (${(_a = r.mode) !== null && _a !== void 0 ? _a : "?"}) — ${(_b = r.detail) !== null && _b !== void 0 ? _b : ""} in ${(_c = r.latency_ms) !== null && _c !== void 0 ? _c : "?"} ms`;
                result.classList.add("ok");
            }
            else {
                result.textContent = `Failed (${(_d = r.mode) !== null && _d !== void 0 ? _d : "?"}) — ${(_e = r.detail) !== null && _e !== void 0 ? _e : ""}${r.error ? `: ${r.error}` : ""}`;
                result.classList.add("error-text");
            }
        }
        catch (e) {
            const msg = e instanceof Error ? e.message : String(e);
            result.textContent = /401|token/i.test(msg)
                ? "Testing failed — set the API token first (write actions need it)."
                : `Testing failed: ${msg}`;
            result.classList.add("error-text");
        }
    });
    const keyInput = document.getElementById("cfg-agent-key");
    (_l = document.getElementById("cfg-agent-key-toggle")) === null || _l === void 0 ? void 0 : _l.addEventListener("click", (ev) => {
        if (!keyInput)
            return;
        const show = keyInput.type === "password";
        keyInput.type = show ? "text" : "password";
        ev.target.textContent = show ? "Hide" : "Show";
    });
    (_o = document.getElementById("cfg-save")) === null || _o === void 0 ? void 0 : _o.addEventListener("click", async () => {
        var _a;
        const result = document.getElementById("cfg-result");
        if (!result)
            return;
        const mode = document.getElementById("cfg-agent-mode").value;
        const timeoutRaw = document.getElementById("cfg-agent-timeout").value.trim();
        const timeout = timeoutRaw ? parseInt(timeoutRaw, 10) : NaN;
        if (timeoutRaw && (!Number.isFinite(timeout) || timeout < 1)) {
            result.textContent = "Timeout must be a positive number of seconds.";
            return;
        }
        const timeoutSec = timeoutRaw ? timeout : 120;
        const remote = document.getElementById("cfg-remote").value.trim();
        let agentUpdate = null;
        if (mode === "command") {
            const command = document.getElementById("cfg-agent-command").value.trim();
            if (!command) {
                result.textContent = "Command mode needs a command (or switch mode to None).";
                return;
            }
            const args = document.getElementById("cfg-agent-args").value
                .split("\n")
                .map((s) => s.trim())
                .filter((s) => s.length > 0);
            agentUpdate = { mode: "command", command, args, timeout_sec: timeoutSec };
        }
        else if (mode === "http") {
            const endpoint = document.getElementById("cfg-agent-endpoint").value.trim();
            const keyRef = document.getElementById("cfg-agent-keyref").value.trim();
            const model = document.getElementById("cfg-agent-model").value.trim();
            if (!/^https?:\/\/\S+$/.test(endpoint)) {
                result.textContent = "Endpoint must be an http(s) URL (e.g. http://127.0.0.1:8080/v1/chat/completions).";
                return;
            }
            if (!keyRef || /\s/.test(keyRef)) {
                result.textContent = "API key reference is required and must be a single token (a secrets.yml name or env:VAR).";
                return;
            }
            agentUpdate = { mode: "http", endpoint, model: model || null, api_key_ref: keyRef, timeout_sec: timeoutSec };
        }
        const update = { remote, agent: agentUpdate, actor: "human:gui" };
        result.textContent = "…";
        try {
            await putJson("/api/config", update);
            // The key field is write-only: if a key was entered, show the one-time
            // instruction for storing it outside git — the API never saw it.
            const keyVal = (_a = keyInput === null || keyInput === void 0 ? void 0 : keyInput.value.trim()) !== null && _a !== void 0 ? _a : "";
            if (mode === "http" && keyVal && agentUpdate) {
                const ref = agentUpdate.api_key_ref;
                result.textContent =
                    `Saved — config updated and committed. Store the API key once, outside git: ` +
                        `add \`${esc(ref)}: "…"\` to <code>.tissue/secrets.yml</code> (gitignored), or export it as <code>TISSUE_AGENT_API_KEY</code>.`;
            }
            else {
                result.textContent = "Saved — config updated and committed.";
            }
            await renderSettings();
        }
        catch (e) {
            const msg = e instanceof Error ? e.message : String(e);
            result.textContent = /401|token/i.test(msg)
                ? "Saving failed — set the API token first (it protects all write actions)."
                : msg;
        }
    });
    await renderTokensSection();
    (_p = document.getElementById("token-issue")) === null || _p === void 0 ? void 0 : _p.addEventListener("click", async () => {
        const result = document.getElementById("token-result");
        const labelEl = document.getElementById("token-label");
        if (!result || !labelEl)
            return;
        const label = labelEl.value.trim();
        if (!label || /\s/.test(label)) {
            result.textContent = "Label is required and must be a single token (e.g. human:tuomas).";
            return;
        }
        result.textContent = "…";
        try {
            const r = await postJson("/api/tokens", {
                label,
                actor: "human:gui",
            });
            const once = document.getElementById("token-once");
            const secretEl = document.getElementById("token-secret");
            if (once && secretEl) {
                once.hidden = false;
                secretEl.value = r.secret;
                secretEl.focus();
                secretEl.select();
            }
            result.textContent = `Issued ${r.id} (${r.label}) — copy the value now; it is shown only here.`;
            labelEl.value = "";
            await renderTokensSection();
        }
        catch (e) {
            const msg = e instanceof Error ? e.message : String(e);
            result.textContent = /401|token/i.test(msg)
                ? "Issuing failed — set the API token first (the token API is itself token-protected)."
                : msg;
        }
    });
    (_q = document.getElementById("token-copy")) === null || _q === void 0 ? void 0 : _q.addEventListener("click", () => {
        var _a;
        const secretEl = document.getElementById("token-secret");
        if (!secretEl)
            return;
        secretEl.select();
        const done = () => {
            const r = document.getElementById("token-result");
            if (r)
                r.textContent = "Copied to clipboard.";
        };
        if ((_a = navigator.clipboard) === null || _a === void 0 ? void 0 : _a.writeText) {
            navigator.clipboard.writeText(secretEl.value).then(done, () => {
                document.execCommand("copy");
                done();
            });
        }
        else {
            document.execCommand("copy");
            done();
        }
    });
}
/** #174: render the masked token list into the Settings page and bind the
 * per-row Revoke buttons. Needs the bearer token; without it the section
 * shows a hint instead of leaking anything. */
async function renderTokensSection() {
    const el = document.getElementById("tokens-list");
    if (!el)
        return;
    try {
        const data = await fetchAuthed("/api/tokens");
        const legacyNote = data.legacy
            ? `<p class="settings-note">A legacy plaintext <code>api_token:</code> line is still in
         <code>config.yml</code> — it migrates to hash-only storage on the next
         <code>tissue token list</code> / <code>token issue</code>.</p>`
            : "";
        const list = data.tokens.length === 0
            ? `<p class="placeholder">No managed tokens.</p>`
            : `<ul class="token-list">` +
                data.tokens
                    .map((t) => `<li>
                <span class="issue-id">${esc(t.id)}</span>
                <span>${esc(t.label)}</span>
                <span class="rec-ts">${esc(t.created)}</span>
                <button class="btn btn-reject" data-revoke="${esc(t.id)}">Revoke</button>
              </li>`)
                    .join("") +
                `</ul>`;
        el.innerHTML = legacyNote + list;
        el.querySelectorAll("[data-revoke]").forEach((b) => {
            b.addEventListener("click", async () => {
                var _a;
                const id = (_a = b.dataset.revoke) !== null && _a !== void 0 ? _a : "";
                const result = document.getElementById("token-result");
                try {
                    await postJson(`/api/tokens/${encodeURIComponent(id)}/revoke`, { actor: "human:gui" });
                    if (result)
                        result.textContent = `Revoked ${id} — it now fails closed.`;
                    await renderTokensSection();
                }
                catch (e) {
                    if (result) {
                        const msg = e instanceof Error ? e.message : String(e);
                        result.textContent = /401|token/i.test(msg)
                            ? "Revoke failed — set the API token first."
                            : msg;
                    }
                }
            });
        });
    }
    catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        el.innerHTML = `<p class="error">${esc(/401|token/i.test(msg)
            ? "Set the API token first (top of this page) to manage tokens."
            : msg)}</p>`;
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
        <button id="edit-submit">Save changes</button>
        <button id="sync-btn" title="Reconcile your edits into the agent stream (records the exact field-level changes since the last sync)">Sync</button>
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
        var _a, _b;
        result.textContent = "…";
        const summary = prompt("One-line summary of your edit (recorded with the exact field-level changes):");
        if (!summary) {
            result.textContent = "Sync cancelled — no summary given.";
            return;
        }
        try {
            // #136: sync computes the field-level diff since the last sync
            // baseline and records it — agents reconcile from that data.
            const r = await postJson(`/api/issues/${encodeURIComponent(issue.id)}/sync`, { summary, actor: "human:gui" });
            if (r.synced) {
                // #137: if an agent is configured, the sync also ran it; show the outcome.
                if (r.agent_run) {
                    const a = r.agent_run;
                    result.textContent = a.success
                        ? `Synced — changes recorded, and the configured agent ran ok (${a.duration_ms} ms).`
                        : a.timed_out
                            ? `Synced — changes recorded; the configured agent timed out (recorded as a failure note).`
                            : `Synced — changes recorded; the configured agent failed (exit ${(_a = a.exit_code) !== null && _a !== void 0 ? _a : "?"}).`;
                }
                else {
                    result.textContent = "Synced — your changes are recorded in the agent stream for agents to reconcile from. (No agent configured — record only.)";
                }
                void route();
            }
            else {
                result.textContent = (_b = r.reason) !== null && _b !== void 0 ? _b : "Nothing to sync — no changes since the last sync.";
            }
        }
        catch (e) {
            result.textContent = e instanceof Error ? e.message : String(e);
        }
    });
}
/** #170: a pending proposal rendered inside the issue's AI tab — same
 * diff + approve/reject actions as the queue card, minus the issue link
 * (we are already on that issue). */
function proposalCardDetailHtml(p) {
    var _a, _b;
    return `<li class="proposal-card" id="proposal-${esc(p.proposal_id)}">
    <div class="proposal-head">
      <span class="proposal-id">${esc(p.proposal_id)}</span>
      <span class="proposal-field">field: <code>${esc(p.field)}</code></span>
      <span class="rec-ts">${esc(p.ts)}</span>
    </div>
    <div class="proposal-diff">
      <div class="diff-row"><span class="diff-label">current</span> <code>${esc(p.current)}</code></div>
      <div class="diff-row"><span class="diff-label">proposed</span> <code class="proposed">${esc(p.proposed)}</code></div>
    </div>
    ${p.rationale ? `<p class="proposal-rationale">${esc(p.rationale)}</p>` : ""}
    <div class="proposal-actions">
      <button class="btn btn-approve" data-issue="${esc((_a = p.issue_id) !== null && _a !== void 0 ? _a : "")}"
        data-proposal="${esc(p.proposal_id)}">Approve</button>
      <button class="btn btn-reject" data-issue="${esc((_b = p.issue_id) !== null && _b !== void 0 ? _b : "")}"
        data-proposal="${esc(p.proposal_id)}">Reject…</button>
      <span class="proposal-result" data-result-for="${esc(p.proposal_id)}"></span>
    </div>
  </li>`;
}
/** Tab bar for the issue detail's Human/AI split (#170). */
function bindDetailTabs(id, active) {
    document.querySelectorAll(".tab").forEach((b) => {
        b.addEventListener("click", () => {
            const t = b.dataset.tab;
            if (t === active)
                return;
            window.location.hash =
                t === "ai"
                    ? `#/issue/${encodeURIComponent(id)}/ai`
                    : `#/issue/${encodeURIComponent(id)}`;
        });
    });
}
/** #170: issue detail with a Human / AI tab split — the human format
 * (front matter + body, editable) on one side, the agent stream,
 * proposals, and last-touch state on the other. */
async function renderDetail(id, tab = "human") {
    var _a;
    const el = app();
    if (!el)
        return;
    try {
        const data = await fetchJson(`/api/issues/${encodeURIComponent(id)}`);
        const issue = data.issue;
        const fm = [
            ["id", esc(issue.id)],
            ["project", `<span class="badge project-badge">${esc(projectOf(issue.id))}</span> <span class="hint">(immutable — the id prefix is the identity, #172)</span>`],
            ["status", `<span class="status ${statusClass(issue.status)}">${esc(issue.status)}</span>`],
            ["last touch", lastTouchBadge(issue)],
            ["release", esc((_a = issue.release) !== null && _a !== void 0 ? _a : "—")],
            ["tags", issue.tags.length ? esc(issue.tags.join(", ")) : "—"],
            ["created", esc(issue.created)],
            ["updated", esc(issue.updated)],
        ]
            .map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`)
            .join("");
        // AI side (#170): pending proposals + the agent stream (#98 AC4, all
        // record types, collapsed by default — heavy agent activity must not
        // break readability, #3).
        let proposals = [];
        try {
            proposals = await fetchJson(`/api/issues/${encodeURIComponent(id)}/proposals`);
        }
        catch (_b) {
            // unreadable stream: the proposals section says so below
        }
        let records = [];
        try {
            records = await fetchJson(`/api/issues/${encodeURIComponent(id)}/records`);
        }
        catch (_c) {
            // no stream or unreadable: the timeline section says so below
        }
        const timelineRows = records
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
        const timeline = `
      <section class="timeline">
        <h3>Agent stream (${records.length} record${records.length === 1 ? "" : "s"}, collapsed)</h3>
        ${records.length > 0 ? timelineRows : `<p class="placeholder">No agent records yet.</p>`}
      </section>`;
        const proposalsHtml = `
      <section class="proposals">
        <h3>Proposals (${proposals.length} pending)</h3>
        ${proposals.length > 0
            ? `<ul class="proposal-list">${proposals
                .map((p) => proposalCardDetailHtml(Object.assign(Object.assign({}, p), { issue_id: id })))
                .join("")}</ul>`
            : `<p class="placeholder">No pending proposals. <a class="issue-link" href="#/review">Open the queue</a></p>`}
      </section>`;
        // #189: surface the last agent-run outcome as a visible banner on the AI
        // tab (in addition to the stream record).
        const lastAgentRun = [...records]
            .reverse()
            .find((r) => { var _a; return ((_a = r.data) === null || _a === void 0 ? void 0 : _a.type) === "agent-run"; });
        const agentBanner = lastAgentRun
            ? (() => {
                var _a, _b, _c;
                const d = lastAgentRun.data;
                const failed = d.timed_out === true || (d.exit_code !== undefined && d.exit_code !== 0);
                const msg = failed
                    ? `agent run failed: ${d.timed_out === true ? `timeout after ${(_a = d.timeout_sec) !== null && _a !== void 0 ? _a : "?"}s` : `exit ${d.exit_code}`}`
                    : `agent run ok: exit ${(_b = d.exit_code) !== null && _b !== void 0 ? _b : "?"} in ${(_c = d.duration_ms) !== null && _c !== void 0 ? _c : "?"} ms`;
                return `<p class="agent-banner ${failed ? "error" : "ok"}">${esc(msg)}</p>`;
            })()
            : "";
        const aiPane = `
      ${agentBanner}
      <dl class="front-matter">
        <dt>last touch</dt><dd>${lastTouchBadge(issue)}</dd>
        ${issue.tags.includes(NEEDS_REVIEW)
            ? `<dt>awaiting decision</dt><dd><a class="issue-link" href="#/review">open the queue</a></dd>`
            : ""}
      </dl>
      ${proposalsHtml}
      ${timeline}`;
        const humanPane = `
      <dl class="front-matter">${fm}</dl>
      ${editFormHtml(issue)}
      <div class="body">${renderMarkdown(issue.body)}</div>`;
        el.innerHTML = `
      <a class="back-link" href="#/issues">&larr; All issues</a>
      <article class="detail">
        <h2>${esc(issue.id)} — ${esc(issue.title)} ${badgeHtml(issue.tags)}</h2>
        ${tokenBarHtml()}
        <div class="tabs" role="tablist">
          <button class="tab${tab === "human" ? " active" : ""}" data-tab="human" role="tab">Human</button>
          <button class="tab${tab === "ai" ? " active" : ""}" data-tab="ai" role="tab">AI${proposals.length > 0 ? ` (${proposals.length} awaiting decision)` : ""}</button>
        </div>
        ${tab === "ai" ? aiPane : humanPane}
      </article>`;
        bindTokenBar();
        bindDetailTabs(id, tab);
        if (tab === "ai") {
            bindProposalActions();
        }
        else {
            wireEditForm(issue);
        }
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
        // #175/#172: refresh the cached project list too — the new-issue form's
        // project selector reads it.
        // #175/#178: refresh the cached project list too (switcher + Projects
        // page + new-issue selector all read it).
        try {
            cachedProjects = await loadProjects();
        }
        catch (_a) {
            cachedProjects = [];
        }
        void populateProjectSelect(allIssues);
    }
    catch (e) {
        const el = app();
        if (el)
            el.innerHTML = `<p class="error">${esc(e instanceof Error ? e.message : String(e))}</p>`;
    }
}
async function route() {
    const hash = window.location.hash || "#/issues";
    const detail = /^#\/issue\/([^/]+)(?:\/(ai))?$/.exec(hash);
    if (detail) {
        updateNav("");
        await renderDetail(decodeURIComponent(detail[1]), detail[2] === "ai" ? "ai" : "human");
        return;
    }
    const releaseDetail = /^#\/releases\/([^/]+)$/.exec(hash);
    if (releaseDetail) {
        updateNav("releases");
        await renderReleaseDetail(decodeURIComponent(releaseDetail[1]));
        return;
    }
    await loadIssues();
    const view = /^#\/(review|releases|projects|settings)/.exec(hash);
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
    if (view && view[1] === "projects") {
        updateNav("projects");
        await renderProjects();
        return;
    }
    if (view && view[1] === "settings") {
        updateNav("settings");
        await renderSettings();
        return;
    }
    updateNav("issues");
    renderIssueList();
}
/** #143: footer line `tissue vX · API vN · LICENSE`, fetched from
 * /api/meta on every load; the served HTML already carries the
 * compiled-in version as the fallback if the fetch fails. */
async function loadMetaFooter() {
    const el = document.getElementById("app-footer");
    if (!el)
        return;
    try {
        const meta = await fetchJson("/api/meta");
        const link = meta.license_url
            ? ` <a href="${esc(meta.license_url)}" rel="noopener noreferrer" target="_blank">${esc(meta.license)}</a>`
            : esc(meta.license);
        el.innerHTML = `tissue v${esc(meta.version)} · API ${esc(meta.api_version)} · ${link}`;
    }
    catch (_a) {
        // meta unavailable: keep the compiled-in fallback already in the HTML
    }
}
function init() {
    void loadMetaFooter();
    const select = document.getElementById("project-select");
    if (select) {
        select.addEventListener("change", () => {
            const el = app();
            if (el && /^#\/(issues)?$/.test(window.location.hash || "#/issues")) {
                renderIssueList();
            }
        });
    }
    bindNewProject();
    window.addEventListener("hashchange", () => void route());
    void route();
}
init();
