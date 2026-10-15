import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError, reportError } from "../api/client";
import { onSettingsChange } from "../api/settings";

// Every saved settings change (token, base URL, agent) bumps an epoch so all
// data hooks refetch: a page stuck on a 403 recovers right after signing in.
let epoch = 0;
const epochListeners = new Set<(n: number) => void>();
onSettingsChange(() => {
  epoch++;
  epochListeners.forEach((l) => l(epoch));
});

export interface AsyncState<T> {
  data: T | undefined;
  error: ApiError | null;
  loading: boolean;
  reload: () => void;
  setData: (updater: T | ((prev: T | undefined) => T)) => void;
}

/**
 * Minimal data hook: runs `fn` when `deps` change, exposes loading/error,
 * keeps previous data during reloads, and surfaces errors as toasts unless
 * `silent` is set.
 */
export function useAsync<T>(fn: () => Promise<T>, deps: unknown[], opts: { silent?: boolean; enabled?: boolean } = {}): AsyncState<T> {
  const [data, setDataState] = useState<T | undefined>(undefined);
  const [error, setError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(true);
  const [tick, setTick] = useState(0);
  const [ep, setEp] = useState(epoch);
  const fnRef = useRef(fn);
  fnRef.current = fn;
  const enabled = opts.enabled ?? true;

  useEffect(() => {
    epochListeners.add(setEp);
    return () => {
      epochListeners.delete(setEp);
    };
  }, []);

  useEffect(() => {
    if (!enabled) {
      setLoading(false);
      return;
    }
    let alive = true;
    setLoading(true);
    setError(null);
    fnRef
      .current()
      .then((d) => {
        if (!alive) return;
        setDataState(d);
        setLoading(false);
      })
      .catch((e) => {
        if (!alive) return;
        const err = e instanceof ApiError ? e : new ApiError(0, "error", e instanceof Error ? e.message : String(e));
        setError(err);
        setLoading(false);
        if (!opts.silent) reportError(err);
      });
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick, enabled, ep]);

  const reload = useCallback(() => setTick((t) => t + 1), []);
  const setData = useCallback((u: T | ((prev: T | undefined) => T)) => {
    setDataState((prev) => (typeof u === "function" ? (u as (p: T | undefined) => T)(prev) : u));
  }, []);
  return { data, error, loading, reload, setData };
}

/** Wrap a mutation: runs it, reports API errors as toasts, returns result or undefined. */
export function useMutation<A extends unknown[], R>(fn: (...args: A) => Promise<R>) {
  const [busy, setBusy] = useState(false);
  const run = useCallback(
    async (...args: A): Promise<R | undefined> => {
      setBusy(true);
      try {
        return await fn(...args);
      } catch (e) {
        reportError(e);
        return undefined;
      } finally {
        setBusy(false);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  return { run, busy };
}

export function useNow(intervalMs = 1000) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs]);
  return now;
}
