import { useEffect, useState } from "react";
import { renderMarkdown } from "../lib/markdown";

export function Markdown({ source, compact, className = "" }: { source: string; compact?: boolean; className?: string }) {
  const [html, setHtml] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    setHtml(null);
    renderMarkdown(source).then((h) => alive && setHtml(h));
    return () => {
      alive = false;
    };
  }, [source]);
  if (html === null) return <div className={`markdown-body ${compact ? "compact" : ""} ${className}`}><div className="skeleton skeleton-line" style={{ width: "70%" }} /><div className="skeleton skeleton-line" style={{ width: "45%" }} /></div>;
  return <div className={`markdown-body ${compact ? "compact" : ""} ${className}`} dangerouslySetInnerHTML={{ __html: html }} />;
}
