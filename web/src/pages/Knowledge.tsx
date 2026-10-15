import { useEffect, useState, type FormEvent } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { api } from "../api/client";
import type { Context, Note } from "../api/types";
import { useRepo } from "../components/RepoShell";
import { toast } from "../components/Toasts";
import { AgentBadge, Badge, EmptyState, ErrorState, Sha, Skeleton, SkeletonRows, Tag } from "../components/ui";
import { countdown, fullDate, summaryLine, timeAgo } from "../lib/format";
import { useAsync, useMutation } from "../lib/useAsync";

export default function Knowledge() {
  const { name } = useRepo();
  const [sp, setSp] = useSearchParams();
  const tags = (sp.get("tags") || "").split(",").filter(Boolean);
  const path = sp.get("path") || "";
  const q = sp.get("q") || "";
  const [qDraft, setQDraft] = useState(q);
  const [pathDraft, setPathDraft] = useState(path);
  const [showNew, setShowNew] = useState(false);
  useEffect(() => setQDraft(q), [q]);
  useEffect(() => setPathDraft(path), [path]);

  const notes = useAsync(() => api.notes(name, { tags, path: path || undefined, q: q || undefined, limit: 100 }), [name, tags.join(","), path, q]);
  const tagCounts = useAsync(() => api.tagCounts(name), [name], { silent: true });

  const update = (patch: Record<string, string>) => {
    const next = new URLSearchParams(sp);
    for (const [k, v] of Object.entries(patch)) v ? next.set(k, v) : next.delete(k);
    setSp(next, { replace: true });
  };
  const toggleTag = (t: string) => update({ tags: (tags.includes(t) ? tags.filter((x) => x !== t) : [...tags, t]).join(",") });
  const base = `/r/${encodeURIComponent(name)}/knowledge`;

  return (
    <div className="two-col">
      <div className="stack">
        <div className="page-head" style={{ marginBottom: 0 }}>
          <div>
            <h1>Knowledge</h1>
            <p className="muted">Decisions, gotchas, specs and how-tos stored next to the code and tagged by path.</p>
          </div>
          <button className="btn primary" onClick={() => setShowNew((s) => !s)}>{showNew ? "Cancel" : "New note"}</button>
        </div>
        {showNew ? <NewNote onCreated={(n) => { setShowNew(false); notes.reload(); tagCounts.reload(); toast(`Created ${n.id}`, "success"); }} /> : null}
        <form
          className="filters"
          style={{ marginBottom: 0 }}
          onSubmit={(e) => {
            e.preventDefault();
            update({ q: qDraft.trim(), path: pathDraft.trim() });
          }}
        >
          <input className="input sm" style={{ width: 240 }} placeholder="Search title, body, tags…" value={qDraft} onChange={(e) => setQDraft(e.target.value)} aria-label="Search" />
          <input className="input sm" style={{ width: 220 }} placeholder="Path, e.g. src/auth/login.ts" value={pathDraft} onChange={(e) => setPathDraft(e.target.value)} aria-label="Path filter" />
          <button className="btn sm" type="submit">Filter</button>
          {tags.length || path || q ? <button type="button" className="btn sm ghost" onClick={() => setSp({}, { replace: true })}>Clear</button> : null}
        </form>
        <div className="row wrap" style={{ gap: 6 }}>
          {tagCounts.data?.map((t) => (
            <button key={t.tag} type="button" className={`chip ns-${t.tag.split(":")[0]} ${tags.includes(t.tag) ? "active" : ""}`} onClick={() => toggleTag(t.tag)} aria-pressed={tags.includes(t.tag)}>
              {t.tag} <span className="count">{t.count}</span>
            </button>
          ))}
          {tagCounts.loading && !tagCounts.data ? <span className="skeleton skeleton-line" style={{ width: 240 }} /> : null}
        </div>
        {notes.error ? <ErrorState error={notes.error} retry={notes.reload} /> : null}
        <div className="box">
          {notes.loading && !notes.data ? (
            <SkeletonRows rows={5} />
          ) : notes.data?.length === 0 ? (
            <EmptyState icon="✎" title="No notes match">{tags.length || path || q ? "Loosen the filters." : "Agents write notes with agit know add."}</EmptyState>
          ) : (
            notes.data?.map((n) => (
              <div key={n.id} className={`note-row ${n.superseded_by ? "superseded" : ""}`}>
                <div className="grow" style={{ minWidth: 0 }}>
                  <div className="row wrap" style={{ gap: 8 }}>
                    <Link className="title" to={`${base}/${encodeURIComponent(n.id)}`}>{n.title}</Link>
                    {n.superseded_by ? <Badge tone="attention" title={`superseded by ${n.superseded_by}`}>superseded</Badge> : null}
                  </div>
                  {n.excerpt ? <div className="excerpt">{n.excerpt.replace(/[`*_#]/g, "")}</div> : null}
                  <div className="tags">
                    {n.tags.map((t) => <Tag key={t} tag={t} onClick={() => toggleTag(t)} active={tags.includes(t)} />)}
                    {n.paths.map((p) => <code key={p} className="small muted">{p}</code>)}
                  </div>
                </div>
                <div className="stack tight small muted" style={{ alignItems: "flex-end", flexShrink: 0 }}>
                  <AgentBadge agent={{ id: n.author }} />
                  <span title={fullDate(n.updated_ms)}>{timeAgo(n.updated_ms)}</span>
                  <Confidence v={n.confidence} />
                </div>
              </div>
            ))
          )}
        </div>
      </div>
      <aside className="sidebar">
        <ContextPanel initialPath={path} />
      </aside>
    </div>
  );
}

export function Confidence({ v }: { v: number }) {
  return (
    <span className="confidence" title={`confidence ${v}`}>
      <span className="bar"><i style={{ width: `${Math.round(v * 100)}%` }} /></span>
      {Math.round(v * 100)}%
    </span>
  );
}

export function ContextPanel({ initialPath = "" }: { initialPath?: string }) {
  const { name } = useRepo();
  const [draft, setDraft] = useState(initialPath);
  const [path, setPath] = useState(initialPath);
  useEffect(() => {
    setDraft(initialPath);
    if (initialPath) setPath(initialPath);
  }, [initialPath]);
  const ctx = useAsync(() => api.context(name, path, 10), [name, path], { enabled: !!path });
  const base = `/r/${encodeURIComponent(name)}`;
  return (
    <section className="context-panel">
      <h3>Context for a path</h3>
      <p className="small muted">What an agent should read before touching a path (<code>GET /context?path=</code>).</p>
      <form className="row" onSubmit={(e) => { e.preventDefault(); setPath(draft.trim()); }}>
        <input className="input sm" placeholder="src/auth/login.ts" value={draft} onChange={(e) => setDraft(e.target.value)} />
        <button className="btn sm" type="submit">Go</button>
      </form>
      {!path ? null : ctx.error ? <div className="mt8"><ErrorState error={ctx.error} retry={ctx.reload} /></div> : !ctx.data ? <Skeleton lines={4} /> : <ContextBody c={ctx.data} base={base} />}
    </section>
  );
}

function ContextBody({ c, base }: { c: Context; base: string }) {
  const now = Date.now();
  return (
    <div>
      <div className="section-title">Notes ({c.notes.length})</div>
      <ul>{c.notes.length === 0 ? <li className="muted">none</li> : c.notes.map((n) => <li key={n.id}><Link to={`${base}/knowledge/${encodeURIComponent(n.id)}`}>{n.title}</Link></li>)}</ul>
      <div className="section-title">Recent commits ({c.recent_commits.length})</div>
      <ul>{c.recent_commits.length === 0 ? <li className="muted">none</li> : c.recent_commits.slice(0, 6).map((k) => <li key={k.id} className="row" style={{ gap: 6 }}><Sha id={k.id} repo={base.split("/r/")[1] ? decodeURIComponent(base.split("/r/")[1]) : undefined} /><span className="truncate">{summaryLine(k.message)}</span></li>)}</ul>
      <div className="section-title">Active leases ({c.leases.length})</div>
      <ul>{c.leases.length === 0 ? <li className="muted">none</li> : c.leases.map((l) => <li key={l.id}><strong>{l.holder}</strong> <span className="muted">{l.mode}</span> {l.expires_ms ? <span className="muted">· {countdown(l.expires_ms, now)}</span> : null}</li>)}</ul>
      <div className="section-title">Open pull requests ({c.open_pulls.length})</div>
      <ul>{c.open_pulls.length === 0 ? <li className="muted">none</li> : c.open_pulls.map((p) => <li key={p.number}><Link to={`${base}/pulls/${p.number}`}>#{p.number} {p.title}</Link></li>)}</ul>
    </div>
  );
}

export function NewNote({ onCreated }: { onCreated: (n: Note) => void }) {
  const { name } = useRepo();
  const [f, setF] = useState({ title: "", body: "", tags: "", paths: "", commits: "", pulls: "", confidence: 0.8 });
  const create = useMutation(() => {
    const split = (s: string) => s.split(/[\n,]/).map((x) => x.trim()).filter(Boolean);
    return api.createNote(name, {
      title: f.title.trim(),
      body: f.body,
      tags: split(f.tags),
      paths: split(f.paths),
      commits: split(f.commits),
      pulls: split(f.pulls).map(Number).filter((n) => !Number.isNaN(n)),
      confidence: f.confidence,
    });
  });
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const n = await create.run();
    if (n) onCreated(n);
  };
  const set = (k: keyof typeof f) => (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => setF({ ...f, [k]: k === "confidence" ? Number(e.target.value) : e.target.value });
  return (
    <form className="box" onSubmit={submit}>
      <div className="box-head"><h3>New note</h3></div>
      <div className="box-body form" style={{ maxWidth: "none" }}>
        <label className="field">Title<input className="input" value={f.title} onChange={set("title")} required autoFocus /></label>
        <label className="field">Body <span className="hint">Markdown</span><textarea className="textarea" rows={6} value={f.body} onChange={set("body")} /></label>
        <div className="row">
          <label className="field">Tags <span className="hint">namespace:value, comma separated (area, kind, lang, service…)</span><input className="input" value={f.tags} onChange={set("tags")} placeholder="area:auth, kind:gotcha" required /></label>
          <label className="field">Paths <span className="hint">globs the note applies to</span><input className="input" value={f.paths} onChange={set("paths")} placeholder="src/auth/**" /></label>
        </div>
        <div className="row">
          <label className="field">Commits <span className="hint">ids, comma separated</span><input className="input mono" value={f.commits} onChange={set("commits")} /></label>
          <label className="field">Pull requests <span className="hint">numbers</span><input className="input" value={f.pulls} onChange={set("pulls")} placeholder="2, 3" /></label>
        </div>
        <label className="field">Confidence: {f.confidence.toFixed(2)}<input type="range" min={0} max={1} step={0.05} value={f.confidence} onChange={set("confidence")} /></label>
        <div className="actions"><button className="btn primary" disabled={create.busy || !f.title.trim() || !f.tags.trim()}>{create.busy ? "Saving…" : "Create note"}</button></div>
      </div>
    </form>
  );
}
