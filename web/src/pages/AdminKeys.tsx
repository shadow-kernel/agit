import { useEffect, useState, type FormEvent } from "react";
import { Link } from "react-router-dom";
import { api } from "../api/client";
import type { Access, AuthorizedKey, Scope } from "../api/types";
import { toast } from "../components/Toasts";
import { Avatar, Badge, Banner, CopyButton, EmptyState, ErrorState, SkeletonRows } from "../components/ui";
import { fullDate, pluralize, timeAgo } from "../lib/format";
import { hasAccess, isAnyAdmin, useAuth } from "../lib/useAuth";
import { useAsync, useMutation } from "../lib/useAsync";
import { ScopeBadge } from "./AdminTokens";

export default function AdminKeys() {
  const auth = useAuth();
  const admin = isAnyAdmin(auth.who);
  const keys = useAsync(() => api.authorizedKeys(), [auth.who?.principal], { silent: true, enabled: admin });
  const repos = useAsync(() => api.repos(), [], { silent: true, enabled: admin });
  const del = useMutation((id: string) => api.deleteAuthorizedKey(id));
  const [busyId, setBusyId] = useState<string | null>(null);
  useEffect(() => {
    document.title = "SSH keys · admin · agit";
    return () => {
      document.title = "agit";
    };
  }, []);

  const doDelete = async (k: AuthorizedKey) => {
    if (!confirm(`Remove key "${k.name}" (${k.fingerprint})? Every session it opened is revoked immediately.`)) return;
    setBusyId(k.id);
    const r = await del.run(k.id);
    setBusyId(null);
    if (r !== undefined) {
      keys.setData((prev) => (prev ?? []).filter((x) => x.id !== k.id));
      toast(`Key ${k.name} removed${r.revoked_sessions ? `, ${pluralize(r.revoked_sessions, "session")} revoked` : ""}`, "info");
    }
  };

  return (
    <main className="container">
      <div className="page-head">
        <div>
          <h1>SSH keys</h1>
          <p className="muted">
            Who may sign in, and what they may touch (<code>/auth/keys</code>). A person runs <code>agit auth login</code>; the client signs a server challenge with its Ed25519 key (<code>~/.ssh/id_ed25519</code>, ssh-agent or an agit key) and receives a session token carrying exactly the scopes listed here. Keys with full rights can also be given at server start with <code>--admin-key</code>; those are not listed.
          </p>
        </div>
        <div className="row"><Link className="btn" to="/admin/tokens">Tokens</Link><Link className="btn" to="/">← Repositories</Link></div>
      </div>

      {auth.loading && !auth.who ? (
        <div className="box"><SkeletonRows rows={3} /></div>
      ) : !auth.who ? (
        <div className="box">
          <EmptyState icon="⚿" title="Sign in required">
            {auth.error ? <><code>{auth.error.code}</code> {auth.error.message}. </> : null}Sign in with an admin token (header ⚙) to manage keys. On the command line, <code>agit auth login --print-token</code> prints a session token to paste.
          </EmptyState>
        </div>
      ) : !admin ? (
        <div className="box">
          <EmptyState icon="⚿" title="Admin access required">
            You are <strong>{auth.who.agent ?? auth.who.principal}</strong> with {auth.who.scopes.length ? auth.who.scopes.map((s) => `${s.repo}: ${s.access}`).join(", ") : "no scopes"}. A server-wide admin is needed to manage keys.
          </EmptyState>
        </div>
      ) : (
        <div className="stack">
          <Banner tone="neutral" icon="⌨" title="How people sign in">
            <code>ssh-keygen -t ed25519</code> once, then <code>agit auth login --url {"<server>"}/api/v1</code>. Pushes and pulls sign in on their own when the server answers 401. The browser cannot use a private key: paste the token from <code>agit auth login --print-token</code> into the settings popover (⚙).
          </Banner>
          <div className="box">
            <div className="box-head">
              <h3>{keys.data ? pluralize(keys.data.length, "authorized key") : "Authorized keys"}</h3>
              <span className="muted">fingerprints as <code>ssh-keygen -lf</code> prints them</span>
            </div>
            {keys.error ? <div className="box-body"><ErrorState error={keys.error} retry={keys.reload} /></div> : keys.loading && !keys.data ? <SkeletonRows rows={3} /> : keys.data?.length === 0 ? (
              <EmptyState icon="⚿" title="No keys authorized yet">Add a public key below; only the admin keys from the server command line can sign in until then.</EmptyState>
            ) : (
              <table className="table tokens-table">
                <thead>
                  <tr>
                    <th>Name</th>
                    <th>Fingerprint</th>
                    <th>Agent</th>
                    <th>Scopes</th>
                    <th>Added</th>
                    <th>Last sign-in</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  {keys.data?.map((k) => (
                    <tr key={k.id}>
                      <td>
                        <div style={{ fontWeight: 600 }}>{k.name}</div>
                        <div className="small muted mono">{k.id}</div>
                      </td>
                      <td><span className="row" style={{ gap: 6 }}><code className="mono small" title={k.public_key}>{k.fingerprint}</code><CopyButton text={k.public_key} label="Copy key" /></span></td>
                      <td>{k.agent ? <span className="row" style={{ gap: 6 }}><Avatar id={k.agent} /> {k.agent}</span> : <span className="subtle">—</span>}</td>
                      <td><div className="row wrap" style={{ gap: 4 }}>{k.scopes.map((s, i) => <ScopeBadge key={i} s={s} />)}</div></td>
                      <td title={fullDate(k.created_ms)}>{timeAgo(k.created_ms)}<div className="small muted">by {k.created_by || "—"}</div></td>
                      <td>{k.last_used_ms ? <span title={fullDate(k.last_used_ms)}>{timeAgo(k.last_used_ms)}</span> : <span className="subtle">never</span>}</td>
                      <td className="right-align"><button className="btn sm danger" onClick={() => doDelete(k)} disabled={busyId === k.id}>Remove</button></td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
          <AddKey repos={(repos.data ?? []).map((r) => r.name)} onAdded={(k) => keys.setData((prev) => [...(prev ?? []), k])} />
        </div>
      )}
    </main>
  );
}

function AddKey({ repos, onAdded }: { repos: string[]; onAdded: (k: AuthorizedKey) => void }) {
  const auth = useAuth();
  const [publicKey, setPublicKey] = useState("");
  const [name, setName] = useState("");
  const [agent, setAgent] = useState("");
  const [scopes, setScopes] = useState<Scope[]>([{ repo: repos[0] ?? "*", access: "write" }]);
  const add = useMutation((body: Parameters<typeof api.addAuthorizedKey>[0]) => api.addAuthorizedKey(body));
  useEffect(() => {
    setScopes((sc) => (sc.length === 1 && sc[0].repo === "*" && repos[0] ? [{ ...sc[0], repo: repos[0] }] : sc));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [repos.join(",")]);
  const canGrant = (s: Scope) => !auth.who || hasAccess(auth.who, s.repo === "*" ? "*" : s.repo, s.access);
  const keyOk = /^ssh-ed25519\s+[A-Za-z0-9+/=]+(\s+.*)?$/.test(publicKey.trim()) || /^[0-9a-f]{64}$/i.test(publicKey.trim());
  const problems: string[] = [];
  if (!publicKey.trim()) problems.push("Paste a public key.");
  else if (!keyOk) problems.push("Only ssh-ed25519 keys are accepted (ssh-keygen -t ed25519); paste the contents of id_ed25519.pub.");
  if (!scopes.length) problems.push("Add at least one scope.");
  scopes.forEach((s, i) => {
    if (!s.repo.trim()) problems.push(`Scope ${i + 1}: repository is empty.`);
    else if (!canGrant(s)) problems.push(`Scope ${i + 1}: you do not hold ${s.access} on ${s.repo}.`);
  });

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (problems.length) return;
    const k = await add.run({ public_key: publicKey.trim(), name: name.trim() || undefined, agent: agent.trim() || undefined, scopes: scopes.map((s) => ({ repo: s.repo.trim(), access: s.access })) });
    if (k) {
      onAdded(k);
      setPublicKey("");
      setName("");
      setAgent("");
      toast(`Key ${k.name} authorized`, "success");
    }
  };
  const setScope = (i: number, patch: Partial<Scope>) => setScopes(scopes.map((s, j) => (j === i ? { ...s, ...patch } : s)));
  const repoOptions = [...new Set(["*", ...repos, ...(auth.who?.scopes.map((s) => s.repo) ?? [])])];

  return (
    <form className="box" onSubmit={submit}>
      <div className="box-head"><h3>Authorize a key</h3></div>
      <div className="box-body stack">
        <label className="field">
          Public key <span className="hint">the one line in <code>~/.ssh/id_ed25519.pub</code>; the comment becomes the default name</span>
          <textarea className="input mono" rows={2} value={publicKey} onChange={(e) => setPublicKey(e.target.value)} placeholder="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA… name@host" spellCheck={false} required />
        </label>
        <div className="form" style={{ maxWidth: "none" }}>
          <div className="row">
            <label className="field">
              Name <span className="hint">optional</span>
              <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="dana's laptop" />
            </label>
            <label className="field">
              Act as agent <span className="hint">optional; sessions from this key always carry this identity</span>
              <input className="input" value={agent} onChange={(e) => setAgent(e.target.value)} placeholder="coder-1" spellCheck={false} />
            </label>
          </div>
        </div>
        <div className="field">
          Scopes <span className="hint">What the key may do: <code>read</code> to see a repository, <code>write</code> to push and open pull requests, <code>admin</code> for policy, keys, webhooks and deletion. <code>*</code> means every repository.</span>
          <div className="stack tight scope-editor">
            {scopes.map((s, i) => (
              <div key={i} className="row wrap">
                <input className="input sm mono" list="key-repo-options" style={{ width: 240 }} value={s.repo} onChange={(e) => setScope(i, { repo: e.target.value })} placeholder="repository or *" aria-label={`Scope ${i + 1} repository`} spellCheck={false} />
                <select className="select sm" value={s.access} onChange={(e) => setScope(i, { access: e.target.value as Access })} aria-label={`Scope ${i + 1} access`}>
                  <option value="read">read</option>
                  <option value="write">write</option>
                  <option value="admin">admin</option>
                </select>
                {!canGrant(s) ? <Badge tone="danger">not held by you</Badge> : <ScopeBadge s={s} />}
                <button type="button" className="btn sm ghost" onClick={() => setScopes(scopes.filter((_, j) => j !== i))} aria-label="Remove scope" disabled={scopes.length === 1}>×</button>
              </div>
            ))}
            <datalist id="key-repo-options">{repoOptions.map((r) => <option key={r} value={r} />)}</datalist>
            <div><button type="button" className="btn sm" onClick={() => setScopes([...scopes, { repo: repos[0] ?? "*", access: "read" }])}>Add scope</button></div>
          </div>
        </div>
        {problems.length && (publicKey || scopes.some((s) => !canGrant(s))) ? <ul className="small" style={{ color: "var(--danger)", margin: 0, paddingLeft: 18 }}>{problems.map((p) => <li key={p}>{p}</li>)}</ul> : null}
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button type="submit" className="btn primary" disabled={add.busy || problems.length > 0}>{add.busy ? "Adding…" : "Authorize key"}</button>
        </div>
      </div>
    </form>
  );
}
