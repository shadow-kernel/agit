import { Link } from "react-router-dom";
import { useRepo } from "../components/RepoShell";
import { TreeBrowser } from "../components/TreeBrowser";
import { Badge } from "../components/ui";
import { fullDate, pluralize, timeAgo } from "../lib/format";

export default function RepoHome() {
  const { name, repo, defaultBranch } = useRepo();
  const base = `/r/${encodeURIComponent(name)}`;
  return (
    <div className="stack">
      <div className="page-head" style={{ marginBottom: 0 }}>
        <div>
          <h1>
            <Link to={base} className="link-plain">{name}</Link>
          </h1>
          <div className="row wrap small muted mt8" style={{ gap: 12 }}>
            {repo ? (
              <>
                <Link className="link-plain" to={`${base}/commits`}><strong>{pluralize(repo.branches, "branch", "branches")}</strong></Link>
                <span>{pluralize(repo.tags, "tag")}</span>
                <span>{repo.objects.toLocaleString()} objects</span>
                <span title={fullDate(repo.created_ms)}>created {timeAgo(repo.created_ms)}</span>
              </>
            ) : null}
          </div>
        </div>
        <div className="row">
          <Badge outline>default: {defaultBranch}</Badge>
        </div>
      </div>
      {repo && !repo.head ? (
        <div className="box"><div className="empty"><h3>This repository is empty</h3><p>Push a first commit with <code>agit push</code>.</p></div></div>
      ) : (
        <TreeBrowser repo={name} rev={defaultBranch} path="" />
      )}
    </div>
  );
}
