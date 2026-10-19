// Icons for the file browser: folders are filled and coloured, files are
// outlined with a glyph for their kind, so the difference shows at a glance.

import type { EntryMode } from "../api/types";

export type FileKind = "dir" | "submodule" | "symlink" | "exec" | "code" | "doc" | "config" | "image" | "archive" | "file" | "up";

const EXT: Record<string, FileKind> = {};
const add = (kind: FileKind, exts: string) => exts.split(" ").forEach((e) => (EXT[e] = kind));
add("code", "rs ts tsx js jsx mjs cjs py go java kt kts c h cc cpp hpp cs rb php swift scala sh bash zsh fish lua dart ex exs erl hs ml clj vue svelte sql gradle groovy r m mm zig nim css scss sass less html htm graphql proto");
add("doc", "md markdown mdx txt rst adoc org tex pdf rtf");
add("config", "json jsonc json5 yml yaml toml ini cfg conf env lock xml plist properties editorconfig gitignore gitattributes agitignore agitmodules dockerignore npmrc nvmrc csv tsv");
add("image", "png jpg jpeg gif svg webp ico bmp tiff avif psd");
add("archive", "zip tar gz tgz bz2 xz 7z rar jar war bin pak exe dll so dylib wasm iso dmg");

const NAMES: Record<string, FileKind> = {
  dockerfile: "config",
  makefile: "code",
  license: "doc",
  "license-mit": "doc",
  "license-apache": "doc",
  readme: "doc",
  changelog: "doc",
  "cargo.lock": "config",
  "package-lock.json": "config",
  "pnpm-lock.yaml": "config",
};

export function fileKind(name: string, mode: EntryMode | "up"): FileKind {
  if (mode === "up") return "up";
  if (mode === "dir") return "dir";
  if (mode === "submodule") return "submodule";
  if (mode === "symlink") return "symlink";
  const lower = name.toLowerCase();
  if (NAMES[lower]) return NAMES[lower];
  const dot = lower.lastIndexOf(".");
  const ext = dot >= 0 ? lower.slice(dot + 1) : lower.replace(/^\./, "");
  if (EXT[ext]) return EXT[ext];
  if (mode === "executable") return "exec";
  return "file";
}

const LABEL: Record<FileKind, string> = {
  dir: "Folder",
  submodule: "Submodule",
  symlink: "Symbolic link",
  exec: "Executable file",
  code: "Source file",
  doc: "Document",
  config: "Configuration or data file",
  image: "Image",
  archive: "Archive or binary file",
  file: "File",
  up: "Parent folder",
};

/** Outline of a page with a folded corner, shared by all file icons. */
const Page = () => <path d="M3.75 1.5h5.69c.2 0 .39.08.53.22l2.81 2.81c.14.14.22.33.22.53v8.69a.75.75 0 0 1-.75.75h-8.5a.75.75 0 0 1-.75-.75V2.25a.75.75 0 0 1 .75-.75Z M9.25 1.75V4.5c0 .14.11.25.25.25h2.75" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinejoin="round" />;

function Glyph({ kind }: { kind: FileKind }) {
  switch (kind) {
    case "dir":
      return <path d="M1.75 3A1.75 1.75 0 0 0 0 4.75v8.5C0 14.22.78 15 1.75 15h12.5A1.75 1.75 0 0 0 16 13.25v-6.5A1.75 1.75 0 0 0 14.25 5H7.81a.25.25 0 0 1-.2-.1l-.9-1.2C6.38 3.26 5.86 3 5.31 3H1.75Z" fill="currentColor" />;
    case "submodule":
      return (
        <>
          <path d="M1.75 3A1.75 1.75 0 0 0 0 4.75v8.5C0 14.22.78 15 1.75 15h12.5A1.75 1.75 0 0 0 16 13.25v-6.5A1.75 1.75 0 0 0 14.25 5H7.81a.25.25 0 0 1-.2-.1l-.9-1.2C6.38 3.26 5.86 3 5.31 3H1.75Z" fill="currentColor" opacity="0.35" />
          <path d="M6 10.5h4M8.5 8.5l2 2-2 2" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
        </>
      );
    case "up":
      return <path d="M8 13V3.5M4 7.25 8 3.25l4 4" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />;
    case "symlink":
      return (
        <>
          <Page />
          <path d="M6 11.5 10 7.5M7 7.5h3v3" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" />
        </>
      );
    case "exec":
      return (
        <>
          <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" fill="none" stroke="currentColor" strokeWidth="1.2" />
          <path d="m4.5 6.25 2 1.75-2 1.75M8 10.25h3.5" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" />
        </>
      );
    case "code":
      return (
        <>
          <Page />
          <path d="M6.5 8 5 9.5 6.5 11M9.5 8 11 9.5 9.5 11" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" />
        </>
      );
    case "doc":
      return (
        <>
          <Page />
          <path d="M5.25 7.5h5.5M5.25 9.5h5.5M5.25 11.5h3.5" fill="none" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" />
        </>
      );
    case "config":
      return (
        <>
          <Page />
          <path d="M6.75 7.25c-.75 0-1 .4-1 1v.5c0 .4-.25.75-.75.75.5 0 .75.35.75.75v.5c0 .6.25 1 1 1M9.25 7.25c.75 0 1 .4 1 1v.5c0 .4.25.75.75.75-.5 0-.75.35-.75.75v.5c0 .6-.25 1-1 1" fill="none" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" strokeLinejoin="round" />
        </>
      );
    case "image":
      return (
        <>
          <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" fill="none" stroke="currentColor" strokeWidth="1.2" />
          <circle cx="5.5" cy="6" r="1.1" fill="currentColor" />
          <path d="m2 12 3.5-3.5 2.5 2.5 2-2 4 3.5" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinejoin="round" />
        </>
      );
    case "archive":
      return (
        <>
          <Page />
          <path d="M7 3v1.25M8 4.25v1.25M7 5.5v1.25M8 6.75V8M6.75 9h2.5v2.5h-2.5Z" fill="none" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" />
        </>
      );
    default:
      return <Page />;
  }
}

/** 16 px icon for a tree entry; `mode="up"` draws the parent-folder arrow. */
export function FileIcon({ name, mode }: { name: string; mode: EntryMode | "up" }) {
  const kind = fileKind(name, mode);
  return (
    <svg className={`ficon ficon-${kind}`} width="16" height="16" viewBox="0 0 16 16" role="img" aria-label={LABEL[kind]}>
      <title>{LABEL[kind]}</title>
      <Glyph kind={kind} />
    </svg>
  );
}

/** Folders (and submodules) first, then files; natural, case-insensitive order inside each group. */
export function sortEntries<T extends { name: string; mode: EntryMode }>(entries: T[]): T[] {
  const group = (m: EntryMode) => (m === "dir" || m === "submodule" ? 0 : 1);
  return [...entries].sort((a, b) => group(a.mode) - group(b.mode) || a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" }));
}
