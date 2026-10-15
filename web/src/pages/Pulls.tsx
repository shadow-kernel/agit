import { Link, useSearchParams } from "react-router-dom";
import { api } from "../api/client";
import { useRepo } from "../components/RepoShell";
import { AgentBadge, Badge, EmptyState, ErrorState, PullStateIcon, SkeletonRows, TaskChip } from "../components/ui";
import { fullDate, pluralize, refShort, timeAgo } from "../lib/format";
import { useAsync } from "../lib/useAsync";
import type { PullState } from "../api/types";

export default function Pulls() {
  const { name } = useRepo();
  const [sp, setSp] = useSearchParams();
  const state = (sp.get("state") || "open") as PullState | "all";
  const pulls = useAsync(() => api.pulls(name, state), [name, state]);
  const base = `/r/${encodeURIComponent(name)}`;
  const states: (PullState | "all")[] = ["open", "merged", "closed", "all"];

  return (
    <div>
      <div className="page-head">
        <div className="segmented" role="tablist">
          {states.map((s) => (
            <button key={s} role="tab" aria-selected={state === s} className={state === s ? "active" : ""} onClick={() => setSp(s === "open" ? {} : { state: s }, { replace: true })}>
              {s[0].toUpperCase() + s.slice(1)}
            </button>
          ))}
        </div>
        <Link to={`${base}/pulls/new`} className="btn primary">New pull request</Link>
      </div>
      {pulls.error ? <ErrorState error={pulls.error} retry={pulls.reload} /> : null}
      <div className="box">
        {pulls.loading && !pulls.data ? (
          <SkeletonRows rows={5} />
        ) : pulls.data?.length === 0 ? (
          <EmptyState icon="⑂" title={`No ${state === "all" ? "" : state} pull requests`} action={state === "open" ? <Link to={`${base}/pulls/new`} className="btn">Open one</Link> : null}>
            Agents open pull requests with <code>POST /repos/{name}/pulls</code>.
          </EmptyState>
        ) : (
          pulls.data?.map((p) => {
            const approvals = p.reviews.filter((r) => r.decision === "approve").length;
            const changesReq = p.reviews.some((r) => r.decision === "request_changes");
            const blockers = p.comments.filter((c) => c.kind === "blocker" && !c.resolved).length;
            const failing = p.checks.filter((c) => c.status === "failure").length;
            const pending = p.checks.filter((c) => c.status === "pending").length;
            return (
              <div key={p.number} className="pr-row">
                <PullStateIcon state={p.state} />
                <div className="grow" style={{ minWidth: 0 }}>
                  <div className="row wrap" style={{ gap: 8 }}>
                    <Link className="title" to={`${base}/pulls/${p.number}`}>{p.title}</Link>
                    <TaskChip task={p.task} repo={name} />
                  </div>
                  {p.intent ? <div className="intent small">{p.intent}</div> : null}
                  <div className="meta">
                    <span>#{p.number}</span>
                    <span>opened {timeAgo(p.created_ms)} by</span>
                    <AgentBadge agent={p.author} repo={name} showModel={false} />
                    <span className="hidden-sm">
                      <span className="ref-pill">{refShort(p.source)}</span> → <span className="ref-pill">{refShort(p.target)}</span>
                    </span>
                    <span title={fullDate(p.updated_ms)}>updated {timeAgo(p.updated_ms)}</span>
                  </div>
                </div>
                <div className="side">
                  {failing ? <Badge tone="danger" title={`${failing} failing checks`}>✕ {failing}</Badge> : pending ? <Badge tone="attention" title="checks pending">● {pending}</Badge> : p.checks.length ? <Badge tone="success" title="all checks passed">✓</Badge> : null}
                  {blockers ? <Badge tone="danger">{pluralize(blockers, "blocker")}</Badge> : null}
                  {changesReq && !approvals ? <Badge tone="danger" outline>changes requested</Badge> : approvals ? <Badge tone="success" outline>{approvals} approved</Badge> : null}
                  {p.comments.length ? <span className="muted small">💬 {p.comments.length}</span> : null}
                </div>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
