import { useEffect, useRef, useState } from "react";
import { Link, Outlet, useParams } from "react-router-dom";
import { ApiError, getApiMode, initApi, onApiMode, onAuthDenied, type ApiMode } from "../api/client";
import { DEFAULT_BASE_URL, loadSettings, saveSettings, type Settings } from "../api/settings";
import { isAnyAdmin, needsSignIn, refreshAuth, useAuth } from "../lib/useAuth";
import { Avatar, Badge } from "./ui";
import { Toasts, toast } from "./Toasts";
import { NotificationBell } from "./Notifications";
import { SearchBox } from "./SearchBox";

export function useApiMode(): ApiMode {
  const [mode, setMode] = useState<ApiMode>(getApiMode());
  useEffect(() => onApiMode(setMode), []);
  return mode;
}

/** Opened by the header button and by the global "sign in" prompt. */
const openSettingsListeners = new Set<(focusToken: boolean) => void>();
export function openSettings(focusToken = false) {
  openSettingsListeners.forEach((l) => l(focusToken));
}

export function Layout() {
  const mode = useApiMode();
  return (
    <>
      <Header mode={mode} />
      <AuthPrompt />
      <Outlet />
      <footer className="footer">
        <span>agit web UI · talks only to the public API (<code>/api/v1</code>)</span>
        <span>
          <a href="https://github.com/shadow-kernel/agit" target="_blank" rel="noreferrer">source</a> · <kbd className="kbd">t</kbd> file finder · <kbd className="kbd">/</kbd> search
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
        <Principal />
        <SettingsPopover />
      </div>
    </header>
  );
}

/** Header chip: who the server thinks we are (`GET /auth/whoami`). */
function Principal() {
  const auth = useAuth();
  if (auth.loading && !auth.who) return null;
  if (!auth.who) {
    return (
      <button className="btn sm who-chip" onClick={() => openSettings(true)} title={auth.error ? `${auth.error.code}: ${auth.error.message}` : "Not signed in"}>
        <span aria-hidden>⚿</span> Sign in
      </button>
    );
  }
  const w = auth.who;
  const label = w.agent ?? (w.root ? "root" : w.principal);
  const scopes = w.root ? "root: full access to every repository" : w.scopes.map((s) => `${s.repo}: ${s.access}`).join(", ") || "no scopes";
  const level = w.root ? "root" : isAnyAdmin(w) ? "admin" : w.scopes.some((s) => s.access === "write") ? "write" : w.scopes.length ? "read" : "none";
  return (
    <button className="btn sm who-chip" onClick={() => openSettings(false)} title={`principal ${w.principal}${w.agent ? ` · agent ${w.agent}` : ""}\n${scopes}`} aria-label={`Signed in as ${label}`}>
      <Avatar id={label} />
      <span className="hidden-sm truncate" style={{ maxWidth: 140 }}>{label}</span>
      <Badge tone={level === "root" || level === "admin" ? "accent" : level === "none" ? "attention" : "neutral"} className="hidden-sm">{level}</Badge>
    </button>
  );
}

/** Global 401/403 handler: one banner, pointing at the token field. */
function AuthPrompt() {
  const [err, setErr] = useState<ApiError | null>(null);
  const auth = useAuth();
  useEffect(() => onAuthDenied((e) => setErr(e)), []);
  // Clear once a sign-in succeeds.
  useEffect(() => {
    if (auth.who && !auth.loading) setErr((e) => (e && e.code !== "policy_violation" ? null : e));
  }, [auth.who, auth.loading]);
  const show = err ?? (needsSignIn(auth) ? auth.error : null);
  if (!show) return null;
  const policy = show.code === "policy_violation";
  const principal = auth.who ? (auth.who.agent ?? (auth.who.root ? "root" : auth.who.principal)) : null;
  return (
    <div className={`auth-banner ${policy ? "policy" : ""}`} role="alert">
      <span aria-hidden>{policy ? "⊘" : "⚿"}</span>
      <div className="grow">
        <strong>{policy ? "Rejected by repository policy" : auth.who ? "Not allowed" : "Sign in required"}</strong>
        <span className="muted"> · <code>{show.code}</code> {show.message}</span>
        {!policy ? <span className="muted small" style={{ display: "block" }}>{principal ? <>You are <strong>{principal}</strong>; this action needs more access. Sign in with a token that has it (header ⚙ → Sign in).</> : "The server requires a bearer token. Paste one in the settings popover (⚙) or ask an admin to create one under /admin/tokens."}</span> : null}
      </div>
      {!policy ? <button className="btn sm accent" onClick={() => openSettings(true)}>Sign in</button> : null}
      <button className="btn sm ghost" onClick={() => setErr(null)} aria-label="Dismiss">×</button>
    </div>
  );
}

