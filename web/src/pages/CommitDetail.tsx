import { Link, useParams } from "react-router-dom";
import { api } from "../api/client";
import { DiffView } from "../components/DiffView";
import { useRepo } from "../components/RepoShell";
import { AgentBadge, Badge, ChangeKindBadge, ChangePath, ErrorState, NoteBadges, Sha, SignatureBadge, Skeleton, Tag, TaskChip } from "../components/ui";
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
  const { commit: c, changes, patch, signature: sig } = detail.data;
  const notes = detail.data.notes ?? [];
  const meta = Object.entries(c.meta ?? {});
  const renames = changes.filter((ch) => ch.kind === "renamed").length;

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
              <SignatureBadge sig={sig} lg />
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
            <dt>Signature</dt>
            <dd>
              <SignatureBadge sig={sig} />
              {sig?.status === "valid" ? (
                <span className="small muted">
                  signer <code>{sig.signer}</code> · key <code title={sig.public_key}>{sig.key_id}</code> ·{" "}
                  {sig.trusted ? "registered for the signer" : <>not registered for <code>{sig.signer}</code> (<Link to={`${base}/settings/keys`}>Keys</Link>)</>}
                </span>
              ) : sig?.status === "invalid" ? (
                <span className="small" style={{ color: "var(--danger)" }}>{sig.reason}</span>
              ) : (
                <span className="small muted">no Ed25519 signature (<code>agit commit --sign</code>)</span>
              )}
            </dd>
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

      {notes.length ? (
        <div className="box">
          <div className="box-head">
            <h3>Knowledge notes</h3>
            <span className="muted">{pluralize(notes.length, "note")} linking this commit or covering a changed path</span>
            <div className="grow" />
            <Link className="small" to={`${base}/knowledge?commit=${detail.data.id}`}>All notes for this commit</Link>
          </div>
          {notes.map((n) => (
            <div key={n.id} className={`note-row ${n.superseded_by ? "superseded" : ""}`}>
              <div className="grow" style={{ minWidth: 0 }}>
                <div className="row wrap" style={{ gap: 8 }}>
                  <Link className="title" to={`${base}/knowledge/${encodeURIComponent(n.id)}`}>{n.title}</Link>
                  <NoteBadges n={n} />
                </div>
                {n.excerpt ? <div className="excerpt">{n.excerpt.replace(/[`*_#]/g, "")}</div> : null}
                <div className="tags">{n.tags.map((t) => <Tag key={t} tag={t} repo={name} />)}{n.paths.map((p) => <code key={p} className="small muted">{p}</code>)}</div>
              </div>
              <div className="stack tight small muted" style={{ alignItems: "flex-end", flexShrink: 0 }}>
                <AgentBadge agent={{ id: n.author }} />
                <span title={fullDate(n.updated_ms)}>{timeAgo(n.updated_ms)}</span>
              </div>
            </div>
          ))}
        </div>
      ) : null}

      <div className="box">
        <div className="box-head">
          <h3>{pluralize(changes.length, "file")} changed</h3>
          <span className="muted">·</span>
          <span className="stat-add">+{stats.add}</span>
          <span className="stat-del">−{stats.del}</span>
          {renames ? <span className="muted">· {pluralize(renames, "rename")}</span> : null}
        </div>
        <div className="file-list">
          {changes.map((ch) => (
            <div key={ch.path} className="box-row">
              <ChangeKindBadge kind={ch.kind} />
              <a href={`#diff-${encodeURIComponent(ch.path)}`} className="grow truncate"><ChangePath change={ch} /></a>
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
