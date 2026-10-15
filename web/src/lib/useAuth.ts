// Who am I? A tiny store around `GET /auth/whoami`, refreshed whenever the
// connection settings change or the API mode flips. Pages use `can()` to hide
// admin-only actions; when the identity is unknown nothing is hidden and the
// server stays the authority.

import { useEffect, useState } from "react";
import { api, ApiError, onApiMode } from "../api/client";
import { onSettingsChange } from "../api/settings";
import type { Access, WhoAmI } from "../api/types";

export interface AuthState {
  who: WhoAmI | null;
  error: ApiError | null;
  loading: boolean;
}

let state: AuthState = { who: null, error: null, loading: true };
const listeners = new Set<(s: AuthState) => void>();
let seq = 0;

function set(next: AuthState) {
  state = next;
  listeners.forEach((l) => l(next));
}

export function getAuth() {
  return state;
}

export async function refreshAuth(): Promise<AuthState> {
  const my = ++seq;
  set({ ...state, loading: true });
  try {
    const who = await api.whoami();
    if (my === seq) set({ who, error: null, loading: false });
  } catch (e) {
    if (my === seq) set({ who: null, error: e instanceof ApiError ? e : new ApiError(0, "error", e instanceof Error ? e.message : String(e)), loading: false });
  }
  return state;
}

let wired = false;
function wire() {
  if (wired) return;
  wired = true;
  onSettingsChange(() => void refreshAuth());
  onApiMode(() => void refreshAuth());
}

export function useAuth(): AuthState {
  const [s, setS] = useState(state);
  useEffect(() => {
    wire();
    listeners.add(setS);
    setS(state);
    return () => {
      listeners.delete(setS);
    };
  }, []);
  return s;
}

const RANK: Record<Access, number> = { read: 0, write: 1, admin: 2 };

/** Does the principal hold `level` on `repo` (`"*"` = server-wide)? */
export function hasAccess(who: WhoAmI | null, repo: string | "*", level: Access): boolean {
  if (!who) return false;
  if (who.root) return true;
  return who.scopes.some((s) => (s.repo === "*" || (repo !== "*" && s.repo === repo)) && RANK[s.access] >= RANK[level]);
}

/** `true` when the action should be offered: identity unknown, or access granted. */
export function can(auth: AuthState, repo: string | "*", level: Access): boolean {
  if (!auth.who) return true;
  return hasAccess(auth.who, repo, level);
}

/** Anything admin anywhere: enough to show the token admin page (the server decides per request). */
export function isAnyAdmin(who: WhoAmI | null): boolean {
  return !!who && (who.root || who.scopes.some((s) => s.access === "admin"));
}

export const needsSignIn = (auth: AuthState) => !!auth.error && (auth.error.status === 401 || auth.error.status === 403);
