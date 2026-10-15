// Persisted client settings (localStorage). Edited from the header popover.

const KEY = "agit.settings";

export interface Settings {
  baseUrl: string;
  token: string;
  agent: string;
}

export const DEFAULT_BASE_URL: string =
  (import.meta.env.VITE_AGIT_API as string | undefined) || "http://localhost:7878/api/v1";

export function loadSettings(): Settings {
  const s: Settings = { baseUrl: DEFAULT_BASE_URL, token: "", agent: "" };
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) Object.assign(s, JSON.parse(raw));
  } catch {
    /* ignore */
  }
  if (!s.baseUrl) s.baseUrl = DEFAULT_BASE_URL;
  return s;
}

export function saveSettings(s: Settings) {
  try {
    localStorage.setItem(KEY, JSON.stringify(s));
  } catch {
    /* ignore */
  }
  listeners.forEach((l) => l(s));
}

const listeners = new Set<(s: Settings) => void>();
export function onSettingsChange(l: (s: Settings) => void) {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}
