import { useEffect, useState, type FormEvent } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { api } from "../api/client";
import { useRepo } from "../components/RepoShell";
import { toast } from "../components/Toasts";
import { useAsync, useMutation } from "../lib/useAsync";

export default function PullNew() {
  const { name, defaultBranch, reloadCounts } = useRepo();
  const [sp] = useSearchParams();
  const navigate = useNavigate();
  const branches = useAsync(() => api.branches(name), [name], { silent: true });
  const asRef = (v: string | null) => (v ? (v.startsWith("refs/") ? v : `refs/heads/${v}`) : "");
  const [form, setForm] = useState({ title: sp.get("title") || "", body: "", intent: sp.get("intent") || "", task: sp.get("task") || "", source: asRef(sp.get("source")), target: asRef(sp.get("target")) || `refs/heads/${defaultBranch}` });
  const create = useMutation((body: Parameters<typeof api.createPull>[1]) => api.createPull(name, body));
  const base = `/r/${encodeURIComponent(name)}`;

  useEffect(() => {
    if (!form.source && branches.data) {
      const first = branches.data.find((b) => b.name !== defaultBranch);
      if (first) setForm((f) => ({ ...f, source: `refs/heads/${first.name}` }));
    }
  }, [branches.data, defaultBranch, form.source]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!form.title.trim() || !form.source) return;
    const pr = await create.run({
      title: form.title.trim(),
      body: form.body || undefined,
      intent: form.intent || undefined,
      task: form.task || undefined,
      source: form.source,
      target: form.target || undefined,
    });
    if (pr) {
      toast(`Opened pull request #${pr.number}`, "success");
      reloadCounts();
      navigate(`${base}/pulls/${pr.number}`);
    }
  };
  const f = (k: keyof typeof form) => ({ value: form[k], onChange: (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>) => setForm({ ...form, [k]: e.target.value }) });
  const opts = branches.data ?? [];

  return (
    <form className="form" onSubmit={submit}>
      <h1>New pull request</h1>
      <div className="row">
        <label className="field grow">
          Source branch
          <select className="select" {...f("source")} required style={{ width: "100%" }}>
            <option value="" disabled>Select…</option>
            {opts.map((b) => <option key={b.name} value={`refs/heads/${b.name}`}>{b.name}{b.ahead ? ` (${b.ahead} ahead)` : ""}</option>)}
          </select>
        </label>
        <span style={{ paddingTop: 24 }} aria-hidden>→</span>
        <label className="field grow">
          Target branch
          <select className="select" {...f("target")} style={{ width: "100%" }}>
            {opts.map((b) => <option key={b.name} value={`refs/heads/${b.name}`}>{b.name}</option>)}
            {!opts.length ? <option value={form.target}>{defaultBranch}</option> : null}
          </select>
        </label>
      </div>
      <label className="field">
        Title
        <input className="input" {...f("title")} required placeholder="What does this change do?" autoFocus />
      </label>
      <label className="field">
        Intent <span className="hint">Why this change exists; shown separately from the title.</span>
        <input className="input" {...f("intent")} placeholder="e.g. Support the next three launch countries" />
      </label>
      <div className="row">
        <label className="field grow">
          Task <span className="hint">Ticket or issue id.</span>
          <input className="input" {...f("task")} placeholder="SHOP-60" />
        </label>
      </div>
      <label className="field">
        Body <span className="hint">Markdown.</span>
        <textarea className="textarea" rows={8} {...f("body")} placeholder="## Why&#10;&#10;## What&#10;&#10;## Risk" />
      </label>
      <div className="actions">
        <button type="button" className="btn" onClick={() => navigate(`${base}/pulls`)}>Cancel</button>
        <button type="submit" className="btn primary" disabled={create.busy || !form.title.trim() || !form.source}>{create.busy ? "Creating…" : "Create pull request"}</button>
      </div>
    </form>
  );
}
