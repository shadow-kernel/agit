import { useEffect, useMemo, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { api } from "../api/client";
import { useAsync } from "../lib/useAsync";
import { shortId } from "../lib/format";

export function BranchSelector({ repo, rev, path = "", mode = "tree" }: { repo: string; rev: string; path?: string; mode?: "tree" | "blob" | "blame" | "commits" }) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const ref = useRef<HTMLDivElement>(null);
  const branches = useAsync(() => api.branches(repo), [repo], { silent: true, enabled: open });
  const tags = useAsync(() => api.tags(repo), [repo], { silent: true, enabled: open });

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => ref.current && !ref.current.contains(e.target as Node) && setOpen(false);
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const items = useMemo(() => {
    const b = (branches.data ?? []).map((x) => ({ name: x.name, id: x.id, kind: "branch" as const, meta: x.ahead || x.behind ? `${x.ahead ?? 0}↑ ${x.behind ?? 0}↓` : "" }));
    const t = (tags.data ?? []).map((x) => ({ name: x.name, id: x.id, kind: "tag" as const, meta: shortId(x.id, 7) }));
    const all = [...b, ...t];
    const needle = q.trim().toLowerCase();
    return needle ? all.filter((x) => x.name.toLowerCase().includes(needle)) : all;
  }, [branches.data, tags.data, q]);

  const target = (name: string) => {
    const base = `/r/${encodeURIComponent(repo)}`;
    if (mode === "commits") return `${base}/commits?rev=${encodeURIComponent(name)}`;
    if (mode === "blob") return `${base}/blob/${encodeURIComponent(name)}/${path}`;
    if (mode === "blame") return `${base}/blame/${encodeURIComponent(name)}/${path}`;
    return path ? `${base}/tree/${encodeURIComponent(name)}/${path}` : `${base}/tree/${encodeURIComponent(name)}`;
  };

  const isId = /^[0-9a-f]{12,64}$/.test(rev);
  return (
    <div className="popover-wrap" ref={ref}>
      <button className="btn sm" onClick={() => setOpen((o) => !o)} aria-haspopup="listbox" aria-expanded={open}>
        <span aria-hidden>{isId ? "◇" : "⑂"}</span>
        <span className="truncate" style={{ maxWidth: 200 }}>{isId ? shortId(rev, 8) : rev}</span>
        <span aria-hidden>▾</span>
      </button>
      {open ? (
        <div className="menu" role="listbox">
          <div className="menu-head">
            <input autoFocus className="input sm" placeholder="Find a branch or tag…" value={q} onChange={(e) => setQ(e.target.value)} />
          </div>
          <div className="menu-list">
            {branches.loading && !branches.data ? <div className="menu-item muted">Loading…</div> : null}
            {items.map((it) => (
              <Link key={`${it.kind}:${it.name}`} className={`menu-item ${it.name === rev ? "current" : ""}`} to={target(it.name)} onClick={() => setOpen(false)}>
                <span className="check">{it.name === rev ? "✓" : ""}</span>
                <span className="truncate">{it.name}</span>
                <span className="meta">{it.kind === "tag" ? "tag" : it.meta}</span>
              </Link>
            ))}
            {!branches.loading && items.length === 0 ? <div className="menu-item muted">No matches</div> : null}
          </div>
        </div>
      ) : null}
    </div>
  );
}
