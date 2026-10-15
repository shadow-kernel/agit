import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api } from "../api/client";
import type { TreeEntry } from "../api/types";
import { useAsync } from "../lib/useAsync";
import { bytes, fullDate, summaryLine, timeAgo } from "../lib/format";
import { BranchSelector } from "./BranchSelector";
import { Markdown } from "./Markdown";
import { AgentBadge, EmptyState, ErrorState, Sha, SkeletonRows, Skeleton } from "./ui";

export function Breadcrumbs({ repo, rev, path, leafIsLink = false }: { repo: string; rev: string; path: string; leafIsLink?: boolean }) {
  const base = `/r/${encodeURIComponent(repo)}`;
  const parts = path ? path.split("/") : [];
  return (
    <nav className="breadcrumbs" aria-label="Path">
      <Link to={`${base}/tree/${encodeURIComponent(rev)}`} className={parts.length ? "" : "current"}>{repo.split("/").pop()}</Link>
      {parts.map((p, i) => {
        const sub = parts.slice(0, i + 1).join("/");
        const last = i === parts.length - 1;
        return (
          <span key={sub} className="row" style={{ gap: 4 }}>
            <span className="sep">/</span>
            {last && !leafIsLink ? <span className="current">{p}</span> : <Link to={`${base}/tree/${encodeURIComponent(rev)}/${sub}`}>{p}</Link>}
          </span>
        );
      })}
    </nav>
  );
}

export function LatestCommitBar({ repo, rev, path }: { repo: string; rev: string; path?: string }) {
  const latest = useAsync(() => api.commits(repo, { rev, path: path || undefined, limit: 1 }), [repo, rev, path], { silent: true });
  const c = latest.data?.[0];
  return (
    <div className="box-row latest-commit static">
      {latest.loading && !c ? (
        <div className="skeleton skeleton-line" style={{ width: "50%", margin: 0 }} />
      ) : c ? (
        <>
          {c.agent ? <AgentBadge agent={c.agent} repo={repo} showModel={false} /> : <strong>{c.author.name}</strong>}
          <Link className="truncate link-plain grow" to={`/r/${encodeURIComponent(repo)}/commit/${c.id}`} title={c.message}>{summaryLine(c.message)}</Link>
          {c.intent ? <span className="intent small hidden-sm truncate" style={{ maxWidth: 260 }}>{c.intent}</span> : null}
          <Sha id={c.id} repo={repo} />
          <span className="muted small nowrap" title={fullDate(c.time_ms)}>{timeAgo(c.time_ms)}</span>
        </>
      ) : (
        <span className="muted">No commits</span>
      )}
    </div>
  );
}

const iconFor = (e: TreeEntry) => (e.mode === "dir" ? "▸" : e.mode === "symlink" ? "↪" : e.mode === "submodule" ? "◫" : e.mode === "executable" ? "⚙" : "▫");

