import { useEffect, useMemo, useState } from "react";
import { useLocation } from "react-router-dom";
import { escapeHtml, highlightLines } from "../lib/highlight";

export function CodeView({ code, lang, raw }: { code: string; lang?: string; raw?: boolean }) {
  const plain = useMemo(() => code.replace(/\n$/, "").split("\n"), [code]);
  const [lines, setLines] = useState<string[] | null>(null);
  const { hash } = useLocation();
  const target = hash.startsWith("#L") ? parseInt(hash.slice(2), 10) : NaN;

  useEffect(() => {
    let alive = true;
    setLines(null);
    if (raw) return;
    highlightLines(code.replace(/\n$/, ""), lang).then((ls) => alive && setLines(ls));
    return () => {
      alive = false;
    };
  }, [code, lang, raw]);

  useEffect(() => {
    if (!Number.isNaN(target)) document.getElementById(`L${target}`)?.scrollIntoView({ block: "center" });
  }, [target, lines]);

  if (raw) {
    return (
      <div className="code-view raw">
        <pre>{code}</pre>
      </div>
    );
  }
  const rendered = lines ?? plain.map(escapeHtml);
  return (
    <div className="code-view">
      <table>
        <tbody>
          {rendered.map((html, i) => (
            <tr key={i} id={`L${i + 1}`} className={target === i + 1 ? "hl" : undefined}>
              <td className="ln" onClick={() => { history.replaceState(null, "", `#L${i + 1}`); window.dispatchEvent(new HashChangeEvent("hashchange")); }}>{i + 1}</td>
              <td className="code" dangerouslySetInnerHTML={{ __html: html || " " }} />
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
