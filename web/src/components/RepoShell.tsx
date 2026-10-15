import { createContext, useContext, useEffect } from "react";
import { NavLink, Outlet, useLocation, useParams } from "react-router-dom";
import { api } from "../api/client";
import type { RepoDetail } from "../api/types";
import { useAsync } from "../lib/useAsync";
import { ErrorState, Skeleton } from "./ui";

export interface RepoCtx {
  name: string;
  repo: RepoDetail | undefined;
  defaultBranch: string;
  openPulls: number | undefined;
  activeLeases: number | undefined;
  reloadCounts: () => void;
}

const Ctx = createContext<RepoCtx | null>(null);
export function useRepo(): RepoCtx {
  const c = useContext(Ctx);
  if (!c) throw new Error("useRepo outside RepoShell");
  return c;
}

export function RepoShell() {
  const { repo: name = "" } = useParams();
  const { pathname } = useLocation();
  const repo = useAsync(() => api.repo(name), [name], { silent: true });
  const pulls = useAsync(() => api.pulls(name, "open"), [name], { silent: true });
  const leases = useAsync(() => api.leases(name), [name], { silent: true });

  useEffect(() => {
    document.title = `${name} · agit`;
    return () => {
      document.title = "agit";
    };
  }, [name]);

  if (repo.error) {
    return (
      <main className="container">
        <ErrorState error={repo.error} retry={repo.reload} />
      </main>
    );
  }
  const now = Date.now();
  const ctx: RepoCtx = {
    name,
    repo: repo.data,
    defaultBranch: repo.data?.default_branch ?? "main",
    openPulls: pulls.data?.length,
    activeLeases: leases.data?.filter((l) => (l.expires_ms ?? Infinity) > now).length,
    reloadCounts: () => {
      pulls.reload();
      leases.reload();
      repo.reload();
    },
  };
  const base = `/r/${encodeURIComponent(name)}`;
  const codeActive = pathname === base || pathname.startsWith(`${base}/tree/`) || pathname.startsWith(`${base}/blob/`);
  const tab = (to: string, label: string, count?: number, forceActive?: boolean) => (
    <NavLink to={to} end={forceActive !== undefined} className={({ isActive }) => ((forceActive ?? isActive) ? "active" : "")}>
      {label}
      {count !== undefined ? <span className="count">{count}</span> : null}
    </NavLink>
  );
  return (
    <Ctx.Provider value={ctx}>
      <main className="container">
        <nav className="tabs repo-tabs" aria-label="Repository sections" style={{ marginTop: 0 }}>
          {tab(base, "Code", undefined, codeActive)}
          {tab(`${base}/commits`, "Commits")}
          {tab(`${base}/pulls`, "Pull requests", ctx.openPulls)}
          {tab(`${base}/agents`, "Agents")}
          {tab(`${base}/leases`, "Leases", ctx.activeLeases)}
          {tab(`${base}/knowledge`, "Knowledge")}
          {tab(`${base}/settings`, "Settings")}
        </nav>
        {repo.loading && !repo.data ? <Skeleton lines={4} /> : <Outlet />}
      </main>
    </Ctx.Provider>
  );
}