export function TreeBrowser({ repo, rev, path, showReadme = true, header }: { repo: string; rev: string; path: string; showReadme?: boolean; header?: React.ReactNode }) {
  const tree = useAsync(() => api.tree(repo, rev, path || undefined), [repo, rev, path]);
  const base = `/r/${encodeURIComponent(repo)}`;
  const navigate = useNavigate();

  // If the path turns out to be a file, go to the blob view.
  useEffect(() => {
    if (tree.data?.kind === "blob") navigate(`${base}/blob/${encodeURIComponent(rev)}/${path}`, { replace: true });
  }, [tree.data, base, rev, path, navigate]);

  const entries = tree.data?.kind === "tree" ? tree.data.entries : [];
  const readme = showReadme ? entries.find((e) => e.mode !== "dir" && /^readme(\.md|\.markdown|\.txt)?$/i.test(e.name)) : undefined;
  const readmePath = readme ? (path ? `${path}/${readme.name}` : readme.name) : null;
  const readmeBlob = useAsync(() => api.tree(repo, rev, readmePath!), [repo, rev, readmePath], { silent: true, enabled: !!readmePath });
  const parent = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";

  return (
    <div className="stack">
      <div className="row wrap between">
        <div className="row wrap grow" style={{ gap: 12 }}>
          <BranchSelector repo={repo} rev={rev} path={path} />
          <Breadcrumbs repo={repo} rev={rev} path={path} />
        </div>
        <div className="row">
          <FileFinder entries={entries} base={`${base}/`} rev={rev} path={path} />
          {header}
        </div>
      </div>
      {tree.error ? <ErrorState error={tree.error} retry={tree.reload} /> : null}
      <div className="box file-table">
        <LatestCommitBar repo={repo} rev={rev} path={path} />
        {tree.loading && !tree.data ? (
          <SkeletonRows rows={8} />
        ) : entries.length === 0 && !tree.error ? (
          <EmptyState icon="▢" title="Empty directory" />
        ) : (
          <>
            {path ? (
              <div className="box-row">
                <span className="icon dir">▸</span>
                <span className="name"><Link to={`${base}/tree/${encodeURIComponent(rev)}${parent ? "/" + parent : ""}`}>..</Link></span>
              </div>
            ) : null}
            {entries.map((e) => {
              const p = path ? `${path}/${e.name}` : e.name;
              const to = e.mode === "dir" ? `${base}/tree/${encodeURIComponent(rev)}/${p}` : `${base}/blob/${encodeURIComponent(rev)}/${p}`;
              return (
                <div key={e.name} className="box-row">
                  <span className={`icon ${e.mode === "dir" ? "dir" : ""}`} aria-hidden>{iconFor(e)}</span>
                  <span className="name truncate"><Link to={to}>{e.name}</Link>{e.mode === "submodule" ? <span className="muted small"> @ {e.id.slice(0, 7)}</span> : null}</span>
                  <span className="size">{e.mode === "dir" ? "" : bytes(e.size)}</span>
                </div>
              );
            })}
          </>
        )}
      </div>
      {readmePath ? (
        <div className="box">
          <div className="box-head"><span aria-hidden>≡</span><Link className="link-plain" to={`${base}/blob/${encodeURIComponent(rev)}/${readmePath}`}>{readme!.name}</Link></div>
          {readmeBlob.data?.kind === "blob" && readmeBlob.data.text != null ? <Markdown source={readmeBlob.data.text} /> : <Skeleton lines={5} />}
        </div>
      ) : null}
    </div>
  );
}

/** Press `t` to focus; filters entries of the current directory. */
export function FileFinder({ entries, base, rev, path }: { entries: TreeEntry[]; base: string; rev: string; path: string }) {
  const [q, setQ] = useState("");
  const [open, setOpen] = useState(false);
  const [sel, setSel] = useState(0);
  const ref = useRef<HTMLInputElement>(null);
  const navigate = useNavigate();

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      if (e.key === "t" && !e.metaKey && !e.ctrlKey && !e.altKey && tag !== "INPUT" && tag !== "TEXTAREA" && tag !== "SELECT") {
        e.preventDefault();
        ref.current?.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  const matches = useMemo(() => {
    const needle = q.trim().toLowerCase();
    if (!needle) return [];
    return entries.filter((e) => e.name.toLowerCase().includes(needle)).slice(0, 12);
  }, [q, entries]);
  useEffect(() => setSel(0), [q]);

  const hrefFor = (e: TreeEntry) => `${base}${e.mode === "dir" ? "tree" : "blob"}/${encodeURIComponent(rev)}/${path ? path + "/" : ""}${e.name}`;

  return (
    <div className="finder">
      <input
        ref={ref}
        className="input sm"
        style={{ width: 220 }}
        placeholder="Go to file  (t)"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        onFocus={() => setOpen(true)}
        onBlur={() => setTimeout(() => setOpen(false), 120)}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") { e.preventDefault(); setSel((s) => Math.min(s + 1, matches.length - 1)); }
          else if (e.key === "ArrowUp") { e.preventDefault(); setSel((s) => Math.max(s - 1, 0)); }
          else if (e.key === "Enter" && matches[sel]) { navigate(hrefFor(matches[sel])); setQ(""); ref.current?.blur(); }
          else if (e.key === "Escape") { setQ(""); ref.current?.blur(); }
        }}
        aria-label="Find a file in this directory"
      />
      {open && q ? (
        <div className="results">
          {matches.length === 0 ? <span className="muted small" style={{ display: "block", padding: "6px 12px" }}>No matching entries here</span> : null}
          {matches.map((e, i) => (
            <Link key={e.name} to={hrefFor(e)} className={i === sel ? "sel" : ""} onClick={() => setQ("")}>
              {e.mode === "dir" ? "▸ " : "▫ "}{e.name}
            </Link>
          ))}
        </div>
      ) : null}
    </div>
  );
}
