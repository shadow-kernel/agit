import { Link } from "react-router-dom";
import type { CommitListItem } from "../api/types";
import { dayKey, dayLabel, fullDate, summaryLine, timeAgo } from "../lib/format";
import { AgentBadge, Badge, Sha, TaskChip } from "./ui";

export function CommitList({ repo, commits, groupByDay = true }: { repo: string; commits: CommitListItem[]; groupByDay?: boolean }) {
  if (!groupByDay) {
    return <div className="box">{commits.map((c) => <CommitRow key={c.id} repo={repo} c={c} />)}</div>;
  }
  const groups: { key: string; label: string; items: CommitListItem[] }[] = [];
  for (const c of commits) {
    const k = dayKey(c.time_ms);
    const g = groups[groups.length - 1];
    if (g && g.key === k) g.items.push(c);
    else groups.push({ key: k, label: dayLabel(c.time_ms), items: [c] });
  }
  return (
    <div>
      {groups.map((g) => (
        <div key={g.key} className="day-group">
          <div className="day-head">
            <span aria-hidden>◷</span> Commits on {g.label}
          </div>
          <div className="box">{g.items.map((c) => <CommitRow key={c.id} repo={repo} c={c} />)}</div>
        </div>
      ))}
    </div>
  );
}

export function CommitRow({ repo, c }: { repo: string; c: CommitListItem }) {
  const base = `/r/${encodeURIComponent(repo)}`;
  return (
    <div className={`commit-row ${c.checkpoint ? "checkpoint" : ""}`}>
      <div className="main">
        <div className="row wrap" style={{ gap: 8 }}>
          <Link className="title" to={`${base}/commit/${c.id}`}>{summaryLine(c.message)}</Link>
          {c.checkpoint ? <Badge>checkpoint</Badge> : null}
          {c.parents.length > 1 ? <Badge tone="done">merge</Badge> : null}
        </div>
        {c.intent ? <div className="intent small">{c.intent}</div> : null}
        <div className="meta">
          {c.agent ? <AgentBadge agent={c.agent} repo={repo} /> : <span><strong>{c.author.name}</strong></span>}
          <span title={fullDate(c.time_ms)}>committed {timeAgo(c.time_ms)}</span>
          <TaskChip task={c.task} repo={repo} />
          {c.agent?.on_behalf_of ? <span className="subtle">on behalf of {c.agent.on_behalf_of}</span> : null}
        </div>
      </div>
      <div className="side">
        <Sha id={c.id} repo={repo} />
        <Link className="btn sm hidden-sm" to={`${base}/tree/${c.id}`} title="Browse files at this commit">‹›</Link>
      </div>
    </div>
  );
}
