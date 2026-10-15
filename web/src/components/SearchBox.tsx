import { useEffect, useRef, useState } from "react";
import { useLocation, useNavigate, useSearchParams } from "react-router-dom";

/** Header search. `/` focuses it; Enter opens `/r/:repo/search?q=`. */
export function SearchBox({ repo }: { repo: string }) {
  const [sp] = useSearchParams();
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const ref = useRef<HTMLInputElement>(null);
  const base = `/r/${encodeURIComponent(repo)}`;
  const onSearchPage = pathname === `${base}/search`;
  const [q, setQ] = useState(onSearchPage ? sp.get("q") ?? "" : "");

  useEffect(() => {
    if (onSearchPage) setQ(sp.get("q") ?? "");
  }, [onSearchPage, sp]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      const editable = tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || (e.target as HTMLElement)?.isContentEditable;
      if (e.key === "/" && !e.metaKey && !e.ctrlKey && !e.altKey && !editable) {
        e.preventDefault();
        ref.current?.focus();
        ref.current?.select();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  const submit = () => {
    const needle = q.trim();
    if (!needle) return;
    const next = new URLSearchParams();
    next.set("q", needle);
    const tab = onSearchPage ? sp.get("tab") : null;
    const rev = onSearchPage ? sp.get("rev") : null;
    if (tab) next.set("tab", tab);
    if (rev) next.set("rev", rev);
    navigate(`${base}/search?${next.toString()}`);
  };

  return (
    <form
      className="search-box"
      role="search"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <input
        ref={ref}
        className="input sm"
        type="search"
        placeholder="Search code, commits, notes…"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") ref.current?.blur();
        }}
        aria-label={`Search ${repo}`}
        spellCheck={false}
      />
      <kbd className="kbd slash" aria-hidden>/</kbd>
    </form>
  );
}
