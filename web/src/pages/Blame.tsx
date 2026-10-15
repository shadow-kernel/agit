import { useEffect, useMemo, useState } from "react";
import { Link, useLocation, useParams } from "react-router-dom";
import { api, ApiError, getApiMode } from "../api/client";
import type { Blame as BlameT, BlameCommit } from "../api/types";
import { BranchSelector } from "../components/BranchSelector";
import { useRepo } from "../components/RepoShell";
import { Breadcrumbs, LatestCommitBar } from "../components/TreeBrowser";
import { AgentBadge, Banner, ErrorState, Sha, Skeleton } from "../components/ui";
import { blame as clientBlame, type BlameProgress } from "../lib/blame";
import { fullDate, langOf, pluralize, summaryLine, timeAgo } from "../lib/format";
import { escapeHtml, highlightLines } from "../lib/highlight";

const MAX_COMMITS = 500;

/** Rendering model shared by the server endpoint and the client-side fallback. */
interface View {
  lines: { no: number; text: string; commit: number }[];
  /** Newest first. */
  commits: BlameCommit[];
  truncated: boolean;
  binary: boolean;
  source: "server" | "client";
}

function fromServer(b: BlameT): View {
  const commits = Object.values(b.commits).sort((a, c) => c.time_ms - a.time_ms);
  const idx = new Map(commits.map((c, i) => [c.id, i]));
  return { lines: b.lines.map((l) => ({ no: l.line, text: l.text, commit: idx.get(l.commit) ?? -1 })), commits, truncated: b.truncated, binary: false, source: "server" };
}

async function fromClient(repo: string, rev: string, path: string, onProgress: (p: BlameProgress) => void, signal: AbortSignal): Promise<View> {
  const r = await clientBlame(repo, rev, path, onProgress, signal);
  return {
    lines: r.lines,
    commits: r.commits.map((c) => ({ id: c.id, author: c.author.name, agent: c.agent?.id ?? null, time_ms: c.time_ms, summary: summaryLine(c.message), intent: c.intent ?? null })),
    truncated: r.truncated,
    binary: r.binary,
    source: "client",
  };
}

/** The route is missing on older servers: a bare 404 without the JSON error envelope. */
const endpointMissing = (e: unknown) => e instanceof ApiError && e.status === 404 && e.code === "http_404";

export default function Blame() {
  const { name } = useRepo();
  const { rev: rawRev = "main", "*": splat = "" } = useParams();
  const rev = decodeURIComponent(rawRev);
  const path = splat.replace(/\/+$/, "");
  const base = `/r/${encodeURIComponent(name)}`;
  const [result, setResult] = useState<View | null>(null);
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
    const run = async (): Promise<View> => {
      try {
        return fromServer(await api.blame(name, rev, path, MAX_COMMITS, ctl.signal));
      } catch (e) {
        if (!endpointMissing(e)) {
          // The server may refuse binaries or directories: mirror the old behaviour for those.
          if (e instanceof ApiError && e.status === 400 && getApiMode() === "live") return fromClient(name, rev, path, setProgress, ctl.signal);
          throw e;
        }
        return fromClient(name, rev, path, setProgress, ctl.signal);
      }
    };
    run()
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
        <Banner tone="attention" icon="◷" title={result.source === "server" ? `History walk stopped after ${MAX_COMMITS} commits` : "History limited to the last 200 commits touching this file"}>
          Lines older than that are attributed to the oldest commit examined{result.source === "server" ? <> (<code>GET /blame?max_commits=</code>)</> : null}.
        </Banner>
      ) : null}
      <div className="box">
        <LatestCommitBar repo={name} rev={rev} path={path} />
        <div className="box-head" style={{ borderTop: "1px solid var(--border)" }}>
          {result ? (
            <span className="muted" title={result.source === "server" ? "GET /repos/{repo}/blame/{rev}?path=" : "computed in the browser from /commits and /tree"}>
              {result.binary ? "Binary file" : `${pluralize(result.lines.length, "line")} · blamed across ${pluralize(distinct, "commit")}`}
              {result.source === "client" ? <span className="subtle"> · client-side</span> : null}
            </span>
          ) : progress.total ? (
            <span className="row muted" style={{ gap: 8 }}>
              <span className="spinner" /> fetching version {Math.min(progress.done + 1, progress.total)} of {progress.total}
              <span className="progress" aria-hidden><i style={{ width: `${Math.round((progress.done / progress.total) * 100)}%` }} /></span>
            </span>
          ) : (
            <span className="row muted" style={{ gap: 8 }}><span className="spinner" /> loading blame…</span>
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

function BlameTable({ result, html, repo, heat, target }: { result: View; html: string[] | null; repo: string; heat: Map<number, number>; target: number }) {
  const rendered = html ?? result.lines.map((l) => escapeHtml(l.text));
  return (
    <div className="code-view blame-view">
      <table>
        <tbody>
          {result.lines.map((l, i) => {
            const c: BlameCommit | undefined = l.commit >= 0 ? result.commits[l.commit] : undefined;
            const start = i === 0 || result.lines[i - 1].commit !== l.commit;
            const h = heat.get(l.commit) ?? 1;
            return (
              <tr key={l.no} id={`L${l.no}`} className={`${start ? "group-start" : ""} ${target === l.no ? "hl" : ""}`}>
                <td className="blame-meta" style={{ boxShadow: `inset 3px 0 0 rgba(var(--heat-rgb), ${(1 - h) * 0.9 + 0.05})` }} title={c ? `${c.summary}${c.intent ? `\n→ ${c.intent}` : ""}\n${c.agent ?? c.author} · ${fullDate(c.time_ms)}` : undefined}>
                  {start && c ? (
                    <>
                      <Sha id={c.id} repo={repo} n={8} />
                      {c.agent ? <AgentBadge agent={{ id: c.agent }} repo={repo} showModel={false} /> : <span className="author truncate">{c.author}</span>}
                      <span className="date" title={fullDate(c.time_ms)}>{timeAgo(c.time_ms)}</span>
                      <span className="msg truncate">
                        <Link className="link-plain" to={`/r/${encodeURIComponent(repo)}/commit/${c.id}`}>{c.summary}</Link>
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
