import { useEffect, useMemo, useState } from "react";
import { Link, useLocation, useParams } from "react-router-dom";
import { ApiError } from "../api/client";
import type { CommitListItem } from "../api/types";
import { BranchSelector } from "../components/BranchSelector";
import { useRepo } from "../components/RepoShell";
import { Breadcrumbs, LatestCommitBar } from "../components/TreeBrowser";
import { AgentBadge, Banner, ErrorState, Sha, Skeleton } from "../components/ui";
import { blame, type BlameProgress, type BlameResult } from "../lib/blame";
import { fullDate, langOf, pluralize, summaryLine, timeAgo } from "../lib/format";
import { escapeHtml, highlightLines } from "../lib/highlight";

export default function Blame() {
  const { name } = useRepo();
  const { rev: rawRev = "main", "*": splat = "" } = useParams();
  const rev = decodeURIComponent(rawRev);
  const path = splat.replace(/\/+$/, "");
  const base = `/r/${encodeURIComponent(name)}`;
  const [result, setResult] = useState<BlameResult | null>(null);
  const [error, setError] = useState<ApiError | null>(null);
  const [progress, setProgress] = useState<BlameProgress>({ done: 0, total: 0 });
  const [html, setHtml] = useState<string[] | null>(null);
  const { hash } = useLocation();
  const target = hash.startsWith("#L") ? parseInt(hash.slice(2), 10) : NaN;

  useEffect(() => {
    const ctl = new AbortController();
    setResult(null);
    setError(null);
    setHtml(null);
    setProgress({ done: 0, total: 0 });
    blame(name, rev, path, setProgress, ctl.signal)
      .then((r) => {
        if (ctl.signal.aborted) return;
        setResult(r);
        if (!r.binary) highlightLines(r.lines.map((l) => l.text).join("\n"), langOf(path)).then((ls) => !ctl.signal.aborted && setHtml(ls));
      })
      .catch((e) => {
        if (ctl.signal.aborted || (e instanceof DOMException && e.name === "AbortError")) return;
        setError(e instanceof ApiError ? e : new ApiError(0, "error", e instanceof Error ? e.message : String(e)));
      });
    return () => ctl.abort();
  }, [name, rev, path]);

  useEffect(() => {
    if (!Number.isNaN(target) && result) document.getElementById(`L${target}`)?.scrollIntoView({ block: "center" });
  }, [target, result]);

  // Age rank per commit (0 = newest) for the heat strip.
  const heat = useMemo(() => {
    if (!result) return new Map<number, number>();
    const used = [...new Set(result.lines.map((l) => l.commit))].filter((i) => i >= 0);
    const sorted = [...used].sort((a, b) => result.commits[b].time_ms - result.commits[a].time_ms);
    const m = new Map<number, number>();
    sorted.forEach((ci, rank) => m.set(ci, sorted.length > 1 ? rank / (sorted.length - 1) : 0));
    return m;
  }, [result]);

  const distinct = result ? new Set(result.lines.map((l) => l.commit)).size : 0;

  return (
    <div className="stack">
      <div className="row wrap" style={{ gap: 12 }}>
        <BranchSelector repo={name} rev={rev} path={path} mode="blame" />
        <Breadcrumbs repo={name} rev={rev} path={path} />
      </div>
      {error ? <ErrorState error={error} /> : null}
      {result?.truncated ? (
        <Banner tone="attention" icon="◷" title="History limited to the last 200 commits touching this file">Lines older than that are attributed to the oldest commit shown.</Banner>
      ) : null}
      <div className="box">
        <LatestCommitBar repo={name} rev={rev} path={path} />
        <div className="box-head" style={{ borderTop: "1px solid var(--border)" }}>
          {result ? (
            <span className="muted">
              {result.binary ? "Binary file" : `${pluralize(result.lines.length, "line")} · blamed across ${pluralize(distinct, "commit")}`}
            </span>
          ) : progress.total ? (
            <span className="row muted" style={{ gap: 8 }}>
              <span className="spinner" /> fetching version {Math.min(progress.done + 1, progress.total)} of {progress.total}
              <span className="progress" aria-hidden><i style={{ width: `${Math.round((progress.done / progress.total) * 100)}%` }} /></span>
            </span>
          ) : (
            <span className="row muted" style={{ gap: 8 }}><span className="spinner" /> loading history…</span>
          )}
          <div className="grow" />
          <div className="segmented" role="tablist" aria-label="View">
            <Link role="tab" to={`${base}/blob/${encodeURIComponent(rev)}/${path}`} className="seg-link">Code</Link>
            <button className="active" role="tab" aria-selected>Blame</button>
          </div>
          <Link className="btn sm" to={`${base}/commits?rev=${encodeURIComponent(rev)}&path=${encodeURIComponent(path)}`}>History</Link>
        </div>
        {!result && !error ? (
          <Skeleton lines={12} />
        ) : result?.binary ? (
          <div className="binary-notice">Binary file. Blame is only available for text files.</div>
        ) : result ? (
          <BlameTable result={result} html={html} repo={name} heat={heat} target={target} />
        ) : null}
      </div>
    </div>
  );
}

function BlameTable({ result, html, repo, heat, target }: { result: BlameResult; html: string[] | null; repo: string; heat: Map<number, number>; target: number }) {
  const rendered = html ?? result.lines.map((l) => escapeHtml(l.text));
  return (
    <div className="code-view blame-view">
      <table>
        <tbody>
          {result.lines.map((l, i) => {
            const c: CommitListItem | undefined = l.commit >= 0 ? result.commits[l.commit] : undefined;
            const start = i === 0 || result.lines[i - 1].commit !== l.commit;
            const h = heat.get(l.commit) ?? 1;
            return (
              <tr key={l.no} id={`L${l.no}`} className={`${start ? "group-start" : ""} ${target === l.no ? "hl" : ""}`}>
                <td className="blame-meta" style={{ boxShadow: `inset 3px 0 0 rgba(var(--heat-rgb), ${(1 - h) * 0.9 + 0.05})` }} title={c ? `${summaryLine(c.message)}${c.intent ? `\n→ ${c.intent}` : ""}\n${c.agent?.id ?? c.author.name} · ${fullDate(c.time_ms)}` : undefined}>
                  {start && c ? (
                    <>
                      <Sha id={c.id} repo={repo} n={8} />
                      {c.agent ? <AgentBadge agent={c.agent} repo={repo} showModel={false} /> : <span className="author truncate">{c.author.name}</span>}
                      <span className="date" title={fullDate(c.time_ms)}>{timeAgo(c.time_ms)}</span>
                      <span className="msg truncate">
                        <Link className="link-plain" to={`/r/${encodeURIComponent(repo)}/commit/${c.id}`}>{summaryLine(c.message)}</Link>
                        {c.intent ? <span className="intent"> {c.intent}</span> : null}
                      </span>
                    </>
                  ) : null}
                </td>
                <td className="ln" onClick={() => { history.replaceState(null, "", `#L${l.no}`); window.dispatchEvent(new HashChangeEvent("hashchange")); }}>{l.no}</td>
                <td className="code" dangerouslySetInnerHTML={{ __html: rendered[i] || " " }} />
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
