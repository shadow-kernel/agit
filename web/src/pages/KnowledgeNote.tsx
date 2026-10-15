import { Link, useNavigate, useParams } from "react-router-dom";
import { api } from "../api/client";
import { Markdown } from "../components/Markdown";
import { useRepo } from "../components/RepoShell";
import { toast } from "../components/Toasts";
import { AgentBadge, Badge, Banner, ErrorState, NoteBadges, Sha, Skeleton, Tag } from "../components/ui";
import { fullDate, timeAgo } from "../lib/format";
import { useAsync, useMutation } from "../lib/useAsync";
import { Confidence, ContextPanel, NoteActions } from "./Knowledge";

export default function KnowledgeNote() {
  const { name, repo } = useRepo();
  const { id = "" } = useParams();
  const note = useAsync(() => api.note(name, id), [name, id]);
  const navigate = useNavigate();
  const del = useMutation(() => api.deleteNote(name, id));
  const base = `/r/${encodeURIComponent(name)}`;

  if (note.error) return <ErrorState error={note.error} retry={note.reload} />;
  if (!note.data) return <div className="box"><Skeleton lines={8} /></div>;
  const n = note.data;
  const draft = !!n.draft, stale = !!n.stale;

  const doDelete = async () => {
    if (!confirm(`Delete note ${n.id}? Prefer superseding it with a newer note.`)) return;
    const r = await del.run();
    if (r !== undefined) {
      toast("Note deleted", "info");
      navigate(`${base}/knowledge`);
    }
  };
  const onChanged = (next?: typeof n | null) => {
    if (next === null) navigate(`${base}/knowledge`);
    else if (next) note.setData(next);
    else note.reload();
  };

  return (
    <div className="two-col">
      <div className="stack">
        <div>
          <Link to={`${base}/knowledge`} className="small">← Knowledge</Link>
          <div className="row wrap mt8" style={{ gap: 10 }}>
            <h1>{n.title}</h1>
            <NoteBadges n={{ draft, stale, superseded_by: n.superseded_by }} />
          </div>
          <div className="row wrap mt8 small muted" style={{ gap: 10 }}>
            <AgentBadge agent={{ id: n.author }} />
            <span title={fullDate(n.created_ms)}>created {timeAgo(n.created_ms)}</span>
            {n.updated_ms !== n.created_ms ? <span title={fullDate(n.updated_ms)}>updated {timeAgo(n.updated_ms)}</span> : null}
            <Confidence v={n.confidence} />
            <code className="subtle">{n.id}</code>
          </div>
          <div className="row wrap mt8" style={{ gap: 4 }}>{n.tags.map((t) => <Tag key={t} tag={t} repo={name} />)}</div>
        </div>
        {draft ? (
          <Banner tone="done" icon="✎" title="Draft note" actions={<NoteActions n={{ id: n.id, draft, stale, title: n.title }} onChanged={onChanged} />}>
            Machine-generated{n.pulls.length ? <> from pull request <Link to={`${base}/pulls/${n.pulls[0]}`}>#{n.pulls[0]}</Link></> : null}; hidden from listings, context and tag counts until accepted. Accept it as is, or edit it via <code>PATCH</code> first and then accept.
          </Banner>
        ) : null}
        {stale ? (
          <Banner tone="attention" icon="◷" title="This note may be out of date" actions={<NoteActions n={{ id: n.id, draft, stale, title: n.title }} onChanged={onChanged} />}>
            A path it covers changed since it was validated{n.valid_at ? <> at <Sha id={n.valid_at} repo={name} /></> : null}. Re-read the note against HEAD; "Validate" records that it still holds.
          </Banner>
        ) : null}
        {n.superseded_by ? (
          <Banner tone="attention" icon="↷" title="This note has been superseded" actions={<Link className="btn sm" to={`${base}/knowledge/${encodeURIComponent(n.superseded_by)}`}>Open newer note</Link>}>
            Newer note: <code>{n.superseded_by}</code>
          </Banner>
        ) : null}
        <div className="box">
          <Markdown source={n.body || "_No body._"} />
        </div>
      </div>
      <aside className="sidebar">
        <section>
          <h3>Validity</h3>
          <dl>
            <dt>Valid at</dt>
            <dd>{n.valid_at ? <Sha id={n.valid_at} repo={name} n={10} /> : <span className="subtle">not tracked</span>}</dd>
            <dt>State</dt>
            <dd>{stale ? <Badge tone="attention">stale</Badge> : n.valid_at && n.paths.length ? <Badge tone="success">fresh</Badge> : <span className="subtle">never stale (no paths or no valid_at)</span>}</dd>
            {n.valid_at && repo?.head && n.valid_at !== repo.head ? <><dt>Since then</dt><dd><Link to={`${base}/compare/${n.valid_at}...${repo.head}`}>compare with HEAD</Link></dd></> : null}
          </dl>
        </section>
        <section>
          <h3>Linked paths</h3>
          {n.paths.length === 0 ? <span className="muted small">none</span> : (
            <div className="stack tight">
              {n.paths.map((p) => (
                <div key={p} className="row small" style={{ gap: 6 }}>
                  <code className="truncate">{p}</code>
                  {/[*?[]/.test(p) ? <Link className="subtle" to={`${base}/commits?path=${encodeURIComponent(p.replace(/\/?\*\*.*$/, ""))}`}>history</Link> : <Link className="subtle" to={`${base}/blob/${encodeURIComponent("HEAD")}/${p}`}>open</Link>}
                </div>
              ))}
            </div>
          )}
        </section>
        <section>
          <h3>Linked commits</h3>
          {n.commits.length === 0 ? <span className="muted small">none</span> : <div className="row wrap" style={{ gap: 4 }}>{n.commits.map((c) => <Sha key={c} id={c} repo={name} />)}</div>}
        </section>
        <section>
          <h3>Linked pull requests</h3>
          {n.pulls.length === 0 ? <span className="muted small">none</span> : <div className="row wrap" style={{ gap: 4 }}>{n.pulls.map((p) => <Link key={p} to={`${base}/pulls/${p}`}><Badge tone="accent">#{p}</Badge></Link>)}</div>}
        </section>
        <ContextPanel initialPath={n.paths[0] && !/[*?[]/.test(n.paths[0]) ? n.paths[0] : ""} />
        <section>
          <button className="btn sm danger" onClick={doDelete} disabled={del.busy}>{draft ? "Discard draft" : "Delete note"}</button>
        </section>
      </aside>
    </div>
  );
}