function SettingsPopover() {
  const [open, setOpen] = useState(false);
  const [s, setS] = useState<Settings>(() => loadSettings());
  const [busy, setBusy] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const tokenRef = useRef<HTMLInputElement>(null);
  const mode = useApiMode();
  const auth = useAuth();

  useEffect(() => {
    const l = (focusToken: boolean) => {
      setS(loadSettings());
      setOpen(true);
      if (focusToken) setTimeout(() => tokenRef.current?.focus(), 30);
    };
    openSettingsListeners.add(l);
    return () => {
      openSettingsListeners.delete(l);
    };
  }, []);

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

  const apply = async (next: Settings, reconnect: boolean) => {
    setBusy(true);
    const prev = loadSettings();
    saveSettings(next);
    if (reconnect || prev.baseUrl !== next.baseUrl) {
      const m = await initApi(true);
      toast(m === "live" ? `Connected to ${next.baseUrl}` : `Server unreachable at ${next.baseUrl}; using mock data`, m === "live" ? "success" : "info");
    }
    const a = await refreshAuth();
    setBusy(false);
    if (a.who) {
      const who = a.who.agent ?? (a.who.root ? "root" : a.who.principal);
      toast(next.token ? `Signed in as ${who}` : `Connected as ${who}`, "success");
      setOpen(false);
    } else if (a.error) {
      toast(`Sign-in failed: ${a.error.message}`, "error", a.error.code);
      tokenRef.current?.focus();
    }
  };
  const signOut = () => {
    const next = { ...s, token: "" };
    setS(next);
    void apply(next, false);
  };

  const who = auth.who;
  return (
    <div className="popover-wrap" ref={ref}>
      <button className="btn sm" onClick={() => { setS(loadSettings()); setOpen((o) => !o); }} aria-haspopup="dialog" aria-expanded={open} title="Settings" aria-label="Settings">
        <span aria-hidden>⚙</span>
      </button>
      {open ? (
        <div className="popover" role="dialog" aria-label="Settings">
          <h3>Sign in</h3>
          <label className="field">
            Bearer token
            <input ref={tokenRef} className="input sm" type="password" value={s.token} onChange={(e) => setS({ ...s, token: e.target.value })} placeholder={who && !s.token ? "open mode: no token needed" : "paste a token"} autoComplete="off" spellCheck={false} />
            <span className="hint">Sent as <code>Authorization: Bearer …</code> and checked with <code>GET /auth/whoami</code>. Signed in with an SSH key on the command line? <code>agit auth login --print-token</code> prints a session token to paste here. With a token set, the live feed polls instead of SSE.</span>
          </label>
          {who ? (
            <div className="who-card">
              <div className="row" style={{ gap: 8 }}>
                <Avatar id={who.agent ?? (who.root ? "root" : who.principal)} />
                <div className="grow" style={{ minWidth: 0 }}>
                  <div className="truncate"><strong>{who.root ? "root" : who.principal}</strong>{who.agent ? <span className="muted"> · agent <code>{who.agent}</code></span> : null}</div>
                  <div className="row wrap small" style={{ gap: 4, marginTop: 2 }}>
                    {who.root ? <Badge tone="accent">root · all repositories</Badge> : who.scopes.length === 0 ? <Badge tone="attention">no scopes</Badge> : who.scopes.map((sc, i) => <Badge key={i} mono tone={sc.access === "admin" ? "accent" : "neutral"}>{sc.repo}: {sc.access}</Badge>)}
                  </div>
                </div>
              </div>
              <div className="row wrap" style={{ gap: 8, marginTop: 8 }}>
                {isAnyAdmin(who) ? <Link className="btn sm" to="/admin/keys" onClick={() => setOpen(false)}>SSH keys</Link> : null}
                {isAnyAdmin(who) ? <Link className="btn sm" to="/admin/tokens" onClick={() => setOpen(false)}>Manage tokens</Link> : null}
                {s.token || loadSettings().token ? <button className="btn sm ghost" onClick={signOut} disabled={busy}>Sign out</button> : null}
              </div>
            </div>
          ) : auth.error ? (
            <div className="small" style={{ color: "var(--danger)" }}><code>{auth.error.code}</code> {auth.error.message}</div>
          ) : null}
          <div className="actions">
            <button className="btn sm accent" onClick={() => apply(s, false)} disabled={busy}>{busy ? "Checking…" : "Sign in"}</button>
          </div>
          <h3>Connection</h3>
          <label className="field">
            API base URL
            <input className="input sm" value={s.baseUrl} onChange={(e) => setS({ ...s, baseUrl: e.target.value })} placeholder={DEFAULT_BASE_URL} spellCheck={false} />
            <span className="hint">Mode: {mode}. Default from <code>VITE_AGIT_API</code>.</span>
          </label>
          <label className="field">
            Acting agent id
            <input className="input sm" value={s.agent} onChange={(e) => setS({ ...s, agent: e.target.value })} placeholder="e.g. daniel or reviewer-1" spellCheck={false} />
            <span className="hint">Sent as <code>X-Agit-Agent</code> for reviews, leases and PRs. A token bound to an agent overrides it.</span>
          </label>
          <div className="actions">
            <button className="btn sm" onClick={() => setOpen(false)}>Cancel</button>
            <button className="btn sm" onClick={() => apply(s, true)} disabled={busy}>Save &amp; reconnect</button>
          </div>
        </div>
      ) : null}
    </div>
  );
}
