// Lazy markdown rendering: marked + DOMPurify.
let loader: Promise<{ marked: typeof import("marked").marked; purify: typeof import("dompurify").default }> | null = null;

function load() {
  if (!loader) {
    loader = Promise.all([import("marked"), import("dompurify")]).then(([m, d]) => {
      m.marked.setOptions({ gfm: true, breaks: false });
      return { marked: m.marked, purify: d.default };
    });
  }
  return loader;
}

export async function renderMarkdown(src: string): Promise<string> {
  const { marked, purify } = await load();
  const html = await marked.parse(src ?? "");
  return purify.sanitize(html, { USE_PROFILES: { html: true }, ADD_ATTR: ["target"] });
}
