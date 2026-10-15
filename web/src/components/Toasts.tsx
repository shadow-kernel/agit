import { useEffect, useState } from "react";
import { onApiError } from "../api/client";

interface Toast {
  id: number;
  tone: "error" | "info" | "success";
  code?: string;
  message: string;
}

let seq = 0;
const listeners = new Set<(t: Toast) => void>();
export function toast(message: string, tone: Toast["tone"] = "info", code?: string) {
  const t = { id: ++seq, tone, message, code };
  listeners.forEach((l) => l(t));
}

export function Toasts() {
  const [items, setItems] = useState<Toast[]>([]);
  useEffect(() => {
    const push = (t: Toast) => {
      setItems((xs) => [...xs.slice(-4), t]);
      setTimeout(() => setItems((xs) => xs.filter((x) => x.id !== t.id)), t.tone === "error" ? 8000 : 4000);
    };
    listeners.add(push);
    const off = onApiError((e) => push({ id: ++seq, tone: "error", code: e.code, message: e.message + (e.retryable ? " (retryable)" : "") }));
    return () => {
      listeners.delete(push);
      off();
    };
  }, []);
  if (!items.length) return null;
  return (
    <div className="toasts" aria-live="polite">
      {items.map((t) => (
        <div key={t.id} className={`toast ${t.tone}`}>
          <span aria-hidden>{t.tone === "error" ? "⚠" : t.tone === "success" ? "✓" : "ℹ"}</span>
          <div className="grow">
            {t.code ? <div className="code">{t.code}</div> : null}
            <div>{t.message}</div>
          </div>
          <button onClick={() => setItems((xs) => xs.filter((x) => x.id !== t.id))} aria-label="Dismiss">×</button>
        </div>
      ))}
    </div>
  );
}
