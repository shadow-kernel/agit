import { useState, type FormEvent } from "react";
import { Link } from "react-router-dom";
import { api } from "../api/client";
import type { Lease, LeaseMode } from "../api/types";
import { useRepo } from "../components/RepoShell";
import { toast } from "../components/Toasts";
import { AgentBadge, Badge, EmptyState, ErrorState, SkeletonRows } from "../components/ui";
import { countdown, fullDate, timeAgo } from "../lib/format";
import { useAsync, useMutation, useNow } from "../lib/useAsync";
import { loadSettings } from "../api/settings";

export default function Leases() {
  const { name, reloadCounts } = useRepo();
  const leases = useAsync(() => api.leases(name), [name]);
  const now = useNow(1000);
  const [filter, setFilter] = useState("");
  const [showExpired, setShowExpired] = useState(true);
  const [showNew, setShowNew] = useState(false);
  const release = useMutation((id: string) => api.releaseLease(name, id));
  const renew = useMutation((id: string) => api.renewLease(name, id, 1800));

  const list = (leases.data ?? [])
    .filter((l) => showExpired || (l.expires_ms ?? Infinity) > now)
    .filter((l) => !filter || l.holder.includes(filter) || l.paths.some((p) => p.includes(filter)))
    .sort((a, b) => (a.expires_ms ?? Infinity) - (b.expires_ms ?? Infinity));
  const active = (leases.data ?? []).filter((l) => (l.expires_ms ?? Infinity) > now);
  const holders = [...new Set(active.map((l) => l.holder))];

  const doRelease = async (l: Lease) => {
    if (!confirm(`Release lease ${l.id} held by ${l.holder}?`)) return;
    const r = await release.run(l.id);
    if (r !== undefined) {
      leases.setData((prev) => (prev ?? []).filter((x) => x.id !== l.id));
      toast(`Released ${l.id}`, "success");
      reloadCounts();
    }
  };
  const doRenew = async (l: Lease) => {
    const r = await renew.run(l.id);
    if (r) {
      leases.setData((prev) => (prev ?? []).map((x) => (x.id === l.id ? r : x)));
      toast(`Renewed ${l.id} for 30 min`, "success");
    }
  };

  return (
    <div className="stack">
      <div className="page-head" style={{ marginBottom: 0 }}>
        <div>
          <h1>Lease board</h1>
          <p className="muted">{active.length} active lease{active.length === 1 ? "" : "s"} held by {holders.length} holder{holders.length === 1 ? "" : "s"}. Leases are advisory, time-limited claims on paths.</p>
        </div>
        <div className="row">
          <input className="input sm" style={{ width: 220 }} placeholder="Filter by holder or path" value={filter} onChange={(e) => setFilter(e.target.value)} />
          <label className="row small muted" style={{ gap: 4 }}><input type="checkbox" checked={showExpired} onChange={(e) => setShowExpired(e.target.checked)} /> expired</label>
          <button className="btn sm" onClick={() => setShowNew((s) => !s)}>{showNew ? "Cancel" : "Claim lease"}</button>
        </div>
      </div>
      {showNew ? <NewLease onCreated={(l) => { leases.setData((prev) => [...(prev ?? []), l]); setShowNew(false); reloadCounts(); }} /> : null}
      {leases.error ? <ErrorState error={leases.error} retry={leases.reload} /> : null}
      <div className="box">
        {leases.loading && !leases.data ? (
          <SkeletonRows rows={4} />
        ) : list.length === 0 ? (
          <EmptyState icon="⚑" title="No leases">Agents claim paths with <code>agit lease claim</code> or <code>POST /repos/{name}/leases</code>.</EmptyState>
        ) : (
          list.map((l) => {
            const exp = l.expires_ms ?? null;
            const expired = exp !== null && exp <= now;
            const total = exp !== null ? exp - l.created_ms : 0;
            const left = exp !== null ? Math.max(0, exp - now) : 0;
            const frac = total > 0 ? left / total : 1;
            const soon = !expired && exp !== null && left < 5 * 60_000;
            return (
              <div key={l.id} className={`lease-card ${expired ? "expired" : ""}`}>
                <div className="stack tight" style={{ minWidth: 160 }}>
                  <AgentBadge agent={{ id: l.holder }} repo={name} />
                  <div className="row small muted" style={{ gap: 6 }}>
                    <Badge tone={l.mode === "exclusive" ? "attention" : "accent"}>{l.mode}</Badge>
                    {l.workspace ? <span>ws:{l.workspace}</span> : null}
                  </div>
                </div>
                <div className="stack tight" style={{ minWidth: 0 }}>
                  <div className="paths">
                    {l.paths.map((p) => (
                      <Link key={p} className="path" to={`/r/${encodeURIComponent(name)}/commits?path=${encodeURIComponent(p.replace(/\/?\*\*.*$/, ""))}`} title="history of this path">{p}</Link>
                    ))}
                  </div>
                  <div className="small muted">
                    {l.note ? <span>{l.note} · </span> : null}
                    <span title={fullDate(l.created_ms)}>claimed {timeAgo(l.created_ms, now)}</span> · <span className="mono">{l.id}</span>
                  </div>
                </div>
                <div className="stack tight" style={{ alignItems: "flex-end" }}>
                  <span className={`expiry ${expired ? "expired" : soon ? "soon" : ""}`} title={exp !== null ? fullDate(exp) : "no expiry"}>{exp === null ? "no expiry" : countdown(exp, now)}</span>
                  {exp !== null ? <div className="progress"><i className={expired ? "over" : soon ? "soon" : ""} style={{ width: `${Math.round(frac * 100)}%` }} /></div> : null}
                  <div className="row">
                    {!expired ? <button className="btn sm" onClick={() => doRenew(l)} disabled={renew.busy}>Renew 30m</button> : null}
                    <button className="btn sm danger" onClick={() => doRelease(l)} disabled={release.busy}>{expired ? "Purge" : "Release"}</button>
                  </div>
                </div>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}

function NewLease({ onCreated }: { onCreated: (l: Lease) => void }) {
  const { name } = useRepo();
  const [holder, setHolder] = useState(loadSettings().agent || "");
  const [paths, setPaths] = useState("");
  const [mode, setMode] = useState<LeaseMode>("exclusive");
  const [ttl, setTtl] = useState(30);
  const [note, setNote] = useState("");
  const create = useMutation(() => api.createLease(name, { holder, paths: paths.split(/[\n,]/).map((s) => s.trim()).filter(Boolean), mode, ttl_secs: ttl * 60, note: note || undefined }));
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const l = await create.run();
    if (l) {
      toast(`Claimed ${l.id}`, "success");
      onCreated(l);
    }
  };
  return (
    <form className="box" onSubmit={submit}>
      <div className="box-head"><h3>Claim a lease</h3></div>
      <div className="box-body form" style={{ maxWidth: "none" }}>
        <div className="row">
          <label className="field">Holder<input className="input" value={holder} onChange={(e) => setHolder(e.target.value)} required placeholder="agent id" /></label>
          <label className="field">Paths <span className="hint">comma separated; literal paths cover subtrees, globs allowed</span><input className="input" value={paths} onChange={(e) => setPaths(e.target.value)} required placeholder="src/auth, docs/auth/**" /></label>
        </div>
        <div className="row">
          <label className="field">Mode<select className="select" value={mode} onChange={(e) => setMode(e.target.value as LeaseMode)}><option value="exclusive">exclusive</option><option value="shared">shared</option></select></label>
          <label className="field">TTL (minutes)<input className="input" type="number" min={1} value={ttl} onChange={(e) => setTtl(Number(e.target.value))} /></label>
        </div>
        <label className="field">Note<input className="input" value={note} onChange={(e) => setNote(e.target.value)} placeholder="what you are doing" /></label>
        <div className="actions"><button className="btn primary" disabled={create.busy || !holder || !paths}>Claim</button></div>
      </div>
    </form>
  );
}
