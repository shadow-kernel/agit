import { useEffect, useState, type FormEvent } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { api } from "../api/client";
import type { Context, Note, NoteSummary, StaleNote } from "../api/types";
import { useRepo } from "../components/RepoShell";
import { toast } from "../components/Toasts";
import { AgentBadge, Badge, EmptyState, ErrorState, NoteBadges, Score, Sha, Skeleton, SkeletonRows, Tag } from "../components/ui";
import { countdown, fullDate, pluralize, summaryLine, timeAgo } from "../lib/format";
import { useAsync, useMutation } from "../lib/useAsync";

export default function Knowledge() {
  const { name } = useRepo();
  const [sp, setSp] = useSearchParams();
  const tags = (sp.get("tags") || "").split(",").filter(Boolean);
  const path = sp.get("path") || "";
  const q = sp.get("q") || "";
  const commit = sp.get("commit") || "";
  const pull = sp.get("pull") || "";
  const drafts = sp.get("drafts") === "1";
  const view = sp.get("view") === "stale" ? "stale" : "all";
  const [qDraft, setQDraft] = useState(q);
  const [pathDraft, setPathDraft] = useState(path);
  const [showNew, setShowNew] = useState(false);
  useEffect(() => setQDraft(q), [q]);
  useEffect(() => setPathDraft(path), [path]);

  const [limit, setLimit] = useState(50);
  const filterKey = [tags.join(","), path, q, commit, pull, drafts].join("|");
  useEffect(() => setLimit(50), [name, filterKey]);
  const notes = useAsync(() => api.notes(name, { tags, path: path || undefined, q: q || undefined, commit: commit || undefined, pull: pull ? Number(pull) : undefined, drafts, limit }), [name, filterKey, limit], { enabled: view === "all" });
  const stale = useAsync(() => api.staleNotes(name, { drafts }), [name, drafts], { enabled: view === "stale" });
  const tagCounts = useAsync(() => api.tagCounts(name), [name], { silent: true });

  const update = (patch: Record<string, string>) => {
    const next = new URLSearchParams(sp);
    for (const [k, v] of Object.entries(patch)) v ? next.set(k, v) : next.delete(k);
    setSp(next, { replace: true });
  };
  const toggleTag = (t: string) => update({ tags: (tags.includes(t) ? tags.filter((x) => x !== t) : [...tags, t]).join(",") });
  const base = `/r/${encodeURIComponent(name)}/knowledge`;
  const anyFilter = tags.length || path || q || commit || pull;
  const reloadAll = () => {
    notes.reload();
    stale.reload();
    tagCounts.reload();
  };

  const list: (NoteSummary | StaleNote)[] | undefined = view === "stale" ? stale.data : notes.data;
  const state = view === "stale" ? stale : notes;

  return (
    <div className="two-col">
      <div className="stack">
        <div className="page-head" style={{ marginBottom: 0 }}>
          <div>
            <h1>Knowledge</h1>
            <p className="muted">Decisions, gotchas, specs and how-tos stored next to the code and tagged by path. Notes remember the commit they were validated against and turn <Badge tone="attention">stale</Badge> when covered paths change.</p>
          </div>
          <button className="btn primary" onClick={() => setShowNew((s) => !s)}>{showNew ? "Cancel" : "New note"}</button>
        </div>
        {showNew ? <NewNote onCreated={(n) => { setShowNew(false); reloadAll(); toast(`Created ${n.id}`, "success"); }} /> : null}
        <div className="row wrap between">
          <div className="segmented" role="tablist" aria-label="Notes view">
            <button role="tab" aria-selected={view === "all"} className={view === "all" ? "active" : ""} onClick={() => update({ view: "" })}>All notes</button>
            <button role="tab" aria-selected={view === "stale"} className={view === "stale" ? "active" : ""} onClick={() => update({ view: "stale" })} title="GET /knowledge/stale: notes whose covered paths changed since they were validated">Stale notes{stale.data ? <span className="count" style={{ marginLeft: 6 }}>{stale.data.length}</span> : null}</button>
          </div>
          <label className="row small muted" style={{ gap: 6 }}>
            <input type="checkbox" checked={drafts} onChange={(e) => update({ drafts: e.target.checked ? "1" : "" })} /> include drafts <span className="subtle">(<code>drafts=1</code>)</span>
          </label>
        </div>
        {view === "all" ? (
          <>
            <form
              className="filters"
              style={{ marginBottom: 0 }}
              onSubmit={(e) => {
                e.preventDefault();
                update({ q: qDraft.trim(), path: pathDraft.trim() });
              }}
            >
              <input className="input sm" style={{ width: 240 }} placeholder="Search title, body, tags…" value={qDraft} onChange={(e) => setQDraft(e.target.value)} aria-label="Search" />
              <input className="input sm" style={{ width: 220 }} placeholder="Path, e.g. src/auth/login.rs" value={pathDraft} onChange={(e) => setPathDraft(e.target.value)} aria-label="Path filter" />
              <button className="btn sm" type="submit">Filter</button>
              {commit ? <Badge mono title="Notes listing this commit">commit {commit.slice(0, 8)} <button type="button" className="chip-x" onClick={() => update({ commit: "" })} aria-label="Clear commit filter">×</button></Badge> : null}
              {pull ? <Badge title="Notes listing this pull request">#{pull} <button type="button" className="chip-x" onClick={() => update({ pull: "" })} aria-label="Clear pull filter">×</button></Badge> : null}
              {anyFilter ? <button type="button" className="btn sm ghost" onClick={() => setSp(drafts ? { drafts: "1" } : {}, { replace: true })}>Clear</button> : null}
            </form>
            <div className="row wrap" style={{ gap: 6 }}>
              {tagCounts.data?.map((t) => (
                <button key={t.tag} type="button" className={`chip ns-${t.tag.split(":")[0]} ${tags.includes(t.tag) ? "active" : ""}`} onClick={() => toggleTag(t.tag)} aria-pressed={tags.includes(t.tag)}>
                  {t.tag} <span className="count">{t.count}</span>
                </button>
              ))}
              {tagCounts.loading && !tagCounts.data ? <span className="skeleton skeleton-line" style={{ width: 240 }} /> : null}
            </div>
          </>
        ) : (
          <p className="small muted" style={{ margin: 0 }}>Notes whose covered <code>paths</code> changed between their <code>valid_at</code> commit and HEAD. Re-read them, then <strong>Validate</strong> to stamp HEAD, or edit/supersede them.</p>
        )}
        {state.error ? <ErrorState error={state.error} retry={state.reload} /> : null}
        <div className="box">
          {state.loading && !list ? (
            <SkeletonRows rows={5} />
          ) : list?.length === 0 ? (
            view === "stale" ? <EmptyState icon="✓" title="Nothing is stale">Every note still matches the paths it covers at HEAD.</EmptyState> : <EmptyState icon="✎" title="No notes match">{anyFilter ? "Loosen the filters." : "Agents write notes with agit know add."}</EmptyState>
          ) : (
            list?.map((n) => <NoteRow key={n.id} n={n} base={base} tags={tags} toggleTag={toggleTag} onChanged={reloadAll} showScore={!!q} />)
          )}
        </div>
        {view === "all" && notes.data && notes.data.length >= limit ? (
          <div className="row load-more">
            <span className="small muted">Showing the first {notes.data.length}</span>
            <button className="btn" onClick={() => setLimit((l) => l + 50)} disabled={notes.loading}>{notes.loading ? "Loading…" : "Load more"}</button>
          </div>
        ) : null}
      </div>
      <aside className="sidebar">
        <ContextPanel initialPath={path} />
      </aside>
    </div>
  );
}

function NoteRow({ n, base, tags, toggleTag, onChanged, showScore }: { n: NoteSummary | StaleNote; base: string; tags: string[]; toggleTag: (t: string) => void; onChanged: () => void; showScore: boolean }) {
  const { name } = useRepo();
  const changed = "changed_paths" in n ? n.changed_paths : [];
  return (
    <div className={`note-row ${n.superseded_by ? "superseded" : ""} ${n.draft ? "draft" : ""}`}>
      <div className="grow" style={{ minWidth: 0 }}>
        <div className="row wrap" style={{ gap: 8 }}>
          <Link className="title" to={`${base}/${encodeURIComponent(n.id)}`}>{n.title}</Link>
          <NoteBadges n={n} />
          {showScore ? <Score v={n.score} /> : null}
        </div>
        {n.excerpt ? <div className="excerpt">{n.excerpt.replace(/[`*_#]/g, "")}</div> : null}
        {changed.length ? <div className="small mt8" style={{ color: "var(--attention)" }}>changed since validation: {changed.map((p) => <code key={p} style={{ marginRight: 6 }}>{p}</code>)}</div> : null}
        <div className="tags">
          {n.tags.map((t) => <Tag key={t} tag={t} onClick={() => toggleTag(t)} active={tags.includes(t)} />)}
          {n.paths.map((p) => <code key={p} className="small muted">{p}</code>)}
        </div>
      </div>
      <div className="stack tight small muted" style={{ alignItems: "flex-end", flexShrink: 0 }}>
        <AgentBadge agent={{ id: n.author }} />
        <span title={fullDate(n.updated_ms)}>{timeAgo(n.updated_ms)}</span>
        <Confidence v={n.confidence} />
        {n.valid_at ? <span className="row" style={{ gap: 4 }}>valid at <Sha id={n.valid_at} repo={name} /></span> : null}
        <NoteActions n={n} onChanged={onChanged} />
      </div>
    </div>
  );
}

/** Validate (stale) / Accept + Discard (draft) buttons shared by the list and the note page. */
export function NoteActions({ n, onChanged, lg }: { n: Pick<NoteSummary, "id" | "draft" | "stale" | "title">; onChanged: (next?: Note | null) => void; lg?: boolean }) {
  const { name } = useRepo();
  const validate = useMutation(() => api.validateNote(name, n.id));
  const accept = useMutation(() => api.acceptNote(name, n.id));
  const discard = useMutation(() => api.deleteNote(name, n.id));
  const busy = validate.busy || accept.busy || discard.busy;
  const cls = `btn ${lg ? "" : "sm"}`;
  const doValidate = async () => {
    const r = await validate.run();
    if (r) {
      toast(`Validated at ${r.valid_at?.slice(0, 8) ?? "HEAD"}`, "success");
      onChanged(r);
    }
  };
  const doAccept = async () => {
    const r = await accept.run();
    if (r) {
      toast("Draft accepted", "success");
      onChanged(r);
    }
  };
  const doDiscard = async () => {
    if (!confirm(`Discard draft "${n.title}"?`)) return;
    const r = await discard.run();
    if (r !== undefined) {
      toast("Draft discarded", "info");
      onChanged(null);
    }
  };
  if (!n.draft && !n.stale) return null;
  return (
    <div className="row" style={{ gap: 6 }}>
      {n.draft ? <button className={`${cls} primary`} onClick={doAccept} disabled={busy} title="POST /knowledge/notes/{id}/accept">Accept</button> : null}
      {n.draft ? <button className={`${cls} danger`} onClick={doDiscard} disabled={busy} title="DELETE /knowledge/notes/{id}">Discard</button> : null}
      {n.stale && !n.draft ? <button className={cls} onClick={doValidate} disabled={busy} title="POST /knowledge/notes/{id}/validate: confirm the note still holds at HEAD">{validate.busy ? "Validating…" : "Validate at HEAD"}</button> : null}
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
  const [files, setFiles] = useState(false);
  const [budget, setBudget] = useState(6000);
  const [budgetDraft, setBudgetDraft] = useState("6000");
  useEffect(() => {
    setDraft(initialPath);
    if (initialPath) setPath(initialPath);
  }, [initialPath]);
  const ctx = useAsync(() => api.context(name, path, { limit: 10, files, budget: files ? budget : undefined }), [name, path, files, budget], { enabled: !!path });
  const base = `/r/${encodeURIComponent(name)}`;
  return (
    <section className="context-panel">
      <h3>Context for a path</h3>
      <p className="small muted">What an agent should read before touching a path (<code>GET /context?path=</code>).</p>
      <form className="row" onSubmit={(e) => { e.preventDefault(); setPath(draft.trim()); }}>
        <input className="input sm" placeholder="src/auth/login.rs" value={draft} onChange={(e) => setDraft(e.target.value)} aria-label="Context path" />
        <button className="btn sm" type="submit">Go</button>
      </form>
      <div className="row wrap small muted mt8" style={{ gap: 8 }}>
        <label className="row" style={{ gap: 4 }}><input type="checkbox" checked={files} onChange={(e) => setFiles(e.target.checked)} /> include files <span className="subtle">(<code>include=files</code>)</span></label>
        {files ? (
          <form className="row" style={{ gap: 4 }} onSubmit={(e) => { e.preventDefault(); const b = Math.max(100, Math.floor(Number(budgetDraft) || 6000)); setBudget(b); setBudgetDraft(String(b)); }}>
            <label className="row" style={{ gap: 4 }}>budget <input className="input sm" type="number" min={100} style={{ width: 90 }} value={budgetDraft} onChange={(e) => setBudgetDraft(e.target.value)} aria-label="Token budget" /></label>
            <button className="btn sm" type="submit">Apply</button>
          </form>
        ) : null}
      </div>
      {!path ? null : ctx.error ? <div className="mt8"><ErrorState error={ctx.error} retry={ctx.reload} /></div> : !ctx.data ? <Skeleton lines={4} /> : <ContextBody c={ctx.data} base={base} repo={name} filesWanted={files} />}
    </section>
  );
}

function ContextBody({ c, base, repo, filesWanted }: { c: Context; base: string; repo: string; filesWanted: boolean }) {
  const now = Date.now();
  const fresh = c.notes.filter((n) => !n.stale), staleNotes = c.notes.filter((n) => n.stale);
  const agents = c.agents_active ?? [];
  return (
    <div>
      <div className="section-title">Active agents ({agents.length})</div>
      {agents.length === 0 ? <ul><li className="muted">nobody touched this path in the last 24h</li></ul> : (
        <div className="row wrap" style={{ gap: 4 }}>{agents.map((a) => <AgentBadge key={a} agent={{ id: a }} repo={repo} />)}</div>
      )}
      <div className="section-title">Notes ({c.notes.length})</div>
      <ul>
        {c.notes.length === 0 ? <li className="muted">none</li> : null}
        {fresh.map((n) => <li key={n.id}><Link to={`${base}/knowledge/${encodeURIComponent(n.id)}`}>{n.title}</Link></li>)}
        {staleNotes.map((n) => <li key={n.id} className="row" style={{ gap: 6 }}><Link to={`${base}/knowledge/${encodeURIComponent(n.id)}`}>{n.title}</Link> <Badge tone="attention">stale</Badge></li>)}
      </ul>
      {c.suggested_tags?.length ? (
        <>
          <div className="section-title">Suggested tags</div>
          <div className="row wrap" style={{ gap: 4 }}>{c.suggested_tags.map((t) => <Tag key={t} tag={t} repo={repo} />)}</div>
        </>
      ) : null}
      <div className="section-title">Recent commits ({c.recent_commits.length})</div>
      <ul>{c.recent_commits.length === 0 ? <li className="muted">none</li> : c.recent_commits.slice(0, 6).map((k) => <li key={k.id} className="row" style={{ gap: 6 }}><Sha id={k.id} repo={repo} /><span className="truncate">{summaryLine(k.message)}</span></li>)}</ul>
      <div className="section-title">Active leases ({c.leases.length})</div>
      <ul>{c.leases.length === 0 ? <li className="muted">none</li> : c.leases.map((l) => <li key={l.id}><strong>{l.holder}</strong> <span className="muted">{l.mode}</span> {l.expires_ms ? <span className="muted">· {countdown(l.expires_ms, now)}</span> : null}</li>)}</ul>
      <div className="section-title">Open pull requests ({c.open_pulls.length})</div>
      <ul>{c.open_pulls.length === 0 ? <li className="muted">none</li> : c.open_pulls.map((p) => <li key={p.number}><Link to={`${base}/pulls/${p.number}`}>#{p.number} {p.title}</Link></li>)}</ul>
      {filesWanted ? (
        <>
          <div className="section-title">Files ({c.files?.length ?? 0})</div>
          {!c.files?.length ? <ul><li className="muted">nothing fits the budget (or the path is binary)</li></ul> : (
            <div className="stack tight">
              {c.files.map((f) => <ContextFileView key={f.path} f={f} base={base} />)}
            </div>
          )}
        </>
      ) : null}
    </div>
  );
}

function ContextFileView({ f, base }: { f: Context["files"] extends (infer U)[] | undefined ? U : never; base: string }) {
  const [open, setOpen] = useState(false);
  const lines = f.text.split("\n").length;
  return (
    <div className="context-file">
      <div className="row" style={{ gap: 6 }}>
        <button type="button" className="btn sm ghost" onClick={() => setOpen((o) => !o)} aria-expanded={open} aria-label="Toggle file">{open ? "▾" : "▸"}</button>
        <Link className="mono small truncate grow" to={`${base}/blob/HEAD/${f.path}`} title={f.path}>{f.path}</Link>
        <span className="subtle small">{pluralize(lines, "line")}</span>
        {f.truncated ? <Badge tone="attention" title="Cut to fit the budget">truncated</Badge> : null}
      </div>
      {open ? <pre className="context-pre">{f.text}{f.truncated ? "\n…" : ""}</pre> : null}
    </div>
  );
}

export function NewNote({ onCreated }: { onCreated: (n: Note) => void }) {
  const { name } = useRepo();
  const [f, setF] = useState({ title: "", body: "", tags: "", paths: "", commits: "", pulls: "", confidence: 0.8, valid_at: "", draft: false });
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
      valid_at: f.valid_at.trim() || undefined,
      draft: f.draft || undefined,
    });
  });
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const n = await create.run();
    if (n) onCreated(n);
  };
  const set = (k: keyof typeof f) => (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => setF({ ...f, [k]: k === "confidence" ? Number(e.target.value) : k === "draft" ? (e.target as HTMLInputElement).checked : e.target.value });
  return (
    <form className="box" onSubmit={submit}>
      <div className="box-head"><h3>New note</h3></div>
      <div className="box-body form" style={{ maxWidth: "none" }}>
        <label className="field">Title<input className="input" value={f.title} onChange={set("title")} required autoFocus /></label>
        <label className="field">Body <span className="hint">Markdown</span><textarea className="textarea" rows={6} value={f.body} onChange={set("body")} /></label>
        <div className="row">
          <label className="field">Tags <span className="hint">namespace:value, comma separated (area, kind, lang, service…)</span><input className="input" value={f.tags} onChange={set("tags")} placeholder="area:auth, kind:gotcha" required /></label>
          <label className="field">Paths <span className="hint">globs the note applies to; staleness is tracked per covered path</span><input className="input" value={f.paths} onChange={set("paths")} placeholder="src/auth/**" /></label>
        </div>
        <div className="row">
          <label className="field">Commits <span className="hint">ids, comma separated</span><input className="input mono" value={f.commits} onChange={set("commits")} /></label>
          <label className="field">Pull requests <span className="hint">numbers</span><input className="input" value={f.pulls} onChange={set("pulls")} placeholder="2, 3" /></label>
        </div>
        <div className="row">
          <label className="field">Valid at <span className="hint">commit id the note was checked against; default HEAD of the default branch</span><input className="input mono" value={f.valid_at} onChange={set("valid_at")} placeholder="HEAD" spellCheck={false} /></label>
          <label className="field">Confidence: {f.confidence.toFixed(2)}<input type="range" min={0} max={1} step={0.05} value={f.confidence} onChange={set("confidence")} /></label>
        </div>
        <label className="row small" style={{ gap: 6 }}><input type="checkbox" checked={f.draft} onChange={set("draft")} /> create as draft (hidden until accepted)</label>
        <div className="actions"><button className="btn primary" disabled={create.busy || !f.title.trim() || !f.tags.trim()}>{create.busy ? "Saving…" : "Create note"}</button></div>
      </div>
    </form>
  );
}

