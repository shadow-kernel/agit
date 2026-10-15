import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { api } from "../api/client";
import { BranchSelector } from "../components/BranchSelector";
import { CodeView } from "../components/CodeView";
import { Markdown } from "../components/Markdown";
import { useRepo } from "../components/RepoShell";
import { Breadcrumbs, LatestCommitBar } from "../components/TreeBrowser";
import { ErrorState, Skeleton } from "../components/ui";
import { bytes, langOf } from "../lib/format";
import { useAsync } from "../lib/useAsync";

export default function Blob() {
  const { name } = useRepo();
  const { rev: rawRev = "main", "*": splat = "" } = useParams();
  const rev = decodeURIComponent(rawRev);
  const path = splat.replace(/\/+$/, "");
  const blob = useAsync(() => api.tree(name, rev, path), [name, rev, path]);
  const [raw, setRaw] = useState(false);
  const [rendered, setRendered] = useState(true);
  const base = `/r/${encodeURIComponent(name)}`;
  const data = blob.data?.kind === "blob" ? blob.data : undefined;
  const isMd = /\.(md|markdown)$/i.test(path);
  const lines = data?.text ? data.text.replace(/\n$/, "").split("\n").length : 0;

  return (
    <div className="stack">
      <div className="row wrap" style={{ gap: 12 }}>
        <BranchSelector repo={name} rev={rev} path={path} mode="blob" />
        <Breadcrumbs repo={name} rev={rev} path={path} />
      </div>
      {blob.error ? <ErrorState error={blob.error} retry={blob.reload} /> : null}
      {blob.data?.kind === "tree" ? (
        <div className="box"><div className="empty"><h3>This is a directory</h3><Link className="btn" to={`${base}/tree/${encodeURIComponent(rev)}/${path}`}>Open directory</Link></div></div>
      ) : null}
      <div className="box">
        <LatestCommitBar repo={name} rev={rev} path={path} />
        <div className="box-head" style={{ borderTop: "1px solid var(--border)" }}>
          {data ? (
            <span className="muted">
              {data.binary ? "Binary file" : `${lines} lines`} · {bytes(data.size)} · <span className="mono" title={data.id}>{data.id.slice(0, 12)}</span>
            </span>
          ) : <span className="skeleton skeleton-line" style={{ width: 160, margin: 0 }} />}
          <div className="grow" />
          {isMd && data && !data.binary ? (
            <div className="segmented">
              <button className={rendered ? "active" : ""} onClick={() => setRendered(true)}>Preview</button>
              <button className={!rendered ? "active" : ""} onClick={() => setRendered(false)}>Code</button>
            </div>
          ) : null}
          <button className={`btn sm ${raw ? "active" : ""}`} onClick={() => setRaw((r) => !r)} disabled={!data || data.binary}>Raw</button>
          <Link className="btn sm" to={`${base}/commits?rev=${encodeURIComponent(rev)}&path=${encodeURIComponent(path)}`}>History</Link>
        </div>
        {blob.loading && !data ? (
          <Skeleton lines={10} />
        ) : data ? (
          data.binary || data.text == null ? (
            <div className="binary-notice">
              <div style={{ fontSize: 28 }} aria-hidden>▣</div>
              <p>Binary file ({bytes(data.size)}). Not rendered.</p>
              <p className="small">Fetch it with <code>GET /repos/{name}/objects/{data.id.slice(0, 12)}…</code></p>
            </div>
          ) : isMd && rendered && !raw ? (
            <Markdown source={data.text} />
          ) : (
            <CodeView code={data.text} lang={langOf(path)} raw={raw} />
          )
        ) : null}
      </div>
    </div>
  );
}
