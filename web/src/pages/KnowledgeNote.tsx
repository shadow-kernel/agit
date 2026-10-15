import { Link, useNavigate, useParams } from "react-router-dom";
import { api } from "../api/client";
import { Markdown } from "../components/Markdown";
import { useRepo } from "../components/RepoShell";
import { toast } from "../components/Toasts";
import { AgentBadge, Badge, Banner, ErrorState, Sha, Skeleton, Tag } from "../components/ui";
import { fullDate, timeAgo } from "../lib/format";
import { useAsync, useMutation } from "../lib/useAsync";
import { Confidence, ContextPanel } from "./Knowledge";

export default function KnowledgeNote() {
  const { name } = useRepo();
  const { id = "" } = useParams();
  const note = useAsync(() => api.note(name, id), [name, id]);
  const navigate = useNavigate();
  const del = useMutation(() => api.deleteNote(name, id));
  const base = `/r/${encodeURIComponent(name)}`;

  if (note.error) return <ErrorState error={note.error} retry={note.reload} />;
  if (!note.data) return <div className="box"><Skeleton lines={8} /></div>;
  const n = note.data;

  const doDelete = async () => {
    if (!confirm(`Delete note ${n.id}? Prefer superseding it with a newer note.`)) return;
    const r = await del.run();
    if (r !== undefined) {
      toast("Note deleted", "info");
      navigate(`${base}/knowledge`);
    }
  };

  return (
    <div className="two-col">
      <div className="stack">
        <div>
          <Link to={`${base}/knowledge`} className="small">← Knowledge</Link>
          <h1 className="mt8">{n.title}</h1>
          <div className="row wrap mt8 small muted" style={{ gap: 10 }}>
            <AgentBadge agent={{ id: n.author }} />
            <span title={fullDate(n.created_ms)}>created {timeAgo(n.created_ms)}</span>
            {n.updated_ms !== n.created_ms ? <span title={fullDate(n.updated_ms)}>updated {timeAgo(n.updated_ms)}</span> : null}
            <Confidence v={n.confidence} />
            <code className="subtle">{n.id}</code>
          </div>
          <div className="row wrap mt8" style={{ gap: 4 }}>{n.tags.map((t) => <Tag key={t} tag={t} repo={name} />)}</div>
        </div>
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
          <button className="btn sm danger" onClick={doDelete} disabled={del.busy}>Delete note</button>
        </section>
      </aside>
    </div>
  );
}
