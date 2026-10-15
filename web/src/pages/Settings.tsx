import { useEffect, useMemo, useState, type FormEvent, type KeyboardEvent } from "react";
import { NavLink, Route, Routes, useNavigate } from "react-router-dom";
import { api } from "../api/client";
import { loadSettings } from "../api/settings";
import { DEFAULT_POLICY, type Policy, type TagRule, type Webhook } from "../api/types";
import { useRepo } from "../components/RepoShell";
import { toast } from "../components/Toasts";
import { Badge, EmptyState, ErrorState, Skeleton, SkeletonRows } from "../components/ui";
import { fullDate, pluralize, timeAgo } from "../lib/format";
import { useAsync, useMutation } from "../lib/useAsync";

export default function Settings() {
  const { name } = useRepo();
  const base = `/r/${encodeURIComponent(name)}/settings`;
  const item = (to: string, label: string, end = false) => (
    <NavLink to={to} end={end} className={({ isActive }) => (isActive ? "active" : "")}>
      {label}
    </NavLink>
  );
  return (
    <div className="settings-layout">
      <nav className="settings-nav" aria-label="Settings sections">
        <div className="group">Repository</div>
        {item(base, "Policy", true)}
        {item(`${base}/webhooks`, "Webhooks")}
        {item(`${base}/rules`, "Knowledge rules")}
        <div className="group">Danger</div>
        {item(`${base}/danger`, "Danger zone")}
      </nav>
      <div className="stack">
        <Routes>
          <Route index element={<PolicySettings />} />
          <Route path="webhooks" element={<WebhookSettings />} />
          <Route path="rules" element={<RulesSettings />} />
          <Route path="danger" element={<DangerZone />} />
        </Routes>
      </div>
    </div>
  );
}

// ---- shared --------------------------------------------------------------------

