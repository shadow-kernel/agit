import { useEffect, useState, type FormEvent } from "react";
import { Link } from "react-router-dom";
import { api } from "../api/client";
import type { Access, Scope, TokenCreated, TokenInfo } from "../api/types";
import { toast } from "../components/Toasts";
import { Avatar, Badge, Banner, CopyButton, EmptyState, ErrorState, SkeletonRows } from "../components/ui";
import { fullDate, pluralize, timeAgo } from "../lib/format";
import { hasAccess, isAnyAdmin, useAuth } from "../lib/useAuth";
import { useAsync, useMutation } from "../lib/useAsync";
import { loadSettings, saveSettings } from "../api/settings";

const TTL_PRESETS: { label: string; secs: number | null }[] = [
  { label: "never expires", secs: null },
  { label: "1 hour", secs: 3600 },
  { label: "1 day", secs: 86_400 },
  { label: "7 days", secs: 7 * 86_400 },
  { label: "30 days", secs: 30 * 86_400 },
];

export default function AdminTokens() {
  const auth = useAuth();
  const admin = isAnyAdmin(auth.who);
  // Only ask the server once we know the principal may list tokens (avoids a guaranteed 403 for read/write tokens).
  const tokens = useAsync(() => api.tokens(), [auth.who?.principal], { silent: true, enabled: admin });
  const repos = useAsync(() => api.repos(), [], { silent: true, enabled: admin });
  const [created, setCreated] = useState<TokenCreated | null>(null);
  const del = useMutation((id: string) => api.deleteToken(id));
  const [busyId, setBusyId] = useState<string | null>(null);
  useEffect(() => {
    document.title = "Tokens · admin · agit";
    return () => {
      document.title = "agit";
    };
  }, []);

  const doDelete = async (t: TokenInfo) => {
    if (!confirm(`Delete token "${t.name}" (${t.id})? Requests using it will be rejected immediately.`)) return;
    setBusyId(t.id);
    const r = await del.run(t.id);
    setBusyId(null);
    if (r !== undefined) {
      tokens.setData((prev) => (prev ?? []).filter((x) => x.id !== t.id));
      toast(`Token ${t.name} deleted`, "info");
    }
  };

  return (
    <main className="container">
      <div className="page-head">
        <div>
          <h1>Access tokens</h1>
          <p className="muted">Scoped bearer tokens for agents and CI (<code>/auth/tokens</code>). A token can only grant scopes its creator holds; <code>*</code> scopes need root or a server-wide admin. People sign in with <Link to="/admin/keys">SSH keys</Link> instead; tokens are for agents, CI and the browser. Every denial and mutation is appended to <code>audit.jsonl</code>.</p>
        </div>
        <div className="row"><Link className="btn" to="/admin/keys">SSH keys</Link><Link className="btn" to="/">← Repositories</Link></div>
      </div>

      {auth.loading && !auth.who ? (
        <div className="box"><SkeletonRows rows={3} /></div>
      ) : !auth.who ? (
        <div className="box">
          <EmptyState icon="⚿" title="Sign in required">
            {auth.error ? <><code>{auth.error.code}</code> {auth.error.message}. </> : null}Sign in with an admin token (header ⚙) to manage tokens.
          </EmptyState>
        </div>
      ) : !admin ? (
        <div className="box">
          <EmptyState icon="⚿" title="Admin access required">
            You are <strong>{auth.who.agent ?? auth.who.principal}</strong> with {auth.who.scopes.length ? auth.who.scopes.map((s) => `${s.repo}: ${s.access}`).join(", ") : "no scopes"}. Sign in with an admin token (header ⚙) to manage tokens.
          </EmptyState>
        </div>
      ) : (
        <div className="stack">
          {created ? <CreatedToken t={created} onDismiss={() => setCreated(null)} /> : null}
          <div className="box">
            <div className="box-head">
              <h3>{tokens.data ? pluralize(tokens.data.length, "token") : "Tokens"}</h3>
              <span className="muted">hashes only; the plaintext is shown once at creation</span>
            </div>
            {tokens.error ? <div className="box-body"><ErrorState error={tokens.error} retry={tokens.reload} /></div> : tokens.loading && !tokens.data ? <SkeletonRows rows={3} /> : tokens.data?.length === 0 ? (
              <EmptyState icon="⚿" title="No tokens yet">Create one below for an agent or a CI job.</EmptyState>
            ) : (
              <table className="table tokens-table">
                <thead>
                  <tr>
                    <th>Name</th>
                    <th>Agent</th>
                    <th>Scopes</th>
                    <th>Created</th>
                    <th>Expires</th>
                    <th>Last used</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  {tokens.data?.map((t) => {
                    const expired = !!t.expires_ms && t.expires_ms < Date.now();
                    return (
                      <tr key={t.id} className={expired ? "expired" : ""}>
                        <td>
                          <div style={{ fontWeight: 600 }}>{t.name}</div>
                          <div className="small muted mono">{t.id}</div>
                        </td>
                        <td>{t.agent ? <span className="row" style={{ gap: 6 }}><Avatar id={t.agent} /> {t.agent}</span> : <span className="subtle">—</span>}</td>
                        <td><div className="row wrap" style={{ gap: 4 }}>{t.scopes.map((s, i) => <ScopeBadge key={i} s={s} />)}</div></td>
                        <td title={fullDate(t.created_ms)}>{timeAgo(t.created_ms)}<div className="small muted">by {t.created_by || "—"}</div></td>
                        <td>{t.expires_ms ? <span title={fullDate(t.expires_ms)} style={expired ? { color: "var(--danger)" } : undefined}>{expired ? "expired " : ""}{timeAgo(t.expires_ms).replace(" ago", expired ? " ago" : "")}{expired ? "" : <span className="small muted"> ({fullDate(t.expires_ms)})</span>}</span> : <span className="muted">never</span>}</td>
                        <td>{t.last_used_ms ? <span title={fullDate(t.last_used_ms)}>{timeAgo(t.last_used_ms)}</span> : <span className="subtle">never</span>}</td>
                        <td className="right-align"><button className="btn sm danger" onClick={() => doDelete(t)} disabled={busyId === t.id}>Delete</button></td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )}
          </div>
          <CreateToken repos={(repos.data ?? []).map((r) => r.name)} onCreated={(t) => { setCreated(t); tokens.setData((prev) => [...(prev ?? []), stripToken(t)]); }} />
        </div>
      )}
    </main>
  );
}

function stripToken(t: TokenCreated): TokenInfo {
  const { token: _token, ...info } = t;
  return info;
}

export function ScopeBadge({ s }: { s: Scope }) {
  return <Badge mono tone={s.access === "admin" ? "accent" : s.access === "write" ? "success" : "neutral"} title={`${s.access} access to ${s.repo === "*" ? "every repository" : s.repo}`}>{s.repo}: {s.access}</Badge>;
}

function CreatedToken({ t, onDismiss }: { t: TokenCreated; onDismiss: () => void }) {
  const useIt = () => {
    saveSettings({ ...loadSettings(), token: t.token });
    toast(`Now using token ${t.name}`, "success");
  };
  return (
    <Banner tone="success" icon="⚿" title={<>Token <strong>{t.name}</strong> created. Copy it now: it is shown only once.</>} actions={<div className="row"><CopyButton text={t.token} label="Copy token" /><button className="btn sm" onClick={useIt} title="Store it in this browser's settings and sign in with it">Use in this browser</button><button className="btn sm ghost" onClick={onDismiss} aria-label="Dismiss">×</button></div>}>
      <code className="token-reveal" aria-label="Plaintext token">{t.token}</code>
      <div className="mt8">id <code>{t.id}</code>{t.agent ? <> · bound to agent <code>{t.agent}</code></> : null}{t.expires_ms ? <> · expires {fullDate(t.expires_ms)}</> : " · never expires"}</div>
    </Banner>
  );
}

function CreateToken({ repos, onCreated }: { repos: string[]; onCreated: (t: TokenCreated) => void }) {
  const auth = useAuth();
  const [name, setName] = useState("");
  const [agent, setAgent] = useState("");
  const [scopes, setScopes] = useState<Scope[]>([{ repo: repos[0] ?? "*", access: "write" }]);
  const [ttl, setTtl] = useState<string>("null");
  const [customTtl, setCustomTtl] = useState(3600);
  const create = useMutation((body: Parameters<typeof api.createToken>[0]) => api.createToken(body));
  useEffect(() => {
    // Default the first scope to a real repository once the list arrives.
    setScopes((sc) => (sc.length === 1 && sc[0].repo === "*" && repos[0] && !isRoot ? [{ ...sc[0], repo: repos[0] }] : sc));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [repos.join(",")]);
  const isRoot = !!auth.who?.root;
  const canGrant = (s: Scope) => !auth.who || hasAccess(auth.who, s.repo === "*" ? "*" : s.repo, s.access);
  const problems: string[] = [];
  if (!name.trim()) problems.push("A name is required.");
  if (!scopes.length) problems.push("Add at least one scope.");
  scopes.forEach((s, i) => {
    if (!s.repo.trim()) problems.push(`Scope ${i + 1}: repository is empty.`);
    else if (!canGrant(s)) problems.push(`Scope ${i + 1}: you do not hold ${s.access} on ${s.repo}${s.repo === "*" ? " (wildcard scopes need root or a server-wide admin)" : ""}.`);
  });
  const ttlSecs = ttl === "custom" ? Math.max(1, Math.floor(customTtl)) : ttl === "null" ? undefined : Number(ttl);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (problems.length) return;
    const t = await create.run({ name: name.trim(), agent: agent.trim() || undefined, scopes: scopes.map((s) => ({ repo: s.repo.trim(), access: s.access })), ttl_secs: ttlSecs });
    if (t) {
      onCreated(t);
      setName("");
      setAgent("");
      toast(`Token ${t.name} created`, "success");
    }
  };
  const setScope = (i: number, patch: Partial<Scope>) => setScopes(scopes.map((s, j) => (j === i ? { ...s, ...patch } : s)));
  const repoOptions = [...new Set([...(isRoot ? ["*"] : []), ...repos, ...(auth.who?.scopes.map((s) => s.repo) ?? [])])];

  return (
    <form className="box" onSubmit={submit}>
      <div className="box-head"><h3>Create token</h3></div>
      <div className="box-body stack">
        <div className="form" style={{ maxWidth: "none" }}>
          <div className="row">
            <label className="field">
              Name
              <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="ci-deploy, coder-1 laptop…" required />
            </label>
            <label className="field">
              Bind to agent <span className="hint">optional; requests with this token always act as this agent</span>
              <input className="input" value={agent} onChange={(e) => setAgent(e.target.value)} placeholder="coder-1" spellCheck={false} />
            </label>
          </div>
        </div>
        <div className="field">
          Scopes <span className="hint">Repository (or <code>*</code> for all) and access level. Reads need <code>read</code>, mutations <code>write</code>; policy, keys, webhooks, rules, repository create/delete and tokens need <code>admin</code>.</span>
          <div className="stack tight scope-editor">
            {scopes.map((s, i) => (
              <div key={i} className="row wrap">
                <input className="input sm mono" list="repo-options" style={{ width: 240 }} value={s.repo} onChange={(e) => setScope(i, { repo: e.target.value })} placeholder="repository or *" aria-label={`Scope ${i + 1} repository`} spellCheck={false} />
                <select className="select sm" value={s.access} onChange={(e) => setScope(i, { access: e.target.value as Access })} aria-label={`Scope ${i + 1} access`}>
                  <option value="read">read</option>
                  <option value="write">write</option>
                  <option value="admin">admin</option>
                </select>
                {!canGrant(s) ? <Badge tone="danger">not held by you</Badge> : <ScopeBadge s={s} />}
                <button type="button" className="btn sm ghost" onClick={() => setScopes(scopes.filter((_, j) => j !== i))} aria-label="Remove scope" disabled={scopes.length === 1}>×</button>
              </div>
            ))}
            <datalist id="repo-options">{repoOptions.map((r) => <option key={r} value={r} />)}</datalist>
            <div><button type="button" className="btn sm" onClick={() => setScopes([...scopes, { repo: repos[0] ?? "*", access: "read" }])}>Add scope</button></div>
          </div>
        </div>
        <div className="row wrap" style={{ gap: 12 }}>
          <label className="field">
            Expiry
            <select className="select" value={ttl} onChange={(e) => setTtl(e.target.value)} aria-label="Time to live">
              {TTL_PRESETS.map((p) => <option key={p.label} value={p.secs === null ? "null" : String(p.secs)}>{p.label}</option>)}
              <option value="custom">custom (seconds)</option>
            </select>
          </label>
          {ttl === "custom" ? (
            <label className="field">
              TTL in seconds
              <input className="input" type="number" min={1} value={customTtl} onChange={(e) => setCustomTtl(Number(e.target.value))} style={{ width: 160 }} />
            </label>
          ) : null}
        </div>
        {problems.length && (name || scopes.some((s) => !canGrant(s))) ? <ul className="small" style={{ color: "var(--danger)", margin: 0, paddingLeft: 18 }}>{problems.map((p) => <li key={p}>{p}</li>)}</ul> : null}
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button type="submit" className="btn primary" disabled={create.busy || problems.length > 0}>{create.busy ? "Creating…" : "Create token"}</button>
        </div>
      </div>
    </form>
  );
}
