import { useEffect, useMemo, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { api } from "../api/client";
import type { CommitListItem, NoteSummary } from "../api/types";
import { BranchSelector } from "../components/BranchSelector";
import { CommitList } from "../components/CommitList";
import { useRepo } from "../components/RepoShell";
import { AgentBadge, EmptyState, ErrorState, NoteBadges, Score, SkeletonRows, Tag } from "../components/ui";
import { fullDate, pluralize, timeAgo } from "../lib/format";
import { buildMatcher, searchCode, type SearchHit, type SearchProgress } from "../lib/search";
import { useAsync } from "../lib/useAsync";

type TabId = "code" | "commits" | "notes";

export default function Search() {
  const { name, defaultBranch } = useRepo();
  const [sp, setSp] = useSearchParams();
  const q = (sp.get("q") || "").trim();
  const tab = (sp.get("tab") as TabId) || "code";
  const rev = sp.get("rev") || defaultBranch;
  const base = `/r/${encodeURIComponent(name)}`;
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [regex, setRegex] = useState(false);
  const [pathFilter, setPathFilter] = useState("");
  const [pathDraft, setPathDraft] = useState("");

  const setTab = (t: TabId) => {
    const next = new URLSearchParams(sp);
    if (t === "code") next.delete("tab");
    else next.set("tab", t);
    setSp(next, { replace: true });
  };

  useEffect(() => {
    document.title = q ? `${q} · search · ${name} · agit` : `search · ${name} · agit`;
  }, [q, name]);

  // Commits and notes are cheap: load them regardless of the active tab so the counts are visible.
  const commits = useAsync(() => api.commits(name, { rev, limit: 500 }), [name, rev], { silent: true, enabled: !!q });
  const notes = useAsync(() => api.notes(name, { q, limit: 100 }), [name, q], { silent: true, enabled: !!q });
  const needle = q.toLowerCase();
  const commitHits = useMemo(() => {
    if (!q) return [];
    const m = buildMatcher(q, { caseSensitive, regex });
    return (commits.data ?? []).filter((c) => {
      const hay = [c.message, c.intent ?? "", c.task ?? "", c.agent?.id ?? "", c.agent?.model ?? "", c.author.name, c.author.email, c.id];
      return m ? hay.some((h) => m(h).length > 0) : hay.some((h) => h.toLowerCase().includes(needle));
    });
  }, [commits.data, q, needle, caseSensitive, regex]);

  const code = useCodeSearch(q ? name : "", rev, q, { caseSensitive, regex, pathFilter }, tab === "code");

  const tabBtn = (id: TabId, label: string, count: number | undefined, busy?: boolean) => (
    <button role="tab" aria-selected={tab === id} className={tab === id ? "active" : ""} onClick={() => setTab(id)}>
      {label} {busy ? <span className="spinner" style={{ width: 10, height: 10 }} /> : count !== undefined ? <span className="count">{count}</span> : null}
    </button>
  );

  return (
    <div className="stack">
      <div className="page-head" style={{ marginBottom: 0 }}>
        <div>
          <h1>Search</h1>
          <p className="muted">{q ? <>Results for <strong>“{q}”</strong> in {name}</> : <>Type a query in the header box (press <kbd className="kbd">/</kbd>).</>}</p>
        </div>
        <BranchSelector repo={name} rev={rev} mode="commits" />
      </div>
      <nav className="tabs" role="tablist" aria-label="Search scopes" style={{ marginBottom: 0 }}>
        {tabBtn("code", "Code", code.progress.phase === "done" ? code.hits.length : code.hits.length || undefined, code.running)}
        {tabBtn("commits", "Commits", q ? commitHits.length : undefined, commits.loading)}
        {tabBtn("notes", "Notes", q ? notes.data?.length : undefined, notes.loading)}
      </nav>

      {!q ? (
        <div className="box"><EmptyState icon="⌕" title="Search this repository">Code is grepped client-side over the selected revision; commits match message, intent, task and agent; notes use the knowledge search.</EmptyState></div>
      ) : tab === "code" ? (
        <div className="stack">
          <form
            className="filters"
            style={{ marginBottom: 0 }}
            onSubmit={(e) => {
              e.preventDefault();
              setPathFilter(pathDraft.trim());
            }}
          >
            <input className="input sm" style={{ width: 220 }} placeholder="Limit to path, e.g. src/auth" value={pathDraft} onChange={(e) => setPathDraft(e.target.value)} aria-label="Path filter" />
            <button className="btn sm" type="submit">Apply</button>
            <label className="row small muted" style={{ gap: 4 }}><input type="checkbox" checked={caseSensitive} onChange={(e) => setCaseSensitive(e.target.checked)} /> match case</label>
            <label className="row small muted" style={{ gap: 4 }}><input type="checkbox" checked={regex} onChange={(e) => setRegex(e.target.checked)} /> regex</label>
            <div className="grow" />
            <CodeProgress p={code.progress} running={code.running} onCancel={code.cancel} error={code.error} />
          </form>
          {regex && !buildMatcher(q, { caseSensitive, regex }) ? <div className="error-box" role="alert"><span aria-hidden>⚠</span><div className="grow"><span className="code">invalid_regex</span><div>“{q}” is not a valid regular expression.</div></div></div> : null}
          {code.hits.length === 0 ? (
            code.running ? (
              <div className="box"><SkeletonRows rows={4} /></div>
            ) : code.progress.phase === "done" ? (
              <div className="box"><EmptyState icon="⌕" title="No code matches">{code.progress.total === 0 ? "No text files to scan at this revision." : `Scanned ${pluralize(code.progress.scanned, "file")}${code.progress.skipped ? `, skipped ${code.progress.skipped} binary or large` : ""}.`}</EmptyState></div>
            ) : null
          ) : (
            code.hits.map((h) => <HitBox key={h.file.path} hit={h} base={base} rev={rev} />)
          )}
        </div>
      ) : tab === "commits" ? (
        commits.error ? (
          <ErrorState error={commits.error} retry={commits.reload} />
        ) : commits.loading && !commits.data ? (
          <div className="box"><SkeletonRows rows={5} /></div>
        ) : commitHits.length === 0 ? (
          <div className="box"><EmptyState icon="◷" title="No matching commits">Searched the latest {commits.data?.length ?? 0} commits on {rev} by message, intent, task, agent and author.</EmptyState></div>
        ) : (
          <>
            <p className="small muted" style={{ margin: 0 }}>{commitHits.length} of the latest {commits.data?.length} commits on <span className="ref-pill">{rev}</span></p>
            <CommitList repo={name} commits={commitHits as CommitListItem[]} groupByDay={false} />
          </>
        )
      ) : notes.error ? (
        <ErrorState error={notes.error} retry={notes.reload} />
      ) : notes.loading && !notes.data ? (
        <div className="box"><SkeletonRows rows={4} /></div>
      ) : notes.data?.length === 0 ? (
        <div className="box"><EmptyState icon="✎" title="No matching notes">Knowledge search is BM25 over title, body, tags and paths; results are ranked best first.</EmptyState></div>
      ) : (
        <div className="box">{notes.data?.map((n) => <NoteHit key={n.id} n={n} base={base} />)}</div>
      )}
    </div>
  );
}

function CodeProgress({ p, running, onCancel, error }: { p: SearchProgress; running: boolean; onCancel: () => void; error: string | null }) {
  if (error) return <span className="small" style={{ color: "var(--danger)" }}>{error}</span>;
  if (p.phase === "listing") return <span className="row small muted" style={{ gap: 8 }}><span className="spinner" /> listing files… {p.listed}</span>;
  const pct = p.total ? Math.round((p.scanned / p.total) * 100) : 100;
  return (
    <span className="row small muted" style={{ gap: 8 }}>
      {running ? <span className="spinner" /> : null}
      <span>{running ? "scanning" : "scanned"} {p.scanned}/{p.total} files{p.skipped ? ` · ${p.skipped} skipped` : ""}{p.hits ? ` · ${pluralize(p.hits, "file")} match` : ""}</span>
      <span className="progress" aria-hidden><i style={{ width: `${pct}%` }} /></span>
      {running ? <button type="button" className="btn sm" onClick={onCancel}>Cancel</button> : null}
    </span>
  );
}

function useCodeSearch(repo: string, rev: string, q: string, opts: { caseSensitive: boolean; regex: boolean; pathFilter: string }, enabled: boolean) {
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [progress, setProgress] = useState<SearchProgress>({ phase: "listing", listed: 0, scanned: 0, total: 0, skipped: 0, hits: 0 });
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cancel, setCancel] = useState<() => void>(() => () => {});
  const { caseSensitive, regex, pathFilter } = opts;

  useEffect(() => {
    if (!repo || !q || !enabled) return;
    setHits([]);
    setError(null);
    setRunning(true);
    setProgress({ phase: "listing", listed: 0, scanned: 0, total: 0, skipped: 0, hits: 0 });
    const h = searchCode(repo, rev, q, { caseSensitive, regex, pathFilter }, setProgress, (hit) => setHits((xs) => [...xs, hit].sort((a, b) => a.file.path.localeCompare(b.file.path))));
    setCancel(() => h.cancel);
    h.promise
      .catch((e) => {
        if (e instanceof DOMException && e.name === "AbortError") return;
        setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => setRunning(false));
    return () => h.cancel();
  }, [repo, rev, q, caseSensitive, regex, pathFilter, enabled]);

  return { hits, progress, running, error, cancel: () => { cancel(); setRunning(false); } };
}

function HitBox({ hit, base, rev }: { hit: SearchHit; base: string; rev: string }) {
  const href = `${base}/blob/${encodeURIComponent(rev)}/${hit.file.path}`;
  return (
    <div className="box search-hit">
      <div className="box-head">
        <Link to={href} className="path">{hit.file.path}</Link>
        <span className="muted">{pluralize(hit.matches.length, "match", "matches")}{hit.more ? "+" : ""}</span>
      </div>
      <table>
        <tbody>
          {hit.matches.map((m) => (
            <tr key={m.line}>
              <td className="ln"><Link to={`${href}#L${m.line}`}>{m.line}</Link></td>
              <td className="code"><Highlighted text={m.text} ranges={m.ranges} /></td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Highlighted({ text, ranges }: { text: string; ranges: [number, number][] }) {
  const parts: React.ReactNode[] = [];
  let last = 0;
  ranges.forEach(([s, e], i) => {
    if (s > last) parts.push(text.slice(last, s));
    parts.push(<mark key={i}>{text.slice(s, e)}</mark>);
    last = e;
  });
  if (last < text.length) parts.push(text.slice(last));
  return <>{parts}</>;
}

function NoteHit({ n, base }: { n: NoteSummary; base: string }) {
  return (
    <div className={`note-row ${n.superseded_by ? "superseded" : ""}`}>
      <div className="grow" style={{ minWidth: 0 }}>
        <div className="row wrap" style={{ gap: 8 }}>
          <Link className="title" to={`${base}/knowledge/${encodeURIComponent(n.id)}`}>{n.title}</Link>
          <NoteBadges n={n} />
          <Score v={n.score} />
        </div>
        {n.excerpt ? <div className="excerpt">{n.excerpt.replace(/[`*_#]/g, "")}</div> : null}
        <div className="tags">{n.tags.map((t) => <Tag key={t} tag={t} repo={base.replace(/^\/r\//, "")} />)}{n.paths.map((p) => <code key={p} className="small muted">{p}</code>)}</div>
      </div>
      <div className="stack tight small muted" style={{ alignItems: "flex-end", flexShrink: 0 }}>
        <AgentBadge agent={{ id: n.author }} />
        <span title={fullDate(n.updated_ms)}>{timeAgo(n.updated_ms)}</span>
      </div>
    </div>
  );
}