/** Chip list with an input to add entries (Enter or comma adds). */
function ListEditor({ values, onChange, placeholder, mono = true, suggestions = [], ariaLabel }: { values: string[]; onChange: (v: string[]) => void; placeholder: string; mono?: boolean; suggestions?: string[]; ariaLabel: string }) {
  const [draft, setDraft] = useState("");
  const add = (raw: string) => {
    const items = raw.split(/[,\s]+/).map((s) => s.trim()).filter(Boolean);
    if (!items.length) return;
    onChange([...new Set([...values, ...items])]);
    setDraft("");
  };
  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter" || e.key === ",") {
      e.preventDefault();
      add(draft);
    } else if (e.key === "Backspace" && !draft && values.length) onChange(values.slice(0, -1));
  };
  const unused = suggestions.filter((s) => !values.includes(s));
  return (
    <div className="chips-editor">
      <div className="chips">
        {values.map((v) => (
          <span key={v} className={`chip ${mono ? "mono" : ""}`}>
            {v}
            <button type="button" aria-label={`Remove ${v}`} onClick={() => onChange(values.filter((x) => x !== v))}>×</button>
          </span>
        ))}
        <input className="chip-input" value={draft} onChange={(e) => setDraft(e.target.value)} onKeyDown={onKey} onBlur={() => draft && add(draft)} placeholder={values.length ? "" : placeholder} aria-label={ariaLabel} spellCheck={false} />
      </div>
      {unused.length ? (
        <div className="row wrap small muted" style={{ gap: 4, marginTop: 6 }}>
          suggestions:
          {unused.map((s) => (
            <button key={s} type="button" className="chip ghost" onClick={() => onChange([...values, s])}>+ {s}</button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function Toggle({ checked, onChange, label, help }: { checked: boolean; onChange: (v: boolean) => void; label: string; help: string }) {
  return (
    <label className="toggle-row">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span>
        <span className="label">{label}</span>
        <span className="help">{help}</span>
      </span>
    </label>
  );
}

function SectionHead({ title, children }: { title: string; children?: React.ReactNode }) {
  return (
    <div className="settings-head">
      <h2>{title}</h2>
      {children ? <p className="muted">{children}</p> : null}
    </div>
  );
}

function actingAgent() {
  return loadSettings().agent || "web-ui";
}

// ---- Policy --------------------------------------------------------------------

function PolicySettings() {
  const { name } = useRepo();
  const policy = useAsync(() => api.policy(name), [name]);
  const [draft, setDraft] = useState<Policy | null>(null);
  const agents = useAsync(() => api.agents(name), [name], { silent: true });
  const branches = useAsync(() => api.branches(name), [name], { silent: true });
  const save = useMutation((p: Policy) => api.putPolicy(name, p));

  useEffect(() => {
    if (policy.data) setDraft({ ...DEFAULT_POLICY, ...policy.data });
  }, [policy.data]);

  const dirty = useMemo(() => !!draft && !!policy.data && JSON.stringify(draft) !== JSON.stringify({ ...DEFAULT_POLICY, ...policy.data }), [draft, policy.data]);
  const set = <K extends keyof Policy>(k: K, v: Policy[K]) => setDraft((d) => (d ? { ...d, [k]: v } : d));

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!draft) return;
    const saved = await save.run({ ...draft, require_approvals: Math.max(0, Math.floor(draft.require_approvals || 0)), max_files_per_commit: Math.max(0, Math.floor(draft.max_files_per_commit || 0)) });
    if (saved) {
      policy.setData(saved);
      toast("Policy saved", "success");
    }
  };

  const agentIds = (agents.data ?? []).map((a) => a.id);
  const branchNames = (branches.data ?? []).map((b) => b.name);

  if (policy.error) return <ErrorState error={policy.error} retry={policy.reload} />;
  if (!draft) return <div className="box"><Skeleton lines={8} /></div>;
  const protectedAny = draft.protected_branches.length > 0;

  return (
    <form onSubmit={submit} className="stack">
      <SectionHead title="Policy">
        Rules enforced server-side at <code>refs/update</code> (direct pushes) and <code>pulls/{"{n}"}/merge</code>. Violations are rejected with <code>403 policy_violation</code>.
      </SectionHead>

      <div className="box">
        <div className="box-head"><h3>Protected branches</h3></div>
        <div className="box-body stack">
          <ListEditor values={draft.protected_branches} onChange={(v) => set("protected_branches", v)} placeholder="main, release/*, refs/heads/prod" suggestions={branchNames} ariaLabel="Protected branches" />
          <p className="small muted">Branch names, globs (<code>release/*</code>) or full ref names. Protected branches cannot be deleted. Everything below applies to protected branches only.</p>
          {!protectedAny ? <p className="small" style={{ color: "var(--attention)" }}>No branches are protected: the rules below are stored but have no effect.</p> : null}
        </div>
      </div>

      <div className="box">
        <div className="box-head"><h3>Merging and pushing</h3></div>
        <div className="box-body stack tight">
          <Toggle checked={draft.require_pull_request} onChange={(v) => set("require_pull_request", v)} label="Require pull requests" help="Protected branches only move through pull request merges; direct pushes are rejected." />
          <Toggle checked={draft.block_on_blockers} onChange={(v) => set("block_on_blockers", v)} label="Block on unresolved blockers" help="Unresolved comments of kind blocker prevent merging." />
          <Toggle checked={draft.require_checks} onChange={(v) => set("require_checks", v)} label="Require passing checks" help="All checks must be success, and at least one check must exist, before a merge." />
          <label className="field" style={{ maxWidth: 320 }}>
            Required approvals
            <input className="input" type="number" min={0} max={50} value={draft.require_approvals} onChange={(e) => set("require_approvals", Number(e.target.value))} />
            <span className="hint">Distinct approving reviewers, counting each reviewer's latest decision and excluding the author. Any request_changes blocks.</span>
          </label>
        </div>
      </div>

      <div className="box">
        <div className="box-head"><h3>Commit requirements</h3></div>
        <div className="box-body stack tight">
          <Toggle checked={draft.require_intent} onChange={(v) => set("require_intent", v)} label="Require intent" help="Every new commit introduced to a protected branch must carry an intent." />
          <Toggle checked={draft.require_agent} onChange={(v) => set("require_agent", v)} label="Require agent identity" help="Every new commit must carry an agent id (commits by humans without --agent are rejected)." />
          <Toggle checked={draft.require_signed} onChange={(v) => set("require_signed", v)} label="Require signed commits" help="Every new commit must carry a valid Ed25519 signature whose key is registered for the signer (POST /repos/{repo}/keys)." />
          <label className="field" style={{ maxWidth: 320 }}>
            Max files per commit
            <input className="input" type="number" min={0} value={draft.max_files_per_commit} onChange={(e) => set("max_files_per_commit", Number(e.target.value))} />
            <span className="hint">Reject commits touching more than this many files. 0 = unlimited.</span>
          </label>
        </div>
      </div>

      <div className="box">
        <div className="box-head"><h3>Allowed agents</h3></div>
        <div className="box-body stack">
          <label className="field">
            Agents allowed to merge
            <ListEditor values={draft.allowed_merge_agents} onChange={(v) => set("allowed_merge_agents", v)} placeholder="reviewer-1, planner-1 (empty = anyone)" suggestions={agentIds} ariaLabel="Allowed merge agents" />
            <span className="hint">Only these agent ids (<code>X-Agit-Agent</code>) may merge into protected branches. Empty allows anyone.</span>
          </label>
          <label className="field">
            Agents allowed to push directly
            <ListEditor values={draft.allowed_push_agents} onChange={(v) => set("allowed_push_agents", v)} placeholder="release-bot (empty = anyone, subject to the PR rule)" suggestions={agentIds} ariaLabel="Allowed push agents" />
            <span className="hint">Only these agent ids may push directly to protected branches. Empty allows anyone, still subject to "Require pull requests".</span>
          </label>
        </div>
      </div>

      <div className="row between wrap">
        <span className="small muted">Saved as <code>{actingAgent()}</code> (change the acting agent in the header settings).</span>
        <div className="row">
          <button type="button" className="btn" disabled={!dirty || save.busy} onClick={() => policy.data && setDraft({ ...DEFAULT_POLICY, ...policy.data })}>Reset</button>
          <button type="submit" className="btn primary" disabled={!dirty || save.busy}>{save.busy ? "Saving…" : "Save policy"}</button>
        </div>
      </div>
    </form>
  );
}

// ---- Webhooks ------------------------------------------------------------------

const KNOWN_KINDS = ["commit", "checkpoint", "merge", "ref.update", "pull.*", "pull.open", "pull.review", "pull.merge", "pull.close", "lease.*", "note.create"];

function WebhookSettings() {
  const { name } = useRepo();
  const hooks = useAsync(() => api.webhooks(name), [name]);
  const [form, setForm] = useState({ url: "", kinds: [] as string[], secret: "" });
  const [custom, setCustom] = useState("");
  const create = useMutation((body: { url: string; kinds?: string[]; secret?: string }) => api.createWebhook(name, body));
  const del = useMutation((id: string) => api.deleteWebhook(name, id));
  const test = useMutation((id: string) => api.testWebhook(name, id));
  const [busyId, setBusyId] = useState<string | null>(null);

  const toggleKind = (k: string) => setForm((f) => ({ ...f, kinds: f.kinds.includes(k) ? f.kinds.filter((x) => x !== k) : [...f.kinds, k] }));
  const validUrl = (() => {
    try {
      const u = new URL(form.url);
      return u.protocol === "http:" || u.protocol === "https:";
    } catch {
      return false;
    }
  })();

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!validUrl) return;
    const wh = await create.run({ url: form.url.trim(), kinds: form.kinds.length ? form.kinds : undefined, secret: form.secret || undefined });
    if (wh) {
      hooks.setData((prev) => [...(prev ?? []), wh]);
      setForm({ url: "", kinds: [], secret: "" });
      toast(`Webhook ${wh.id} created`, "success");
    }
  };
  const doTest = async (w: Webhook) => {
    setBusyId(w.id);
    const r = await test.run(w.id);
    setBusyId(null);
    if (r) toast(`Ping queued for ${w.url}`, "success");
  };
  const doDelete = async (w: Webhook) => {
    if (!confirm(`Delete webhook ${w.id} (${w.url})?`)) return;
    setBusyId(w.id);
    const r = await del.run(w.id);
    setBusyId(null);
    if (r !== undefined) {
      hooks.setData((prev) => (prev ?? []).filter((x) => x.id !== w.id));
      toast("Webhook deleted", "info");
    }
  };

  return (
    <div className="stack">
      <SectionHead title="Webhooks">
        Events are POSTed as <code>{"{repo, event}"}</code> with <code>X-Agit-Event</code>, <code>X-Agit-Delivery</code> and, when a secret is set, <code>X-Agit-Signature: blake3=…</code>. Deliveries retry 3 times with backoff.
      </SectionHead>

      <div className="box">
        <div className="box-head"><h3>{hooks.data ? pluralize(hooks.data.length, "webhook") : "Webhooks"}</h3></div>
        {hooks.error ? <div className="box-body"><ErrorState error={hooks.error} retry={hooks.reload} /></div> : null}
        {hooks.loading && !hooks.data ? (
          <SkeletonRows rows={2} />
        ) : hooks.data?.length === 0 ? (
          <EmptyState icon="⇢" title="No webhooks">Add an endpoint below to receive repository events.</EmptyState>
        ) : (
          hooks.data?.map((w) => (
            <div key={w.id} className="box-row webhook-row">
              <div className="grow" style={{ minWidth: 0 }}>
                <div className="row wrap" style={{ gap: 8 }}>
                  <span className="mono truncate" style={{ fontWeight: 600 }}>{w.url}</span>
                  {w.active ? <Badge tone="success">active</Badge> : <Badge tone="danger">disabled</Badge>}
                  {w.has_secret ? <Badge tone="accent" title="Deliveries are signed with the configured secret">signed</Badge> : <Badge outline title="No secret configured; deliveries are unsigned">no secret</Badge>}
                </div>
                <div className="row wrap small muted" style={{ gap: 6, marginTop: 4 }}>
                  <code>{w.id}</code>
                  <span title={fullDate(w.created_ms)}>created {timeAgo(w.created_ms)}</span>
                  <span>·</span>
                  {w.kinds.length ? w.kinds.map((k) => <Badge key={k} mono>{k}</Badge>) : <span>all events</span>}
                </div>
              </div>
              <div className="row">
                <button className="btn sm" onClick={() => doTest(w)} disabled={busyId === w.id} title="POST /webhooks/{id}/test queues a ping event">Test</button>
                <button className="btn sm danger" onClick={() => doDelete(w)} disabled={busyId === w.id}>Delete</button>
              </div>
            </div>
          ))
        )}
      </div>

      <form className="box" onSubmit={submit}>
        <div className="box-head"><h3>Add webhook</h3></div>
        <div className="box-body stack">
          <label className="field">
            Payload URL
            <input className="input" type="url" value={form.url} onChange={(e) => setForm({ ...form, url: e.target.value })} placeholder="https://ci.example.com/hooks/agit" required spellCheck={false} />
            {form.url && !validUrl ? <span className="hint" style={{ color: "var(--danger)" }}>Enter an absolute http(s) URL.</span> : null}
          </label>
          <div className="field">
            Event kinds <span className="hint">Exact kinds or prefixes with <code>*</code>. None selected = every event.</span>
            <div className="row wrap" style={{ gap: 6 }}>
              {[...new Set([...KNOWN_KINDS, ...form.kinds])].map((k) => (
                <button key={k} type="button" className={`chip mono ${form.kinds.includes(k) ? "active" : ""}`} onClick={() => toggleKind(k)} aria-pressed={form.kinds.includes(k)}>{k}</button>
              ))}
              <input
                className="input sm"
                style={{ width: 140 }}
                placeholder="custom kind"
                value={custom}
                onChange={(e) => setCustom(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && custom.trim()) {
                    e.preventDefault();
                    toggleKind(custom.trim());
                    setCustom("");
                  }
                }}
                aria-label="Custom event kind"
              />
            </div>
          </div>
          <label className="field" style={{ maxWidth: 420 }}>
            Secret <span className="hint">Optional. Used to sign deliveries; never returned by the API.</span>
            <input className="input" type="password" value={form.secret} onChange={(e) => setForm({ ...form, secret: e.target.value })} autoComplete="new-password" />
          </label>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button type="submit" className="btn primary" disabled={!validUrl || create.busy}>{create.busy ? "Adding…" : "Add webhook"}</button>
          </div>
        </div>
      </form>
    </div>
  );
}

// ---- Knowledge rules -----------------------------------------------------------

function RulesSettings() {
  const { name } = useRepo();
  const rules = useAsync(() => api.rules(name), [name]);
  const [rows, setRows] = useState<{ pattern: string; tags: string }[] | null>(null);
  const save = useMutation((r: TagRule[]) => api.putRules(name, r));

  useEffect(() => {
    if (rules.data) setRows(rules.data.map((r) => ({ pattern: r.pattern, tags: r.tags.join(", ") })));
  }, [rules.data]);

  const parsed: TagRule[] = useMemo(() => (rows ?? []).map((r) => ({ pattern: r.pattern.trim(), tags: r.tags.split(/[,\s]+/).map((t) => t.trim()).filter(Boolean) })).filter((r) => r.pattern || r.tags.length), [rows]);
  const problems = parsed.flatMap((r, i) => {
    const out: string[] = [];
    if (!r.pattern) out.push(`Row ${i + 1}: pattern is empty`);
    if (!r.tags.length) out.push(`Row ${i + 1}: at least one tag is required`);
    for (const t of r.tags) if (!t.includes(":")) out.push(`Row ${i + 1}: tag "${t}" needs a namespace (namespace:value)`);
    return out;
  });
  const dirty = !!rules.data && JSON.stringify(parsed) !== JSON.stringify(rules.data);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (problems.length) return;
    const saved = await save.run(parsed);
    if (saved) {
      rules.setData(saved);
      toast("Knowledge rules saved", "success");
    }
  };

  if (rules.error) return <ErrorState error={rules.error} retry={rules.reload} />;
  if (!rows) return <div className="box"><Skeleton lines={5} /></div>;

  return (
    <form className="stack" onSubmit={submit}>
      <SectionHead title="Knowledge rules">
        Path patterns that suggest tags for notes and context lookups (<code>GET /context?path=</code> returns <code>suggested_tags</code>). First match wins per tag; patterns are globs like <code>src/auth/**</code>.
      </SectionHead>
      <div className="box">
        <table className="table rules-table">
          <thead>
            <tr>
              <th style={{ width: "40%" }}>Pattern</th>
              <th>Tags</th>
              <th style={{ width: 48 }} />
            </tr>
          </thead>
          <tbody>
            {rows.length === 0 ? (
              <tr><td colSpan={3}><span className="muted small">No rules yet.</span></td></tr>
            ) : (
              rows.map((r, i) => (
                <tr key={i}>
                  <td><input className="input sm mono" value={r.pattern} onChange={(e) => setRows(rows.map((x, j) => (j === i ? { ...x, pattern: e.target.value } : x)))} placeholder="src/billing/**" aria-label={`Pattern ${i + 1}`} spellCheck={false} /></td>
                  <td>
                    <input className="input sm" value={r.tags} onChange={(e) => setRows(rows.map((x, j) => (j === i ? { ...x, tags: e.target.value } : x)))} placeholder="area:billing, service:billing" aria-label={`Tags ${i + 1}`} spellCheck={false} />
                  </td>
                  <td className="right-align">
                    <button type="button" className="btn sm ghost" onClick={() => setRows(rows.filter((_, j) => j !== i))} aria-label="Remove rule">×</button>
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
        <div className="box-row static" style={{ background: "var(--bg-subtle)" }}>
          <button type="button" className="btn sm" onClick={() => setRows([...rows, { pattern: "", tags: "" }])}>Add rule</button>
          <div className="grow" />
          {rows.length > 1 ? <span className="muted small">Order matters: earlier rules win.</span> : null}
        </div>
      </div>
      {problems.length ? (
        <ul className="small" style={{ color: "var(--danger)", margin: 0, paddingLeft: 18 }}>
          {problems.slice(0, 5).map((p) => <li key={p}>{p}</li>)}
        </ul>
      ) : null}
      <div className="row" style={{ justifyContent: "flex-end" }}>
        <button type="button" className="btn" disabled={!dirty || save.busy} onClick={() => rules.data && setRows(rules.data.map((r) => ({ pattern: r.pattern, tags: r.tags.join(", ") })))}>Reset</button>
        <button type="submit" className="btn primary" disabled={!dirty || !!problems.length || save.busy}>{save.busy ? "Saving…" : "Save rules"}</button>
      </div>
    </form>
  );
}

// ---- Danger zone ---------------------------------------------------------------

function DangerZone() {
  const { name, repo } = useRepo();
  const navigate = useNavigate();
  const [confirming, setConfirming] = useState(false);
  const [typed, setTyped] = useState("");
  const del = useMutation(() => api.deleteRepo(name));
  const matches = typed === name;

  const doDelete = async (e: FormEvent) => {
    e.preventDefault();
    if (!matches) return;
    const r = await del.run();
    if (r) {
      toast(`Deleted repository ${name}`, "info");
      navigate("/");
    }
  };

  return (
    <div className="stack">
      <SectionHead title="Danger zone">Irreversible actions. Agents keep their local clones; the server copy and its pull requests, notes, leases and events are gone.</SectionHead>
      <div className="box danger-box">
        <div className="box-row static" style={{ alignItems: "flex-start", padding: 16 }}>
          <div className="grow">
            <div style={{ fontWeight: 600 }}>Delete this repository</div>
            <div className="small muted">
              Removes <code>{name}</code>{repo ? <> ({pluralize(repo.branches, "branch", "branches")}, {repo.objects.toLocaleString()} objects)</> : null} from the server via <code>DELETE /repos/{name}</code>.
            </div>
          </div>
          {!confirming ? <button className="btn danger" onClick={() => setConfirming(true)}>Delete repository</button> : null}
        </div>
        {confirming ? (
          <form className="box-body stack" style={{ borderTop: "1px solid var(--danger)" }} onSubmit={doDelete}>
            <label className="field">
              Type <strong>{name}</strong> to confirm
              <input className="input mono" value={typed} onChange={(e) => setTyped(e.target.value)} autoFocus spellCheck={false} autoComplete="off" aria-label="Repository name confirmation" />
            </label>
            <div className="row" style={{ justifyContent: "flex-end" }}>
              <button type="button" className="btn" onClick={() => { setConfirming(false); setTyped(""); }}>Cancel</button>
              <button type="submit" className="btn danger" disabled={!matches || del.busy}>{del.busy ? "Deleting…" : "I understand, delete this repository"}</button>
            </div>
          </form>
        ) : null}
      </div>
    </div>
  );
}
