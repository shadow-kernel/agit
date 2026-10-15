import { useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { api } from "../api/client";
import { BranchSelector } from "../components/BranchSelector";
import { CommitList } from "../components/CommitList";
import { useRepo } from "../components/RepoShell";
import { EmptyState, ErrorState, SkeletonRows } from "../components/ui";
import { useAsync } from "../lib/useAsync";

export default function Commits() {
  const { name, defaultBranch } = useRepo();
  const [sp, setSp] = useSearchParams();
  const rev = sp.get("rev") || defaultBranch;
  const agent = sp.get("agent") || "";
  const task = sp.get("task") || "";
  const path = sp.get("path") || "";
  const [limit, setLimit] = useState(50);
  const [showCheckpoints, setShowCheckpoints] = useState(false);

  const commits = useAsync(() => api.commits(name, { rev, agent: agent || undefined, task: task || undefined, path: path || undefined, limit }), [name, rev, agent, task, path, limit]);
  const agents = useAsync(() => api.agents(name), [name], { silent: true });

  const set = (k: string, v: string) => {
    const next = new URLSearchParams(sp);
    if (v) next.set(k, v);
    else next.delete(k);
    setSp(next, { replace: true });
  };

  const visible = useMemo(() => (commits.data ?? []).filter((c) => showCheckpoints || !c.checkpoint), [commits.data, showCheckpoints]);
  const hiddenCheckpoints = (commits.data?.length ?? 0) - (commits.data ?? []).filter((c) => !c.checkpoint).length;

  return (
    <div>
      <div className="filters">
        <BranchSelector repo={name} rev={rev} mode="commits" />
        <select className="select sm" value={agent} onChange={(e) => set("agent", e.target.value)} aria-label="Filter by agent">
          <option value="">All agents</option>
          {agents.data?.map((a) => <option key={a.id} value={a.id}>{a.id}</option>)}
          {agent && !agents.data?.some((a) => a.id === agent) ? <option value={agent}>{agent}</option> : null}
        </select>
        <input className="input sm" style={{ width: 140 }} placeholder="Task (e.g. SHOP-60)" value={task} onChange={(e) => set("task", e.target.value)} aria-label="Filter by task" />
        <input className="input sm" style={{ width: 220 }} placeholder="Path filter" value={path} onChange={(e) => set("path", e.target.value)} aria-label="Filter by path" />
        <label className="row small muted" style={{ gap: 4 }}>
          <input type="checkbox" checked={showCheckpoints} onChange={(e) => setShowCheckpoints(e.target.checked)} /> checkpoints{hiddenCheckpoints && !showCheckpoints ? ` (${hiddenCheckpoints})` : ""}
        </label>
        {agent || task || path ? <button className="btn sm ghost" onClick={() => setSp(rev !== defaultBranch ? { rev } : {}, { replace: true })}>Clear filters</button> : null}
        <div className="grow" />
        {commits.loading && commits.data ? <span className="spinner" /> : null}
      </div>
      {commits.error ? <ErrorState error={commits.error} retry={commits.reload} /> : null}
      {commits.loading && !commits.data ? (
        <div className="box"><SkeletonRows rows={8} /></div>
      ) : visible.length === 0 ? (
        <div className="box"><EmptyState icon="◷" title="No commits match">{agent || task || path ? "Try widening the filters." : "This branch has no history yet."}</EmptyState></div>
      ) : (
        <>
          <CommitList repo={name} commits={visible} />
          {(commits.data?.length ?? 0) >= limit ? (
            <div className="row" style={{ justifyContent: "center" }}>
              <button className="btn" onClick={() => setLimit((l) => l + 50)}>Load more</button>
            </div>
          ) : null}
        </>
      )}
    </div>
  );
}
