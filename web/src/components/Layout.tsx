import { useEffect, useRef, useState } from "react";
import { Link, Outlet, useParams } from "react-router-dom";
import { getApiMode, initApi, onApiMode, type ApiMode } from "../api/client";
import { DEFAULT_BASE_URL, loadSettings, saveSettings, type Settings } from "../api/settings";
import { Badge } from "./ui";
import { Toasts, toast } from "./Toasts";
import { NotificationBell } from "./Notifications";
import { SearchBox } from "./SearchBox";

export function useApiMode(): ApiMode {
  const [mode, setMode] = useState<ApiMode>(getApiMode());
  useEffect(() => onApiMode(setMode), []);
  return mode;
}

export function Layout() {
  const mode = useApiMode();
  return (
    <>
      <Header mode={mode} />
      <Outlet />
      <footer className="footer">
        <span>agit web UI · talks only to the public API (<code>/api/v1</code>)</span>
        <span>
          <a href="https://github.com/agit-vcs/agit" target="_blank" rel="noreferrer">source</a> · <kbd className="kbd">t</kbd> file finder · <kbd className="kbd">/</kbd> search
        </span>
      </footer>
      <Toasts />
    </>
  );
}

function Logo() {
  return (
    <svg viewBox="0 0 32 32" aria-hidden>
      <rect width="32" height="32" rx="7" fill="currentColor" />
      <circle cx="10" cy="9" r="3" fill="var(--bg)" />
      <circle cx="10" cy="23" r="3" fill="var(--bg)" />
      <circle cx="22" cy="14" r="3" fill="var(--bg)" />
      <path d="M10 12v8M22 17c0 4-4 4-9 5" stroke="var(--bg)" strokeWidth="2" fill="none" strokeLinecap="round" />
    </svg>
  );
}

function Header({ mode }: { mode: ApiMode }) {
  const { repo } = useParams();
  return (
    <header className="header">
      <Link to="/" className="logo">
        <Logo />
        <span>agit</span>
      </Link>
      {repo ? (
        <nav className="crumbs" aria-label="Breadcrumb">
          <span className="sep">/</span>
          <Link to={`/r/${encodeURIComponent(repo)}`}>{repo}</Link>
        </nav>
      ) : null}
      <div className="spacer" />
      <div className="right">
        {repo ? <SearchBox repo={repo} /> : null}
        {mode === "mock" ? (
          <Badge className="mock-badge" title="The server could not be reached (or VITE_AGIT_MOCK=1); showing fixture data.">
            mock data
          </Badge>
        ) : mode === "unknown" ? (
          <Badge>connecting…</Badge>
        ) : null}
        {repo ? <NotificationBell repo={repo} /> : null}
        <SettingsPopover />
      </div>
    </header>
  );
}

function SettingsPopover() {
  const [open, setOpen] = useState(false);
  const [s, setS] = useState<Settings>(() => loadSettings());
  const ref = useRef<HTMLDivElement>(null);
  const mode = useApiMode();

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const save = async () => {
    saveSettings(s);
    setOpen(false);
    const m = await initApi(true);
    toast(m === "live" ? `Connected to ${s.baseUrl}` : `Server unreachable at ${s.baseUrl}; using mock data`, m === "live" ? "success" : "info");
  };

  return (
    <div className="popover-wrap" ref={ref}>
      <button className="btn sm" onClick={() => { setS(loadSettings()); setOpen((o) => !o); }} aria-haspopup="dialog" aria-expanded={open} title="Settings">
        <span aria-hidden>⚙</span>
        {s.agent ? <span className="hidden-sm">{s.agent}</span> : <span className="hidden-sm">Settings</span>}
      </button>
      {open ? (
        <div className="popover" role="dialog" aria-label="Settings">
          <h3>Connection</h3>
          <label className="field">
            API base URL
            <input className="input sm" value={s.baseUrl} onChange={(e) => setS({ ...s, baseUrl: e.target.value })} placeholder={DEFAULT_BASE_URL} spellCheck={false} />
            <span className="hint">Mode: {mode}. Default from <code>VITE_AGIT_API</code>.</span>
          </label>
          <label className="field">
            Bearer token
            <input className="input sm" type="password" value={s.token} onChange={(e) => setS({ ...s, token: e.target.value })} placeholder="optional" autoComplete="off" />
            <span className="hint">Sent as <code>Authorization: Bearer …</code>. With a token set, the live feed polls instead of SSE.</span>
          </label>
          <label className="field">
            Acting agent id
            <input className="input sm" value={s.agent} onChange={(e) => setS({ ...s, agent: e.target.value })} placeholder="e.g. daniel or reviewer-1" spellCheck={false} />
            <span className="hint">Sent as <code>X-Agit-Agent</code> for reviews, leases and PRs.</span>
          </label>
          <div className="actions">
            <button className="btn sm" onClick={() => setOpen(false)}>Cancel</button>
            <button className="btn sm accent" onClick={save}>Save &amp; reconnect</button>
          </div>
        </div>
      ) : null}
    </div>
  );
}
