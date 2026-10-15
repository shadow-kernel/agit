import { useEffect } from "react";
import { Link } from "react-router-dom";
import { api } from "../api/client";
import { useAsync } from "../lib/useAsync";
import { fullDate, shortId, timeAgo } from "../lib/format";
import { Badge, EmptyState, ErrorState, Skeleton } from "../components/ui";

export default function RepoList() {
  const repos = useAsync(() => api.repos(), []);
  useEffect(() => {
    document.title = "Repositories · agit";
  }, []);
  return (
    <main className="container">
      <div className="page-head">
        <div>
          <h1>Repositories</h1>
          <p className="muted">Everything agents are committing to, as served by <code>GET /repos</code>.</p>
        </div>
      </div>
      {repos.error ? <ErrorState error={repos.error} retry={repos.reload} /> : null}
      {repos.loading && !repos.data ? (
        <div className="repo-grid">
          {[0, 1].map((i) => <div key={i} className="repo-card"><Skeleton lines={3} /></div>)}
        </div>
      ) : repos.data?.length === 0 ? (
        <div className="box"><EmptyState icon="▢" title="No repositories yet">Create one with <code>POST /repos</code> or <code>agit push</code>.</EmptyState></div>
      ) : (
        <div className="repo-grid">
          {repos.data?.map((r) => (
            <Link key={r.name} to={`/r/${encodeURIComponent(r.name)}`} className="repo-card">
              <div className="row between">
                <span className="name">{r.name}</span>
                <Badge outline>⑂ {r.default_branch}</Badge>
              </div>
              <div className="row wrap mt8 small muted" style={{ gap: 12 }}>
                <span>head <span className="sha">{r.head ? shortId(r.head) : "empty"}</span></span>
                <span title={fullDate(r.created_ms)}>created {timeAgo(r.created_ms)}</span>
              </div>
            </Link>
          ))}
        </div>
      )}
    </main>
  );
}
