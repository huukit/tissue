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

const NEEDS_REVIEW = "needs-review";

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
    <a class="issue-link issue-title" href="#/${encodeURIComponent(issue.id)}">${title}</a>
    <span class="status ${statusClass(issue.status)}">${esc(issue.status)}</span>
    ${badgeHtml(issue.tags)}
    <span class="issue-updated" title="updated">${updated}</span>
  </li>`;
}

function issueNumber(id: string): number {
  const m = /^([A-Z]+)-(\d+)$/.exec(id);
  return m ? parseInt(m[2], 10) : Number.MAX_SAFE_INTEGER;
}

async function renderList(): Promise<void> {
  const el = app();
  if (!el) return;
  try {
    const data = await fetchJson<{ issues: IssueSummary[] }>("/api/issues");
    const issues = [...data.issues].sort((a, b) => issueNumber(a.id) - issueNumber(b.id));
    if (issues.length === 0) {
      el.innerHTML = `<p class="loading">No issues found in this issue repo.</p>`;
      return;
    }
    el.innerHTML = `<ul class="issue-list">${issues.map(rowHtml).join("")}</ul>`;
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
      <a class="back-link" href="#">&larr; All issues</a>
      <article class="detail">
        <h2>${esc(issue.id)} — ${esc(issue.title)} ${badgeHtml(issue.tags)}</h2>
        <dl class="front-matter">${fm}</dl>
        <div class="body">${renderMarkdown(issue.body)}</div>
      </article>`;
  } catch (e) {
    el.innerHTML = `<p class="error">${esc(e instanceof Error ? e.message : String(e))}</p>`;
  }
}

function route(): void {
  const hash = window.location.hash;
  const m = /^#\/([^/]+)$/.exec(hash);
  if (m) {
    void renderDetail(decodeURIComponent(m[1]));
  } else {
    void renderList();
  }
}

window.addEventListener("hashchange", route);
route();
