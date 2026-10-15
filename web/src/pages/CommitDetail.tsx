import { Link, useParams } from "react-router-dom";
import { api } from "../api/client";
import { DiffView } from "../components/DiffView";
import { useRepo } from "../components/RepoShell";
import { AgentBadge, Badge, ChangeKindBadge, ErrorState, Sha, Skeleton, TaskChip } from "../components/ui";
import { bodyOf, fullDate, pluralize, summaryLine, timeAgo } from "../lib/format";
import { useAsync } from "../lib/useAsync";
import { parseUnifiedDiff } from "../lib/diff";
import { useMemo } from "react";

export default function CommitDetail() {
  const { name } = useRepo();
  const { id = "" } = useParams();
  const detail = useAsync(() => api.commit(name, id), [name, id]);
  const base = `/r/${encodeURIComponent(name)}`;
  const stats = useMemo(() => {
    const files = parseUnifiedDiff(detail.data?.patch ?? "");
    return { add: files.reduce((s, f) => s + f.additions, 0), del: files.reduce((s, f) => s + f.deletions, 0) };
  }, [detail.data?.patch]);

  if (detail.error) return <ErrorState error={detail.error} retry={detail.reload} />;
  if (!detail.data) return <div className="box"><Skeleton lines={6} /></div>;
  const { commit: c, changes, patch } = detail.data;
  const meta = Object.entries(c.meta ?? {});

  return (
    <div className="stack">
      <div className="box">
        <div className="commit-head">
          <div className="row wrap between" style={{ alignItems: "flex-start" }}>
            <div className="grow">
              <h1>{summaryLine(c.message)}</h1>
              {c.intent ? <div className="intent">{c.intent}</div> : null}
              {bodyOf(c.message) ? <div className="message-body">{bodyOf(c.message)}</div> : null}
            </div>
            <div className="row">
              {c.checkpoint ? <Badge>checkpoint</Badge> : null}
              <Link className="btn sm" to={`${base}/tree/${detail.data.id}`}>Browse files</Link>
            </div>
          </div>
        </div>
        <div className="box-row static" style={{ background: "var(--bg-subtle)", borderTop: "1px solid var(--border)", flexWrap: "wrap", rowGap: 4 }}>
          {c.agent ? <AgentBadge agent={c.agent} repo={name} /> : null}
          <span><strong>{c.author.name}</strong> <span className="muted">&lt;{c.author.email}&gt;</span></span>
          <span className="muted" title={fullDate(c.time_ms)}>committed {timeAgo(c.time_ms)}</span>
          <div className="grow" />
          <span className="muted small">commit</span> <Sha id={detail.data.id} n={12} />
        </div>
        <div className="box-body">
          <dl className="meta-grid">
            <dt>Task</dt><dd>{c.task ? <TaskChip task={c.task} repo={name} /> : <span className="subtle">—</span>}</dd>
            <dt>Agent</dt><dd>{c.agent ? <>{c.agent.id}{c.agent.model ? <Badge mono>{c.agent.model}</Badge> : null}{c.agent.on_behalf_of ? <span className="muted small">on behalf of {c.agent.on_behalf_of}</span> : null}</> : <span className="subtle">human</span>}</dd>
            <dt>Run id</dt><dd>{c.agent?.run_id ? <code>{c.agent.run_id}</code> : <span className="subtle">—</span>}</dd>
            <dt>{pluralize(c.parents.length, "Parent")}</dt><dd>{c.parents.length ? c.parents.map((p) => <Sha key={p} id={p} repo={name} />) : <span className="subtle">root commit</span>}</dd>
            <dt>Tree</dt><dd><Sha id={c.tree} /></dd>
            <dt>Time</dt><dd>{fullDate(c.time_ms)} <span className="muted small">({c.time_ms} ms)</span></dd>
            {meta.length ? (
              <>
                <dt>Meta</dt>
                <dd><code style={{ whiteSpace: "pre-wrap" }}>{meta.map(([k, v]) => `${k} = ${JSON.stringify(v)}`).join("\n")}</code></dd>
              </>
            ) : null}
          </dl>
        </div>
      </div>

      <div className="box">
        <div className="box-head">
          <h3>{pluralize(changes.length, "file")} changed</h3>
          <span className="muted">·</span>
          <span className="stat-add">+{stats.add}</span>
          <span className="stat-del">−{stats.del}</span>
        </div>
        <div className="file-list">
          {changes.map((ch) => (
            <div key={ch.path} className="box-row">
              <ChangeKindBadge kind={ch.kind} />
              <a href={`#diff-${encodeURIComponent(ch.path)}`} className="grow truncate">{ch.path}</a>
              {ch.kind !== "deleted" ? <Link className="btn sm ghost" to={`${base}/blob/${detail.data!.id}/${ch.path}`}>view</Link> : null}
              <span className="muted small mono">{ch.old?.mode && ch.new?.mode && ch.old.mode !== ch.new.mode ? `${ch.old.mode} → ${ch.new.mode}` : ch.new?.mode ?? ch.old?.mode}</span>
            </div>
          ))}
        </div>
      </div>

      <DiffView patch={patch} changes={changes} />
    </div>
  );
}
